//! **The PI farmer** (R-SOCIETY stage B, packet SOC-B6): a bot that runs
//! planetary colonies in its home system and sells what they make, the way
//! a player does from a station with the planet window open.
//!
//! [`ROLE`] is the `pi` row of [`crate::roles::ROLES`] and the `pi_farmer`
//! career's role. Docked, it asks for one [`Act::Pi`] and a dwell; the
//! driver's arm is one [`run`]. In space outside a run it goes home. A
//! [`run`] is one visit to the planet window, every step a client call:
//!
//! 1. **Tend** each colony (`planetMgr.GetPlanetsForChar`, then the
//!    planet's moniker `GetPlanetInfo`): restart every extractor whose
//!    program has expired ([`restarts`], one `UserUpdateNetwork` with an
//!    `INSTALLPROGRAM` per extractor, the same heads and radius), and
//!    launch the command centre's commodities ([`launch_of`],
//!    `UserLaunchCommodities`).
//! 2. **Sell** the planetary commodities in the hangar ([`sale_plan`]).
//! 3. **Grow**: with fewer colonies than Interplanetary Consolidation
//!    allows (`level + 1` [V], capped by [`MAX_COLONIES`]), found one.
//!    The system's planets are read off the overview once (an undock, a
//!    look, a dock: [`survey`]); the next visit picks a planet
//!    ([`pick_planet`]), buys that planet type's command centre from an
//!    ask at the home station (E5b's NPCs sell them), loads it into the
//!    hold, undocks and submits **one templated colony** ([`Template`],
//!    [`fit`]): the command centre (upgraded as far as Command Center
//!    Upgrades and the wallet allow), one extractor control unit with as
//!    many heads as the power and CPU carry on the planet's best resource,
//!    one basic industry facility on that resource's P1 schematic, and the
//!    two routes extractor -> factory -> command centre. Then it docks.
//!
//! **The layout is validated the way the client validates it**: the
//! client's colony code is shared client/server Python, and
//! `eve_server::planet` is its port, so the bot dry-runs every candidate
//! stream against its own copy of the static table ([`fit`]) before it
//! sends one, and sizes the routes off the dry-run colony. The yield it
//! routes is the server's own survey quote (`GetProgramResultInfo`, the
//! survey window's call).
//!
//! **Export.** The server serves two ways out (`docs/lanes/planet.md`
//! §10-§11): a command centre's launch lands the goods in the station
//! hangar **[A, server]**, and the customs office takes a launchpad's
//! goods for a tax. A launchpad does not fit a level-0 command centre's
//! CPU, and the launch needs no flight, so the farmer **launches from the
//! command centre** and never uses the office. Hauling elsewhere is the
//! trader's.
//!
//! **What it makes feeds manufacturing**: P1 into P2 into fuel blocks and
//! structure components (R-SOCIETY §1 row 10). B6's template makes P1;
//! [`crate::pi_p2`] (FU-PI2) founds P2 colonies when the planet, or the
//! P1 another of the bot's colonies launches, yields a P2 recipe's two
//! inputs, and feeds an imported input through the customs office on
//! every visit.
//!
//! Every rule here is **[A] policy** -- what a player would do -- except
//! the client's calls and ids, cited [V].

use crate::brain::{self, Act, Memory, Observation};
use crate::courier::{Here, Move, next_move};
use crate::pricing::{self, Quote};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::verbs::{Book, SellLine};
use crate::{Conn, DOCK_RANGE, Error, ItemRow, ObjectRef, Result};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::planet::{
    self as sim, Colonies, Colony, ColonyEdit, Command, Head, Id, PinKind, ProgramQuote, Statics,
    SurfacePoint,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Badger **[V-data]**: an NPC-sold industrial whose hold takes a
/// command centre (1,000 m3 packaged [V-data]); a frigate's does not.
pub const BADGER: i64 = crate::hulls::BADGER;
/// `typeInterplanetaryConsolidation` **[V]**: colonies allowed is its
/// level + 1 (`clientColony.PostValidateCreatePin`).
pub const SKILL_INTERPLANETARY_CONSOLIDATION: i64 = 2495;
/// `typeCommandCenterUpgrade` **[V]**: the highest command-centre level.
pub const SKILL_COMMAND_CENTER_UPGRADES: i64 = 2505;
/// The skill levels the kit asks for. **[A]**: two planets and a level-2
/// command centre is a new PI player's first week; falsified by farmers
/// that never grow past one planet a month in.
pub const KIT_LEVEL: i64 = 2;

/// What the farmer flies and learns (E8's closed economy buys it, GM setup
/// gives it): a hull for the command centre, and the two PI skills.
pub const PI_KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(BADGER),
    modules: None,
    ammo: None,
    skills: &[
        (SKILL_COMMAND_CENTER_UPGRADES, KIT_LEVEL),
        (SKILL_INTERPLANETARY_CONSOLIDATION, KIT_LEVEL),
    ],
    more: &[],
};

/// `groupPlanetaryLinks`' one type, 2280 "Link" **[V]**: the type the
/// server builds every `CREATELINK` as (`service/planet.rs`
/// `TYPE_PLANETARY_LINK`), so the dry run must use it too.
pub const LINK_TYPE: i64 = 2280;
/// `categoryPlanetaryCommodities` **[V]**: P1 to P4, what a colony makes.
pub const CATEGORY_PLANETARY_COMMODITIES: i64 = 43;
/// The fifteen P1 types **[V-data]** (group 1042): the farmer's basket.
/// The bot ships no type table, so the signal reads these by id.
pub const P1_TYPES: &[i64] = &[
    2389, 2390, 2392, 2393, 2395, 2396, 2397, 2398, 2399, 2400, 2401, 3645, 3683, 3779, 9828,
];

/// The extraction program's length, hours. **[A]**: a player who looks in
/// once a day sets about a day; falsified by extractors idle most of the
/// day in `[soc]` farmer sessions (too short) or farmers whose income
/// lags a price move by days (too long).
pub const PROGRAM_HOURS: f64 = 23.0;
/// The most colonies a farmer runs, whatever its skill. **[A]**: falsified
/// by a farmer whose tending visit outlasts its session.
pub const MAX_COLONIES: usize = 3;
/// The command-centre level the template upgrades to at most. **[A]**:
/// level 2 carries a ten-head extractor and a factory; falsified by
/// colonies that stop at level 0 with the skill trained and the ISK there.
pub const MAX_LEVEL: i64 = 2;
/// The share of the wallet one colony may cost (pins and upgrade). **[A]**
pub const WALLET_SHARE: f64 = 0.5;
/// Pins sit this far (radians) from the command centre. **[A]**: close,
/// so links are short and cheap.
pub const PIN_SPACING: f64 = 0.02;
/// Planets chosen among, by the bot's seed, so farmers spread. **[A]**
pub const SPREAD: usize = brain::SPREAD;
/// How long a warp, an undock or a dock may take. **[A]** the courier's.
const CEILING: Duration = crate::courier::TRAVEL_CEILING;
const POLL: Duration = Duration::from_millis(250);

/// The `MISSING` lines a run can end on.
pub const MISSING_STATICS: &str = "the PI static table (eve-bots --content)";
pub const MISSING_HULL: &str = "a hull whose hold takes a command centre";

/// Farms planets, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "pi",
    greeting: "o7 -- farming planets, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

// ---- what the bot remembers between visits ---------------------------------

/// What the last [`run`] left for the next docked turn, by bot name. Per
/// process; a restart forgets it, which costs one look.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    /// The last run could not load a command centre: ask for a hull.
    pub lacks_hull: bool,
    /// Planet types whose command centre found no ask here.
    pub no_centre: BTreeSet<i64>,
}

static NOTES: Mutex<BTreeMap<String, Note>> = Mutex::new(BTreeMap::new());
/// The planets each system's overview showed: `(planet, planet type)`.
static PLANETS: Mutex<BTreeMap<i64, Vec<(i64, i64)>>> = Mutex::new(BTreeMap::new());

pub fn note_of(name: &str) -> Note {
    NOTES
        .lock()
        .map(|n| n.get(name).cloned().unwrap_or_default())
        .unwrap_or_default()
}

pub fn set_note(name: &str, note: Note) {
    if let Ok(mut n) = NOTES.lock() {
        n.insert(name.to_owned(), note);
    }
}

fn planets_seen(system: i64) -> Option<Vec<(i64, i64)>> {
    PLANETS.lock().ok().and_then(|p| p.get(&system).cloned())
}

fn remember_planets(system: i64, planets: Vec<(i64, i64)>) {
    if let Ok(mut p) = PLANETS.lock() {
        p.insert(system, planets);
    }
}

// ---- the role's rules -------------------------------------------------------

/// A planetary commodity (P1 and up) in a hangar or hold.
pub fn is_commodity_row(row: &ItemRow) -> bool {
    !row.singleton && row.category_id == CATEGORY_PLANETARY_COMMODITIES
}

