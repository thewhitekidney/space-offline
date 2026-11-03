//! **The site runner** (W16 packet CP55): a bot that runs the combat
//! anomalies of its system, the way a player does from the Agency.
//!
//! What it does, every step a player verb:
//!
//! 1. **Docked**, it arms itself exactly as the escort does
//!    ([`crate::escort::docked`]: a Rifter, two guns, rounds, a spare load in
//!    the hold) and unloads the loot of its last trip, then sells it
//!    ([`Act::SellLoot`], E10b: loot-only stacks into NPC bids, T1 loot
//!    listed or refined, rounds kept up to [`escort::AMMO_KEEP`]). With no
//!    Rifter to be had, it runs sites in its rookie ship with the civilian
//!    gun (E10b).
//! 2. **In space**, it asks for [`Act::Sites`], whose driver arm is one
//!    [`scout`]: read the Agency's combat-anomaly counts
//!    (`dungeonInstanceCacheMgr.GetCombatAnomaliesCount`, C2 §3), and, with
//!    a count for its own system, the own system's rows
//!    (`GetCombatAnomalyInstances`); pick a site on the [`Board`]; warp to
//!    its `siteID` through the S8 resolver (`signal_tracker::anomaly_target`)
//!    and land.
//! 3. **On the site's grid** (GRID-B): the park holds only that grid's
//!    balls, and the site's structures and NPCs come in with the grid
//!    reconcile within about a second of landing. So it waits
//!    [`SITE_SETTLE`] of quiet before it calls the site clear -- the
//!    miners' `BELT_WAITS` pattern (`brain::to_a_belt`).
//! 4. **While anything is there** to fight or loot, it plays the escort's
//!    rules with no boss ([`crate::escort::in_space`]): engage the nearest
//!    NPC within `GUARD_RADIUS`, orbit, lock, fire, then loot each NPC wreck.
//!    Each fight restarts the settle, so a gated wave (CP28b's
//!    `triggerSpawn`) that comes after the first is fought too.
//! 5. **Quiet for [`SITE_SETTLE`]**: the site is cleared; [`Act::Sites`]
//!    again, for the next one. A full hold sends it home (docked, step 1
//!    unloads it).
//!
//! **Which site** ([`pick`], held on a [`Board`]): never the site this
//! runner ran last, and never a site another runner on the same server and
//! system holds a live claim on ([`CLAIM_TTL`]). Among the rest, the one at
//! `seed % n` in id order, so runners with different characters start
//! apart. Two runners in one system therefore never pick the same site
//! twice in a row. The board is bot-side and per process (the bots of one
//! `eve-bots`); it is the runners' own etiquette, not a server lock -- the
//! server's answer to contention is the supply floor (C7 §6.2), and C3 §9
//! says counts, not locks, are the knob.
//!
//! **Not civilian** (ruling 5, plan §4.7): the courtesy rule does not apply,
//! so a human on the site does not send it away.
//!
//! **What it knows** is what a pilot knows (C2 §3 policy): rows for its own
//! system, counts for the rest. It does not roam: a system whose count is
//! zero is a failed [`Act::Sites`], and three of those send it home
//! (`escort`'s recovery rule).
//!
//! Every rule here is **[A] policy** -- what a player would do -- except the
//! client's own Agency calls, which are cited [V].

use crate::brain::{self, Act, Homing, Memory, Observation, SHORT_WAIT};
use crate::escort;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Agency's service **[V]** (`evedungeons/client/anomalyTracker`, C2 §3).
pub const AGENCY: &str = "dungeonInstanceCacheMgr";
/// `{solarSystemID: n}` over every system **[V]** (`anomalyTracker:238`).
pub const COUNT_METHOD: &str = "GetCombatAnomaliesCount";
/// `{solarSystemID: [row, ...] | n}`: rows for the caller's own system
/// **[V]** (`anomalyTracker`'s combat tracker, C2 §3).
pub const ROWS_METHOD: &str = "GetCombatAnomalyInstances";

/// How long a site's grid must be quiet -- nothing to fight, nothing to
/// loot -- before the runner calls it clear. **[A]** The grid reconcile adds
/// a site's balls within about a second of landing
/// (`eve_server::grid::RECONCILE_EVERY`), the site stage places the room
/// once a second, and its squads spawn on arrival; four seconds is twice
/// that chain. Falsified by a runner that warps off a site before its rats
/// appear, or before a gated wave comes.
pub const SITE_SETTLE: Duration = Duration::from_secs(4);

/// How long another runner's claim on a site keeps it off this runner's
/// list. **[A]**: CP28b's `SITE_VACANT`, the time an abandoned pocket
/// stays. Falsified by runners queueing behind a claim nobody is running.
pub const CLAIM_TTL: Duration = Duration::from_secs(900);

/// How long the runner spends getting to one wreck before letting it go.
/// **[A]**: a frigate crosses a grid's worth of wrecks in this; falsified by
/// a runner that leaves loot a player would have gone for.
pub const LOOT_PATIENCE: Duration = Duration::from_secs(60);

/// The picks a [`Board`] remembers per server and system, for the log and
/// the tests. **[A]**
pub const HISTORY: usize = 64;

/// How far from the anomaly's position to land, metres: the escort's own
/// warp standoff. **[A]**
pub const WARP_RANGE: f64 = escort::WARP_STANDOFF;

/// How long a warp may take before it counts as stuck. **[A]**, a
/// ceiling, not a duration.
pub const TRAVEL_CEILING: Duration = Duration::from_secs(120);

