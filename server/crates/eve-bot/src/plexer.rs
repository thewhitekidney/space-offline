//! **The FW plexer** (R-SOCIETY §1 row 13, stage B packet SOC-B9): the
//! `fw_plexer` career. A bot enlists in a militia, flies to its warzone,
//! captures the enemy's complexes and keeps clear of pilots it does not
//! trust. Every step is a player verb.
//!
//! 1. **Enlist** ([`ensure_enlisted`]). It asks
//!    `fwCharacterEnlistmentMgr.GetMyEnlistment` [V] (CP53a). An enlisted
//!    pilot keeps its militia. Otherwise it reads the warzones
//!    (`fwWarzoneSolarsystem.GetAllWarzones` [V]) and the cooldown
//!    (`GetMyDirectEnlistmentCooldownTimestamp` [V]). Outside the cooldown it
//!    flies to a station of its militia's faction (the server enlists only
//!    there, FW-4) and enlists directly (`CreateMyDirectEnlistment(factionID)` [V]) in its
//!    faction's militia ([`choose_militia`]). Its rank
//!    (`facWarMgr.GetMyCharacterRankInfo` [V]) goes in the act's note.
//! 2. **Find the front** ([`fronts`]). It reads
//!    `fwWarzoneSolarsystem.GetAllWarzonesOccupationStates` [V], the
//!    server's own warzone list (the client's `warzonespositiondata`
//!    territory, extracted to `facwar/systems.tsv` by FW-3; without the
//!    bundle file it is the CP53b rule, an [A] stand-in). The front is every
//!    system its militia occupies or attacks.
//! 3. **Find a complex** ([`scan`]). In a front system and in space, it
//!    registers with the signal tracker (`scanMgr.SignalTrackerRegister`)
//!    and reads the next `OnSignalTrackerFullState` push [V]. That is the
//!    sensor overlay a player sees. The Agency's
//!    `GetFactionWarfareInstances` answers `{}` on this server, so the
//!    overlay is the only list. A complex is an anomaly of archetype 33 to
//!    36 ([`COMPLEX_ARCHETYPES`] [V]). It is **worth running** when the
//!    enemy militia owns it ([`worth`]). R-FW §6 [I]: "completing it is a
//!    capture *against* the owner". In the enemy's system that raises the
//!    score toward the flip. In the bot's own system it lowers the score.
//!    Either way it helps the bot's militia.
//! 4. **Capture** it. The bot warps to the complex through the site
//!    runner's [`crate::site_runner::BOARD`] (so two pilots do not pick the
//!    same complex) and lands. It then clears the grid with the site
//!    runner's rules (`site_runner::ROLE.in_space`: fight, loot, settle),
//!    and the settled grid asks for the next [`Act::Plex`]. The server's
//!    capture is the C3 §6 completion: the squads are gone after a kill.
//!    After `site_spawn::SITE_VACANT` with nobody on the grid, `facwar::tick`
//!    credits the VP. There is no capture timer or button on this server.
//! 5. **Move on** ([`next_front`]). When nothing here is worth running (or
//!    this system is [`Plan::avoid`]ed), it flies to the nearest front
//!    system with a station, by [`crate::travel::go`]. The route goes
//!    through low-sec: a plexer routes at [`career_floor`], the one career
//!    allowed under the bot's [`crate::brain::Policy::security_floor`].
//! 6. **Avoid** ([`flees`]). In space, a human outside the bot's fleet on
//!    the grid counts as an enemy militia pilot **[A]**. More of them than
//!    the bot's `risk` trait stands ([`BOLD_STANDS`]) sends it to dock (the
//!    escort's way home, [`Homing::Courtesy`]). The system is then avoided
//!    for [`HOT_FOR`].
//!
//! **Income.** The ISK comes from loot and bounties, which the escort's
//! docked rules sell (`Act::SellLoot`), and from militia LP: the server
//! pays the pilots inside a completed complex (FU-FWLP, `fw_lp.rs`).
//! 7. **Spend the LP** ([`spend_lp`]). Docked (a sortie that starts in a
//!    station, or one that flew to a front station), the bot reads its
//!    militia's corporation (`facWarMgr.GetFactionMilitiaCorporation`
//!    [V]) and hands that corporation's store to [`crate::lp_spend`],
//!    which prices each offer net of its ISK and required items, buys
//!    the items, redeems the best one worth [`MIN_ISK_PER_LP`] or more
//!    and sells the product (X-LPSTORE).
//!
//! Bot-side state ([`Plan`], one per bot name, in [`PLANS`]) is per
//! process and is never persisted. It holds the trait the driver hands
//! over, the militia, the complexes already run and the systems to avoid.

use crate::brain::{Act, Homing, Memory, Observation, Place};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::{Map, Trips};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The enlistment office [V] (`fwEnlistmentSvc`, CP53a).
pub const ENLISTMENT: &str = "fwCharacterEnlistmentMgr";
/// The warzone service [V] (`fwWarzoneSvc`, CP18/CP53b).
pub const WARZONES: &str = "fwWarzoneSolarsystem";
/// The militia office [V] (`facWarSvc.facWarMgr`, CP53a).
pub const FAC_WAR: &str = "facWarMgr";
/// The sensor overlay's registration [V] (`sensorSuiteService`, CP17).
pub const SCAN_MGR: &str = "scanMgr";
/// The overlay's full state, `(anomalies, signatures, statics,
/// structures)` [V].
pub const FULL_STATE: &str = "OnSignalTrackerFullState";

/// `dunArchetypeComplex{Novice,Small,Medium,Large}` [V] (`appConst:569-572`,
/// R-FW §6). Battlefields (68) are not complexes, and the server spawns none.
pub const COMPLEX_ARCHETYPES: [i64; 4] = [33, 34, 35, 36];

/// **The plexer's security floor.** Every system whose security is above
/// 0 passes, because the client rounds such a system up to 0.05 at least
/// (`pseudo_security`). Null-sec does not pass. This is the career's
/// exception to the bot's own floor ([`career_floor`]): FW is fought in
/// low-sec.
pub const LOW_SEC_FLOOR: f64 = 0.05;

/// The `risk` trait before the driver has handed one over (a bot outside
/// society mode). **[A]** the trait's middle. Falsified by nothing: a
/// society bot always has its own.
pub const DEFAULT_RISK: f64 = 0.5;

/// **How many hostile pilots the boldest bot stands** **[A]**. A bot with
/// risk `r` stays while the hostiles on its grid are at most
/// `r * BOLD_STANDS`. So a timid bot (`r < 0.5`) docks at the first, a
/// middling one stands one, and only the boldest stand two. Falsified by
/// plexers that never finish a complex while friends are about (too low),
/// or that stay to die to a gang (too high).
pub const BOLD_STANDS: f64 = 2.0;

/// **How long a system where the bot met hostiles stays avoided** **[A]**.
/// Falsified by plexers that come back into the same gang again and again
/// (too short), or a warzone left empty for an evening (too long).
pub const HOT_FOR: Duration = Duration::from_secs(30 * 60);

/// **How long a complex the bot ran stays off its list**, and a front
/// system with nothing worth running stays avoided **[A]**. The server
/// keeps a completed site listed until it despawns, `SITE_VACANT` (15 min)
/// after the grid empties; this is that and some slack. Falsified by a
/// plexer that warps back to a site it cleared, or that sits out a system
/// whose complexes respawned.
pub const RAN_FOR: Duration = Duration::from_secs(20 * 60);

/// **How long a bot that could not enlist anywhere leaves the career alone**
/// **[A]** (no route to any office at its floor, a partial map, standing too
/// low for every militia). Falsified by a plexer that sits idle after the
/// map or its standing were mended (too long), or by `plex: no station of
/// militia` repeating in the log (too short).
pub const NO_OFFICE_FOR: Duration = Duration::from_secs(20 * 60);

/// How long [`scan`] waits for the overlay's full state **[A]**. The
/// tracker sends it on its next tick (`signal_tracker::tick`, about a
/// second). Falsified by `plex: no overlay` notes on a quiet server.
pub const FULL_STATE_WAIT: Duration = Duration::from_secs(5);

/// **Empty overlays before a front system counts as empty** **[A]**. A
/// system's sites reach the tracker only after a pilot arrives, so the
/// first look can be empty. Falsified by a plexer that leaves a system
/// whose complexes were about to appear.
pub const EMPTY_LOOKS: u32 = 2;

/// How far from a complex to land, metres: the site runner's standoff.
pub const WARP_RANGE: f64 = crate::site_runner::WARP_RANGE;

/// How long a warp may take before it counts as stuck: the site runner's.
const WARP_CEILING: Duration = crate::site_runner::TRAVEL_CEILING;

const POLL: Duration = Duration::from_millis(250);