/// Docked: a Badger when the last run lacked a hull, then one [`Act::Pi`]
/// and a dwell. Tending needs no hull, so none is asked for until a build
/// needs one.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.claim = None;
    m.homing = None;
    if note_of(&m.name).lacks_hull {
        let active = o.ship.map(|s| s.ship_id);
        let flying = o.ship.and_then(|s| s.type_id);
        if flying != Some(BADGER) {
            if let Some(b) = o.hangar.iter().find(|r| {
                r.type_id == BADGER
                    && Some(r.item_id) != active
                    && !m.combat.untrained.contains(&r.type_id)
            }) {
                return vec![Act::Board(b.item_id)];
            }
            return brain::gm_or_missing(m, true, false, false, MISSING_HULL);
        }
    }
    let rest = m.jitter(m.policy.dwell);
    vec![Act::Pi, Act::Wait(rest)]
}

/// In space outside a run (a dropped connection mid-build): home.
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

/// GM setup: the two PI skills, and a Badger when lacking one.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines: Vec<String> = PI_KIT
        .skills
        .iter()
        .map(|(s, l)| format!("/giveskill {s} {l}"))
        .collect();
    if lacks.ship {
        lines.push(format!("/createitem {BADGER}"));
    }
    lines
}

// ---- the colony as the wire shows it, pure --------------------------------------

/// One pin of a colony, the fields the farmer reads (`pin_row`'s keys).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PinView {
    pub id: i64,
    pub type_id: i64,
    pub contents: BTreeMap<i64, i64>,
    pub program_type: Option<i64>,
    pub expiry_time: Option<i64>,
    pub install_time: Option<i64>,
    pub head_radius: f64,
    pub heads: usize,
    pub schematic_id: Option<i64>,
    pub last_launch_time: Option<i64>,
}

/// A colony as `GetPlanetInfo` answers it for its owner.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColonyView {
    pub planet_id: i64,
    pub planet_type_id: i64,
    pub radius: f64,
    /// `currentSimTime`: the server's clock as of the read.
    pub sim_time: i64,
    pub level: i64,
    pub pins: Vec<PinView>,
}

/// The value under `name` in a `KeyVal` (an instance's attribute dict) or
/// a plain dict, `(value, key)` pairs.
pub(crate) fn kv<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let entries = match v {
        PyValue::Instance { state, .. } => match state.as_ref() {
            PyValue::Dict(e) => e,
            _ => return None,
        },
        PyValue::Dict(e) => e,
        _ => return None,
    };
    entries
        .iter()
        .find(|(_, k)| match k {
            PyValue::Bytes(b) => b == name.as_bytes(),
            PyValue::Unicode(s) => s == name,
            _ => false,
        })
        .map(|(value, _)| value)
}

pub(crate) fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

pub(crate) fn real(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

pub(crate) fn seq(v: Option<&PyValue>) -> &[PyValue] {
    match v {
        Some(PyValue::List(items) | PyValue::Tuple(items)) => items,
        _ => &[],
    }
}

/// `{typeID: quantity}`, int-keyed (`contents_dict`).
fn int_dict(v: Option<&PyValue>) -> BTreeMap<i64, i64> {
    let Some(PyValue::Dict(entries)) = v else {
        return BTreeMap::new();
    };
    entries
        .iter()
        .filter_map(|(q, t)| Some((int(Some(t))?, int(Some(q))?)))
        .collect()
}

/// **Read a colony** off `GetPlanetInfo`'s answer: `None` when it carries
/// no pins (no colony of ours there).
pub fn read_colony(v: &PyValue) -> Option<ColonyView> {
    let pins = kv(v, "pins")?;
    let pins: Vec<PinView> = seq(Some(pins))
        .iter()
        .filter_map(|p| {
            Some(PinView {
                id: int(kv(p, "id"))?,
                type_id: int(kv(p, "typeID"))?,
                contents: int_dict(kv(p, "contents")),
                program_type: int(kv(p, "programType")),
                expiry_time: int(kv(p, "expiryTime")),
                install_time: int(kv(p, "installTime")),
                head_radius: real(kv(p, "headRadius")).unwrap_or(0.0),
                heads: seq(kv(p, "heads")).len(),
                schematic_id: int(kv(p, "schematicID")),
                last_launch_time: int(kv(p, "lastLaunchTime")),
            })
        })
        .collect();
    if pins.is_empty() {
        return None;
    }
    Some(ColonyView {
        planet_id: int(kv(v, "planetID")).unwrap_or(0),
        planet_type_id: int(kv(v, "planetTypeID")).unwrap_or(0),
        radius: real(kv(v, "radius")).unwrap_or(0.0),
        sim_time: int(kv(v, "currentSimTime")).unwrap_or(0),
        level: int(kv(v, "level")).unwrap_or(0),
        pins,
    })
}

/// The head radius of a program `hours` long: `planetCommon.
/// GetProgramLengthFromHeadRadius` [V] inverted, inside the client's range.
pub fn radius_for_hours(hours: f64) -> f64 {
    let span = sim::RADIUS_DRILLAREAMAX - sim::RADIUS_DRILLAREAMIN;
    (sim::RADIUS_DRILLAREAMIN + (hours - 1.0) / 335.0 * span)
        .clamp(sim::RADIUS_DRILLAREAMIN, sim::RADIUS_DRILLAREAMAX)
}

/// The one input of a single-input schematic.
fn single_input(statics: &Statics, schematic_id: i64) -> Option<i64> {
    match statics.schematic(schematic_id)?.inputs.as_slice() {
        [(t, _)] => Some(*t),
        _ => None,
    }
}

/// **Restart Extractors**: an `INSTALLPROGRAM` for every extraction unit
/// with heads whose program has run out (or was never installed), past the
/// install cooldown [V] -- the same resource (else the input a factory of
/// the colony wants), the same radius (else [`PROGRAM_HOURS`]'s).
pub fn restarts(view: &ColonyView, statics: &Statics) -> Vec<Command> {
    let fallback = view
        .pins
        .iter()
        .filter_map(|p| p.schematic_id)
        .find_map(|s| single_input(statics, s));
    view.pins
        .iter()
        .filter(|p| statics.pin_kind(p.type_id) == Some(PinKind::Ecu) && p.heads > 0)
        .filter(|p| p.expiry_time.is_none_or(|e| e <= view.sim_time))
        .filter(|p| {
            p.install_time
                .is_none_or(|i| view.sim_time >= i + sim::PROGRAM_INSTALLATION_COOLDOWN)
        })
        .filter_map(|p| {
            let resource = p.program_type.or(fallback)?;
            let radius =
                if (sim::RADIUS_DRILLAREAMIN..=sim::RADIUS_DRILLAREAMAX).contains(&p.head_radius) {
                    p.head_radius
                } else {
                    radius_for_hours(PROGRAM_HOURS)
                };
            Some(Command::InstallProgram {
                pin_id: Id::Real(p.id),
                program_type: Some(resource),
                head_radius: radius,
            })
        })
        .collect()
}

/// Is `type_id` a planetary commodity in the static table's item rows?
fn is_commodity(statics: &Statics, type_id: i64) -> bool {
    statics
        .item_rows()
        .iter()
        .any(|r| r.type_id == type_id && r.category_id == CATEGORY_PLANETARY_COMMODITIES)
}

/// **The launch**: the command centre and every commodity it holds, when
/// it holds one and its launch cooldown (`LAUNCH_CYCLE_TIME` [V]) is over.
pub fn launch_of(view: &ColonyView, statics: &Statics) -> Option<(i64, BTreeMap<i64, i64>)> {
    let cc = view
        .pins
        .iter()
        .find(|p| statics.pin_kind(p.type_id) == Some(PinKind::Command))?;
    if cc
        .last_launch_time
        .is_some_and(|t| view.sim_time < t + sim::LAUNCH_CYCLE_TIME)
    {
        return None;
    }
    let goods: BTreeMap<i64, i64> = cc
        .contents
        .iter()
        .filter(|(t, q)| **q > 0 && is_commodity(statics, **t))
        .map(|(t, q)| (*t, *q))
        .collect();
    (!goods.is_empty()).then_some((cc.id, goods))
}

// ---- the template, pure ------------------------------------------------------

/// **One templated colony** for a planet type: which pins, which resource
/// and which P1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template {
    pub planet_type: i64,
    pub cc_type: i64,
    pub ecu_type: i64,
    pub factory_type: i64,
    pub schematic_id: i64,
    pub resource: i64,
    /// The resource's quality on this planet (any scale: it only ranks).
    pub quality: f64,
    pub product: i64,
}