/// How long [`scout`] waits for each burst while the warp runs.
const POLL: Duration = Duration::from_millis(250);

/// Runs the anomalies: not a civilian (ruling 5).
pub static ROLE: RoleSpec = RoleSpec {
    name: "sites",
    greeting: "o7 -- running the anomalies here",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// Docked: the escort's rules -- arm, unload the loot, dwell, undock.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    escort::docked(m, o)
}

/// In space: steps 3-5 of the module header.
///
/// The escort's rules run first, every turn, so its recovery, repair,
/// re-arm, fight and loot rules all hold here. Only its quiet tail with no
/// boss -- "warp to the belt", or patrol -- is replaced by the site rules.
/// In a fleet with a boss the runner flies the escort's wing instead. [A]
///
/// On a quiet turn, in order: a full hold goes home; an NPC the escort
/// passed over is closed on ([`far_hostile`]); a wreck is looted
/// ([`loot`]); then the settle, then the next site.
///
/// `Memory::dwell_until` is unused in space by the other roles (the escort
/// clears it); here it is the moment the current grid counts as clear --
/// or, while `Memory::claim` names a wreck, when to give that wreck up --
/// and `None` means "start the settle on the next quiet turn".
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(SHORT_WAIT)];
    };
    let clear_at = m.dwell_until;
    let acts = escort::in_space(m, o);
    if m.homing.is_some() || park.boss.is_some() || o.fleet.is_some_and(|f| f.boss.is_some()) {
        return acts;
    }
    let quiet = match acts.last() {
        Some(Act::Wait(d)) => *d == escort::PATROL_WAIT,
        Some(Act::WarpToItem { id, .. }) => park
            .rocks
            .iter()
            .chain(park.belts.iter())
            .any(|b| b.id == *id),
        _ => false,
    };
    if !quiet {
        // A fight or a wreck: the settle starts again once it is over.
        m.dwell_until = None;
        m.claim = None;
        return acts;
    }
    m.dwell_until = clear_at;
    // What the escort said on the way (a kill, say) is kept.
    let mut out: Vec<Act> = acts
        .into_iter()
        .filter(|a| matches!(a, Act::Say(_)))
        .collect();
    if o.hold_full {
        m.homing = Some(Homing::HoldFull);
        m.claim = None;
        out.extend(brain::go_home(m, o, park));
        return out;
    }
    if let Some(act) = far_hostile(m, park) {
        out.push(act);
        return out;
    }
    if let Some(act) = loot(m, o, park) {
        out.push(act);
        return out;
    }
    if park.ego_warping {
        out.push(Act::Wait(SHORT_WAIT));
        return out;
    }
    match m.dwell_until {
        None => {
            m.dwell_until = Some(o.now + SITE_SETTLE);
            out.push(Act::Wait(SHORT_WAIT));
        }
        Some(t) if o.now < t => out.push(Act::Wait(SHORT_WAIT)),
        Some(_) => {
            // Cleared (or nowhere yet): the next site. The settle starts
            // on landing.
            m.dwell_until = None;
            m.looted.clear();
            out.push(Act::Sites);
        }
    }
    out
}

/// **An NPC left on the grid** that the escort's rules pass over (beyond
/// their `GUARD_RADIUS`, shooting nobody): the site is not clear, so close
/// in -- warp from beyond the client's warp floor, else approach -- until
/// the escort's rules take it. [A]
fn far_hostile(m: &mut Memory, park: &brain::ParkView) -> Option<Act> {
    let ego = park.ego;
    let h = park
        .hostiles
        .iter()
        .min_by(|a, b| ego.surface_to(&a.ball).total_cmp(&ego.surface_to(&b.ball)))?;
    m.dwell_until = None;
    m.claim = None;
    if !escort::warp_lands_short(ego.surface_to(&h.ball), escort::WARP_STANDOFF) {
        return Some(Act::WarpToItem {
            id: h.ball.id,
            range: escort::WARP_STANDOFF,
        });
    }
    if park.ego_tether == Some(h.ball.id) {
        return Some(Act::Wait(SHORT_WAIT));
    }
    Some(Act::Follow {
        id: h.ball.id,
        range: escort::WARP_STANDOFF,
    })
}