/// Plexes the warzone: not a civilian.
pub static ROLE: RoleSpec = RoleSpec {
    name: "plex",
    greeting: "o7 -- out plexing for the militia",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// Docked: the site runner's (the escort's) rules. Arm, unload and sell the
/// loot, dwell, undock. Once a complex has been run since the last spend
/// ([`Plan::owed`]) the bot asks for [`Act::Plex`] before it undocks,
/// which docked is the LP spend only ([`sortie`], step 7): the capture's
/// LP has been paid by then (X-LPSTORE).
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let mut acts = (crate::site_runner::ROLE.docked)(m, o);
    if PLANS.get(&m.name).owed
        && let Some(i) = acts.iter().position(|a| *a == Act::Undock)
    {
        acts.insert(i, Act::Plex);
    }
    acts
}

/// **In space**: step 6 of the module header first, then the site
/// runner's rules, whose "next site" is this career's [`Act::Plex`].
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let risk = PLANS.risk(&m.name);
    if m.homing.is_none() && flees(o.humans.len(), risk) {
        m.homing = Some(Homing::Courtesy);
        m.claim = None;
        if let Place::InSpace { system, .. } = o.place {
            PLANS.avoid(&m.name, system, o.now + HOT_FOR);
        }
    }
    // No office to enlist at (`NO_OFFICE_FOR`): the "next site" is a wait,
    // not another refused sortie.
    let backoff = PLANS.get(&m.name).not_before.is_some_and(|t| o.now < t);
    (crate::site_runner::ROLE.in_space)(m, o)
        .into_iter()
        .map(|a| match a {
            Act::Sites if backoff => Act::Wait(crate::brain::SHORT_WAIT),
            Act::Sites => Act::Plex,
            a => a,
        })
        .collect()
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// The site runner's kit: gunnery, a Rifter, guns and rounds.
fn setup(lacks: Lacks) -> Vec<String> {
    (crate::site_runner::ROLE.setup)(lacks)
}

// ---------------------------------------------------------------------------
// The rules, pure
// ---------------------------------------------------------------------------

/// **Does the bot dock away from `hostiles` pilots** at `risk`? Only when
/// there are more than `risk * BOLD_STANDS` of them.
pub fn flees(hostiles: usize, risk: f64) -> bool {
    hostiles > 0 && hostiles as f64 > risk.clamp(0.0, 1.0) * BOLD_STANDS
}

/// The floor a plexer routes at: the bot's own floor, lowered to
/// [`LOW_SEC_FLOOR`] (never raised: a test map with [`crate::travel::ANY_SECURITY`]
/// keeps it).
pub fn career_floor(policy_floor: f64) -> f64 {
    policy_floor.min(LOW_SEC_FLOOR)
}

/// **The militia a bot joins** when it is not enlisted. The one it would
/// prefer (a caller's choice), if that is a warzone militia. Otherwise the
/// one at `seed % n` in id order **[A]**: a character's faction is not on
/// the wire, so the bot's name stands for it. Falsified by an owner who
/// wants bots to fight for their race's empire; then read the faction from
/// the character's school corp.
pub fn choose_militia(warzones: &[[i64; 2]], seed: u64, prefer: Option<i64>) -> Option<i64> {
    let all: BTreeSet<i64> = warzones.iter().flatten().copied().collect();
    if let Some(p) = prefer.filter(|p| all.contains(p)) {
        return Some(p);
    }
    let all: Vec<i64> = all.into_iter().collect();
    (!all.is_empty()).then(|| all[(seed % all.len() as u64) as usize])
}

/// **The militias to try, best first** (FW-4): with `prefer`, that one; then
/// the rest by the jumps to their nearest office at `floor` (ties: the lowest
/// faction id), so a bot in space with no choice joins the militia it can
/// reach soonest **[A]** ("near home"; a character's faction is not on the
/// wire). A militia with no reachable office is left out. Each comes with
/// that office. Falsified by an owner who wants bots to fight for their
/// race's empire.
pub fn militias_by_office(
    map: &Map,
    here: i64,
    warzones: &[[i64; 2]],
    floor: f64,
    prefer: Option<i64>,
) -> Vec<(i64, i64)> {
    let all: BTreeSet<i64> = warzones.iter().flatten().copied().collect();
    let mut out: Vec<(bool, u32, i64, i64)> = all
        .into_iter()
        .filter_map(|f| {
            let (jumps, st) = office_jumps(map, here, f, floor)?;
            Some((prefer != Some(f), jumps, f, st))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, _, f, st)| (f, st)).collect()
}

/// The militia `militia` fights in its warzone.
pub fn enemy_of(warzones: &[[i64; 2]], militia: i64) -> Option<i64> {
    warzones.iter().find_map(|[a, b]| {
        if *a == militia {
            Some(*b)
        } else if *b == militia {
            Some(*a)
        } else {
            None
        }
    })
}

/// **A warzone system's distance from the front** (`_adjacencyState` [V],
/// `factionwarfare/const`): `FL`, `SL`, `BL`. Backline systems pay almost
/// nothing **[A]**, so the plexer prefers the front: the order is the
/// preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Line {
    Frontline,
    Secondline,
    Backline,
}

/// **How many more jumps a better line may cost** **[A]** before the plexer
/// runs the complexes of a worse one: a backline or second-line bot flies up
/// to this far for a better line. Falsified by plexers that spend their time
/// in transit (too far) or farm backline complexes with a front in reach
/// (too near).
pub const LINE_REACH: u32 = 4;

/// One warzone system's holder, challenger and line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Front {
    pub occupier: i64,
    pub attacker: i64,
    pub line: Line,
}

/// **The front of `militia`**: every system it occupies or attacks.
pub fn fronts(all: &BTreeMap<i64, Front>, militia: i64) -> BTreeMap<i64, Front> {
    all.iter()
        .filter(|(_, f)| f.occupier == militia || f.attacker == militia)
        .map(|(s, f)| (*s, *f))
        .collect()
}

/// One FW complex on the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Complex {
    /// What a warp names (`siteID`).
    pub site_id: i64,
    pub archetype: i64,
    /// The militia that owns it (`factionID`).
    pub faction: Option<i64>,
}

/// **Is `c` worth running for a pilot whose enemy is `enemy`?** It is a
/// complex the enemy owns (module header, step 3).
pub fn worth(c: &Complex, enemy: i64) -> bool {
    COMPLEX_ARCHETYPES.contains(&c.archetype) && c.faction == Some(enemy)
}

/// **The next front system to fly to**: among the systems other than `here`,
/// not in `avoid`, with a station on the map to dock at, the nearest by jumps
/// at `floor`, except that a better [`Line`] wins when it is within
/// [`LINE_REACH`] jumps of the nearest. Ties go to the lower system id.
/// Returns the system and its lowest station id **[A]** (any station will do
/// for a base).
pub fn next_front(
    map: &Map,
    here: i64,
    fronts: &BTreeMap<i64, Front>,
    avoid: &BTreeSet<i64>,
    floor: f64,
) -> Option<(i64, i64)> {
    let cands: Vec<(u32, Line, i64, i64)> = fronts
        .iter()
        .filter(|(s, _)| **s != here && !avoid.contains(s))
        .filter_map(|(&s, f)| {
            let station = map.stations_in(s).into_iter().min()?;
            Some((map.jumps(here, s, floor)?, f.line, s, station))
        })
        .collect();
    let nearest = cands.iter().map(|c| c.0).min()?;
    cands
        .into_iter()
        .filter(|c| c.0 <= nearest.saturating_add(LINE_REACH))
        .min_by_key(|&(jumps, line, s, _)| (line, jumps, s))
        .map(|(_, _, s, station)| (s, station))
}

/// **Is a better line within [`LINE_REACH`] jumps of `here`?** (`here` is on
/// the front list; a system with a station to dock at, not avoided.)
pub fn better_line_near(
    map: &Map,
    here: i64,
    fronts: &BTreeMap<i64, Front>,
    avoid: &BTreeSet<i64>,
    floor: f64,
) -> bool {
    let Some(mine) = fronts.get(&here).map(|f| f.line) else {
        return false;
    };
    let better = fronts
        .iter()
        .filter(|(_, f)| f.line < mine)
        .map(|(s, _)| *s);
    better.into_iter().any(|s| {
        s != here
            && !avoid.contains(&s)
            && !map.stations_in(s).is_empty()
            && map.jumps(here, s, floor).is_some_and(|j| j <= LINE_REACH)
    })
}

// ---------------------------------------------------------------------------
// The wire shapes
// ---------------------------------------------------------------------------

/// The attribute dict of a new-style object (`ObjectEx`, `__new__` form:
/// the header's element 1), of an old-style instance (`KeyVal`), or a dict.
fn state_of(v: &PyValue) -> Option<&PyValue> {
    match v {
        PyValue::ObjectEx { header, .. } => match header.as_ref() {
            PyValue::Tuple(xs) => xs.get(1),
            _ => None,
        },
        PyValue::Instance { state, .. } => Some(state.as_ref()),
        PyValue::Dict(_) => Some(v),
        _ => None,
    }
}