/// A pin type's `planetRestriction` (1632) allows `planet_type`.
fn fits_planet(statics: &Statics, type_id: i64, planet_type: i64) -> bool {
    let r = statics
        .pin_type(type_id)
        .map_or(0.0, |t| t.attr(sim::attr::PLANET_RESTRICTION))
        .trunc() as i64;
    r == 0 || r == planet_type
}

/// The cheapest pin type of `kind` that `planet_type` allows, from
/// `among` (or every pin type).
pub(crate) fn cheapest(
    statics: &Statics,
    kind: PinKind,
    planet_type: i64,
    among: Option<&[i64]>,
) -> Option<i64> {
    statics
        .pin_types()
        .map(|t| t.type_id)
        .filter(|t| among.is_none_or(|a| a.contains(t)))
        .filter(|t| statics.pin_kind(*t) == Some(kind) && fits_planet(statics, *t, planet_type))
        .min_by(|a, b| {
            statics
                .base_price(*a)
                .total_cmp(&statics.base_price(*b))
                .then(a.cmp(b))
        })
}

/// **The template** for `planet_type` at `qualities` (resource -> quality):
/// the cheapest command centre and extraction unit the planet allows, and
/// the best resource that a single-input schematic turns into one product
/// in a factory the planet allows. `None`: nothing on this planet can be
/// farmed with the table.
pub fn template(
    statics: &Statics,
    planet_type: i64,
    qualities: &BTreeMap<i64, f64>,
) -> Option<Template> {
    let cc_type = cheapest(statics, PinKind::Command, planet_type, None)?;
    let ecu_type = cheapest(statics, PinKind::Ecu, planet_type, None)?;
    let mut by_quality: Vec<(i64, f64)> = qualities
        .iter()
        .filter(|(_, q)| **q > 0.0)
        .map(|(t, q)| (*t, *q))
        .collect();
    by_quality.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for (resource, quality) in by_quality {
        for s in statics.schematics() {
            let (&[(input, _)], &[(product, _)]) = (s.inputs.as_slice(), s.outputs.as_slice())
            else {
                continue;
            };
            if input != resource {
                continue;
            }
            let Some(factory_type) = cheapest(
                statics,
                PinKind::Process,
                planet_type,
                Some(&s.pin_type_ids),
            ) else {
                continue;
            };
            return Some(Template {
                planet_type,
                cc_type,
                ecu_type,
                factory_type,
                schematic_id: s.schematic_id,
                resource,
                quality,
                product,
            });
        }
    }
    None
}

/// **The planet to settle**: among `planets` (`(planet, type)`, the
/// overview's) that are not ours and whose type is not in `skip`, those
/// with a [`template`]; the [`SPREAD`] best by resource quality, the one at
/// `seed % n` so farmers spread.
pub fn pick_planet(
    statics: &Statics,
    planets: &[(i64, i64)],
    ours: &BTreeSet<i64>,
    skip: &BTreeSet<i64>,
    seed: u64,
) -> Option<(i64, Template)> {
    let mut c: Vec<(i64, Template)> = planets
        .iter()
        .filter(|(p, t)| !ours.contains(p) && !skip.contains(t))
        .filter_map(|&(p, t)| Some((p, template(statics, t, statics.resources(t)?)?)))
        .collect();
    c.sort_by(|a, b| b.1.quality.total_cmp(&a.1.quality).then(a.0.cmp(&b.0)));
    c.truncate(SPREAD);
    if c.is_empty() {
        return None;
    }
    let i = (seed % c.len() as u64) as usize;
    Some(c.swap_remove(i))
}

/// Where a colony goes and whose it is: the dry run's [`ColonyEdit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    pub planet_id: i64,
    pub owner_id: i64,
    pub planet_type: i64,
    /// Metres (`GetPlanetInfo`'s `radius`).
    pub radius: f64,
    /// Where the command centre stands: `theta` (the client's longitude)
    /// on the equator.
    pub theta: f64,
}

/// A fitted layout: the level, the heads, their radius, and the bill.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub level: i64,
    pub heads: Vec<Head>,
    pub head_radius: f64,
    /// ISK: the pins' base prices and the upgrade (the CC is the item).
    pub cost: f64,
}

/// A quote that answers what the server quoted: the dry run's
/// `INSTALLPROGRAM` yield.
struct Fixed(i64);

impl ProgramQuote for Fixed {
    fn quote(&self, _: &Colony, _: i64, _: i64, _: f64) -> i64 {
        self.0
    }
}

const ECU: (i64, i64) = (1, 1);
const FACTORY: (i64, i64) = (1, 2);

pub(crate) fn temp((a, b): (i64, i64)) -> Id {
    Id::Temporary(vec![a, b])
}

pub(crate) fn at(theta: f64) -> SurfacePoint {
    SurfacePoint::new(std::f64::consts::FRAC_PI_2, theta)
}

/// `n` heads on a ring around `centre`, far enough apart not to overlap
/// (two head radii between neighbours) and inside `influence` when it is
/// known. Near the equator the polar angle and the longitude are both
/// arc lengths, so the ring is round.
pub fn ring(centre: SurfacePoint, n: usize, head_radius: f64, influence: f64) -> Vec<Head> {
    let n = n.max(1);
    let mut d = if n == 1 {
        2.0 * head_radius
    } else {
        (head_radius / (std::f64::consts::PI / n as f64).sin()).max(2.0 * head_radius) * 1.05
    };
    if influence > 0.0 {
        d = d.min(influence * 0.9);
    }
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            Head {
                head_id: i as i64,
                latitude: centre.latitude + d * a.cos(),
                longitude: centre.longitude + d * a.sin(),
            }
        })
        .collect()
}

/// The stream without routes: the command centre (`cc_item`, the item in
/// the hold [V]), its upgrade, the extraction unit east of it and the
/// factory west, linked unit -> factory -> centre, the heads, the schematic
/// and the program.
pub fn base_stream(t: &Template, cc_item: i64, site: &Site, layout: &Layout) -> Vec<Command> {
    let cc = Id::Real(cc_item);
    let mut v = vec![Command::CreatePin {
        pin_id: cc.clone(),
        type_id: t.cc_type,
        point: at(site.theta),
    }];
    if layout.level > 0 {
        v.push(Command::UpgradeCommandCenter {
            pin_id: cc.clone(),
            level: layout.level,
        });
    }
    v.push(Command::CreatePin {
        pin_id: temp(ECU),
        type_id: t.ecu_type,
        point: at(site.theta + PIN_SPACING),
    });
    v.push(Command::CreatePin {
        pin_id: temp(FACTORY),
        type_id: t.factory_type,
        point: at(site.theta - PIN_SPACING),
    });
    v.push(Command::CreateLink {
        endpoint1: temp(ECU),
        endpoint2: temp(FACTORY),
        level: 0,
    });
    v.push(Command::CreateLink {
        endpoint1: temp(FACTORY),
        endpoint2: cc,
        level: 0,
    });
    for h in &layout.heads {
        v.push(Command::AddExtractorHead {
            pin_id: temp(ECU),
            head_id: h.head_id,
            point: h.point(),
        });
    }
    v.push(Command::SetSchematic {
        pin_id: temp(FACTORY),
        schematic_id: t.schematic_id,
    });
    v.push(Command::InstallProgram {
        pin_id: temp(ECU),
        program_type: Some(t.resource),
        head_radius: layout.head_radius,
    });
    v
}

/// Apply `commands` to an empty copy of the colonies, as the server will:
/// the colony it makes, or the client's refusal.
pub fn dry_run(
    statics: &Statics,
    site: &Site,
    commands: &[Command],
    quote: i64,
) -> std::result::Result<(Colony, sim::ApplyReport), sim::PiError> {
    let mut colonies = Colonies::new();
    let report = colonies.apply(
        statics,
        &ColonyEdit {
            planet_id: site.planet_id,
            owner_id: site.owner_id,
            planet_type_id: site.planet_type,
            planet_radius: site.radius,
            now: eve_server::pyutil::filetime_now(),
            link_type_id: LINK_TYPE,
            quote: &Fixed(quote),
        },
        commands,
    )?;
    let colony = colonies
        .get(site.planet_id, site.owner_id)
        .cloned()
        .ok_or(sim::PiError::CannotManagePlanetWithoutCommandCenter)?;
    Ok((colony, report))
}