/// **Loot the grid's wrecks**, nearest first, one at a time: approach it,
/// then Loot All inside the client's loot range. `m.claim` is the wreck
/// being approached and `m.dwell_until` when to give it up
/// ([`LOOT_PATIENCE`]). A refused approach, or the patience run out, lets
/// the wreck go. `None`: no wreck left to loot.
///
/// Unlike the escort's rule, distance does not filter: every wreck in the
/// park is on the site's grid (GRID-B), and the park's own estimate of the
/// ship's place drifts while it orbits, so the approach is what brings the
/// ship's ball back to the truth. [A]
fn loot(m: &mut Memory, o: &Observation, park: &brain::ParkView) -> Option<Act> {
    let reach = crate::LOOT_RANGE * brain::RANGE_MARGIN;
    let deadline = m.dwell_until;
    if let Some(id) = m.claim {
        let gone = |m: &mut Memory| {
            m.looted.push(id);
            m.claim = None;
            m.dwell_until = None;
        };
        match park.wrecks.iter().find(|w| w.id == id) {
            Some(w) if park.ego.surface_to(w) < reach => {
                gone(m);
                return Some(Act::Loot(id));
            }
            Some(_)
                if deadline.is_some_and(|t| o.now < t) && o.failed(brain::ActKind::Follow) == 0 =>
            {
                if park.ego_tether == Some(id) {
                    return Some(Act::Wait(SHORT_WAIT));
                }
                return Some(Act::Follow {
                    id,
                    range: escort::WRECK_APPROACH,
                });
            }
            _ => gone(m),
        }
    }
    let ego = park.ego;
    let wreck = park
        .wrecks
        .iter()
        .filter(|w| !m.looted.contains(&w.id))
        .min_by(|a, b| ego.surface_to(a).total_cmp(&ego.surface_to(b)))?;
    if ego.surface_to(wreck) < reach {
        m.looted.push(wreck.id);
        m.dwell_until = None;
        return Some(Act::Loot(wreck.id));
    }
    m.claim = Some(wreck.id);
    m.dwell_until = Some(o.now + LOOT_PATIENCE);
    Some(Act::Follow {
        id: wreck.id,
        range: escort::WRECK_APPROACH,
    })
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

/// The escort's kit: the gunnery skill, a Rifter, its guns and rounds.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {} 1", escort::SKILL_GUNNERY)];
    if lacks.ship {
        lines.push(format!("/createitem {}", escort::RIFTER));
    }
    if lacks.module {
        lines.push(format!("/createitem {} {}", escort::GUN, escort::WANT_GUNS));
    }
    if lacks.ammo {
        lines.push(format!(
            "/createitem {} {}",
            escort::AMMO,
            escort::SETUP_ROUNDS
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// The Agency's answers
// ---------------------------------------------------------------------------

/// One own-system row of `GetCombatAnomalyInstances`: a `KeyVal` whose
/// `siteID` is what a warp names (C2 §3 row fields [V]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteRow {
    pub site_id: i64,
    pub dungeon_id: Option<i64>,
    /// The row's `difficulty` (the scan difficulty, 1-5 [V], C2 §3):
    /// what [`difficulty_cap`] weighs.
    pub difficulty: Option<i64>,
    pub position: Option<[f64; 3]>,
}

impl SiteRow {
    pub fn from_value(v: &PyValue) -> Option<SiteRow> {
        let attrs = crate::attributes(v);
        let int = |k: &str| match get(attrs, k) {
            Some(PyValue::Int(n)) => Some(*n),
            _ => None,
        };
        let position = match get(attrs, "position") {
            Some(PyValue::Tuple(xs)) | Some(PyValue::List(xs)) if xs.len() == 3 => {
                let mut p = [0.0; 3];
                for (i, x) in xs.iter().enumerate() {
                    p[i] = match x {
                        PyValue::Real(r) => *r,
                        PyValue::Int(n) => *n as f64,
                        _ => return None,
                    };
                }
                Some(p)
            }
            _ => None,
        };
        Some(SiteRow {
            site_id: int("siteID")?,
            dungeon_id: int("dungeonID"),
            difficulty: int("difficulty"),
            position,
        })
    }
}

/// A key of an attribute dict, as `str` or `unicode`.
fn get<'a>(attrs: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    attrs
        .dict_as_kv()?
        .into_iter()
        .find(|(k, _)| crate::text(k).as_deref() == Some(key))
        .map(|(_, v)| v)
}

/// `{solarSystemID: n}`, as the counts answer it. A row list (the own
/// system, in `GetCombatAnomalyInstances`) counts its rows.
pub fn read_counts(v: &PyValue) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    for (k, v) in v.dict_as_kv().unwrap_or_default() {
        let PyValue::Int(ssid) = k else { continue };
        let n = match v {
            PyValue::Int(n) => *n,
            PyValue::List(rows) => rows.len() as i64,
            _ => continue,
        };
        out.insert(*ssid, n);
    }
    out
}

/// The rows `system` has in a `Get*Instances` answer; an int (a count, as
/// every other system gets) has none.
pub fn read_rows(v: &PyValue, system: i64) -> Vec<SiteRow> {
    v.dict_as_kv()
        .unwrap_or_default()
        .into_iter()
        .find(|(k, _)| **k == PyValue::Int(system))
        .and_then(|(_, rows)| match rows {
            PyValue::List(rows) => Some(rows.iter().filter_map(SiteRow::from_value).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Which site: the rule and the board
// ---------------------------------------------------------------------------

/// **The pick rule**, pure: among `sites`, drop `last` (this runner's
/// previous site) and every site in `held` (another runner's live claim);
/// of the rest, in id order, the one at `seed % n`. `None` when nothing is
/// left.
pub fn pick(sites: &[i64], seed: u64, last: Option<i64>, held: &BTreeSet<i64>) -> Option<i64> {
    let free: BTreeSet<i64> = sites
        .iter()
        .copied()
        .filter(|s| Some(*s) != last && !held.contains(s))
        .collect();
    let free: Vec<i64> = free.into_iter().collect();
    if free.is_empty() {
        return None;
    }
    Some(free[(seed % free.len() as u64) as usize])
}

/// Which server and system a claim is on.
pub type Pitch = (SocketAddr, i64);

#[derive(Debug, Clone, Copy)]
struct Claim {
    site: i64,
    at: Instant,
}

#[derive(Debug, Default)]
struct Pitches {
    /// Per pitch, each runner's newest pick.
    claims: BTreeMap<Pitch, BTreeMap<i64, Claim>>,
    /// Per pitch, `(runner, site)` in pick order, the newest [`HISTORY`].
    history: BTreeMap<Pitch, Vec<(i64, i64)>>,
}

/// **The runners' claims**, one per runner per pitch: what [`pick`] reads
/// and what each pick writes, under one lock so two runners cannot pick at
/// once. See the module header for why this is bot-side.
#[derive(Debug)]
pub struct Board {
    inner: Mutex<Option<Pitches>>,
}

impl Default for Board {
    fn default() -> Board {
        Board::new()
    }
}

impl Board {
    pub const fn new() -> Board {
        Board {
            inner: Mutex::new(None),
        }
    }

    /// Pick for `runner` on `pitch` at `now` and claim the pick: the
    /// runner's own last claim is `last`, and every other runner's claim
    /// younger than [`CLAIM_TTL`] is `held`.
    pub fn choose(
        &self,
        pitch: Pitch,
        runner: i64,
        seed: u64,
        sites: &[i64],
        now: Instant,
    ) -> Option<i64> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let p = guard.get_or_insert_with(Pitches::default);
        let claims = p.claims.entry(pitch).or_default();
        let last = claims.get(&runner).map(|c| c.site);
        let held: BTreeSet<i64> = claims
            .iter()
            .filter(|(r, c)| **r != runner && now.saturating_duration_since(c.at) < CLAIM_TTL)
            .map(|(_, c)| c.site)
            .collect();
        let site = pick(sites, seed, last, &held)?;
        claims.insert(runner, Claim { site, at: now });
        let h = p.history.entry(pitch).or_default();
        h.push((runner, site));
        if h.len() > HISTORY {
            h.remove(0);
        }
        Some(site)
    }

    /// The site `runner` picked last on `pitch`, if it holds one.
    pub fn last(&self, pitch: Pitch, runner: i64) -> Option<i64> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()?
            .claims
            .get(&pitch)?
            .get(&runner)
            .map(|c| c.site)
    }

    /// `(runner, site)` for every pick on `pitch`, oldest first.
    pub fn picks(&self, pitch: Pitch) -> Vec<(i64, i64)> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .and_then(|p| p.history.get(&pitch).cloned())
            .unwrap_or_default()
    }
}

/// The board every runner of this process shares ([`Act::Sites`]'s
/// driver arm).
pub static BOARD: Board = Board::new();

// ---------------------------------------------------------------------------
// The driver arm
// ---------------------------------------------------------------------------

/// What one [`scout`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scouted {
    /// The Agency's combat-anomaly counts, every system.
    pub counts: BTreeMap<i64, i64>,
    /// The site warped to, or (through a gate) the site the runner is in.
    pub site: i64,
    /// The acceleration gate taken to the next room of that site, when the
    /// grid had one (Y-COMBAT); the Agency was not asked then.
    pub gate: Option<i64>,
}

/// **One [`Act::Sites`]**: read the Agency, pick on `board`, warp to the
/// pick and land. In space, bound.
pub fn scout(c: &mut Conn, board: &Board) -> Result<Scouted> {
    if c.session().station_id().is_some() {
        return Err(Error::NotSent("sites: docked".to_owned()));
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("sites: in space with no system".to_owned()))?;
    let me = c
        .session()
        .char_id()
        .ok_or_else(|| Error::NotSent("sites: no character".to_owned()))?;
    // Y-COMBAT: a gate on this grid leads on through the site; the next site
    // is for when there is none (or the gate does not take this hull).
    let server = c.socket()?.peer_addr()?;
    if let Some(gate) = gate_on_grid(c)
        && through(c, gate)?
    {
        let site = board.last((server, system), me).unwrap_or(0);
        return Ok(Scouted {
            counts: BTreeMap::new(),
            site,
            gate: Some(gate),
        });
    }
    // Y-ESCAL: the runner's own escalation, in this system, comes first: a
    // player opens the Expeditions list (`journal.GetEscalations`) before the
    // Agency's anomalies. One this hull does not fit is left alone.
    let cap = c
        .world_now()
        .ego_ball()
        .and_then(|b| b.type_id())
        .and_then(difficulty_cap);
    if let Some(site) = escalation_here(c, system, cap)? {
        return Ok(Scouted {
            counts: BTreeMap::new(),
            site,
            gate: None,
        });
    }
    let counts = read_counts(&agency(c, COUNT_METHOD)?);
    if counts.get(&system).copied().unwrap_or(0) <= 0 {
        return Err(Error::Refused(format!(
            "sites: the Agency counts no combat anomaly in {system}"
        )));
    }
    let rows = read_rows(&agency(c, ROWS_METHOD)?, system);
    // Y-COMBAT, the decision point: only the sites this hull can run.
    let ids: Vec<i64> = rows
        .iter()
        .filter(|r| fits(r.difficulty, cap))
        .map(|r| r.site_id)
        .collect();
    let site = board
        .choose((server, system), me, me as u64, &ids, Instant::now())
        .ok_or_else(|| {
            Error::Refused(format!(
                "sites: none of {system}'s {} anomalies is free",
                ids.len()
            ))
        })?;
    let outcome = c.warp_to_item(site, WARP_RANGE)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("sites: warp to {site}: {key}")));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))?;
    Ok(Scouted {
        counts,
        site,
        gate: None,
    })
}