/// An attribute of `v` by name, `str` or `unicode`.
fn attr<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    state_of(v)?
        .dict_as_kv()?
        .into_iter()
        .find(|(k, _)| crate::text(k).as_deref() == Some(key))
        .map(|(_, v)| v)
}

fn int_attr(v: &PyValue, key: &str) -> Option<i64> {
    match attr(v, key)? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn int(v: &PyValue) -> Option<i64> {
    match v {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// `GetMyEnlistment`: `(myFaction, direct, corp)` [V]; the first.
pub fn read_enlistment(v: &PyValue) -> Option<i64> {
    match v {
        PyValue::Tuple(xs) | PyValue::List(xs) => xs.first().and_then(int),
        _ => None,
    }
}

/// `GetAllWarzones`: `{warzoneID: [factionA, factionB]}` [V].
pub fn read_warzones(v: &PyValue) -> Vec<[i64; 2]> {
    v.dict_as_kv()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, sides)| match sides {
            PyValue::List(xs) | PyValue::Tuple(xs) if xs.len() == 2 => {
                Some([int(&xs[0])?, int(&xs[1])?])
            }
            _ => None,
        })
        .collect()
}

/// `GetAllWarzonesOccupationStates`: `{warzoneID: {ssid: OccupationState}}`
/// [V], each state's `_occupierID` and `_attackerID`.
pub fn read_occupation(v: &PyValue) -> BTreeMap<i64, Front> {
    let mut out = BTreeMap::new();
    for (_, inner) in v.dict_as_kv().unwrap_or_default() {
        for (ssid, state) in inner.dict_as_kv().unwrap_or_default() {
            let (Some(ssid), Some(occupier), Some(attacker)) = (
                int(ssid),
                int_attr(state, "_occupierID"),
                int_attr(state, "_attackerID"),
            ) else {
                continue;
            };
            let line = match attr(state, "_adjacencyState") {
                Some(PyValue::Bytes(b)) if b.as_slice() == b"SL" => Line::Secondline,
                Some(PyValue::Bytes(b)) if b.as_slice() == b"BL" => Line::Backline,
                // "FL", or no word: the front (the old behaviour)
                _ => Line::Frontline,
            };
            out.insert(
                ssid,
                Front {
                    occupier,
                    attacker,
                    line,
                },
            );
        }
    }
    out
}

/// `GetMyCharacterRankInfo`: `KeyVal(currentRank, factionID)` or `None`.
pub fn read_rank(v: &PyValue) -> Option<i64> {
    int_attr(v, "currentRank")
}

/// The complexes of an `OnSignalTrackerFullState(ssid, fullState)`'s
/// arguments: element 0 of the state is `{siteBallID: CosmicAnomalyInfo}`
/// [V], and only the FW archetypes are kept. In site id order.
pub fn read_complexes(args: &[PyValue]) -> Vec<Complex> {
    let Some(PyValue::Tuple(parts)) = args.get(1) else {
        return Vec::new();
    };
    let mut out: Vec<Complex> = parts
        .first()
        .and_then(PyValue::dict_as_kv)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, info)| {
            Some(Complex {
                site_id: int_attr(info, "siteID")?,
                archetype: int_attr(info, "archetypeID")?,
                faction: int_attr(info, "factionID"),
            })
        })
        .filter(|c| COMPLEX_ARCHETYPES.contains(&c.archetype))
        .collect();
    out.sort_by_key(|c| c.site_id);
    out
}

/// How many anomalies of any kind an overlay's full state lists.
fn anomaly_count(args: &[PyValue]) -> usize {
    match args.get(1) {
        Some(PyValue::Tuple(parts)) => parts
            .first()
            .and_then(PyValue::dict_as_kv)
            .map_or(0, |d| d.len()),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// The plan, per bot
// ---------------------------------------------------------------------------

/// What one plexer remembers between sorties (module header).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// The `risk` trait; `None` until the driver hands it over.
    pub risk: Option<f64>,
    /// `system -> until when` not to come back.
    pub avoid: BTreeMap<i64, Instant>,
    /// `site -> when` the bot warped to it.
    pub ran: BTreeMap<i64, Instant>,
    /// Empty overlays in a row, per system.
    pub empty_looks: BTreeMap<i64, u32>,
    /// A complex was run since the last LP spend: its LP may be waiting.
    pub owed: bool,
    /// No enlistment was possible anywhere: no sortie until then
    /// ([`NO_OFFICE_FOR`]).
    pub not_before: Option<Instant>,
}

impl Plan {
    /// The systems avoided at `now`.
    pub fn avoided(&self, now: Instant) -> BTreeSet<i64> {
        self.avoid
            .iter()
            .filter(|(_, until)| now < **until)
            .map(|(s, _)| *s)
            .collect()
    }

    fn forget(&mut self, now: Instant) {
        self.avoid.retain(|_, until| now < *until);
        self.not_before = self.not_before.filter(|t| now < *t);
        self.ran
            .retain(|_, at| now.saturating_duration_since(*at) < RAN_FOR);
    }
}

/// Every plexer's [`Plan`], by bot name, under one lock.
#[derive(Debug)]
pub struct Plans {
    inner: Mutex<Option<BTreeMap<String, Plan>>>,
}

impl Default for Plans {
    fn default() -> Plans {
        Plans::new()
    }
}

impl Plans {
    pub const fn new() -> Plans {
        Plans {
            inner: Mutex::new(None),
        }
    }

    /// Run `f` on `name`'s plan.
    pub fn with<T>(&self, name: &str, f: impl FnOnce(&mut Plan) -> T) -> T {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(guard
            .get_or_insert_with(BTreeMap::new)
            .entry(name.to_owned())
            .or_default())
    }

    /// `name`'s risk, or [`DEFAULT_RISK`].
    pub fn risk(&self, name: &str) -> f64 {
        self.with(name, |p| p.risk.unwrap_or(DEFAULT_RISK))
    }

    /// Keep `name` out of `system` until `until`.
    pub fn avoid(&self, name: &str, system: i64, until: Instant) {
        self.with(name, |p| {
            let at = p.avoid.entry(system).or_insert(until);
            *at = (*at).max(until);
        });
    }

    /// A copy of `name`'s plan.
    pub fn get(&self, name: &str) -> Plan {
        self.with(name, |p| p.clone())
    }
}

/// The plans of every plexer in this process.
pub static PLANS: Plans = Plans::new();

// ---------------------------------------------------------------------------
// The LP store (module header, step 7)
// ---------------------------------------------------------------------------

/// The LP wallet [V] (`loyaltyPointsWalletSvc`, CP37b).
pub const LP_SVC: &str = "LPSvc";
/// The LP store [V] (`lpStoreSvc`, CP37b).
pub const LP_STORE: &str = "LPStoreMgr";

/// `maxLoyaltyStoreBulkOffers` [V] (`appConst.pyc`): the most offers one
/// `TakeOfferForCharacter` redeems.
pub const MAX_REDEEM: i64 = 100;

/// **The least ISK an LP must fetch** for the bot to redeem an offer
/// **[A]**: the product's `P_mkt x qty`, less [`SALE_COST`] and the offer's
/// ISK part, over its LP. Falsified by plexers sitting on LP while humans
/// trade the same offers at a profit (too high), or by a store's products
/// glutting the market under what an LP is worth to a human (too low).
pub const MIN_ISK_PER_LP: f64 = 500.0;

/// **What selling costs**, a share of the product's `P_mkt` **[A]**: the
/// undercut, the broker's fee and the slip of selling into bids. Falsified
/// by `[econ]` showing the plexer's sales well under its estimate.
pub const SALE_COST: f64 = 0.10;

/// **The share of the wallet an LP spend may use** for the offers' ISK
/// part **[A]**: the rest stays for hulls and rounds. Falsified by a
/// plexer that cannot replace its ship after a spend.
pub const SPEND_SHARE: f64 = 0.5;

/// One LP store offer as the store window reads it (C4 §5.1 [V]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpOffer {
    pub offer_id: i64,
    pub type_id: i64,
    pub qty: i64,
    pub lp_cost: i64,
    pub isk_cost: i64,
    /// `(typeID, qty)` the offer also takes.
    pub req_items: Vec<(i64, i64)>,
}