/// **Fit the template**: the highest command-centre level up to
/// `max_level` and the most heads (ten down to one) whose stream the
/// colony code accepts (power, CPU, the planet's pins) and whose bill fits
/// `budget`. `None`: not even one head at level 0.
pub fn fit(
    statics: &Statics,
    t: &Template,
    cc_item: i64,
    site: &Site,
    max_level: i64,
    budget: f64,
) -> Option<Layout> {
    let head_radius = radius_for_hours(PROGRAM_HOURS);
    let influence = statics
        .pin_type(t.ecu_type)
        .map_or(0.0, |p| p.attr(sim::attr::ECU_AREA_OF_INFLUENCE));
    let pins = statics.base_price(t.ecu_type) + statics.base_price(t.factory_type);
    for level in (0..=max_level.clamp(0, sim::MAX_COMMAND_CENTRE_LEVEL)).rev() {
        let cost = pins + sim::command_centre_upgrade_cost(0, level).unwrap_or(0) as f64;
        if cost > budget {
            continue;
        }
        for n in (1..=sim::ECU_MAX_HEADS).rev() {
            let layout = Layout {
                level,
                heads: ring(at(site.theta + PIN_SPACING), n, head_radius, influence),
                head_radius,
                cost,
            };
            if dry_run(statics, site, &base_stream(t, cc_item, site, &layout), 1).is_ok() {
                return Some(layout);
            }
        }
    }
    None
}

/// **The whole stream**: [`base_stream`] and the two routes, extraction
/// unit -> factory (the unit's peak output at the server's `quote`, what
/// `ValidateCreateRoute` caps a route at [V]) and factory -> command
/// centre (one cycle's product), checked by a last dry run.
pub fn with_routes(
    statics: &Statics,
    t: &Template,
    cc_item: i64,
    site: &Site,
    layout: &Layout,
    quote: i64,
) -> std::result::Result<Vec<Command>, sim::PiError> {
    let mut v = base_stream(t, cc_item, site, layout);
    let (colony, report) = dry_run(statics, site, &v, quote)?;
    let real = |id: Id| {
        report
            .id_map
            .iter()
            .find(|(t, _)| *t == id)
            .map(|(_, r)| *r)
    };
    let out_of = |pin: Option<i64>, ty: i64| {
        pin.and_then(|p| colony.pin(p))
            .and_then(|p| p.product_max_output(statics).get(&ty).copied())
            .unwrap_or(0)
    };
    let mined = out_of(real(temp(ECU)), t.resource);
    let made = out_of(real(temp(FACTORY)), t.product);
    v.push(Command::CreateRoute {
        route_id: temp((2, 1)),
        path: vec![temp(ECU), temp(FACTORY)],
        type_id: t.resource,
        quantity: mined,
    });
    v.push(Command::CreateRoute {
        route_id: temp((2, 2)),
        path: vec![temp(FACTORY), Id::Real(cc_item)],
        type_id: t.product,
        quantity: made,
    });
    dry_run(statics, site, &v, quote)?;
    Ok(v)
}

// ---- the wire shape -------------------------------------------------------------

pub(crate) fn id_value(id: &Id) -> PyValue {
    match id {
        Id::Real(n) => PyValue::Int(*n),
        Id::Temporary(parts) => PyValue::Tuple(parts.iter().map(|n| PyValue::Int(*n)).collect()),
    }
}

/// **`CommandStream.Serialize`** [V]: one `(commandID, argsTuple)` per
/// command, the argument order `docs/lanes/planet.md` §3.2 tabulates.
pub fn encode(commands: &[Command]) -> PyValue {
    let p = |pt: &SurfacePoint| [PyValue::Real(pt.latitude), PyValue::Real(pt.longitude)];
    PyValue::List(
        commands
            .iter()
            .map(|c| {
                let args: Vec<PyValue> = match c {
                    Command::CreatePin {
                        pin_id,
                        type_id,
                        point,
                    } => [
                        vec![id_value(pin_id), PyValue::Int(*type_id)],
                        p(point).to_vec(),
                    ]
                    .concat(),
                    Command::RemovePin { pin_id } => vec![id_value(pin_id)],
                    Command::CreateLink {
                        endpoint1,
                        endpoint2,
                        level,
                    }
                    | Command::SetLinkLevel {
                        endpoint1,
                        endpoint2,
                        level,
                    } => vec![
                        id_value(endpoint1),
                        id_value(endpoint2),
                        PyValue::Int(*level),
                    ],
                    Command::RemoveLink {
                        endpoint1,
                        endpoint2,
                    } => vec![id_value(endpoint1), id_value(endpoint2)],
                    Command::CreateRoute {
                        route_id,
                        path,
                        type_id,
                        quantity,
                    } => vec![
                        id_value(route_id),
                        PyValue::List(path.iter().map(id_value).collect()),
                        PyValue::Int(*type_id),
                        PyValue::Int(*quantity),
                    ],
                    Command::RemoveRoute { route_id } => vec![id_value(route_id)],
                    Command::SetSchematic {
                        pin_id,
                        schematic_id,
                    } => vec![id_value(pin_id), PyValue::Int(*schematic_id)],
                    Command::UpgradeCommandCenter { pin_id, level } => {
                        vec![id_value(pin_id), PyValue::Int(*level)]
                    }
                    Command::AddExtractorHead {
                        pin_id,
                        head_id,
                        point,
                    }
                    | Command::MoveExtractorHead {
                        pin_id,
                        head_id,
                        point,
                    } => [
                        vec![id_value(pin_id), PyValue::Int(*head_id)],
                        p(point).to_vec(),
                    ]
                    .concat(),
                    Command::KillExtractorHead { pin_id, head_id } => {
                        vec![id_value(pin_id), PyValue::Int(*head_id)]
                    }
                    Command::InstallProgram {
                        pin_id,
                        program_type,
                        head_radius,
                    } => vec![
                        id_value(pin_id),
                        program_type.map_or(PyValue::None, PyValue::Int),
                        PyValue::Real(*head_radius),
                    ],
                };
                PyValue::Tuple(vec![PyValue::Int(c.opcode()), PyValue::Tuple(args)])
            })
            .collect(),
    )
}

// ---- the sale, pure ---------------------------------------------------------------

/// What to do with one commodity stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sale {
    /// Into the best bid, at its price.
    Bid(SellLine),
    /// Listed for [`crate::escort::LIST_DAYS`] at the seller's ask.
    List(SellLine),
    /// No bid at the floor and no reference price: kept.
    Keep { type_id: i64, quantity: i64 },
}

/// **The farmer's sale** of the commodities in `stacks` at `station`: into
/// the best bid at or above the seller's floor ([`crate::builder::
/// sell_limit`]: the NPC floor, else the clamp's low end on `P_ref`, else
/// any bid); else listed at [`pricing::ask`] on a `P_ref`; else kept.
pub fn sale_plan(
    stacks: &[ItemRow],
    books: &BTreeMap<i64, Book>,
    p_refs: &BTreeMap<i64, f64>,
    station: i64,
) -> Vec<Sale> {
    stacks
        .iter()
        .filter(|r| is_commodity_row(r) && r.quantity > 0)
        .map(|r| {
            let line = |price: f64| SellLine {
                station_id: station,
                type_id: r.type_id,
                item_id: r.item_id,
                price,
                quantity: r.quantity,
            };
            let p_ref = p_refs.get(&r.type_id).copied();
            let empty = Book {
                asks: Vec::new(),
                bids: Vec::new(),
            };
            let book = books.get(&r.type_id).unwrap_or(&empty);
            let floor = crate::builder::sell_limit(p_ref, book).unwrap_or(pricing::TICK);
            if let Some(b) = book.best_bid().filter(|b| b.price >= floor) {
                return Sale::Bid(line(b.price));
            }
            match p_ref.and_then(|p| pricing::ask(&Quote::at_ref(p))) {
                Some(ask) => Sale::List(line(ask)),
                None => Sale::Keep {
                    type_id: r.type_id,
                    quantity: r.quantity,
                },
            }
        })
        .collect()
}

// ---- the run ----------------------------------------------------------------------

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tend {
    /// Set when the run could do nothing: what it lacked.
    pub missing: Option<String>,
    /// Colonies tended.
    pub colonies: usize,
    /// Extractors restarted.
    pub restarted: usize,
    /// Commodity units launched to the hangar.
    pub launched: i64,
    /// The sale.
    pub sales: Vec<Sale>,
    /// Planets the overview showed, when this run looked.
    pub surveyed: Option<usize>,
    /// The planet a colony was founded on, and its layout (level, heads
    /// per extraction unit).
    pub founded: Option<(i64, i64, usize)>,
    /// The P2 the founded colony makes, when it is a P2 colony (FU-PI2).
    pub p2: Option<i64>,
    /// The P3 or P4 the founded colony makes (X-PI3).
    pub p3: Option<i64>,
    /// Units imported onto colonies' launchpads (FU-PI2, X-PI3).
    pub fed: i64,
    /// Units bought for a P3 colony's launchpad (X-PI3).
    pub stocked: i64,
    /// Units of P3 and P4 exported through the customs office (X-PI3).
    pub exported: i64,
    /// Refusals met on the way (each a user-error key or a reason).
    pub refusals: Vec<String>,
    /// Wallet before and after.
    pub before: f64,
    pub after: f64,
}