/// The service and method of the journal's Expeditions list **[V]**
/// (`journal.GetEscalations`).
pub const ESCALATION_SERVICE: &str = "dungeonExplorationMgr";
pub const ESCALATION_METHOD: &str = "GetMyEscalatingPathDetails";
/// `CmdWarpToStuff`'s subject for an escalation's instance **[V]**
/// (`warpSubjects.EPIC_INSTANCE`).
pub const ESCALATION_SUBJECT: &[u8] = b"epinstance";

/// One row of the Expeditions list: what a runner weighs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscalationRow {
    pub instance_id: i64,
    pub system: i64,
    /// `destDungeon.difficulty`.
    pub difficulty: Option<i64>,
    /// The wall FILETIME it runs out.
    pub expiry: i64,
}

/// The rows of an old-style `Rowset` (`header`, `lines`) by their column
/// names; a row without its `instanceID`, `solarSystemID` or `expiryTime`
/// is skipped, as one without a `destDungeon` is by the client.
pub fn read_escalations(v: &PyValue) -> Vec<EscalationRow> {
    let attrs = crate::attributes(v);
    let (Some(PyValue::List(header)), Some(PyValue::List(lines))) =
        (get(attrs, "header"), get(attrs, "lines"))
    else {
        return Vec::new();
    };
    let names: Vec<Option<String>> = header.iter().map(crate::text).collect();
    lines
        .iter()
        .filter_map(|line| {
            let PyValue::List(cells) = line else {
                return None;
            };
            let cell = |name: &str| {
                let i = names.iter().position(|n| n.as_deref() == Some(name))?;
                cells.get(i)
            };
            let int = |name: &str| match cell(name) {
                Some(PyValue::Int(n)) => Some(*n),
                _ => None,
            };
            let dest = cell("destDungeon")?;
            if matches!(dest, PyValue::None) {
                return None;
            }
            let difficulty = match get(crate::attributes(dest), "difficulty") {
                Some(PyValue::Int(n)) => Some(*n),
                _ => None,
            };
            Some(EscalationRow {
                instance_id: int("instanceID")?,
                system: int("solarSystemID")?,
                difficulty,
                expiry: int("expiryTime")?,
            })
        })
        .collect()
}