/// `GetAvailableOffersFromCorp`: a list of `KeyVal`s with `offerID`,
/// `typeID`, `qty`, `lpCost`, `iskCost` and `reqItems` [V].
pub fn read_offers(v: &PyValue) -> Vec<LpOffer> {
    let (PyValue::List(rows) | PyValue::Tuple(rows)) = v else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let req_items = match attr(r, "reqItems") {
                Some(PyValue::List(xs) | PyValue::Tuple(xs)) => xs
                    .iter()
                    .filter_map(|x| match x {
                        PyValue::Tuple(p) | PyValue::List(p) => {
                            Some((int(p.first()?)?, int(p.get(1)?)?))
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Some(LpOffer {
                offer_id: int_attr(r, "offerID")?,
                type_id: int_attr(r, "typeID")?,
                qty: int_attr(r, "qty")?,
                lp_cost: int_attr(r, "lpCost")?,
                isk_cost: int_attr(r, "iskCost")?,
                req_items,
            })
        })
        .collect()
}

/// `GetAllMyCharacterWalletLPBalances`: `[(corpID, balance)]` [V]; `corp`'s.
pub fn read_lp(v: &PyValue, corp: i64) -> i64 {
    let (PyValue::List(rows) | PyValue::Tuple(rows)) = v else {
        return 0;
    };
    rows.iter()
        .find_map(|r| match r {
            PyValue::Tuple(p) | PyValue::List(p) if p.first().and_then(int) == Some(corp) => {
                p.get(1).and_then(int)
            }
            _ => None,
        })
        .unwrap_or(0)
}

/// **Spend the militia LP** (module header, step 7), docked: the
/// militia's corporation, then [`crate::lp_spend::spend`] over that
/// corporation's store alone.
pub fn spend_lp(c: &mut Conn, militia: i64) -> Result<crate::lp_spend::Spend> {
    if c.session().station_id().is_none() {
        return Err(Error::NotSent("plex: spend LP: not docked".to_owned()));
    }
    let corp = int(&value(
        c,
        FAC_WAR,
        "GetFactionMilitiaCorporation",
        vec![PyValue::Int(militia)],
    )?)
    .ok_or_else(|| Error::Refused(format!("plex: militia {militia} has no corporation")))?;
    crate::lp_spend::spend(c, Some(&[corp]))
}

/// [`spend_lp`] as a note: a refusal is noted, never fatal to the sortie.
/// Either way the LP is no longer owed ([`Plan::owed`]) until the next
/// complex.
fn soft_spend(c: &mut Conn, name: &str, militia: i64) -> Result<String> {
    PLANS.with(name, |p| p.owed = false);
    match spend_lp(c, militia) {
        Ok(s) => Ok(s.summary()),
        Err(Error::Refused(why) | Error::NotSent(why)) => Ok(format!("LP: {why}")),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// The driver arm
// ---------------------------------------------------------------------------

/// What one [`sortie`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Sortie {
    /// Warped to a complex here and landed.
    Warped {
        militia: i64,
        rank: Option<i64>,
        system: i64,
        complex: Complex,
    },
    /// Nothing worth running here: flew to the next front system and
    /// docked there.
    Travelled {
        militia: i64,
        rank: Option<i64>,
        to: i64,
        station: i64,
        jumps: u32,
        /// What the docked LP spend did ([`spend_lp`]).
        lp: Option<String>,
    },
    /// Asked docked: only the LP was spent ([`spend_lp`]).
    Spent { militia: i64, lp: String },
}

impl Sortie {
    /// The act log's note.
    pub fn summary(&self) -> String {
        match self {
            Sortie::Warped {
                militia,
                rank,
                system,
                complex,
            } => format!(
                "militia {militia} rank {}; complex {} (archetype {}, owner {}) in {system}",
                rank.map_or("-".to_owned(), |r| r.to_string()),
                complex.site_id,
                complex.archetype,
                complex.faction.map_or("-".to_owned(), |f| f.to_string()),
            ),
            Sortie::Travelled {
                militia,
                rank,
                to,
                station,
                jumps,
                lp,
            } => format!(
                "militia {militia} rank {}; nothing to run here, flew {jumps} jumps to front {to} (station {station}){}",
                rank.map_or("-".to_owned(), |r| r.to_string()),
                lp.as_ref().map_or(String::new(), |n| format!("; {n}")),
            ),
            Sortie::Spent { militia, lp } => format!("militia {militia}; docked: {lp}"),
        }
    }
}

/// **One [`Act::Plex`]**: enlist if need be, then warp to a complex worth
/// running here, or fly to the next front system. `name` keys the bot's
/// [`Plan`]; `risk` is its trait, when the driver has a mind; `prefer` is
/// the militia to join when not enlisted; `trips` is the map and the bot's
/// own floor (the plexer routes at [`career_floor`] of it).
pub fn sortie(
    c: &mut Conn,
    name: &str,
    risk: Option<f64>,
    prefer: Option<i64>,
    trips: Option<Trips<'_>>,
) -> Result<Sortie> {
    if let Some(r) = risk {
        PLANS.with(name, |p| p.risk = Some(r));
    }
    let now = Instant::now();
    PLANS.with(name, |p| p.forget(now));
    let warzones = read_warzones(&value(c, WARZONES, "GetAllWarzones", vec![])?);
    let militia = ensure_enlisted(
        c,
        name,
        &warzones,
        crate::brain::name_hash(name),
        prefer,
        trips.map(|t| Trips {
            map: t.map,
            floor: career_floor(t.floor),
        }),
    )?;
    let enemy = enemy_of(&warzones, militia)
        .ok_or_else(|| Error::Refused(format!("plex: militia {militia} is in no warzone")))?;
    let rank = value(c, FAC_WAR, "GetMyCharacterRankInfo", vec![])
        .ok()
        .and_then(|v| read_rank(&v));
    let all = read_occupation(&value(
        c,
        WARZONES,
        "GetAllWarzonesOccupationStates",
        vec![],
    )?);
    let front = fronts(&all, militia);
    let here = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("plex: no system".to_owned()))?;
    let in_space = c.session().station_id().is_none();
    let mut lp = None;
    if !in_space {
        // Docked, the act is the LP spend and nothing else: the escort's
        // rules undock the bot, and the sortie flies from space.
        return Ok(Sortie::Spent {
            militia,
            lp: soft_spend(c, name, militia)?,
        });
    }
    let avoided = PLANS.get(name).avoided(now);

    // Not on the front when a better line is within reach: fly there instead.
    let climb = trips
        .is_some_and(|t| better_line_near(t.map, here, &front, &avoided, career_floor(t.floor)));
    if in_space && front.contains_key(&here) && !avoided.contains(&here) && !climb {
        let (complexes, anomalies) = scan(c)?;
        let ran = PLANS.get(name).ran;
        let ids: Vec<i64> = complexes
            .iter()
            .filter(|x| worth(x, enemy) && !ran.contains_key(&x.site_id))
            .map(|x| x.site_id)
            .collect();
        if !ids.is_empty() {
            let me = c
                .session()
                .char_id()
                .ok_or_else(|| Error::NotSent("plex: no character".to_owned()))?;
            let server = c.socket()?.peer_addr()?;
            let site = crate::site_runner::BOARD
                .choose((server, here), me, me as u64, &ids, now)
                .ok_or_else(|| Error::Refused(format!("plex: every complex in {here} is held")))?;
            PLANS.with(name, |p| {
                p.ran.insert(site, now);
                p.owed = true;
                p.empty_looks.remove(&here);
            });
            let complex = complexes
                .iter()
                .copied()
                .find(|x| x.site_id == site)
                .expect("picked from the list");
            warp(c, site)?;
            return Ok(Sortie::Warped {
                militia,
                rank,
                system: here,
                complex,
            });
        }
        let looks = PLANS.with(name, |p| {
            let n = p.empty_looks.entry(here).or_insert(0);
            if anomalies == 0 {
                *n += 1;
            } else {
                *n = EMPTY_LOOKS;
            }
            *n
        });
        if looks < EMPTY_LOOKS {
            return Err(Error::Refused(format!(
                "plex: the overlay in {here} lists nothing yet"
            )));
        }
        PLANS.with(name, |p| p.empty_looks.remove(&here));
        PLANS.avoid(name, here, now + RAN_FOR);
    }

    let trips =
        trips.ok_or_else(|| Error::NotSent("plex: no map to find the front on".to_owned()))?;
    let floor = career_floor(trips.floor);
    let avoided = PLANS.get(name).avoided(now);
    let (to, station) = next_front(trips.map, here, &front, &avoided, floor).ok_or_else(|| {
        Error::Refused(format!(
            "plex: no front system of militia {militia} to fly to from {here}"
        ))
    })?;
    let trip = crate::travel::go(
        c,
        Trips {
            map: trips.map,
            floor,
        },
        station,
    )?;
    if c.session().station_id().is_some() {
        lp = Some(soft_spend(c, name, militia)?);
    }
    Ok(Sortie::Travelled {
        militia,
        rank,
        to,
        station,
        jumps: trip.jumps,
        lp,
    })
}

/// **Enlisted, or enlist** (module header, step 1). Returns the militia.
///
/// The server enlists a pilot only at a station of the militia's own faction
/// (FW-4, #100; the client's `GetCurrentStationFactionIDs`), so a bot flies
/// to one first, as a player does ([`crate::travel::go`]). With no `prefer`
/// it tries the warzone militias by their nearest office ([`militias_by_office`]):
/// an unenlisted bot is in space, so "near home" is the office it reaches
/// soonest, and one docked at an office joins that militia **[A]**. A
/// militia that refuses (standing) or has no reachable office gives way to
/// the next; when all do, the bot's plan holds its sorties off for
/// [`NO_OFFICE_FOR`] and the error comes back. When the map knows no station
/// factions (no bundle), the bot just asks, where it is.
pub fn ensure_enlisted(
    c: &mut Conn,
    name: &str,
    warzones: &[[i64; 2]],
    seed: u64,
    prefer: Option<i64>,
    trips: Option<Trips<'_>>,
) -> Result<i64> {
    if let Some(f) = read_enlistment(&value(c, ENLISTMENT, "GetMyEnlistment", vec![])?) {
        return Ok(f);
    }
    if let PyValue::Int(until) = value(
        c,
        ENLISTMENT,
        "GetMyDirectEnlistmentCooldownTimestamp",
        vec![],
    )? {
        return Err(Error::Refused(format!(
            "plex: enlistment cooldown until {until}"
        )));
    }
    let docked = c.session().station_id();
    let Some(t) = trips.filter(|t| t.map.knows_station_factions()) else {
        // No station factions known (no bundle): ask where the bot is.
        let f = choose_militia(warzones, seed, prefer)
            .ok_or_else(|| Error::Refused("plex: no warzone militia".to_owned()))?;
        return enlist_at_hand(c, f);
    };
    let here = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("plex: no system".to_owned()))?;
    // Docked at a militia's station, with no choice made: that militia.
    let home = docked.and_then(|st| t.map.faction_of_station(st));
    let prefer = prefer.or(home.filter(|f| warzones.iter().flatten().any(|w| w == f)));
    let order = militias_by_office(t.map, here, warzones, t.floor, prefer);
    let mut last = None;
    for (f, office) in order {
        if docked.is_none_or(|st| t.map.faction_of_station(st) != Some(f)) {
            crate::travel::go(c, t, office)?;
        }
        match enlist_at_hand(c, f) {
            Ok(f) => return Ok(f),
            // The office said no (standing, a corporation's list): the next.
            Err(Error::Refused(why)) => last = Some(why),
            Err(e) => return Err(e),
        }
    }
    PLANS.with(name, |p| {
        p.not_before = Some(Instant::now() + NO_OFFICE_FOR)
    });
    Err(Error::Refused(last.unwrap_or_else(|| {
        format!("plex: no station of any warzone militia to enlist at from {here}")
    })))
}

/// Ask for the enlistment where the bot is, and read it back.
fn enlist_at_hand(c: &mut Conn, f: i64) -> Result<i64> {
    value(
        c,
        ENLISTMENT,
        "CreateMyDirectEnlistment",
        vec![PyValue::Int(f)],
    )?;
    match read_enlistment(&value(c, ENLISTMENT, "GetMyEnlistment", vec![])?) {
        Some(got) if got == f => Ok(f),
        other => Err(Error::Refused(format!(
            "plex: enlisted in {f}, but the office says {other:?}"
        ))),
    }
}

/// **The nearest station of `faction`** by jumps at `floor` (ties: the lowest
/// station id), where a bot may enlist in its militia.
pub fn nearest_office(map: &Map, here: i64, faction: i64, floor: f64) -> Option<i64> {
    office_jumps(map, here, faction, floor).map(|(_, st)| st)
}

fn office_jumps(map: &Map, here: i64, faction: i64, floor: f64) -> Option<(u32, i64)> {
    map.stations_of_faction(faction)
        .into_iter()
        .filter_map(|st| Some((map.jumps(here, map.system_of(st)?, floor)?, st)))
        .min()
}

/// **The overlay** (module header, step 3): register, then read the next
/// full state. Returns its complexes and how many anomalies it listed.
pub fn scan(c: &mut Conn) -> Result<(Vec<Complex>, usize)> {
    let outcome = c.call(SCAN_MGR, "SignalTrackerRegister", vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!(
            "plex: SignalTrackerRegister: {key}"
        )));
    }
    let read = |args: &[PyValue]| (read_complexes(args), anomaly_count(args));
    for frame in &outcome.notifications {
        if let Some((event, args)) = crate::notification(frame)
            && event == FULL_STATE
        {
            return Ok(read(&args));
        }
    }
    let deadline = Instant::now() + FULL_STATE_WAIT;
    while Instant::now() < deadline {
        for e in c.pump(POLL)? {
            if let crate::Event::Push(event, args) = e
                && event == FULL_STATE
            {
                return Ok(read(&args));
            }
        }
    }
    Err(Error::Refused(
        "plex: no overlay (OnSignalTrackerFullState) after registering".to_owned(),
    ))
}