impl Tend {
    /// The act line's note.
    pub fn summary(&self) -> String {
        if let Some(m) = &self.missing {
            return format!("MISSING {m}");
        }
        let (bids, lists, kept) = self.sales.iter().fold((0, 0, 0), |(b, l, k), s| match s {
            Sale::Bid(_) => (b + 1, l, k),
            Sale::List(_) => (b, l + 1, k),
            Sale::Keep { .. } => (b, l, k + 1),
        });
        let mut s = format!(
            "{} colonies, {} restarted, {} units launched; sold {bids} into bids, listed {lists}, kept {kept}",
            self.colonies, self.restarted, self.launched
        );
        if let Some(n) = self.surveyed {
            s.push_str(&format!("; surveyed {n} planets"));
        }
        if let Some((planet, level, heads)) = self.founded {
            s.push_str(&format!(
                "; founded a colony on {planet} (level {level}, {heads} heads)"
            ));
        }
        if let Some(p2) = self.p2 {
            s.push_str(&format!("; it makes P2 {p2}"));
        }
        if let Some(p3) = self.p3 {
            s.push_str(&format!("; it makes P3/P4 {p3}"));
        }
        if self.stocked > 0 {
            s.push_str(&format!("; bought {} units of inputs", self.stocked));
        }
        if self.fed > 0 {
            s.push_str(&format!("; imported {} units", self.fed));
        }
        if self.exported > 0 {
            s.push_str(&format!("; exported {} units", self.exported));
        }
        if !self.refusals.is_empty() {
            s.push_str(&format!("; refused {:?}", self.refusals));
        }
        s.push_str(&format!(
            "; wallet {:.2} -> {:.2} ISK",
            self.before, self.after
        ));
        s
    }
}

/// **One visit to the planet window** (module header), docked at home to
/// start and to end. `content` holds the PI static table; `name` is the
/// bot's ([`Note`]) and `seed` its spread.
pub fn run(c: &mut Conn, content: Option<&Content>, name: &str, seed: u64) -> Result<Tend> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("pi: not docked".to_owned()))?;
    let mut tend = Tend::default();
    let Some(statics) = content
        .and_then(|c| c.try_get::<sim::Installed>())
        .map(sim::Installed::statics)
        .filter(|s| !s.is_empty())
    else {
        tend.missing = Some(MISSING_STATICS.to_owned());
        return Ok(tend);
    };
    tend.before = c.cash_balance()?;
    let mut note = note_of(name);

    // 1. Tend.
    let colonies = colonies(c)?;
    tend.colonies = colonies.len();
    let mut views = Vec::new();
    for &planet in &colonies {
        let Some(view) = planet_info(c, planet)?.as_ref().and_then(read_colony) else {
            continue;
        };
        let restart = restarts(&view, statics);
        if !restart.is_empty() {
            match submit(c, planet, &restart)? {
                Ok(_) => tend.restarted += restart.len(),
                Err(why) => tend.refusals.push(why),
            }
        }
        if let Some((pin, goods)) = launch_of(&view, statics) {
            match launch(c, planet, pin, &goods)? {
                Ok(()) => tend.launched += goods.values().sum::<i64>(),
                Err(why) => tend.refusals.push(why),
            }
        }
        views.push(view);
    }

    // 1b. The colonies' trade (FU-PI2, X-PI3): the hangar's P1 (P2, P3) to
    // the customs office and onto the launchpads, what the hangar lacks of
    // a P3 colony's inputs bought first, and the P3 and P4 products out
    // through the office. What a colony imports is not sold.
    let (keep, trade) = crate::pi_p3::tend(c, statics, home, &views)?;
    tend.fed = trade.fed;
    tend.stocked = trade.bought;
    tend.exported = trade.exported;
    tend.refusals.extend(trade.refusals);

    // 2. Sell.
    tend.sales = sell(c, home, &keep)?;

    // 3. Grow.
    let skills = known_skills(c)?;
    let allowed = (skills
        .get(&SKILL_INTERPLANETARY_CONSOLIDATION)
        .copied()
        .unwrap_or(0)
        + 1)
    .clamp(1, MAX_COLONIES as i64) as usize;
    if colonies.len() < allowed
        && let Some(system) = c.session().solar_system_id2()
    {
        match planets_seen(system) {
            None => {
                let seen = survey(c, home, statics)?;
                tend.surveyed = Some(seen.len());
                remember_planets(system, seen);
            }
            Some(planets) => {
                let ours: BTreeSet<i64> = colonies.iter().copied().collect();
                let max_level = skills
                    .get(&SKILL_COMMAND_CENTER_UPGRADES)
                    .copied()
                    .unwrap_or(0)
                    .min(MAX_LEVEL);
                let made = crate::pi_p2::made_by(&views, statics);
                let goods = crate::pi_p3::goods_of(&views, statics);
                let place = Settle {
                    planets: &planets,
                    ours: &ours,
                    made: &made,
                    goods: &goods,
                    max_level,
                    home,
                    seed,
                };
                found(c, statics, &place, &mut note, &mut tend)?;
            }
        }
    }
    set_note(name, note);
    tend.after = c.cash_balance()?;
    Ok(tend)
}

/// Where and how a [`found`] may settle.
struct Settle<'a> {
    planets: &'a [(i64, i64)],
    ours: &'a BTreeSet<i64>,
    /// The P1 the bot's colonies make (a P2 colony's imports).
    made: &'a BTreeSet<i64>,
    /// Everything the bot's colonies make (a P3 colony's inputs, X-PI3).
    goods: &'a BTreeSet<i64>,
    max_level: i64,
    home: i64,
    seed: u64,
}

/// What [`found`] builds: B6's P1 colony, or FU-PI2's P2 colony.
enum Plan {
    P1(Template),
    P2(crate::pi_p2::P2Template),
    P3(crate::pi_p3::P3Template),
}

impl Plan {
    fn cc_type(&self) -> i64 {
        match self {
            Plan::P1(t) => t.cc_type,
            Plan::P2(t) => t.cc_type,
            Plan::P3(t) => t.cc_type,
        }
    }

    fn planet_type(&self) -> i64 {
        match self {
            Plan::P1(t) => t.planet_type,
            Plan::P2(t) => t.planet_type,
            Plan::P3(t) => t.planet_type,
        }
    }
}

/// **The colony to found**: a P2 colony when one is reachable and the
/// market does not price it under its inputs ([`crate::pi_p2::choose`],
/// [`crate::pi_p2::P2_FIRST`]), else B6's [`pick_planet`].
fn plan(
    c: &mut Conn,
    statics: &Statics,
    place: &Settle<'_>,
    skip: &BTreeSet<i64>,
) -> Result<Option<(i64, Plan)>> {
    use crate::pi_p2;
    if let Some((p, t)) = crate::pi_p3::pick(
        c,
        statics,
        place.planets,
        place.ours,
        skip,
        place.goods,
        place.seed,
    )? {
        return Ok(Some((p, Plan::P3(t))));
    }
    if pi_p2::P2_FIRST {
        let cands = pi_p2::candidates(statics, place.planets, place.ours, skip, place.made);
        if !cands.is_empty() {
            let prices = match pi_p2::prices_of(c, &pi_p2::priced_types(statics, &cands)) {
                Ok(p) => p,
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => BTreeMap::new(),
            };
            if let Some((p, t)) = pi_p2::choose(statics, cands, &prices, place.seed) {
                return Ok(Some((p, Plan::P2(t))));
            }
        }
    }
    Ok(
        pick_planet(statics, place.planets, place.ours, skip, place.seed)
            .map(|(p, t)| (p, Plan::P1(t))),
    )
}