/// The escalation to run in `system`, by `cap` and the clock: the first by
/// id that has not run out and that the hull fits. Pure.
pub fn pick_escalation(
    rows: &[EscalationRow],
    system: i64,
    cap: Option<i64>,
    now: i64,
) -> Option<i64> {
    rows.iter()
        .filter(|r| r.system == system && r.expiry > now && fits(r.difficulty, cap))
        .map(|r| r.instance_id)
        .min()
}

/// **Follow an escalation of this system** (Y-ESCAL): read the Expeditions
/// list, and warp to the instance as the transmission's Warp To does. `None`
/// when the list has nothing here for this hull (or the server refuses the
/// warp: the Agency's anomalies are next).
pub fn escalation_here(c: &mut Conn, system: i64, cap: Option<i64>) -> Result<Option<i64>> {
    let outcome = c.call(ESCALATION_SERVICE, ESCALATION_METHOD, vec![])?;
    let Some(value) = outcome.value().filter(|_| outcome.refusal().is_none()) else {
        return Ok(None);
    };
    let rows = read_escalations(&value);
    let Some(id) = pick_escalation(&rows, system, cap, eve_server::pyutil::filetime_now()) else {
        return Ok(None);
    };
    let outcome = c.bound_call_kw(
        "CmdWarpToStuff",
        vec![
            PyValue::Bytes(ESCALATION_SUBJECT.to_vec()),
            PyValue::Int(id),
        ],
        vec![(
            PyValue::Real(WARP_RANGE),
            PyValue::Bytes(b"minRange".to_vec()),
        )],
    )?;
    if outcome.refusal().is_some() {
        return Ok(None);
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))?;
    Ok(Some(id))
}

/// **The decision point** (Y-COMBAT): the highest anomaly difficulty a hull
/// runs, by its size class (`hull_choice::HULLS`), or `None` for a hull the
/// chooser does not know (no cap).
///
/// **[A]**: frigates 2, destroyers 3, cruisers 4 (and, PROG-PVE,
/// battlecruisers 4, battleships 5). The shape is the data's:
/// the dungeons' own gate ship lists admit frigates at difficulty 1,
/// destroyers at 2, battlecruisers at 3 and battleships at 4-5
/// (`typelists` 482, 478, 392/415/441, 459 [V-data]), so a class runs
/// sites that admit one class more than itself. The cut is ours. Falsified
/// by runners that lose hulls to a site (too high) or find none in their
/// band's difficulties (too low; high-sec holds 1-2, low-sec 2-3, null
/// 3-5, `content/population.tsv`).
pub fn difficulty_cap(hull: i64) -> Option<i64> {
    use crate::hull_choice::{Class, HULLS};
    let h = HULLS.iter().find(|h| h.type_id == hull)?;
    Some(match h.class {
        Class::Frigate => 2,
        Class::Destroyer => 3,
        Class::Cruiser => 4,
        // PROG-PVE **[A]**: a battlecruiser runs what a cruiser does (the
        // sites that admit battlecruisers start at difficulty 3, the cap
        // is not raised past 4 for a hull one rung above the cruiser), a
        // battleship all five (the gate lists that admit battleships are
        // difficulty 4 and 5 [V-data]). Falsified by a battleship that
        // finds no difficulty-5 anomaly in its band (then it idles) or by
        // battlecruisers lost at difficulty 4.
        Class::Battlecruiser => 4,
        Class::Battleship => 5,
    })
}

/// Whether a row's difficulty is within `cap`. A row with no difficulty, or
/// no cap, fits.
pub fn fits(difficulty: Option<i64>, cap: Option<i64>) -> bool {
    match (difficulty, cap) {
        (Some(d), Some(c)) => d <= c,
        _ => true,
    }
}