/// A call's value, a refusal as [`Error::Refused`].
fn value(c: &mut Conn, service: &str, method: &str, args: Vec<PyValue>) -> Result<PyValue> {
    let outcome = c.call(service, method, args)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("plex: {method}: {key}")));
    }
    Ok(outcome.value().unwrap_or(PyValue::None))
}

/// Warp to `site` and land.
fn warp(c: &mut Conn, site: i64) -> Result<()> {
    let outcome = c.warp_to_item(site, WARP_RANGE)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("plex: warp to {site}: {key}")));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + WARP_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "plex: still warping after the ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, ParkView, Policy, ShipInfo};
    use eve_marshal::ObjectForm;

    /// Invented militias, warzones and systems (never client ids).
    const A: i64 = 599_001;
    const B: i64 = 599_002;
    const C: i64 = 599_003;
    const D: i64 = 599_004;
    const S1: i64 = 30_900_001;
    const S2: i64 = 30_900_002;
    const S3: i64 = 30_900_003;
    const S4: i64 = 30_900_004;

    fn zones() -> Vec<[i64; 2]> {
        vec![[A, B], [C, D]]
    }

    #[test]
    fn the_plex_row_is_not_civilian() {
        assert!(!ROLE.civilian);
        assert_eq!(ROLE.name(), "plex");
    }

    /// The flight rule as a table: nobody never sends it away; a timid bot
    /// docks at the first, a middling one stands one, the boldest stand
    /// `BOLD_STANDS`.
    #[test]
    fn a_timid_bot_docks_at_the_first_hostile_and_a_bold_one_stands_more() {
        let most = BOLD_STANDS as usize;
        assert!(!flees(0, 0.0));
        assert!(flees(1, 0.0));
        assert!(!flees(1, 0.5));
        assert!(flees(2, 0.5));
        assert!(!flees(most, 1.0));
        assert!(flees(most + 1, 1.0));
        assert!(flees(1, -3.0), "the trait is clamped");
    }

    #[test]
    fn the_career_floor_lowers_the_bots_floor_to_low_sec_never_raises_it() {
        assert_eq!(career_floor(crate::travel::SECURITY_FLOOR), LOW_SEC_FLOOR);
        assert_eq!(
            career_floor(crate::travel::ANY_SECURITY),
            crate::travel::ANY_SECURITY
        );
        let map = Map::new().with_security(S1, 0.3).with_security(S2, 0.0);
        let floor = career_floor(crate::travel::SECURITY_FLOOR);
        assert!(map.allowed(S1, floor), "low-sec passes");
        assert!(!map.allowed(S2, floor), "null-sec does not");
        assert!(
            !map.allowed(S1, crate::travel::SECURITY_FLOOR),
            "the bot's own floor would refuse low-sec"
        );
    }

    #[test]
    fn a_bot_joins_its_preferred_militia_else_one_by_its_seed() {
        assert_eq!(choose_militia(&zones(), 0, Some(D)), Some(D));
        assert_eq!(choose_militia(&zones(), 0, None), Some(A));
        assert_eq!(choose_militia(&zones(), 1, None), Some(B));
        assert_eq!(choose_militia(&zones(), 5, Some(42)), Some(B));
        assert_eq!(choose_militia(&[], 0, None), None);
        assert_eq!(enemy_of(&zones(), A), Some(B));
        assert_eq!(enemy_of(&zones(), D), Some(C));
        assert_eq!(enemy_of(&zones(), 42), None);
    }

    /// Only the enemy's complexes are worth running, never the bot's own
    /// militia's, a third party's, or a site that is not a complex.
    /// Invented offers and products.
    const ITEM_X: i64 = 990_801;
    const ITEM_Y: i64 = 990_802;

    #[test]
    fn the_store_and_the_wallet_read_back() {
        let row = kv(vec![
            ("offerID", PyValue::Int(7)),
            ("corpID", PyValue::Int(C)),
            ("typeID", PyValue::Int(ITEM_X)),
            ("qty", PyValue::Int(2)),
            ("lpCost", PyValue::Int(300)),
            ("iskCost", PyValue::Int(40)),
            ("akCost", PyValue::Int(0)),
            (
                "reqItems",
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(ITEM_Y),
                    PyValue::Int(3),
                ])]),
            ),
        ]);
        let got = read_offers(&PyValue::List(vec![row]));
        assert_eq!(
            got,
            vec![LpOffer {
                offer_id: 7,
                type_id: ITEM_X,
                qty: 2,
                lp_cost: 300,
                isk_cost: 40,
                req_items: vec![(ITEM_Y, 3)],
            }]
        );
        let wallet = PyValue::List(vec![
            PyValue::Tuple(vec![PyValue::Int(C), PyValue::Int(55)]),
            PyValue::Tuple(vec![PyValue::Int(D), PyValue::Int(66)]),
        ]);
        assert_eq!(read_lp(&wallet, D), 66);
        assert_eq!(read_lp(&wallet, A), 0);
    }

    #[test]
    fn only_the_enemys_complexes_are_worth_running() {
        let c = |archetype, faction| Complex {
            site_id: 1,
            archetype,
            faction,
        };
        for a in COMPLEX_ARCHETYPES {
            assert!(worth(&c(a, Some(B)), B));
            assert!(!worth(&c(a, Some(A)), B), "its own militia's");
            assert!(!worth(&c(a, Some(C)), B), "another warzone's");
            assert!(!worth(&c(a, None), B));
        }
        assert!(!worth(&c(24, Some(B)), B), "a combat anomaly");
    }

    /// The front is where the militia holds or attacks; the next system is
    /// the nearest one at the floor that has a station and is not avoided.
    #[test]
    fn the_next_front_is_the_nearest_with_a_station_not_avoided() {
        let all = BTreeMap::from([
            (
                S2,
                Front {
                    occupier: A,
                    attacker: B,
                    line: Line::Frontline,
                },
            ),
            (
                S3,
                Front {
                    occupier: B,
                    attacker: A,
                    line: Line::Frontline,
                },
            ),
            (
                S4,
                Front {
                    occupier: C,
                    attacker: D,
                    line: Line::Frontline,
                },
            ),
        ]);
        let front = fronts(&all, A);
        assert_eq!(front.keys().copied().collect::<Vec<_>>(), vec![S2, S3]);
        // S1 - S2 - S3, and S4 beside S1.
        let map = Map::new()
            .with_gate(S1, 1, S2, 2)
            .with_gate(S2, 3, S1, 4)
            .with_gate(S2, 5, S3, 6)
            .with_gate(S3, 7, S2, 8)
            .with_gate(S1, 9, S4, 10)
            .with_gate(S4, 11, S1, 12)
            .with_station(60_900_003, S3)
            .with_station(60_900_002, S2)
            .with_station(60_900_001, S2);
        let any = crate::travel::ANY_SECURITY;
        let none = BTreeSet::new();
        assert_eq!(
            next_front(&map, S1, &front, &none, any),
            Some((S2, 60_900_001))
        );
        assert_eq!(
            next_front(&map, S1, &front, &BTreeSet::from([S2]), any),
            Some((S3, 60_900_003)),
            "avoided"
        );
        assert_eq!(
            next_front(&map, S2, &front, &none, any),
            Some((S3, 60_900_003)),
            "never here"
        );
        let low = map.clone().with_security(S1, 0.3).with_security(S2, -0.2);
        assert_eq!(
            next_front(&low, S1, &front, &none, LOW_SEC_FLOOR),
            None,
            "never through null-sec"
        );
        let bare = Map::new().with_gate(S1, 1, S2, 2);
        assert_eq!(
            next_front(&bare, S1, &front, &none, any),
            None,
            "no station"
        );
    }

    fn kv(pairs: Vec<(&str, PyValue)>) -> PyValue {
        PyValue::Dict(
            pairs
                .into_iter()
                .map(|(k, v)| (v, PyValue::Bytes(k.as_bytes().to_vec())))
                .collect(),
        )
    }

    /// A new-style object, as the server writes one.
    fn object(pairs: Vec<(&str, PyValue)>) -> PyValue {
        PyValue::ObjectEx {
            form: ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Global(b"fixture.Class".to_vec())]),
                kv(pairs),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        }
    }

    #[test]
    fn the_offices_answers_read_back() {
        let enlisted = PyValue::Tuple(vec![PyValue::Int(A), PyValue::Int(A), PyValue::None]);
        assert_eq!(read_enlistment(&enlisted), Some(A));
        assert_eq!(
            read_enlistment(&PyValue::Tuple(vec![PyValue::None; 3])),
            None
        );
        let warzones = PyValue::Dict(vec![
            (
                PyValue::List(vec![PyValue::Int(A), PyValue::Int(B)]),
                PyValue::Int(98),
            ),
            (
                PyValue::List(vec![PyValue::Int(C), PyValue::Int(D)]),
                PyValue::Int(99),
            ),
        ]);
        assert_eq!(read_warzones(&warzones), zones());
        let state = eve_server::facwar::occupation_state(
            A,
            B,
            eve_server::facwar::Adjacency::Frontline,
            C,
            None,
        );
        let occupation = PyValue::Dict(vec![(
            PyValue::Dict(vec![(state, PyValue::Int(S1))]),
            PyValue::Int(98),
        )]);
        assert_eq!(
            read_occupation(&occupation),
            BTreeMap::from([(
                S1,
                Front {
                    occupier: A,
                    attacker: B,
                    line: Line::Frontline
                }
            )])
        );
        let rank = PyValue::Instance {
            class: b"utillib.KeyVal".to_vec(),
            state: Box::new(kv(vec![
                ("currentRank", PyValue::Int(3)),
                ("factionID", PyValue::Int(A)),
            ])),
        };
        assert_eq!(read_rank(&rank), Some(3));
        assert_eq!(read_rank(&PyValue::None), None);
    }

    /// The overlay's full state: complexes kept, other anomalies dropped,
    /// in site id order.
    #[test]
    fn the_overlay_lists_the_complexes_and_drops_other_anomalies() {
        let info = |site: i64, archetype: i64, faction: i64| {
            object(vec![
                ("siteID", PyValue::Int(site)),
                ("archetypeID", PyValue::Int(archetype)),
                ("factionID", PyValue::Int(faction)),
            ])
        };
        let anomalies = PyValue::Dict(vec![
            (info(8_200_000_002, 34, B), PyValue::Int(1)),
            (info(8_200_000_001, 33, A), PyValue::Int(2)),
            (info(8_200_000_003, 24, 500_999), PyValue::Int(3)),
        ]);
        let empty = PyValue::Dict(vec![]);
        let args = vec![
            PyValue::Int(S1),
            PyValue::Tuple(vec![anomalies, empty.clone(), empty.clone(), empty]),
        ];
        assert_eq!(
            read_complexes(&args),
            vec![
                Complex {
                    site_id: 8_200_000_001,
                    archetype: 33,
                    faction: Some(A)
                },
                Complex {
                    site_id: 8_200_000_002,
                    archetype: 34,
                    faction: Some(B)
                },
            ]
        );
        assert_eq!(anomaly_count(&args), 3);
        assert!(read_complexes(&[]).is_empty());
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn quiet_grid(now: Instant) -> Observation {
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: S1,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(crate::escort::RIFTER),
        });
        o.guns = vec![crate::brain::GunInfo {
            item_id: 2,
            type_id: crate::escort::GUN,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(100),
            capacity: 100,
            range: 1_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.lock_range = 20_000.0;
        o.park = Some(ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: Some(ball(60_900_001, 1.0e9)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        o
    }

    /// A settled grid asks for the next complex ([`Act::Plex`], never the
    /// site runner's [`Act::Sites`]); a hostile pilot over the bot's
    /// tolerance sends it to dock and marks the system avoided.
    #[test]
    fn a_quiet_grid_plexes_on_and_a_hostile_over_the_tolerance_sends_it_home() {
        let now = Instant::now();
        let mut m = Memory::new("Plexer-quiet", Policy::fast());
        let mut o = quiet_grid(now);
        assert_eq!(
            in_space(&mut m, &o),
            vec![Act::Wait(crate::brain::SHORT_WAIT)]
        );
        o.now = now + crate::site_runner::SITE_SETTLE * 2;
        assert_eq!(in_space(&mut m, &o).last(), Some(&Act::Plex));

        let timid = "Plexer-timid";
        PLANS.with(timid, |p| p.risk = Some(0.0));
        let mut m = Memory::new(timid, Policy::fast());
        let mut o = quiet_grid(now);
        o.humans = vec![ball(77, 5_000.0)];
        let acts = in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::Courtesy));
        assert!(
            matches!(acts.last(), Some(Act::WarpToItem { id: 60_900_001, .. })),
            "{acts:?}"
        );
        assert!(PLANS.get(timid).avoided(now).contains(&S1));

        let bold = "Plexer-bold";
        PLANS.with(bold, |p| p.risk = Some(1.0));
        let mut m = Memory::new(bold, Policy::fast());
        let acts = in_space(&mut m, &o);
        assert_eq!(m.homing, None, "{acts:?}");
        assert!(PLANS.get(bold).avoided(now).is_empty());
    }

    /// FW-4: the office a bot flies to is the nearest station of its militia.
    #[test]
    fn the_nearest_office_is_the_nearest_station_of_the_militia() {
        const FACTION: i64 = 599_501;
        const OTHER: i64 = 599_502;
        let map = Map::new()
            .with_gate(S1, 1, S2, 2)
            .with_gate(S2, 2, S1, 1)
            .with_gate(S2, 3, S3, 4)
            .with_gate(S3, 4, S2, 3)
            .with_security(S1, 0.3)
            .with_security(S2, 0.3)
            .with_security(S3, 0.3)
            .with_station(70_001, S1)
            .with_station(70_002, S2)
            .with_station(70_003, S3)
            .with_station_faction(70_001, OTHER)
            .with_station_faction(70_002, FACTION)
            .with_station_faction(70_003, FACTION);
        assert!(map.knows_station_factions());
        assert_eq!(nearest_office(&map, S1, FACTION, 0.0), Some(70_002));
        assert_eq!(nearest_office(&map, S3, FACTION, 0.0), Some(70_003));
        assert_eq!(nearest_office(&map, S1, 599_509, 0.0), None);
    }

    /// FW-4 fix: an unenlisted bot is in space, so "near home" is the militia
    /// whose office it reaches soonest; `prefer` goes first; a militia with
    /// no reachable office is left out.
    #[test]
    fn militias_are_tried_by_their_nearest_office() {
        const ST_A: i64 = 70_011;
        const ST_B: i64 = 70_012;
        const ST_D: i64 = 70_014;
        // S1 - S2 - S3, and S4 alone; A's office at S3, B's at S2, D's at S4.
        let map = Map::new()
            .with_gate(S1, 1, S2, 2)
            .with_gate(S2, 2, S1, 1)
            .with_gate(S2, 3, S3, 4)
            .with_gate(S3, 4, S2, 3)
            .with_station(ST_A, S3)
            .with_station(ST_B, S2)
            .with_station(ST_D, S4)
            .with_station_faction(ST_A, A)
            .with_station_faction(ST_B, B)
            .with_station_faction(ST_D, D);
        let any = crate::travel::ANY_SECURITY;
        assert_eq!(
            militias_by_office(&map, S1, &zones(), any, None),
            vec![(B, ST_B), (A, ST_A)],
            "nearest first, the unreachable D left out"
        );
        assert_eq!(
            militias_by_office(&map, S1, &zones(), any, Some(A)),
            vec![(A, ST_A), (B, ST_B)],
            "the preferred one first"
        );
        assert!(
            militias_by_office(&map, S4, &zones(), any, None)
                .first()
                .is_some_and(|x| *x == (D, ST_D))
        );
    }

    /// F6: a better line within reach wins over a nearer, worse one; far
    /// away it does not; a bot on a worse line sees the better one.
    #[test]
    fn the_front_is_preferred_to_the_second_line_and_the_back() {
        let f = |line| Front {
            occupier: A,
            attacker: B,
            line,
        };
        // S1 - S2 - S3 - S4 in a row; S2 is backline, S3 secondline, S4 frontline.
        let map = Map::new()
            .with_gate(S1, 1, S2, 2)
            .with_gate(S2, 2, S1, 1)
            .with_gate(S2, 3, S3, 4)
            .with_gate(S3, 4, S2, 3)
            .with_gate(S3, 5, S4, 6)
            .with_gate(S4, 6, S3, 5)
            .with_station(60_900_002, S2)
            .with_station(60_900_003, S3)
            .with_station(60_900_004, S4);
        let any = crate::travel::ANY_SECURITY;
        let none = BTreeSet::new();
        let all = BTreeMap::from([
            (S2, f(Line::Backline)),
            (S3, f(Line::Secondline)),
            (S4, f(Line::Frontline)),
        ]);
        assert_eq!(
            next_front(&map, S1, &all, &none, any),
            Some((S4, 60_900_004)),
            "the frontline, three jumps, over the backline, one"
        );
        assert_eq!(
            next_front(&map, S1, &all, &BTreeSet::from([S4]), any),
            Some((S3, 60_900_003)),
            "the second line over the back"
        );
        // Out of reach: the frontline is LINE_REACH + 1 jumps beyond the nearest.
        let mut far = map.clone();
        let mut prev = S4;
        for i in 0..LINE_REACH as i64 {
            let next = 30_900_100 + i;
            far = far.with_gate(prev, 20 + 2 * i, next, 21 + 2 * i).with_gate(
                next,
                21 + 2 * i,
                prev,
                20 + 2 * i,
            );
            prev = next;
        }
        let far = far.with_station(60_900_099, prev);
        let all = BTreeMap::from([(S2, f(Line::Backline)), (prev, f(Line::Frontline))]);
        assert_eq!(
            next_front(&far, S1, &all, &none, any),
            Some((S2, 60_900_002)),
            "too far for a better line"
        );
        // On the backline with a front in reach: leave; on the front: stay.
        let all = BTreeMap::from([(S2, f(Line::Backline)), (S4, f(Line::Frontline))]);
        assert!(better_line_near(&map, S2, &all, &none, any));
        assert!(!better_line_near(&map, S4, &all, &none, any));
        assert!(!better_line_near(
            &far,
            S2,
            &BTreeMap::from([(S2, f(Line::Backline)), (prev, f(Line::Frontline)),]),
            &none,
            any
        ));
    }

    /// The overlay's line word is read: FL, SL, BL; none means the front.
    #[test]
    fn the_occupation_states_line_is_read() {
        for (adj, want) in [
            (eve_server::facwar::Adjacency::Frontline, Line::Frontline),
            (eve_server::facwar::Adjacency::Secondline, Line::Secondline),
            (eve_server::facwar::Adjacency::Backline, Line::Backline),
        ] {
            let state = eve_server::facwar::occupation_state(A, B, adj, C, None);
            let occ = PyValue::Dict(vec![(
                PyValue::Dict(vec![(state, PyValue::Int(S1))]),
                PyValue::Int(98),
            )]);
            assert_eq!(read_occupation(&occ)[&S1].line, want);
        }
    }

    /// F3: while the plan holds a `not_before`, the "next site" is a wait.
    #[test]
    fn a_bot_with_no_office_waits_instead_of_asking_again() {
        let name = "Plexer-nooffice";
        let now = Instant::now();
        let mut m = Memory::new(name, Policy::fast());
        let mut o = quiet_grid(now);
        o.now = now + crate::site_runner::SITE_SETTLE * 2;
        PLANS.with(name, |p| p.not_before = Some(o.now + NO_OFFICE_FOR));
        assert_eq!(
            in_space(&mut m, &o).last(),
            Some(&Act::Wait(crate::brain::SHORT_WAIT))
        );
        PLANS.with(name, |p| p.forget(o.now + NO_OFFICE_FOR));
        assert_eq!(PLANS.get(name).not_before, None, "forgotten when over");
    }

    // ---- the harness: a fixture complex over a socket ------------------------

    use eve_server::celestial::{Band, Universe};
    use eve_server::dungeon_data::{self, Dungeon, Entry, Object, Room};
    use eve_server::population::{Diffs, Row};
    use std::net::SocketAddr;
    use std::sync::Arc;

    /// The fixture warzone's two militias (`facwar::Table::fixture`, the
    /// content fixture's own invented rows).
    fn fixture_sides() -> [i64; 2] {
        eve_server::facwar::Table::fixture().warzones()[0].factions
    }

    const HOME: i64 = 30_000_001;
    const REGION: i64 = 19_059_001;
    const CONSTELLATION: i64 = 29_059_001;
    const DUNGEON: i64 = 999_591;
    const STRUCTURE: i64 = 990_591;
    const NOVICE: i64 = COMPLEX_ARCHETYPES[0];

    /// The sandbox home as an invented low-sec system of side A.
    fn universe() -> Universe {
        let a = fixture_sides()[0];
        let system = format!(
            "{HOME}\t{REGION}\t{CONSTELLATION}\t{a}\t\t0.3\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture front"
        );
        Universe::parse(
            &[system],
            &[format!("{REGION}\t\t\t\t\tFixture region")],
            &[format!(
                "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
            )],
        )
        .expect("the invented rows parse")
    }

    /// One Novice complex owned by side A: a structure, no squads.
    fn dungeons() -> dungeon_data::Table {
        let d = Dungeon {
            id: DUNGEON,
            archetype: NOVICE,
            faction: Some(fixture_sides()[0]),
            difficulty: Some(1),
            entry_type: Some(dungeon_data::ENTRY_ANOMALY),
            name_id: Some(DUNGEON + 1),
            entry_object: Some(1),
            rooms: vec![Room {
                id: 1,
                ..Room::default()
            }],
            objects: vec![Object {
                room: 1,
                id: 1,
                type_id: STRUCTURE,
                pos: [0.0; 3],
                radius: Some(1_000.0),
                ..Object::default()
            }],
            ..Dungeon::default()
        };
        dungeon_data::Table {
            dungeons: BTreeMap::from([(DUNGEON, d)]),
            ..dungeon_data::Table::default()
        }
    }

    /// One low-sec FW row: a Novice complex per warzone system.
    fn population() -> eve_server::population::Table {
        eve_server::population::Table::new(vec![Row {
            key: "fx_fw_low".to_owned(),
            band: Band::Low,
            archetypes: vec![NOVICE],
            entry: Entry::Anomaly,
            count: 1.0,
            difficulties: Diffs::In(vec![1]),
            respawn_s: 600,
            ttl_s: 172_800,
        }])
        .expect("one row")
    }

    /// Invented station-owning corporations of the two sides.
    const OFFICE_A: i64 = 1_999_591;
    const OFFICE_B: i64 = 1_999_592;

    fn offices() -> eve_server::npc_data::Table {
        let [a, b] = fixture_sides();
        let mut t = eve_server::npc_data::Table::default();
        for (corp, faction) in [(OFFICE_A, a), (OFFICE_B, b)] {
            let row = eve_server::npc_data::Corporation {
                corp_id: corp,
                faction_id: Some(faction),
                ..eve_server::npc_data::Corporation::default()
            };
            t.corporations.insert(corp, row);
        }
        t
    }

    fn server(names: &[&str]) -> SocketAddr {
        let mut config = eve_server::server::Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = names
            .iter()
            .enumerate()
            .map(|(i, n)| eve_server::model::Character {
                character_id: 90_000_591 + i as i64,
                name: (*n).to_owned(),
                ship_id: 100_000_591 + i as i64,
                // Docked at the office of its own side (FW-4): "PlexerB"
                // at B's, the rest at A's.
                station_owner_id: if n.ends_with('B') { OFFICE_B } else { OFFICE_A },
                ..eve_server::model::Character::default()
            })
            .collect();
        let mut content = eve_server::content::Content::fixture();
        content.insert("npc_data", offices());
        content.insert("universe", universe());
        content.insert("dungeon_data", dungeons());
        content.insert("population", population());
        config.content = Some(Arc::new(content));
        let server = eve_server::server::Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    /// Logged in, undocked in the capsule and bound.
    fn in_space_at(addr: SocketAddr, name: &str) -> Conn {
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(crate::driver::DEV_TOKEN).expect("handshake");
        c.select_character_named(name).expect("selects");
        // The office is in the station (FW-4): a pilot enlists there, then
        // undocks.
        let [a, b] = fixture_sides();
        let side = if name.ends_with('B') { b } else { a };
        value(
            &mut c,
            ENLISTMENT,
            "CreateMyDirectEnlistment",
            vec![PyValue::Int(side)],
        )
        .expect("enlists at its office");
        c.undock().expect("undocks");
        let deadline = Instant::now() + Duration::from_secs(20);
        while c.session().solar_system_id().is_none() && Instant::now() < deadline {
            c.pump(POLL).expect("pumps");
        }
        let system = c.session().solar_system_id().expect("in space");
        c.bind_ballpark(system).expect("binds");
        while !c.world().has_state() && Instant::now() < deadline {
            c.pump(POLL).expect("pumps");
        }
        assert!(c.world().has_state(), "a park state");
        c
    }

    /// **FW-4 (#100)**: the office enlists only at a station of the
    /// militia's faction. Docked at B's, a pilot who asks for A is refused
    /// and one who asks for B is in; the plexer then needs no flight.
    #[test]
    fn a_pilot_enlists_only_at_a_station_of_the_militia() {
        let [a, b] = fixture_sides();
        let addr = server(&["PlexerB"]);
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(crate::driver::DEV_TOKEN).expect("handshake");
        c.select_character_named("PlexerB").expect("selects");
        let warzones =
            read_warzones(&value(&mut c, WARZONES, "GetAllWarzones", vec![]).expect("zones"));
        let refused = c
            .call(
                ENLISTMENT,
                "CreateMyDirectEnlistment",
                vec![PyValue::Int(a)],
            )
            .expect("answered");
        assert!(refused.refusal().is_some(), "A's militia, B's station");
        assert_eq!(
            ensure_enlisted(&mut c, "PlexerB", &warzones, 0, Some(b), None).expect("in"),
            b
        );
    }

    /// **FW-4 fix, in space**: an unenlisted bot has `docked == None`. With
    /// a map knowing one office (B's, at the station it just left) it joins
    /// B, whatever its seed says; with a map that knows none of the
    /// warzone's offices it is refused, and its plan holds it off.
    #[test]
    fn an_unenlisted_bot_in_space_joins_the_militia_it_can_reach() {
        let [a, b] = fixture_sides();
        let addr = server(&["PlexerB"]);
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(crate::driver::DEV_TOKEN).expect("handshake");
        c.select_character_named("PlexerB").expect("selects");
        let st = c.session().station_id().expect("docked");
        c.undock().expect("undocks");
        let deadline = Instant::now() + Duration::from_secs(20);
        while c.session().solar_system_id().is_none() && Instant::now() < deadline {
            c.pump(POLL).expect("pumps");
        }
        assert!(c.session().station_id().is_none(), "in space");
        let warzones =
            read_warzones(&value(&mut c, WARZONES, "GetAllWarzones", vec![]).expect("zones"));
        let any = crate::travel::ANY_SECURITY;
        // No office of the warzone on this map: refused, and held off.
        let elsewhere = Map::new()
            .with_station(70_021, 30_900_021)
            .with_station_faction(70_021, 599_521);
        let name = "PlexerB-nooffice";
        let err = ensure_enlisted(
            &mut c,
            name,
            &warzones,
            0,
            None,
            Some(Trips {
                map: &elsewhere,
                floor: any,
            }),
        )
        .expect_err("no office");
        assert!(matches!(err, Error::Refused(_)), "{err:?}");
        assert!(PLANS.get(name).not_before.is_some(), "backed off");
        // B's office is the station it left: with seed 0 (A) it still joins B.
        let system = c.session().solar_system_id2().expect("a system");
        let map = Map::new()
            .with_station(st, system)
            .with_station_faction(st, b);
        let got = ensure_enlisted(
            &mut c,
            "PlexerB-near",
            &warzones,
            0,
            None,
            Some(Trips {
                map: &map,
                floor: any,
            }),
        )
        .expect("joins the reachable militia");
        assert_eq!(got, b);
        assert_ne!(got, a);
        assert!(PLANS.get("PlexerB-near").not_before.is_none());
    }

    /// **The harness test**: in a warzone system with one fixture complex
    /// owned by side A, a side-B plexer enlists, reads the front and the
    /// overlay, and warps to the complex and lands; a side-A plexer finds
    /// nothing worth running there and, with no map, cannot fly on.
    #[test]
    fn a_plexer_enlists_finds_the_enemys_complex_and_warps_to_it() {
        let [a, b] = fixture_sides();
        let addr = server(&["PlexerB", "PlexerA"]);
        let mut c = in_space_at(addr, "PlexerB");
        // The system's sites reach the tracker once a pilot is there.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = Vec::new();
        while seen.is_empty() && Instant::now() < deadline {
            seen = scan(&mut c).map(|(x, _)| x).unwrap_or_default();
        }
        assert!(
            seen.iter().any(|x| x.faction == Some(a)),
            "the fixture complex is on the overlay: {seen:?}"
        );

        let warzones =
            read_warzones(&value(&mut c, WARZONES, "GetAllWarzones", vec![]).expect("zones"));
        assert_eq!(
            ensure_enlisted(&mut c, "PlexerB", &warzones, 0, Some(a), None).expect("enlisted"),
            b,
            "an enlisted pilot keeps its militia"
        );
        let got = sortie(&mut c, "PlexerB", Some(1.0), Some(b), None).expect("a sortie");
        let Sortie::Warped {
            militia,
            system,
            complex,
            ..
        } = got
        else {
            panic!("warped, not {got:?}")
        };
        assert_eq!((militia, system), (b, HOME));
        assert_eq!(complex.faction, Some(a));
        assert_eq!(complex.archetype, NOVICE);
        let mine =
            read_enlistment(&value(&mut c, ENLISTMENT, "GetMyEnlistment", vec![]).expect("asks"));
        assert_eq!(mine, Some(b), "enlisted in the preferred militia");
        assert!(
            !c.world_now().ego_ball().expect("a ball").warp_commanded(),
            "landed"
        );
        assert!(PLANS.get("PlexerB").ran.contains_key(&complex.site_id));

        // Side A: its own complex is not worth running, and with no map
        // there is nowhere to fly.
        let mut c = in_space_at(addr, "PlexerA");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut listed = 0;
        while listed == 0 && Instant::now() < deadline {
            listed = scan(&mut c).map(|(_, n)| n).unwrap_or(0);
        }
        assert!(listed > 0, "the overlay lists the complex");
        match sortie(&mut c, "PlexerA", None, Some(a), None) {
            Err(Error::NotSent(why)) => assert!(why.contains("no map"), "{why}"),
            other => panic!("no flight without a map, not {other:?}"),
        }
        assert!(
            PLANS.get("PlexerA").avoided(Instant::now()).contains(&HOME),
            "nothing worth running: the system is avoided"
        );
    }
}