/// Pick a planet and a plan, get its command centre into the hold, undock,
/// submit the template, dock.
fn found(
    c: &mut Conn,
    statics: &Statics,
    place: &Settle<'_>,
    note: &mut Note,
    tend: &mut Tend,
) -> Result<()> {
    let (home, seed, max_level) = (place.home, place.seed, place.max_level);
    let Some((planet, plan)) = plan(c, statics, place, &note.no_centre)? else {
        return Ok(());
    };
    let cc_item = match centre_in_hold(c, home, plan.cc_type())? {
        Hold::Ready(id) => {
            note.lacks_hull = false;
            id
        }
        Hold::NoAsk => {
            note.no_centre.insert(plan.planet_type());
            tend.refusals.push(format!(
                "no command centre {} for sale here",
                plan.cc_type()
            ));
            return Ok(());
        }
        Hold::NoRoom(why) => {
            note.lacks_hull = true;
            tend.refusals.push(format!("{MISSING_HULL}: {why}"));
            return Ok(());
        }
    };
    let owner = c.session().char_id().unwrap_or(0);
    let budget = c.cash_balance()? * WALLET_SHARE;

    leave(c)?;
    let built = (|| -> Result<std::result::Result<(i64, usize), String>> {
        let t = match &plan {
            Plan::P2(t) => {
                return crate::pi_p2::found(
                    c, statics, planet, t, place.made, cc_item, owner, budget, max_level, seed,
                );
            }
            Plan::P3(t) => {
                return crate::pi_p3::found(
                    c, statics, planet, t, cc_item, owner, budget, max_level, seed,
                );
            }
            Plan::P1(t) => *t,
        };
        let info = planet_info(c, planet)?.unwrap_or(PyValue::None);
        let planet_type = int(kv(&info, "planetTypeID")).unwrap_or(t.planet_type);
        let radius = real(kv(&info, "radius")).unwrap_or(0.0);
        // The survey window's reading picks the resource; the bot's table
        // only ranked the planets.
        let t = match template(statics, planet_type, &resource_info(c, planet)?) {
            Some(read) if read.cc_type == t.cc_type => read,
            _ => t,
        };
        let site = Site {
            planet_id: planet,
            owner_id: owner,
            planet_type,
            radius,
            theta: (seed % 628) as f64 / 100.0,
        };
        let Some(layout) = fit(statics, &t, cc_item, &site, max_level, budget) else {
            return Ok(Err(
                "no layout fits the power, the CPU and the wallet".to_owned()
            ));
        };
        let quote = program_quote(c, planet, t.resource, &layout)?;
        if quote <= 0 {
            return Ok(Err(format!("the survey quotes nothing for {}", t.resource)));
        }
        let stream = match with_routes(statics, &t, cc_item, &site, &layout, quote) {
            Ok(s) => s,
            Err(e) => return Ok(Err(format!("the routes: {}", e.message_id()))),
        };
        Ok(submit(c, planet, &stream)?.map(|_| (layout.level, layout.heads.len())))
    })();
    fly_home(c, home)?;
    match built? {
        Ok((level, heads)) => {
            tend.founded = Some((planet, level, heads));
            match &plan {
                Plan::P2(t) => tend.p2 = Some(t.product),
                Plan::P3(t) => tend.p3 = Some(t.product),
                Plan::P1(_) => {}
            }
        }
        Err(why) => tend.refusals.push(why),
    }
    Ok(())
}

// ---- over the wire ------------------------------------------------------------------

/// `planetMgr.GetPlanetsForChar()` [V]: the planets we have colonies on.
fn colonies(c: &mut Conn) -> Result<Vec<i64>> {
    let outcome = c.call("planetMgr", "GetPlanetsForChar", vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("GetPlanetsForChar: {key}")));
    }
    let v = outcome.value();
    Ok(seq(v.as_ref())
        .iter()
        .filter_map(|r| int(kv(r, "planetID")))
        .collect())
}