/// How near the runner must be to a gate to call it, metres from the gate's
/// surface by the runner's own park. **[A]**: inside the client's default
/// 2,500 m (`gateActivationRange`, `maxStargateJumpingDistance` [V]) with
/// room for the park's drift and the ship's own radius.
pub const GATE_APPROACH: f64 = 2_000.0;

/// The range it closes to, so that [`GATE_APPROACH`] is met with margin.
/// **[A]**.
pub const GATE_FOLLOW: f64 = 1_000.0;

/// The acceleration gate nearest the ship on this grid: the slim that says
/// `gateActivationRange` (the server's mark, `combat_site::KEY_GATE_RANGE`).
pub fn gate_on_grid(c: &mut Conn) -> Option<i64> {
    let world = c.world_now();
    let ego = world.ego_ball()?;
    let at = ego.motion.position;
    world
        .balls()
        .filter(|b| b.slim.is_some_and(|s| s.gate_range.is_some()))
        .min_by(|a, b| {
            let d = |t: &crate::park::Tracked| {
                eve_server::space_state::distance(t.motion.position, at) - t.radius
            };
            d(a).total_cmp(&d(b))
        })
        .map(|b| b.ball_id)
}

/// **Take a gate**: close to [`GATE_APPROACH`], call
/// `keeper.ActivateAccelerationGate`, and ride the warp to the next room.
/// `Ok(false)` when the gate does not admit this hull (`DunShipCannotWarp`,
/// `DunBlacklistCannotWarp`: the runner leaves the site as it would a full
/// one); any other refusal is an error.
pub fn through(c: &mut Conn, gate: i64) -> Result<bool> {
    let near = |c: &mut Conn| {
        let w = c.world_now();
        match (w.ego_ball(), w.get(gate)) {
            (Some(e), Some(g)) => {
                eve_server::space_state::distance(e.motion.position, g.motion.position) - g.radius
                    <= GATE_APPROACH
            }
            _ => false,
        }
    };
    if !near(c) {
        let far = {
            let w = c.world_now();
            match (w.ego_ball(), w.get(gate)) {
                (Some(e), Some(g)) => {
                    eve_server::space_state::distance(e.motion.position, g.motion.position)
                        - g.radius
                }
                _ => return Ok(false),
            }
        };
        if far > crate::MIN_WARP_DISTANCE {
            let outcome = c.warp_to_item(gate, GATE_FOLLOW)?;
            if let Some(key) = outcome.refusal() {
                return Err(Error::Refused(format!("sites: warp to gate {gate}: {key}")));
            }
            settle(c, warping)?;
            settle(c, |c| !warping(c))?;
        } else {
            c.follow(gate, GATE_FOLLOW)?;
            settle(c, |c| near(c))?;
        }
    }
    let outcome = c.call(
        "keeper",
        "ActivateAccelerationGate",
        vec![PyValue::Int(gate)],
    )?;
    if let Some(key) = outcome.refusal() {
        if key.contains("DunShipCannotWarp") || key.contains("DunBlacklistCannotWarp") {
            return Ok(false);
        }
        return Err(Error::Refused(format!("sites: gate {gate}: {key}")));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))?;
    Ok(true)
}

fn agency(c: &mut Conn, method: &str) -> Result<PyValue> {
    let outcome = c.call(AGENCY, method, vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("sites: {method}: {key}")));
    }
    outcome
        .value()
        .ok_or_else(|| Error::Protocol(format!("sites: {method} answered nothing")))
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