/// A call on the planet's moniker, `Moniker('planetMgr', planetID)` [V].
pub(crate) fn on_planet(
    c: &mut Conn,
    planet: i64,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::Answer> {
    c.moniker_call("planetMgr", PyValue::Int(planet), method, args, vec![])
}

/// `GetPlanetInfo()` on the planet [V]: its facts, and our colony's.
pub(crate) fn planet_info(c: &mut Conn, planet: i64) -> Result<Option<PyValue>> {
    match on_planet(c, planet, "GetPlanetInfo", vec![]) {
        Ok(a) => Ok(a.value),
        Err(Error::Refused(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GetPlanetResourceInfo()` [V]: `{resourceTypeID: quality}`.
pub(crate) fn resource_info(c: &mut Conn, planet: i64) -> Result<BTreeMap<i64, f64>> {
    let a = on_planet(c, planet, "GetPlanetResourceInfo", vec![])?;
    let Some(PyValue::Dict(entries)) = a.value else {
        return Ok(BTreeMap::new());
    };
    Ok(entries
        .iter()
        .filter_map(|(q, t)| Some((int(Some(t))?, real(Some(q))?)))
        .collect())
}

/// `GetProgramResultInfo(pinID, typeID, heads, headRadius)` [V]: the
/// survey window's quote, `qtyPerCycle` of its three.
fn program_quote(c: &mut Conn, planet: i64, resource: i64, layout: &Layout) -> Result<i64> {
    let heads = PyValue::List(
        layout
            .heads
            .iter()
            .map(|h| {
                PyValue::Tuple(vec![
                    PyValue::Int(h.head_id),
                    PyValue::Real(h.latitude),
                    PyValue::Real(h.longitude),
                ])
            })
            .collect(),
    );
    let a = on_planet(
        c,
        planet,
        "GetProgramResultInfo",
        vec![
            id_value(&temp(ECU)),
            PyValue::Int(resource),
            heads,
            PyValue::Real(layout.head_radius),
        ],
    )?;
    Ok(int(seq(a.value.as_ref()).first()).unwrap_or(0))
}

/// `UserUpdateNetwork(serializedChanges)` [V]: `Ok(Ok(colony))`, or the
/// refusal's key.
pub(crate) fn submit(
    c: &mut Conn,
    planet: i64,
    commands: &[Command],
) -> Result<std::result::Result<PyValue, String>> {
    match on_planet(c, planet, "UserUpdateNetwork", vec![encode(commands)]) {
        Ok(a) => match (a.refusal(), a.value) {
            (Some(key), _) => Ok(Err(key)),
            (None, Some(v)) => Ok(Ok(v)),
            (None, None) => Ok(Err("UserUpdateNetwork: no answer".to_owned())),
        },
        Err(Error::Refused(why)) => Ok(Err(why)),
        Err(e) => Err(e),
    }
}

/// `UserLaunchCommodities(commandPinID, {typeID: qty})` [V].
fn launch(
    c: &mut Conn,
    planet: i64,
    pin: i64,
    goods: &BTreeMap<i64, i64>,
) -> Result<std::result::Result<(), String>> {
    let dict = PyValue::Dict(
        goods
            .iter()
            .map(|(t, q)| (PyValue::Int(*q), PyValue::Int(*t)))
            .collect(),
    );
    match on_planet(
        c,
        planet,
        "UserLaunchCommodities",
        vec![PyValue::Int(pin), dict],
    ) {
        Ok(a) => Ok(a.refusal().map_or(Ok(()), Err)),
        Err(Error::Refused(why)) => Ok(Err(why)),
        Err(e) => Err(e),
    }
}

/// The value under `name` anywhere in `v` (a skill entry's attributes).
fn field<'a>(v: &'a PyValue, name: &str, depth: usize) -> Option<&'a PyValue> {
    if depth > 6 {
        return None;
    }
    if let Some(x) = kv(v, name) {
        return Some(x);
    }
    match v {
        PyValue::Dict(e) => e.iter().find_map(|(x, _)| field(x, name, depth + 1)),
        PyValue::Tuple(items) | PyValue::List(items) => {
            items.iter().find_map(|i| field(i, name, depth + 1))
        }
        PyValue::Instance { state, .. } => field(state, name, depth + 1),
        PyValue::ObjectEx { header, .. } => field(header, name, depth + 1),
        _ => None,
    }
}

/// `skillMgr2.GetMySkillHandler().GetSkills()` [V]: trained levels.
fn known_skills(c: &mut Conn) -> Result<BTreeMap<i64, i64>> {
    let handler = c
        .call("skillMgr2", "GetMySkillHandler", vec![])?
        .value()
        .as_ref()
        .and_then(|v| crate::object_refs(v).into_iter().next());
    let Some(handler): Option<ObjectRef> = handler else {
        return Ok(BTreeMap::new());
    };
    let Some(PyValue::Dict(entries)) = c.call_on(&handler, "GetSkills", vec![], vec![])?.value()
    else {
        return Ok(BTreeMap::new());
    };
    Ok(entries
        .iter()
        .filter_map(|(entry, key)| {
            let level = int(field(entry, "_trainedSkillLevel", 0)).unwrap_or(0);
            Some((int(Some(key))?, level))
        })
        .collect())
}

/// Sell the hangar's commodities by [`sale_plan`], all but the `keep`
/// types (a P2 colony's imports).
fn sell(c: &mut Conn, station: i64, keep: &BTreeSet<i64>) -> Result<Vec<Sale>> {
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| is_commodity_row(r) && !keep.contains(&r.type_id))
        .collect();
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
    for s in &plan {
        match s {
            Sale::Bid(l) => c.sell(&[*l], 0, crate::escort::LIST_BROKER_FEE)?,
            Sale::List(l) => c.sell(
                &[*l],
                crate::escort::LIST_DAYS,
                crate::escort::LIST_BROKER_FEE,
            )?,
            Sale::Keep { .. } => {}
        }
    }
    Ok(plan)
}

/// Where the command centre stands.
enum Hold {
    /// In the active ship's hold, this item.
    Ready(i64),
    /// None here, and nobody sells one at this station.
    NoAsk,
    /// It could not go into the hold.
    NoRoom(String),
}

/// **The command centre of `cc_type` in the hold**: one already there, or
/// one from the hangar, or one bought from the cheapest ask here (at most
/// the consumer's cap, [`crate::acquire::plan`]) and loaded.
fn centre_in_hold(c: &mut Conn, station: i64, cc_type: i64) -> Result<Hold> {
    if let Some(r) = c.cargo()?.iter().find(|r| r.type_id == cc_type) {
        return Ok(Hold::Ready(r.item_id));
    }
    let mut stack = c.hangar_items()?.into_iter().find(|r| r.type_id == cc_type);
    if stack.is_none() {
        let book = match c.market_orders(cc_type) {
            Ok(b) => b,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => return Ok(Hold::NoAsk),
        };
        let mut prices = pricing::Prices::read(c)?;
        let cap = prices.reservation(c, cc_type)?;
        let isk = c.cash_balance()?;
        let Some((ask, k)) = crate::acquire::plan(&book, station, cap, isk, 1)
            .into_iter()
            .next()
        else {
            return Ok(Hold::NoAsk);
        };
        match c.buy(
            station,
            cc_type,
            ask.price,
            k,
            0,
            crate::builder::BROKER_FEE,
        ) {
            Ok(_) => {}
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => return Ok(Hold::NoAsk),
        }
        stack = c.hangar_items()?.into_iter().find(|r| r.type_id == cc_type);
    }
    let Some(stack) = stack else {
        return Ok(Hold::NoAsk);
    };
    match c.load_cargo(stack.item_id, Some(1)) {
        Ok(_) => {}
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => return Ok(Hold::NoRoom(e.to_string())),
    }
    Ok(match c.cargo()?.iter().find(|r| r.type_id == cc_type) {
        Some(r) => Hold::Ready(r.item_id),
        None => Hold::NoRoom("loaded, not in the hold".to_owned()),
    })
}

/// **Look at the system's planets** on the overview: undock, read every
/// ball whose type is a planet type the table knows, dock.
pub fn survey(c: &mut Conn, home: i64, statics: &Statics) -> Result<Vec<(i64, i64)>> {
    leave(c)?;
    let seen: Vec<(i64, i64)> = c
        .world_now()
        .balls()
        .filter_map(|b| Some((b.ball_id, b.type_id()?)))
        .filter(|(_, t)| statics.resources(*t).is_some())
        .collect();
    fly_home(c, home)?;
    Ok(seen)
}

/// Undock and bind the system's park.
pub(crate) fn leave(c: &mut Conn) -> Result<()> {
    if c.session().station_id().is_some() {
        c.undock_repaired()?;
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("pi: in space with no system".to_owned()))?;
    if c.ballpark().is_none() {
        c.bind_ballpark(system)?;
    }
    settle(c, |c| c.world().has_state())
}

/// Back to the home station and docked (the courier's moves).
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
                    "pi: home station {home} is not in this system's park"
                )));
            }
        }
    }
    Err(Error::Refused(format!("pi: not docked at {home}")))
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "pi: still moving after the travel ceiling".into(),
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

    // Invented ids and rows only.
    const STATION: i64 = 61_000_001;
    const SHIP: i64 = 100_000_301;
    const SPARE: i64 = 100_000_302;
    const STACK: i64 = 100_000_303;
    const PT: i64 = 990_660_001;
    const OTHER_PT: i64 = 990_660_002;
    const CC: i64 = 990_660_011;
    const ECU_T: i64 = 990_660_012;
    const FACTORY_T: i64 = 990_660_015;
    const RES: i64 = 990_660_021;
    const RES2: i64 = 990_660_024;
    const PROD: i64 = 990_660_022;
    const SCHEMATIC: i64 = 99_066;
    const PLANET: i64 = 40_966_001;
    const PLANET_B: i64 = 40_966_002;
    const OWNER: i64 = 90_000_601;
    const CC_ITEM: i64 = 100_000_311;

    fn statics() -> Statics {
        Statics::parse(&format!(
            "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0;1632={PT}.0\n\
             pintype\t{ECU_T}\t1063\t0.0\t45000.0\t0\t49=400.0;15=2600.0;1690=110.0;1691=550.0;1689=0.3;1632={PT}.0\n\
             pintype\t{FACTORY_T}\t1028\t0.0\t75000.0\t0\t49=200.0;15=800.0;1632={PT}.0\n\
             pintype\t{LINK_TYPE}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
             schematic\t{SCHEMATIC}\t1800\t{FACTORY_T}\t{RES}=3000\t{PROD}=20\n\
             resource\t{PT}\t{RES}=0.5;{RES2}=0.9\n\
             itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture PI Centre\n\
             itemtype\t{RES}\t1032\t42\t0.005\t0.0\tFixture PI Resource\n\
             itemtype\t{PROD}\t1042\t43\t0.19\t0.0\tFixture PI Product\n"
        ))
        .expect("the invented rows parse")
    }

    fn site() -> Site {
        Site {
            planet_id: PLANET,
            owner_id: OWNER,
            planet_type: PT,
            radius: 3_000_000.0,
            theta: 1.0,
        }
    }

    fn row(item_id: i64, type_id: i64, group_id: i64, category_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: OWNER,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity,
            group_id,
            category_id,
            singleton: category_id == crate::courier::CATEGORY_SHIP,
        }
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

    fn docked_obs(ship_type: Option<i64>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: ship_type,
        });
        o
    }

    #[test]
    fn the_pi_row_is_civilian_and_its_kit_holds_both_skills() {
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "pi");
        assert_eq!(PI_KIT.hull, Some(BADGER));
        for s in [
            SKILL_COMMAND_CENTER_UPGRADES,
            SKILL_INTERPLANETARY_CONSOLIDATION,
        ] {
            assert!(PI_KIT.skills.iter().any(|(k, _)| *k == s));
        }
        let lacks = |ship| Lacks {
            ship,
            module: false,
            ammo: false,
            isk: false,
        };
        assert_eq!(setup(lacks(false)).len(), PI_KIT.skills.len());
        assert!(
            setup(lacks(true))
                .iter()
                .any(|l| l.contains(&BADGER.to_string()))
        );
    }

    /// Docked: one visit and a rest; after a run that lacked a hull, a
    /// Badger in the hangar is boarded, else asked for (GM here).
    #[test]
    fn docked_tends_and_asks_for_a_hull_only_when_a_build_lacked_one() {
        let name = "pi-a";
        let mut m = memory(name);
        assert_eq!(docked(&mut m, &docked_obs(None)).first(), Some(&Act::Pi));
        set_note(
            name,
            Note {
                lacks_hull: true,
                ..Note::default()
            },
        );
        let mut o = docked_obs(Some(brain::MERLIN));
        o.hangar = vec![row(SPARE, BADGER, 28, crate::courier::CATEGORY_SHIP, 1)];
        assert_eq!(docked(&mut m, &o), vec![Act::Board(SPARE)]);
        assert_eq!(
            docked(&mut m, &docked_obs(Some(brain::MERLIN))),
            vec![Act::GmSetup {
                ship: true,
                module: false,
                ammo: false
            }]
        );
        assert_eq!(
            docked(&mut m, &docked_obs(Some(BADGER))).first(),
            Some(&Act::Pi)
        );
    }

    /// The template takes the best resource a factory can use: the better
    /// one here has no schematic, so the other is farmed.
    #[test]
    fn the_template_farms_the_best_resource_a_factory_takes() {
        let s = statics();
        let t = template(&s, PT, s.resources(PT).unwrap()).expect("a template");
        assert_eq!(
            (t.cc_type, t.ecu_type, t.factory_type),
            (CC, ECU_T, FACTORY_T)
        );
        assert_eq!(
            (t.resource, t.product, t.schematic_id),
            (RES, PROD, SCHEMATIC)
        );
        assert!(
            template(&s, OTHER_PT, s.resources(PT).unwrap()).is_none(),
            "wrong planet"
        );
        assert!(template(&s, PT, &BTreeMap::from([(RES2, 1.0)])).is_none());
    }

    #[test]
    fn a_planet_is_picked_by_seed_among_the_farmable_ones_not_ours() {
        let s = statics();
        let planets = [(PLANET, PT), (PLANET_B, PT), (PLANET + 9, OTHER_PT)];
        let none = BTreeSet::new();
        let a = pick_planet(&s, &planets, &none, &none, 0).unwrap().0;
        let b = pick_planet(&s, &planets, &none, &none, 1).unwrap().0;
        assert_ne!(a, b, "two seeds, two planets");
        let ours = BTreeSet::from([PLANET]);
        assert_eq!(
            pick_planet(&s, &planets, &ours, &none, 0).unwrap().0,
            PLANET_B
        );
        assert!(pick_planet(&s, &planets, &none, &BTreeSet::from([PT]), 0).is_none());
    }

    /// The fit is the most heads the level-0 centre carries; the colony
    /// code itself refuses one more.
    #[test]
    fn the_fit_is_the_most_heads_the_power_carries_and_one_more_is_refused() {
        let s = statics();
        let t = template(&s, PT, s.resources(PT).unwrap()).unwrap();
        let layout = fit(&s, &t, CC_ITEM, &site(), 0, f64::INFINITY).expect("a layout");
        assert_eq!(layout.level, 0);
        let n = layout.heads.len();
        assert!((1..sim::ECU_MAX_HEADS).contains(&n), "{n} heads");
        let more = Layout {
            heads: ring(
                at(site().theta + PIN_SPACING),
                n + 1,
                layout.head_radius,
                0.3,
            ),
            ..layout.clone()
        };
        let err = dry_run(&s, &site(), &base_stream(&t, CC_ITEM, &site(), &more), 1).unwrap_err();
        assert!(
            matches!(
                err,
                sim::PiError::CannotAddToColonyPowerUsageExceeded
                    | sim::PiError::CannotAddToColonyCpuUsageExceeded
            ),
            "{err:?}"
        );
        assert!(fit(&s, &t, CC_ITEM, &site(), 0, 1.0).is_none(), "the bill");
    }

    /// The routes carry the unit's peak and one factory cycle, the server's
    /// own decoder reads the stream back, and the program runs the days
    /// asked for.
    #[test]
    fn the_stream_routes_extractor_to_factory_to_centre_and_round_trips() {
        let s = statics();
        let t = template(&s, PT, s.resources(PT).unwrap()).unwrap();
        let layout = fit(&s, &t, CC_ITEM, &site(), 0, f64::INFINITY).unwrap();
        let stream = with_routes(&s, &t, CC_ITEM, &site(), &layout, 500).expect("routes fit");
        let routes: Vec<(i64, i64)> = stream
            .iter()
            .filter_map(|c| match c {
                Command::CreateRoute {
                    type_id, quantity, ..
                } => Some((*type_id, *quantity)),
                _ => None,
            })
            .collect();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].0, RES);
        assert!(routes[0].1 >= 500, "the peak, not the mean: {routes:?}");
        assert_eq!(routes[1], (PROD, 20));
        let (colony, _) = dry_run(&s, &site(), &stream, 500).unwrap();
        assert_eq!(colony.routes.len(), 2);
        let length = sim::program_length_from_head_radius(layout.head_radius);
        assert!((length - PROGRAM_HOURS).abs() < 1e-6);
        // The wire shape: (opcode, args) pairs, one per command.
        let PyValue::List(wire) = encode(&stream) else {
            panic!("a list")
        };
        assert_eq!(wire.len(), stream.len());
        for (w, c) in wire.iter().zip(&stream) {
            let PyValue::Tuple(pair) = w else {
                panic!("{w:?}")
            };
            assert_eq!(pair[0], PyValue::Int(c.opcode()));
        }
    }

    fn keyval(pairs: Vec<(&str, PyValue)>) -> PyValue {
        eve_server::pyutil::keyval(pairs)
    }

    fn ecu_pin(expiry: Option<i64>, install: Option<i64>, program: Option<i64>) -> PyValue {
        keyval(vec![
            ("id", PyValue::Int(7)),
            ("typeID", PyValue::Int(ECU_T)),
            ("contents", PyValue::Dict(vec![])),
            ("programType", program.map_or(PyValue::None, PyValue::Int)),
            ("expiryTime", expiry.map_or(PyValue::None, PyValue::Int)),
            ("installTime", install.map_or(PyValue::None, PyValue::Int)),
            ("headRadius", PyValue::Real(0.02)),
            (
                "heads",
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(0),
                    PyValue::Real(1.5),
                    PyValue::Real(1.0),
                ])]),
            ),
        ])
    }

    fn colony(now: i64, pins: Vec<PyValue>) -> ColonyView {
        read_colony(&keyval(vec![
            ("planetID", PyValue::Int(PLANET)),
            ("currentSimTime", PyValue::Int(now)),
            ("pins", PyValue::List(pins)),
        ]))
        .expect("a colony")
    }

    /// An expired program restarts with its own resource and radius; a
    /// running one, or one inside the install cooldown, does not; a
    /// stopped one takes the factory's input.
    #[test]
    fn expired_extractors_restart_and_running_ones_do_not() {
        let s = statics();
        let now = 1_000 * sim::HOUR;
        let cool = sim::PROGRAM_INSTALLATION_COOLDOWN;
        let expired = colony(
            now,
            vec![ecu_pin(Some(now - 1), Some(now - 2 * cool), Some(RES))],
        );
        assert_eq!(
            restarts(&expired, &s),
            vec![Command::InstallProgram {
                pin_id: Id::Real(7),
                program_type: Some(RES),
                head_radius: 0.02
            }]
        );
        let running = colony(
            now,
            vec![ecu_pin(Some(now + 1), Some(now - cool), Some(RES))],
        );
        assert!(restarts(&running, &s).is_empty());
        let cooling = colony(now, vec![ecu_pin(Some(now), Some(now - 1), Some(RES))]);
        assert!(restarts(&cooling, &s).is_empty());
        let factory = keyval(vec![
            ("id", PyValue::Int(8)),
            ("typeID", PyValue::Int(FACTORY_T)),
            ("schematicID", PyValue::Int(SCHEMATIC)),
        ]);
        let stopped = colony(now, vec![ecu_pin(None, None, None), factory]);
        match restarts(&stopped, &s).as_slice() {
            [Command::InstallProgram { program_type, .. }] => assert_eq!(*program_type, Some(RES)),
            other => panic!("{other:?}"),
        }
    }

    /// The centre's commodities are launched, raw resources are not, and
    /// nothing goes inside the launch cooldown.
    #[test]
    fn the_centre_launches_its_commodities_after_the_cooldown() {
        let s = statics();
        let now = 1_000 * sim::HOUR;
        let cc = |last: Option<i64>| {
            keyval(vec![
                ("id", PyValue::Int(5)),
                ("typeID", PyValue::Int(CC)),
                (
                    "contents",
                    PyValue::Dict(vec![
                        (PyValue::Int(40), PyValue::Int(PROD)),
                        (PyValue::Int(900), PyValue::Int(RES)),
                    ]),
                ),
                ("lastLaunchTime", last.map_or(PyValue::None, PyValue::Int)),
            ])
        };
        assert_eq!(
            launch_of(&colony(now, vec![cc(None)]), &s),
            Some((5, BTreeMap::from([(PROD, 40)])))
        );
        let soon = now - sim::LAUNCH_CYCLE_TIME + 1;
        assert_eq!(launch_of(&colony(now, vec![cc(Some(soon))]), &s), None);
    }

    fn book(bid: Option<f64>) -> Book {
        Book {
            asks: Vec::new(),
            bids: bid
                .map(|price| crate::verbs::BookOrder {
                    order_id: 1,
                    type_id: PROD,
                    station_id: STATION,
                    price,
                    vol_remaining: 1_000,
                    bid: true,
                })
                .into_iter()
                .collect(),
        }
    }

    /// A commodity goes into a bid at the floor, else is listed on a
    /// `P_ref`, else kept; a bid under the floor is not taken; a hull is
    /// never offered.
    #[test]
    fn commodities_sell_into_bids_at_the_floor_list_on_a_price_or_are_kept() {
        let stacks = [
            row(STACK, PROD, 1042, CATEGORY_PLANETARY_COMMODITIES, 30),
            row(SPARE, BADGER, 28, crate::courier::CATEGORY_SHIP, 1),
        ];
        let station = STATION;
        let any_bid = sale_plan(
            &stacks,
            &BTreeMap::from([(PROD, book(Some(5.0)))]),
            &BTreeMap::new(),
            station,
        );
        assert!(matches!(any_bid.as_slice(), [Sale::Bid(l)] if l.price == 5.0 && l.quantity == 30));
        let p_ref = 100.0;
        let refs = BTreeMap::from([(PROD, p_ref)]);
        let low = sale_plan(
            &stacks,
            &BTreeMap::from([(PROD, book(Some(1.0)))]),
            &refs,
            station,
        );
        match low.as_slice() {
            [Sale::List(l)] => assert_eq!(Some(l.price), pricing::ask(&Quote::at_ref(p_ref))),
            other => panic!("{other:?}"),
        }
        let kept = sale_plan(&stacks, &BTreeMap::new(), &BTreeMap::new(), station);
        assert_eq!(
            kept,
            vec![Sale::Keep {
                type_id: PROD,
                quantity: 30
            }]
        );
    }

    #[test]
    fn the_radius_for_a_length_inverts_the_clients_formula() {
        for h in [1.0, PROGRAM_HOURS, 336.0] {
            let r = radius_for_hours(h);
            assert!((sim::program_length_from_head_radius(r) - h).abs() < 1e-9);
        }
        assert_eq!(radius_for_hours(1_000.0), sim::RADIUS_DRILLAREAMAX);
    }
}