/// Read pushes until `done` holds or [`TRAVEL_CEILING`] passes.
fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + TRAVEL_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "sites: still warping after the travel ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, Hostile, ParkView, Place, Policy, ShipInfo};

    const A: i64 = 8_100_000_001;
    const B: i64 = 8_100_000_002;
    const C: i64 = 8_100_000_003;
    const RUNNER1: i64 = 90_000_001;
    const RUNNER2: i64 = 90_000_002;

    fn pitch() -> Pitch {
        ("127.0.0.1:1".parse().expect("literal"), 30_000_001)
    }

    #[test]
    fn the_sites_row_is_not_civilian() {
        assert!(!ROLE.civilian);
        assert_eq!(ROLE.name(), "sites");
    }

    /// The rule as a table: last and held are never picked, the seed spreads
    /// over the rest in id order, and nothing free is `None`.
    #[test]
    fn the_pick_skips_the_last_site_and_every_held_one() {
        let none = BTreeSet::new();
        let held_b = BTreeSet::from([B]);
        let all = [C, A, B];
        type Row<'a> = (u64, Option<i64>, &'a BTreeSet<i64>, Option<i64>);
        let rows: [Row; 7] = [
            (0, None, &none, Some(A)),
            (1, None, &none, Some(B)),
            (2, None, &none, Some(C)),
            (0, Some(A), &none, Some(B)),
            (0, Some(A), &held_b, Some(C)),
            (1, Some(A), &held_b, Some(C)),
            (0, Some(C), &BTreeSet::from([A, B]), None),
        ];
        for (seed, last, held, want) in rows {
            assert_eq!(
                pick(&all, seed, last, held),
                want,
                "{seed} {last:?} {held:?}"
            );
        }
        assert_eq!(pick(&[], 0, None, &none), None);
    }

    /// **Two runners in one system never pick the same site twice in a
    /// row**, whatever their seeds, over many rounds of alternate picks:
    /// each pick differs from the runner's own last and from the other's
    /// standing claim.
    #[test]
    fn two_runners_never_pick_the_same_site_twice_in_a_row() {
        let sites = [A, B, C];
        for seeds in [(0, 0), (1, 1), (0, 3), (7, 2)] {
            let board = Board::new();
            let now = Instant::now();
            let mut mine: BTreeMap<i64, i64> = BTreeMap::new();
            for round in 0..20 {
                for (runner, seed) in [(RUNNER1, seeds.0), (RUNNER2, seeds.1)] {
                    let other = if runner == RUNNER1 { RUNNER2 } else { RUNNER1 };
                    let got = board
                        .choose(pitch(), runner, seed, &sites, now)
                        .expect("three sites, two runners: one is always free");
                    assert_ne!(
                        Some(&got),
                        mine.get(&runner),
                        "round {round}: twice in a row"
                    );
                    assert_ne!(
                        Some(&got),
                        mine.get(&other),
                        "round {round}: the other's site"
                    );
                    mine.insert(runner, got);
                }
            }
            let picks = board.picks(pitch());
            assert_eq!(picks.len(), 40);
            for w in picks.windows(2) {
                assert_ne!(w[0].1, w[1].1, "consecutive picks on one pitch differ");
            }
        }
    }

    /// A claim older than [`CLAIM_TTL`] no longer holds a site; another
    /// pitch's claims never do.
    #[test]
    fn a_stale_claim_or_another_pitch_holds_nothing() {
        let board = Board::new();
        let then = Instant::now();
        assert_eq!(board.choose(pitch(), RUNNER1, 0, &[A], then), Some(A));
        assert_eq!(board.choose(pitch(), RUNNER2, 0, &[A], then), None);
        let other: Pitch = (pitch().0, 30_000_002);
        assert_eq!(board.choose(other, RUNNER2, 0, &[A], then), Some(A));
        assert_eq!(
            board.choose(pitch(), RUNNER2, 0, &[A], then + CLAIM_TTL),
            Some(A)
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

    /// The Agency's shapes: counts as ints, the own system as rows whose
    /// `siteID` is the warp's id.
    #[test]
    fn the_agency_reads_counts_and_own_rows() {
        let row = kv(vec![
            ("siteID", PyValue::Int(A)),
            ("dungeonID", PyValue::Int(999_001)),
            ("difficulty", PyValue::Int(2)),
            (
                "position",
                PyValue::Tuple(vec![
                    PyValue::Real(1.0),
                    PyValue::Real(2.0),
                    PyValue::Real(3.0),
                ]),
            ),
        ]);
        let answer = PyValue::Dict(vec![
            (PyValue::Int(2), PyValue::Int(30_000_002)),
            (PyValue::List(vec![row]), PyValue::Int(30_000_001)),
        ]);
        assert_eq!(
            read_counts(&answer),
            BTreeMap::from([(30_000_001, 1), (30_000_002, 2)])
        );
        assert_eq!(
            read_rows(&answer, 30_000_001),
            vec![SiteRow {
                site_id: A,
                dungeon_id: Some(999_001),
                difficulty: Some(2),
                position: Some([1.0, 2.0, 3.0]),
            }]
        );
        assert!(
            read_rows(&answer, 30_000_002).is_empty(),
            "a count has no rows"
        );
    }

    fn gun() -> crate::brain::GunInfo {
        crate::brain::GunInfo {
            item_id: 2,
            type_id: escort::GUN,
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
        }
    }

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    /// In space with guns: a quiet grid settles before the next site; an NPC
    /// is fought (the escort's rule) and restarts the settle.
    #[test]
    fn a_quiet_grid_settles_then_runs_the_next_site_and_a_rat_is_fought() {
        let mut m = Memory::new("Runner1", Policy::fast());
        let now = Instant::now();
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        o.guns = vec![gun()];
        o.lock_range = 20_000.0;
        let mut park = ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        };
        o.park = Some(park.clone());
        // Landed: the settle starts.
        assert_eq!(in_space(&mut m, &o), vec![Act::Wait(SHORT_WAIT)]);
        // Still settling.
        assert_eq!(in_space(&mut m, &o), vec![Act::Wait(SHORT_WAIT)]);
        // A rat comes in with the grid: the escort's rules fight it.
        park.hostiles = vec![Hostile {
            ball: ball(50, 5_000.0),
            name: "Rat".into(),
            type_id: 1,
            shooting: None,
        }];
        o.park = Some(park.clone());
        let fight = in_space(&mut m, &o);
        assert!(
            fight.iter().any(|a| matches!(a, Act::Orbit { id: 50, .. })),
            "{fight:?}"
        );
        // It dies: quiet again, and the settle starts over.
        park.hostiles.clear();
        o.park = Some(park.clone());
        let after = in_space(&mut m, &o);
        assert_eq!(after.last(), Some(&Act::Wait(SHORT_WAIT)), "{after:?}");
        // Past the settle: the next site.
        o.now = now + SITE_SETTLE * 2;
        assert_eq!(in_space(&mut m, &o).last(), Some(&Act::Sites));
    }

    /// The escort's rules pass over a wreck and a rat beyond its guard
    /// radius; on the site's grid the runner approaches the wreck and loots
    /// it in reach, and closes on the rat -- by warp from beyond the warp
    /// floor. Nothing is left before the next site.
    #[test]
    fn far_wrecks_are_looted_and_far_rats_closed_on() {
        let beyond = escort::GUARD_RADIUS * 1.5;
        assert!(beyond < crate::MIN_WARP_DISTANCE, "the table's premise");
        let mut m = Memory::new("Runner1", Policy::fast());
        let now = Instant::now();
        let mut o = armed(now);
        let mut park = empty_park();
        park.wrecks = vec![ball(60, beyond)];
        o.park = Some(park.clone());
        assert_eq!(
            in_space(&mut m, &o),
            vec![Act::Follow {
                id: 60,
                range: escort::WRECK_APPROACH
            }]
        );
        park.ego_tether = Some(60);
        o.park = Some(park.clone());
        assert_eq!(in_space(&mut m, &o), vec![Act::Wait(SHORT_WAIT)]);
        park.wrecks = vec![ball(60, 0.0)];
        o.park = Some(park.clone());
        assert_eq!(in_space(&mut m, &o), vec![Act::Loot(60)]);
        // A wreck never reached is let go after the patience.
        park.wrecks.push(ball(61, beyond));
        park.ego_tether = None;
        o.park = Some(park.clone());
        assert!(matches!(
            in_space(&mut m, &o)[..],
            [Act::Follow { id: 61, .. }]
        ));
        o.now = now + LOOT_PATIENCE * 2;
        assert_eq!(
            in_space(&mut m, &o),
            vec![Act::Wait(SHORT_WAIT)],
            "settling"
        );
        assert!(m.looted.contains(&61));

        park.wrecks.clear();
        park.hostiles = vec![Hostile {
            ball: ball(70, beyond),
            name: "Rat".into(),
            type_id: 1,
            shooting: None,
        }];
        o.park = Some(park.clone());
        assert!(matches!(
            in_space(&mut m, &o)[..],
            [Act::Follow { id: 70, .. }]
        ));
        park.hostiles[0].ball = ball(70, crate::MIN_WARP_DISTANCE * 2.0);
        o.park = Some(park);
        assert!(matches!(
            in_space(&mut m, &o)[..],
            [Act::WarpToItem { id: 70, .. }]
        ));
    }

    fn armed(now: Instant) -> Observation {
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        o.guns = vec![gun()];
        o.lock_range = 20_000.0;
        o
    }

    fn empty_park() -> ParkView {
        ParkView {
            ego: ball(1, 0.0),
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

    /// A full hold on a quiet grid goes home rather than to another site.
    #[test]
    fn a_full_hold_goes_home() {
        let mut m = Memory::new("Runner1", Policy::fast());
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(escort::RIFTER),
        });
        o.guns = vec![gun()];
        o.hold_full = true;
        o.park = Some(ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: Some(ball(60_000_001, 1.0e9)),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        let acts = in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::HoldFull));
        assert!(
            matches!(acts.last(), Some(Act::WarpToItem { id: 60_000_001, .. })),
            "{acts:?}"
        );
    }

    fn expeditions(rows: &[(i64, i64, Option<i64>, i64)]) -> PyValue {
        use eve_server::pyutil::{bstr, instance, keyval};
        let header = ["instanceID", "destDungeon", "expiryTime", "solarSystemID"];
        let lines = rows
            .iter()
            .map(|(id, system, difficulty, expiry)| {
                let dest = match difficulty {
                    Some(d) => keyval(vec![("difficulty", PyValue::Int(*d))]),
                    None => PyValue::None,
                };
                PyValue::List(vec![
                    PyValue::Int(*id),
                    dest,
                    PyValue::Int(*expiry),
                    PyValue::Int(*system),
                ])
            })
            .collect();
        instance(
            "eve.common.script.sys.rowset.Rowset",
            vec![
                (
                    "header",
                    PyValue::List(header.iter().map(|h| bstr(h)).collect()),
                ),
                ("lines", PyValue::List(lines)),
            ],
        )
    }

    /// The Expeditions rowset is read by column name, a row with no
    /// `destDungeon` is skipped (as the client does), and an empty or
    /// unexpected answer reads as nothing.
    #[test]
    fn the_expeditions_list_is_read_by_column_name() {
        let v = expeditions(&[(A, 7, Some(3), 100), (B, 7, None, 100), (C, 8, Some(4), 90)]);
        let rows = read_escalations(&v);
        assert_eq!(
            rows,
            vec![
                EscalationRow {
                    instance_id: A,
                    system: 7,
                    difficulty: Some(3),
                    expiry: 100
                },
                EscalationRow {
                    instance_id: C,
                    system: 8,
                    difficulty: Some(4),
                    expiry: 90
                },
            ]
        );
        assert!(read_escalations(&expeditions(&[])).is_empty());
        assert!(read_escalations(&PyValue::None).is_empty());
    }

    /// Own system only, not run out, within the hull's cap, lowest id first.
    #[test]
    fn an_escalation_is_followed_in_this_system_by_the_clock_and_the_cap() {
        let row = |id, system, difficulty, expiry| EscalationRow {
            instance_id: id,
            system,
            difficulty,
            expiry,
        };
        let rows = [
            row(C, 7, Some(3), 100),
            row(B, 7, Some(5), 100),
            row(A, 8, Some(3), 100),
        ];
        let cap = difficulty_cap(escort::RIFTER);
        let hard = cap.map(|c| c + 3);
        assert_eq!(pick_escalation(&rows, 7, None, 50), Some(B));
        assert_eq!(pick_escalation(&rows, 8, None, 50), Some(A));
        assert_eq!(pick_escalation(&rows, 9, None, 50), None);
        assert_eq!(pick_escalation(&rows, 7, None, 100), None, "run out");
        let low = [row(B, 7, hard, 100), row(C, 7, cap, 100)];
        assert_eq!(pick_escalation(&low, 7, cap, 50), Some(C), "a hull's cap");
    }
}
