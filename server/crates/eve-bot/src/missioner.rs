//! **The mission runner** (R-SOCIETY SOC-B5): a bot that runs an agent's
//! missions the way a player does -- kill missions in its combat ship, and
//! couriers when it has no ship to fight in or its agent hands it one.
//!
//! [`ROLE`] is the `--loop missions` row and the `mission_runner` career's
//! role. Every step is a player verb (`verbs.rs`); the agent's own state on
//! the server is the bot's memory, so nothing about a mission is kept
//! bot-side and a reconnect loses nothing.
//!
//! 1. **Docked** it arms itself exactly as the escort does
//!    ([`escort::docked`]: a Rifter, two guns, rounds; unload and sell the
//!    last trip's loot), dwells, and where the escort would undock it asks
//!    for [`Act::Mission`] instead. That act's arm is [`run`], which docked
//!    is [`desk`]:
//!    - **what is already accepted** (`GetMyJournalDetails`): a mission the
//!      agent will now complete is completed and its rewards counted (the
//!      wallet's change, and the objective's `loyaltyPoints`); a kill
//!      under way sends the bot out again; a courier under way is hauled
//!      ([`courier::run`]); a kill whose pocket is gone is quit;
//!    - **a new mission**: the policy's agent (`eve-bots --agent ID`), or
//!      else the agents of this station ([`candidates`], from
//!      `agentMgr.GetAgents`), are talked to; the offer is classified by its
//!      keywords ([`offer_kind`]) and [`choose`] takes it, hauls it, or
//!      declines it. A kill taken: undock.
//! 2. **In space** it plays the site runner's rules
//!    ([`crate::site_runner`]'s `in_space`, itself the escort's): fight
//!    whatever is on the grid, loot the wrecks, and once the grid has been
//!    quiet for `SITE_SETTLE`, where the site runner would look for the next
//!    anomaly, it asks for [`Act::Mission`], whose arm in space is
//!    [`field`]: the journal's kill mission, and then
//!    - the objective met: fly to the agent's station and dock
//!      ([`travel::go`]); docked, step 1 completes it;
//!    - another system: fly to the agent's station, undock;
//!    - off the pocket's grid: `WarpToLocation('dungeon', ...)` on the
//!      agent, the journal bookmark's "warp to";
//!    - on the grid, quiet, not cleared: a refusal, which sends the bot
//!      home ([`Homing::Recovery`]).
//!
//! **Kill or courier** (the `[A]` rule, [`ship_level`] and [`choose`]): a
//! bot whose ship has guns and a hull in [`HULL_LEVELS`] runs **kill
//! missions** at security agents (division 24) up to the hull's level and
//! declines couriers (a combat hull's hold is small, and the cargo is not
//! known before accepting); a bot with no gun or no such hull runs
//! **couriers** at distribution agents (division 22) up to
//! [`COURIER_MAX_LEVEL`] and declines kills. A kill above the hull's
//! level, or any other kind of mission, is declined.
//!
//! **Every rule here is [A] policy** -- what a player would do -- except
//! the client's own calls and the agent's buttons, which the verbs cite.

use crate::brain::{self, Act, ActKind, Homing, Memory, Observation};
use crate::courier::{self, Delivery};
use crate::escort;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::{self, Map, Trips};
use crate::verbs::{AgentTurn, BUTTON_ACCEPT, BUTTON_COMPLETE, BUTTON_DECLINE, BUTTON_QUIT};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The agent service **[V]** (`eveMoniker.GetAgent`, `docs/re/agents-missions.md`).
pub const AGENT_SERVICE: &str = "agentMgr";

/// `agentDivisionSecurity` and `agentDivisionDistribution`: the divisions
/// whose pools are kill missions and couriers (C4 §3.3,
/// `mission_pool::Table::key`) **[V]**.
pub const DIVISION_SECURITY: i64 = 24;
pub const DIVISION_DISTRIBUTION: i64 = 22;

/// The agent type that draws from the pool (`agentTypeBasicAgent`) **[V]**.
pub const AGENT_TYPE_BASIC: i64 = eve_server::mission_pool::TYPE_BASIC;

/// `missionStateAccepted` in a journal row (`mission.rs::mission_state`)
/// **[V]**.
pub const MISSION_STATE_ACCEPTED: i64 = 2;

/// **The level of kill mission a hull runs**, by its group. **[A]**, the
/// players' rule of thumb: frigates (and the rookie ship's corvette) level
/// 1, destroyers 2, cruisers 3, battlecruisers and battleships 4. The group
/// ids are `invGroups` rows **[I]** (frigate 25, cruiser 26, battleship 27,
/// corvette 237, assault frigate 324, heavy assault cruiser 358, combat
/// battlecruiser 419, destroyer 420, attack battlecruiser 1201). A hull
/// whose group is not here runs couriers only. Falsified by a bot in a hull
/// a player would run kills in that only hauls, or by a frigate losing
/// level-2 pockets.
pub const HULL_LEVELS: &[(i64, i64)] = &[
    (25, 1),
    (237, 1),
    (324, 1),
    (420, 2),
    (26, 3),
    (358, 3),
    (419, 4),
    (1201, 4),
    (27, 4),
];

/// The highest courier level a bot hauls at a distribution agent. **[A]**:
/// a level-1 courier fits any hull's hold; higher levels carry more than a
/// frigate holds and want standings a new pilot lacks. Falsified by a
/// hauler that never finds a courier, or one whose loads never fit.
pub const COURIER_MAX_LEVEL: i64 = 1;

/// How many agents one visit to the desk talks to. **[A]**
pub const AGENTS_PER_VISIT: usize = 3;

/// How many offers one agent may have declined in one visit before the
/// bot tries the next agent. **[A]** (Declining costs nothing here: the
/// server keeps no decline timer.)
pub const DECLINES_PER_AGENT: usize = 2;

/// On the pocket's grid: the ship within this of the pocket's centre.
/// **[A]**: the client's own warp floor, so anything nearer is reached by
/// approaching, not warping. Falsified by a bot that warps to a pocket it
/// is already fighting in.
pub const POCKET_GRID: f64 = crate::MIN_WARP_DISTANCE;

/// Where to land from the pocket's centre: the escort's warp standoff.
/// **[A]**
pub const WARP_RANGE: f64 = escort::WARP_STANDOFF;

/// How long a warp may take before it counts as stuck: the courier's
/// ceiling.
pub const TRAVEL_CEILING: Duration = courier::TRAVEL_CEILING;

/// How long the warp's settle waits for each burst.
const POLL: Duration = Duration::from_millis(250);

/// The `MISSING` line of a desk visit that found nothing to run.
pub const MISSING_AGENT_ACT: &str =
    "an agent at this station whose missions this ship can run (or eve-bots --agent ID)";

/// Runs missions, and fights in them: not a civilian.
pub static ROLE: RoleSpec = RoleSpec {
    name: "missions",
    greeting: "o7 -- running missions",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

// ---------------------------------------------------------------------------
// The role: pure rules
// ---------------------------------------------------------------------------

/// Docked: the escort's rules -- arm, unload and sell the loot, dwell --
/// and then the desk ([`Act::Mission`]) where the escort would undock. A
/// bot the escort's rules cannot arm (no Rifter to be had, so `MISSING`)
/// but which sits in a hull goes to the desk to haul ([`ship_level`] 0).
/// After a desk visit that failed, a `MISSING` line and its wait first.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let mut acts = escort::docked(m, o);
    let unarmed_hull = acts.iter().any(|a| matches!(a, Act::Missing(_))) && in_a_hull(o);
    if unarmed_hull {
        acts = brain::dwell_then_undock(m, o);
    }
    if acts.last() != Some(&Act::Undock) {
        return acts;
    }
    acts.pop();
    if o.failed(ActKind::Mission) > 0 {
        acts.push(Act::Missing(MISSING_AGENT_ACT));
        acts.push(Act::Wait(brain::MISSING_WAIT));
    }
    acts.push(Act::Mission {
        agent: m.policy.agent,
        level: ship_level(o),
    });
    acts
}

/// In space: the site runner's rules, with its "next anomaly" made the
/// field visit ([`Act::Mission`]). A failed field visit sends the bot home.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if m.homing.is_none() && o.failed(ActKind::Mission) > 0 {
        m.homing = Some(Homing::Recovery);
    }
    let acts = (crate::site_runner::ROLE.in_space)(m, o);
    acts.into_iter()
        .map(|a| match a {
            Act::Sites => Act::Mission {
                agent: m.policy.agent,
                level: 0,
            },
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

/// The escort's kit: the gunnery skill, a Rifter, its guns and rounds.
fn setup(lacks: Lacks) -> Vec<String> {
    (crate::site_runner::ROLE.setup)(lacks)
}

fn in_a_hull(o: &Observation) -> bool {
    o.ship
        .and_then(|s| s.type_id)
        .is_some_and(|t| t != brain::CAPSULE)
}

/// **The level of kill mission this ship runs** (the module header's
/// rule): 0 -- couriers only -- with no gun, or a hull whose group is not
/// in [`HULL_LEVELS`]. Docked, the active ship is a hangar row.
pub fn ship_level(o: &Observation) -> i64 {
    if o.guns.is_empty() {
        return 0;
    }
    let Some(ship) = o.ship else {
        return 0;
    };
    o.hangar
        .iter()
        .find(|r| r.item_id == ship.ship_id)
        .map_or(0, |r| hull_level(r.group_id))
}

/// [`HULL_LEVELS`]' level for a hull group, 0 for any other.
pub fn hull_level(group_id: i64) -> i64 {
    HULL_LEVELS
        .iter()
        .find(|(g, _)| *g == group_id)
        .map_or(0, |(_, l)| *l)
}

// ---------------------------------------------------------------------------
// What the agents say: readers, pure
// ---------------------------------------------------------------------------

/// What an offer is, from its keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Kill,
    Courier,
    /// Talk, mining, anything else.
    Other,
}

/// **The offer's kind** from `GetMissionKeywords`: a kill names its pocket
/// (`dungeonLocationID`), a courier its drop-off (`objectiveDestinationID`)
/// -- the keywords the briefing's prose is filled from **[I]** (our
/// server's `mission.rs::keywords_of`). Falsified by a kill offer whose
/// keywords carry no `dungeonLocationID`.
pub fn offer_kind(keywords: &PyValue) -> Kind {
    if crate::dict_get(keywords, "dungeonLocationID").is_some() {
        Kind::Kill
    } else if crate::dict_get(keywords, "objectiveDestinationID").is_some() {
        Kind::Courier
    } else {
        Kind::Other
    }
}

/// What to do with an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    Kill,
    Courier,
    /// Decline it, and why.
    Decline(&'static str),
}

/// **The kill-or-courier rule** (module header), pure: a fighter
/// (`ship_level` > 0) takes a kill it runs, a hauler (0) a courier;
/// anything else is declined.
pub fn choose(kind: Kind, agent_level: i64, ship_level: i64) -> Take {
    match kind {
        Kind::Kill if ship_level == 0 => Take::Decline("no ship to fight in"),
        Kind::Kill if agent_level > ship_level => Take::Decline("above this hull's level"),
        Kind::Kill => Take::Kill,
        Kind::Courier if ship_level > 0 => Take::Decline("a fighter does not haul"),
        Kind::Courier => Take::Courier,
        Kind::Other => Take::Decline("not a kill or a courier"),
    }
}

/// One `GetAgents` row, the columns the desk reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRow {
    pub id: i64,
    pub agent_type: i64,
    pub division: i64,
    pub level: i64,
    pub station: Option<i64>,
    pub system: Option<i64>,
    /// The agent's corporation and faction (PROG-PVE: the standings an
    /// agent level asks for are toward these and the agent).
    pub corp: Option<i64>,
    pub faction: Option<i64>,
}

/// **`agentMgr.GetAgents`** read: a `Rowset` instance whose state is
/// `header` (column names) and `lines` (one list per agent)
/// (`service/agents.rs::AGENT_COLUMNS`) **[V]**. Columns by name, so their
/// order does not matter; a row without an id is skipped.
pub fn read_agents(v: &PyValue) -> Vec<AgentRow> {
    let state = crate::attributes(v);
    let (Some(PyValue::List(header)), Some(PyValue::List(lines))) = (
        crate::dict_get(state, "header"),
        crate::dict_get(state, "lines"),
    ) else {
        return Vec::new();
    };
    let names: Vec<String> = header
        .iter()
        .map(|h| crate::text(h).unwrap_or_default())
        .collect();
    let col = |name: &str| names.iter().position(|n| n == name);
    let (id, ty, div, lvl, st, sys) = (
        col("agentID"),
        col("agentTypeID"),
        col("divisionID"),
        col("level"),
        col("stationID"),
        col("solarsystemID"),
    );
    let (corp, faction) = (col("corporationID"), col("factionID"));
    lines
        .iter()
        .filter_map(|line| {
            let PyValue::List(cells) = line else {
                return None;
            };
            let int = |c: Option<usize>| match cells.get(c?) {
                Some(PyValue::Int(n)) => Some(*n),
                _ => None,
            };
            Some(AgentRow {
                id: int(id)?,
                agent_type: int(ty).unwrap_or_default(),
                division: int(div).unwrap_or_default(),
                level: int(lvl).unwrap_or(1),
                station: int(st),
                system: int(sys),
                corp: int(corp),
                faction: int(faction),
            })
        })
        .collect()
}

/// **The agents to talk to** at `station` (the module header's rule):
/// basic agents of the security division up to `ship_level`, or, with
/// `ship_level` 0, of the distribution division up to
/// [`COURIER_MAX_LEVEL`]; the highest level first, then the lowest id; at
/// most [`AGENTS_PER_VISIT`].
pub fn candidates(rows: &[AgentRow], station: i64, ship_level: i64) -> Vec<AgentRow> {
    candidates_with(rows, station, ship_level, None)
}

/// The standings toward the pilot, `{fromID: standing}`
/// (`standingMgr.GetCharStandings`): an absent id is 0.
pub type Standings = BTreeMap<i64, f64>;

/// **Whether the pilot may use the agent** (PROG-PVE): the client's own
/// `eveCfg.CanUseAgent` [V] over the standings toward the agent's faction,
/// corporation and the agent, as the server's `mission::can_use` runs it
/// (`mission_pool::can_use_agent`): level 1 is open to all, level 2 needs a
/// standing of 1.0 toward any of the three, level 3 needs 3.0, level 4 5.0,
/// level 5 7.0, and none of the three may sit under -2.0. Skill bonuses to
/// standing are not read **[A]** (falsified by a pilot with Connections who
/// is refused at the threshold the bot counts as open).
pub fn usable(a: &AgentRow, st: &Standings) -> bool {
    let of = |id: Option<i64>| id.and_then(|i| st.get(&i)).copied().unwrap_or(0.0);
    eve_server::mission_pool::can_use_agent(
        a.agent_type,
        a.level,
        eve_server::mission_pool::Standings {
            faction: of(a.faction),
            corp: of(a.corp),
            agent: of(Some(a.id)),
        },
        false,
    )
}

/// [`candidates`] with the pilot's standings (PROG-PVE): an agent whose level
/// the standings do not open is left out (the bot would only be told there
/// is no offer), and an agent in another station of this station's system
/// is a candidate too, after the ones here of the same level, so a pilot
/// whose standings have risen is not held at the level its home station
/// offers. `None` standings: every agent counts as open.
pub fn candidates_with(
    rows: &[AgentRow],
    station: i64,
    ship_level: i64,
    standings: Option<&Standings>,
) -> Vec<AgentRow> {
    let (division, max) = if ship_level > 0 {
        (DIVISION_SECURITY, ship_level)
    } else {
        (DIVISION_DISTRIBUTION, COURIER_MAX_LEVEL)
    };
    let system = rows
        .iter()
        .find(|a| a.station == Some(station))
        .and_then(|a| a.system);
    let mut out: Vec<AgentRow> = rows
        .iter()
        .filter(|a| {
            (a.station == Some(station) || (system.is_some() && a.system == system))
                && a.agent_type == AGENT_TYPE_BASIC
                && a.division == division
                && a.level <= max
                && standings.is_none_or(|st| usable(a, st))
        })
        .copied()
        .collect();
    out.sort_by_key(|a| (std::cmp::Reverse(a.level), a.station != Some(station), a.id));
    out.truncate(AGENTS_PER_VISIT);
    out
}

/// **`standingMgr.GetCharStandings`** read: a dict `{fromID: DBRow(fromID,
/// standing)}` **[V]** (`standingsvc.py:129`). A row that is not that shape is
/// skipped.
pub fn read_standings(v: &PyValue) -> Standings {
    let PyValue::Dict(entries) = v else {
        return Standings::new();
    };
    entries
        .iter()
        .filter_map(|(value, key)| {
            let PyValue::Int(from) = key else {
                return None;
            };
            let PyValue::DbRow(row) = value else {
                return None;
            };
            match row.values().get(1) {
                Some(eve_marshal::DbValue::Real(s)) => Some((*from, *s)),
                _ => None,
            }
        })
        .collect()
}

/// A kill mission's pocket, from its journal bookmark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pocket {
    pub system: i64,
    pub position: [f64; 3],
}

/// One `GetMyJournalDetails` mission row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Job {
    pub agent: i64,
    pub mission: i64,
    pub accepted: bool,
    /// The `'dungeon'` bookmark, while a pocket is in space.
    pub pocket: Option<Pocket>,
}

/// **`agentMgr.GetMyJournalDetails`** read: `[missions, offers]`, each
/// mission a 10-tuple `(missionState, importantMission, typeLabel, nameID,
/// agentID, expirationTime, bookmarks, remoteOfferable, remoteCompletable,
/// contentID)`, a bookmark a `KeyVal` with `locationType`, `locationID`
/// (the system) and `x`, `y`, `z` (`journal.py:413-451`) **[V]**.
pub fn read_journal(v: &PyValue) -> Vec<Job> {
    let rows = match v {
        PyValue::List(parts) | PyValue::Tuple(parts) => match parts.first() {
            Some(PyValue::List(rows)) => rows.as_slice(),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    rows.iter()
        .filter_map(|row| {
            let PyValue::Tuple(f) = row else {
                return None;
            };
            let int = |i: usize| match f.get(i) {
                Some(PyValue::Int(n)) => Some(*n),
                _ => None,
            };
            let pocket = match f.get(6) {
                Some(PyValue::List(marks)) => marks.iter().find_map(read_bookmark),
                _ => None,
            };
            Some(Job {
                agent: int(4)?,
                mission: int(9)?,
                accepted: int(0)? == MISSION_STATE_ACCEPTED,
                pocket,
            })
        })
        .collect()
}

fn read_bookmark(b: &PyValue) -> Option<Pocket> {
    let s = crate::attributes(b);
    if crate::dict_get(s, "locationType")
        .and_then(crate::text)
        .as_deref()
        != Some("dungeon")
    {
        return None;
    }
    let real = |k: &str| match crate::dict_get(s, k)? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    };
    let system = match crate::dict_get(s, "locationID")? {
        PyValue::Int(n) => *n,
        _ => return None,
    };
    Some(Pocket {
        system,
        position: [real("x")?, real("y")?, real("z")?],
    })
}

/// `completionStatus > 0` in the objective document: finished
/// (`agentDialogueUtil.py:409`) **[V]**.
pub fn objective_met(objective: &PyValue) -> bool {
    matches!(crate::dict_get(objective, "completionStatus"), Some(PyValue::Int(n)) if *n > 0)
}

fn int_of(v: &PyValue, key: &str) -> Option<i64> {
    match crate::dict_get(v, key)? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The act's arm
// ---------------------------------------------------------------------------

/// What one [`Act::Mission`] knows: the policy's agent, the ship's level
/// ([`ship_level`]) and, when the bot has a map, how to route.
#[derive(Debug, Clone, Copy)]
pub struct Errand<'a> {
    pub agent: Option<i64>,
    pub level: i64,
    pub trips: Option<Trips<'a>>,
}

/// One thing a visit did.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Turned in, and what it paid.
    Completed {
        agent: i64,
        mission: Option<i64>,
        isk: f64,
        lp: i64,
    },
    Accepted {
        agent: i64,
        mission: Option<i64>,
    },
    Declined {
        agent: i64,
        mission: Option<i64>,
        why: &'static str,
    },
    /// An agent the rule rules out before talking: its level.
    Skipped {
        agent: i64,
        why: &'static str,
    },
    Quit {
        agent: i64,
        mission: i64,
    },
    Hauled {
        agent: i64,
        delivery: Delivery,
    },
    Travelled {
        station: i64,
        jumps: u32,
    },
    Undocked,
    Warped {
        agent: i64,
    },
    /// The LP spend after a turn-in ([`crate::lp_spend`]), as a note.
    LpSpent {
        note: String,
    },
}

/// What one [`run`] did, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub steps: Vec<Step>,
}

impl Report {
    /// The missions turned in, and the ISK and LP they paid.
    pub fn paid(&self) -> (usize, f64, i64) {
        self.steps
            .iter()
            .fold((0, 0.0, 0), |(n, isk, lp), s| match s {
                Step::Completed { isk: i, lp: l, .. } => (n + 1, isk + i, lp + l),
                _ => (n, isk, lp),
            })
    }

    /// One line for the act log.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .steps
            .iter()
            .map(|s| match s {
                Step::Completed {
                    agent,
                    mission,
                    isk,
                    lp,
                } => format!(
                    "completed mission {} for agent {agent}: {isk:+.2} ISK, {lp} LP",
                    opt(*mission)
                ),
                Step::Accepted { agent, mission } => {
                    format!("accepted kill mission {} from agent {agent}", opt(*mission))
                }
                Step::Declined {
                    agent,
                    mission,
                    why,
                } => format!(
                    "declined mission {} from agent {agent} ({why})",
                    opt(*mission)
                ),
                Step::Skipped { agent, why } => format!("skipped agent {agent} ({why})"),
                Step::Quit { agent, mission } => {
                    format!("quit mission {mission} for agent {agent}")
                }
                Step::Hauled { agent, delivery } => format!(
                    "hauled courier {} for agent {agent}{}",
                    delivery.leg.mission_id,
                    if delivery.completed {
                        ", completed"
                    } else {
                        ""
                    }
                ),
                Step::Travelled { station, jumps } => {
                    format!("flew to station {station} ({jumps} jumps)")
                }
                Step::Undocked => "undocked".to_owned(),
                Step::Warped { agent } => format!("warped to agent {agent}'s pocket"),
                Step::LpSpent { note } => format!("LP: {note}"),
            })
            .collect();
        parts.join("; ")
    }
}

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "?".to_owned(), |n| n.to_string())
}

/// **One [`Act::Mission`]**: [`desk`] docked, [`field`] in space. A desk
/// visit that turned a mission in (completed, or a haul the agent called
/// complete) then spends the LP it paid ([`spend_lp`], X-LPSTORE).
pub fn run(c: &mut Conn, e: &Errand<'_>) -> Result<Report> {
    if c.session().station_id().is_some() {
        let mut r = desk(c, e)?;
        let paid = r.steps.iter().any(|s| {
            matches!(s, Step::Completed { .. })
                || matches!(s, Step::Hauled { delivery, .. } if delivery.completed)
        });
        if paid && c.session().station_id().is_some() {
            r.steps.push(Step::LpSpent { note: spend_lp(c)? });
        }
        Ok(r)
    } else {
        field(c, e)
    }
}

/// **Spend the LP a turn-in paid** ([`crate::lp_spend::spend`] over every
/// corporation the pilot holds LP with, the agents' own). A refusal is a
/// note and never fails the visit.
fn spend_lp(c: &mut Conn) -> Result<String> {
    match crate::lp_spend::spend(c, None) {
        Ok(s) => Ok(s.summary()),
        Err(Error::Refused(why) | Error::NotSent(why)) => Ok(why),
        Err(e) => Err(e),
    }
}

/// **Docked: at the agents' desk** (module header, step 1). Ok when it
/// turned something in, took or hauled a mission, or set out for a kill;
/// a refusal when no agent had anything this ship runs.
pub fn desk(c: &mut Conn, e: &Errand<'_>) -> Result<Report> {
    let mut r = Report::default();
    // What is already accepted, with any agent.
    for job in journal(c)?.into_iter().filter(|j| j.accepted) {
        let turn = c.talk_to_agent(job.agent)?;
        if turn.action_for(BUTTON_COMPLETE).is_some() {
            r.steps.push(complete(c, job.agent, &turn)?);
            continue;
        }
        match offer_kind(&keywords(c, job.agent, Some(job.mission))?) {
            Kind::Kill if job.pocket.is_some() && e.level > 0 => {
                undock(c)?;
                r.steps.push(Step::Undocked);
                return Ok(r);
            }
            Kind::Kill if job.pocket.is_some() => {}
            Kind::Courier => {
                let delivery = courier::run(c, job.agent)?;
                r.steps.push(Step::Hauled {
                    agent: job.agent,
                    delivery,
                });
                return Ok(r);
            }
            // A kill whose pocket is gone (a restart forgets pockets) can
            // never be finished, nor anything this bot does not run. [A]
            Kind::Kill | Kind::Other => {
                c.agent_button(job.agent, &turn, BUTTON_QUIT)?;
                r.steps.push(Step::Quit {
                    agent: job.agent,
                    mission: job.mission,
                });
            }
        }
    }

    // A new mission.
    let here = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("missions: not docked".to_owned()))?;
    let rows = agents(c)?;
    let agents: Vec<AgentRow> = match e.agent {
        Some(id) => vec![
            rows.iter()
                .find(|a| a.id == id)
                .copied()
                .unwrap_or(AgentRow {
                    id,
                    agent_type: AGENT_TYPE_BASIC,
                    division: 0,
                    // An agent GetAgents does not list: level 1. [A]
                    level: 1,
                    station: None,
                    system: None,
                    corp: None,
                    faction: None,
                }),
        ],
        None => {
            // PROG-PVE: only the agents these standings open. A refused
            // read leaves the pilot at level 1's open door.
            let st = match standings(c) {
                Ok(st) => st,
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => Standings::new(),
            };
            candidates_with(&rows, here, e.level, Some(&st))
        }
    };
    for agent in agents {
        if e.level > 0 && agent.division == DIVISION_SECURITY && agent.level > e.level {
            r.steps.push(Step::Skipped {
                agent: agent.id,
                why: "above this hull's level",
            });
            continue;
        }
        if let Some(station) = agent.station.filter(|s| *s != here)
            && let Some(trips) = e.trips
        {
            let trip = travel::go(c, trips, station)?;
            r.steps.push(Step::Travelled {
                station,
                jumps: trip.jumps,
            });
        }
        for _ in 0..=DECLINES_PER_AGENT {
            let turn = c.talk_to_agent(agent.id)?;
            if turn.action_for(BUTTON_ACCEPT).is_none() {
                // No offer, standings too low, or a mission already under
                // way that the journal pass left (a kill with no ship).
                break;
            }
            let kind = offer_kind(&keywords(c, agent.id, turn.content_id)?);
            match choose(kind, agent.level, e.level) {
                Take::Kill => {
                    c.agent_button(agent.id, &turn, BUTTON_ACCEPT)?;
                    r.steps.push(Step::Accepted {
                        agent: agent.id,
                        mission: turn.content_id,
                    });
                    undock(c)?;
                    r.steps.push(Step::Undocked);
                    return Ok(r);
                }
                Take::Courier => {
                    let delivery = courier::run(c, agent.id)?;
                    r.steps.push(Step::Hauled {
                        agent: agent.id,
                        delivery,
                    });
                    return Ok(r);
                }
                Take::Decline(why) => {
                    c.agent_button(agent.id, &turn, BUTTON_DECLINE)?;
                    r.steps.push(Step::Declined {
                        agent: agent.id,
                        mission: turn.content_id,
                        why,
                    });
                }
            }
        }
    }
    if r.steps.iter().any(|s| matches!(s, Step::Completed { .. })) {
        return Ok(r);
    }
    Err(Error::Refused(format!(
        "missions: nothing this ship runs at station {here} (ship level {}){}",
        e.level,
        if r.steps.is_empty() {
            String::new()
        } else {
            format!(": {}", r.summary())
        }
    )))
}

/// **In space: the kill under way** (module header, step 2).
pub fn field(c: &mut Conn, e: &Errand<'_>) -> Result<Report> {
    let mut r = Report::default();
    let Some((job, pocket)) = journal(c)?
        .into_iter()
        .filter(|j| j.accepted)
        .find_map(|j| Some((j, j.pocket?)))
    else {
        return Err(Error::NotSent(
            "missions: no kill mission under way".to_owned(),
        ));
    };
    let objective = c.mission_objective(job.agent)?;
    let here = c.session().solar_system_id2();
    if objective_met(&objective) || here != Some(pocket.system) {
        let (station, system) = agent_location(c, job.agent)?;
        let trip = fly(c, e, station, system)?;
        r.steps.push(Step::Travelled {
            station,
            jumps: trip.jumps,
        });
        if !objective_met(&objective) {
            undock(c)?;
            r.steps.push(Step::Undocked);
        }
        return Ok(r);
    }
    let gap = {
        let world = c.world_now();
        world
            .ego_ball()
            .map(|b| eve_server::space_state::distance(b.motion.position, pocket.position))
    };
    if gap.is_some_and(|g| g <= POCKET_GRID) {
        return Err(Error::Refused(format!(
            "missions: on agent {}'s pocket, quiet, and it is not cleared",
            job.agent
        )));
    }
    warp_to_pocket(c, job.agent)?;
    r.steps.push(Step::Warped { agent: job.agent });
    Ok(r)
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

fn journal(c: &mut Conn) -> Result<Vec<Job>> {
    let outcome = c.call(AGENT_SERVICE, "GetMyJournalDetails", vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!(
            "missions: GetMyJournalDetails: {key}"
        )));
    }
    Ok(outcome
        .value()
        .as_ref()
        .map(read_journal)
        .unwrap_or_default())
}

/// The pilot's standings (`standingMgr.GetCharStandings`).
fn standings(c: &mut Conn) -> Result<Standings> {
    let outcome = c.call("standingMgr", "GetCharStandings", vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("missions: GetCharStandings: {key}")));
    }
    Ok(outcome
        .value()
        .as_ref()
        .map(read_standings)
        .unwrap_or_default())
}

fn agents(c: &mut Conn) -> Result<Vec<AgentRow>> {
    let outcome = c.call(AGENT_SERVICE, "GetAgents", vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("missions: GetAgents: {key}")));
    }
    Ok(outcome
        .value()
        .as_ref()
        .map(read_agents)
        .unwrap_or_default())
}

/// `GetMissionKeywords(contentID)` on the agent's moniker
/// (`agentDialogueWindow.py`) **[V]**.
fn keywords(c: &mut Conn, agent: i64, mission: Option<i64>) -> Result<PyValue> {
    let answer = c.moniker_call(
        AGENT_SERVICE,
        PyValue::Int(agent),
        "GetMissionKeywords",
        vec![mission.map_or(PyValue::None, PyValue::Int)],
        vec![],
    )?;
    Ok(answer.value.unwrap_or(PyValue::None))
}

/// `GetAgentLocationWrap()` on the agent's moniker: its station and system
/// (`service/agents.rs::agent_location`) **[V]**.
fn agent_location(c: &mut Conn, agent: i64) -> Result<(i64, i64)> {
    let answer = c.moniker_call(
        AGENT_SERVICE,
        PyValue::Int(agent),
        "GetAgentLocationWrap",
        vec![],
        vec![],
    )?;
    let v = answer.value.unwrap_or(PyValue::None);
    match (int_of(&v, "locationID"), int_of(&v, "solarsystemID")) {
        (Some(station), Some(system)) => Ok((station, system)),
        _ => Err(Error::Refused(format!(
            "missions: agent {agent} has no station ({v:?})"
        ))),
    }
}

/// Press Complete, and count what it paid: the wallet's change, and the
/// objective's `loyaltyPoints` as the document stated them before.
fn complete(c: &mut Conn, agent: i64, turn: &AgentTurn) -> Result<Step> {
    let objective = c.mission_objective(agent)?;
    let lp = int_of(&objective, "loyaltyPoints").unwrap_or(0);
    let before = c.cash_balance()?;
    let done = c.agent_button(agent, turn, BUTTON_COMPLETE)?;
    if !done.completed {
        return Err(Error::Refused(format!(
            "missions: agent {agent} did not call the mission complete"
        )));
    }
    let after = c.cash_balance()?;
    Ok(Step::Completed {
        agent,
        mission: turn.content_id,
        isk: after - before,
        lp,
    })
}

fn undock(c: &mut Conn) -> Result<()> {
    let outcome = c.undock_repaired()?;
    match outcome.refusal() {
        Some(key) => Err(Error::Refused(format!("missions: undock: {key}"))),
        None => Ok(()),
    }
}

/// To `station` and docked: over the bot's map, or, with none, inside the
/// one system the agent's station is in.
fn fly(c: &mut Conn, e: &Errand<'_>, station: i64, system: i64) -> Result<travel::Trip> {
    let local;
    let trips = match e.trips {
        Some(t) if t.map.system_of(station).is_some() => t,
        _ => {
            local = Map::new().with_station(station, system);
            Trips {
                map: &local,
                floor: travel::ANY_SECURITY,
            }
        }
    };
    travel::go(c, trips, station)
}

/// The journal bookmark's "warp to": `WarpToLocation(locationType,
/// locationNumber, warpRange, fleet, referringAgentID, formationSettings)`
/// on the agent's moniker, six positionals (`movementFunctions.py:458`)
/// **[V]**; then the warp, flown out.
fn warp_to_pocket(c: &mut Conn, agent: i64) -> Result<()> {
    let answer = c.moniker_call(
        AGENT_SERVICE,
        PyValue::Int(agent),
        "WarpToLocation",
        vec![
            crate::bstr("dungeon"),
            PyValue::Int(0),
            PyValue::Real(WARP_RANGE),
            PyValue::Bool(false),
            PyValue::Int(agent),
            PyValue::None,
        ],
        vec![],
    )?;
    if let Some(key) = answer.outcome.refusal() {
        return Err(Error::Refused(format!(
            "missions: WarpToLocation for agent {agent}: {key}"
        )));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))
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
                "missions: the warp did not start or land in the travel ceiling".into(),
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
    use crate::verbs::ItemRow;

    const STATION: i64 = 61_500_001;
    const ELSEWHERE: i64 = 61_500_002;
    const SHIP: i64 = 1_500_001;
    /// An invented hull group, in no table.
    const ODD_GROUP: i64 = 999_501;

    fn bstr(s: &str) -> PyValue {
        PyValue::Bytes(s.as_bytes().to_vec())
    }

    fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
        PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
    }

    // ---- PROG-PVE: standings open the agent levels ----------------------

    const CORP: i64 = 1_995_901;
    const FACTION: i64 = 598_901;

    fn owned(id: i64, level: i64, station: i64, system: Option<i64>) -> AgentRow {
        AgentRow {
            system,
            corp: Some(CORP),
            faction: Some(FACTION),
            ..agent(id, DIVISION_SECURITY, level, station)
        }
    }

    /// The client's own gate (`eveCfg.CanUseAgent` [V]): level 1 is open, and
    /// level L needs a standing of `2(L-1)-1` toward the faction, the corp or
    /// the agent, none under -2.
    #[test]
    fn standings_open_levels_two_to_five_at_the_clients_thresholds() {
        let a = |level| owned(1, level, STATION, None);
        let st = |f: f64, c: f64, ag: f64| Standings::from([(FACTION, f), (CORP, c), (1, ag)]);
        assert!(usable(&a(1), &Standings::new()), "level 1 needs nothing");
        for (level, need) in [(2, 1.0), (3, 3.0), (4, 5.0), (5, 7.0)] {
            assert!(!usable(&a(level), &Standings::new()), "L{level} at 0");
            assert!(
                usable(&a(level), &st(need, 0.0, 0.0)),
                "L{level} by faction"
            );
            assert!(usable(&a(level), &st(0.0, need, 0.0)), "L{level} by corp");
            assert!(usable(&a(level), &st(0.0, 0.0, need)), "L{level} by agent");
            assert!(
                !usable(&a(level), &st(need - 0.01, 0.0, 0.0)),
                "L{level} short"
            );
        }
        // Any of the three under -2 shuts the door whatever the others say.
        assert!(!usable(&a(2), &st(9.0, -2.01, 0.0)));
    }

    #[test]
    fn a_promoted_pilot_is_sent_to_the_higher_level_agent_first() {
        let rows = [
            owned(1, 1, STATION, Some(7)),
            owned(2, 2, STATION, Some(7)),
            owned(3, 3, STATION, Some(7)),
        ];
        let ids = |v: Vec<AgentRow>| v.iter().map(|a| a.id).collect::<Vec<_>>();
        let none = Standings::new();
        // The ship runs level 3 but the pilot's name opens only level 1.
        assert_eq!(
            ids(candidates_with(&rows, STATION, 3, Some(&none))),
            vec![1]
        );
        // A faction standing of 1.0 opens level 2: the level-2 agent first.
        let one = Standings::from([(FACTION, 1.0)]);
        assert_eq!(
            ids(candidates_with(&rows, STATION, 3, Some(&one))),
            vec![2, 1]
        );
        // 3.0 opens level 3.
        let three = Standings::from([(CORP, 3.0)]);
        assert_eq!(
            ids(candidates_with(&rows, STATION, 3, Some(&three))),
            vec![3, 2, 1]
        );
        // The ship still caps it: a level-2 ship never takes level 3.
        assert_eq!(
            ids(candidates_with(&rows, STATION, 2, Some(&three))),
            vec![2, 1]
        );
        // Without standings the old rule holds.
        assert_eq!(ids(candidates_with(&rows, STATION, 3, None)), vec![3, 2, 1]);
        assert_eq!(ids(candidates(&rows, STATION, 3)), vec![3, 2, 1]);
    }

    #[test]
    fn an_agent_in_another_station_of_the_system_counts_after_the_ones_here() {
        let rows = [
            owned(1, 1, STATION, Some(7)),
            owned(2, 2, ELSEWHERE, Some(7)),
            owned(3, 2, STATION, Some(7)),
            owned(4, 3, ELSEWHERE, Some(8)),
        ];
        let ids = |v: Vec<AgentRow>| v.iter().map(|a| a.id).collect::<Vec<_>>();
        let open = Standings::from([(FACTION, 9.0)]);
        // Level 2 here before level 2 next door; another system is out.
        assert_eq!(
            ids(candidates_with(&rows, STATION, 3, Some(&open))),
            vec![3, 2, 1]
        );
        // Without a system to compare, only this station's agents.
        let lone = [owned(1, 1, STATION, None), owned(2, 2, ELSEWHERE, None)];
        assert_eq!(ids(candidates_with(&lone, STATION, 3, None)), vec![1]);
    }

    #[test]
    fn the_standings_answer_is_a_dict_of_rows() {
        use eve_marshal::{DbRow, DbRowDescriptor, DbType, DbValue};
        let d = DbRowDescriptor::from_pairs(&[("fromID", DbType::I4), ("standing", DbType::R5)])
            .expect("layout");
        let row = |from: i64, s: f64| {
            PyValue::DbRow(Box::new(
                DbRow::new(d.clone(), vec![DbValue::Int(from), DbValue::Real(s)]).expect("row"),
            ))
        };
        let v = PyValue::Dict(vec![
            (row(CORP, 1.5), PyValue::Int(CORP)),
            (row(FACTION, -0.5), PyValue::Int(FACTION)),
            (PyValue::Int(3), PyValue::Int(9)),
        ]);
        assert_eq!(
            read_standings(&v),
            Standings::from([(CORP, 1.5), (FACTION, -0.5)])
        );
        assert!(read_standings(&PyValue::None).is_empty());
    }

    #[test]
    fn the_missions_row_fights() {
        assert!(!ROLE.civilian);
        assert_eq!(ROLE.name(), "missions");
    }

    /// The rule as a table: a fighter takes a kill at or under its hull's
    /// level and declines one above it and every courier; a hauler takes a
    /// courier and declines every kill; anything else is declined.
    #[test]
    fn kill_or_courier_follows_the_ships_level() {
        let rows = [
            (Kind::Kill, 1, 1, Take::Kill),
            (Kind::Kill, 1, 2, Take::Kill),
            (Kind::Courier, 3, 0, Take::Courier),
        ];
        for (kind, agent, ship, want) in rows {
            assert_eq!(choose(kind, agent, ship), want, "{kind:?} {agent} {ship}");
        }
        for (kind, agent, ship) in [
            (Kind::Kill, 2, 1),
            (Kind::Kill, 1, 0),
            (Kind::Courier, 1, 2),
            (Kind::Other, 1, 1),
            (Kind::Other, 1, 0),
        ] {
            assert!(
                matches!(choose(kind, agent, ship), Take::Decline(_)),
                "{kind:?} {agent} {ship}"
            );
        }
    }

    #[test]
    fn every_listed_hull_runs_some_level_and_any_other_none() {
        for (group, level) in HULL_LEVELS {
            assert_eq!(hull_level(*group), *level);
            assert!(*level > 0);
        }
        assert_eq!(hull_level(ODD_GROUP), 0);
    }

    fn agent(id: i64, division: i64, level: i64, station: i64) -> AgentRow {
        AgentRow {
            id,
            agent_type: AGENT_TYPE_BASIC,
            division,
            level,
            station: Some(station),
            system: None,
            corp: None,
            faction: None,
        }
    }

    /// A fighter talks to this station's security agents, highest level it
    /// runs first; a hauler to its distribution agents; at most
    /// `AGENTS_PER_VISIT`.
    #[test]
    fn the_candidates_are_this_stations_agents_of_the_ships_kind() {
        let top = HULL_LEVELS.iter().map(|(_, l)| *l).max().unwrap();
        let mut rows = vec![
            agent(5, DIVISION_SECURITY, 1, STATION),
            agent(4, DIVISION_SECURITY, 2, STATION),
            agent(3, DIVISION_SECURITY, top + 1, STATION),
            agent(2, DIVISION_SECURITY, 1, ELSEWHERE),
            agent(1, DIVISION_DISTRIBUTION, COURIER_MAX_LEVEL, STATION),
            agent(6, DIVISION_DISTRIBUTION, COURIER_MAX_LEVEL + 1, STATION),
        ];
        rows.push(AgentRow {
            agent_type: AGENT_TYPE_BASIC + 1,
            ..agent(7, DIVISION_SECURITY, 1, STATION)
        });
        let ids = |v: Vec<AgentRow>| v.iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(candidates(&rows, STATION, 1)), vec![5]);
        assert_eq!(ids(candidates(&rows, STATION, 2)), vec![4, 5]);
        assert_eq!(ids(candidates(&rows, STATION, 0)), vec![1]);
        let many: Vec<AgentRow> = (0..10)
            .map(|i| agent(100 + i, DIVISION_SECURITY, 1, STATION))
            .collect();
        assert_eq!(candidates(&many, STATION, 1).len(), AGENTS_PER_VISIT);
    }

    #[test]
    fn an_offer_is_known_by_its_keywords() {
        let kill = dict(vec![("dungeonLocationID", PyValue::Int(1))]);
        let courier = dict(vec![("objectiveDestinationID", PyValue::Int(1))]);
        assert_eq!(offer_kind(&kill), Kind::Kill);
        assert_eq!(offer_kind(&courier), Kind::Courier);
        assert_eq!(offer_kind(&PyValue::Dict(vec![])), Kind::Other);
    }

    /// A rowset read by column name, and a journal row with its pocket.
    #[test]
    fn the_agent_rowset_and_the_journal_read() {
        let rowset = PyValue::Instance {
            class: b"rowset".to_vec(),
            state: Box::new(dict(vec![
                (
                    "header",
                    PyValue::List(
                        ["level", "agentID", "divisionID", "agentTypeID", "stationID"]
                            .iter()
                            .map(|s| bstr(s))
                            .collect(),
                    ),
                ),
                (
                    "lines",
                    PyValue::List(vec![PyValue::List(vec![
                        PyValue::Int(2),
                        PyValue::Int(7),
                        PyValue::Int(DIVISION_SECURITY),
                        PyValue::Int(AGENT_TYPE_BASIC),
                        PyValue::Int(STATION),
                    ])]),
                ),
            ])),
        };
        assert_eq!(
            read_agents(&rowset),
            vec![AgentRow {
                level: 2,
                ..agent(7, DIVISION_SECURITY, 2, STATION)
            }]
        );

        let mark = PyValue::Instance {
            class: b"keyval".to_vec(),
            state: Box::new(dict(vec![
                ("locationType", bstr("dungeon")),
                ("locationID", PyValue::Int(39_500_001)),
                ("x", PyValue::Real(1.0)),
                ("y", PyValue::Int(2)),
                ("z", PyValue::Real(3.0)),
            ])),
        };
        let row = |state, marks| {
            PyValue::Tuple(vec![
                PyValue::Int(state),
                PyValue::Bool(false),
                bstr("label"),
                PyValue::Int(1),
                PyValue::Int(7),
                PyValue::Int(0),
                PyValue::List(marks),
                PyValue::Bool(false),
                PyValue::Bool(false),
                PyValue::Int(999_501),
            ])
        };
        let journal = PyValue::List(vec![
            PyValue::List(vec![
                row(MISSION_STATE_ACCEPTED, vec![mark]),
                row(MISSION_STATE_ACCEPTED - 1, vec![]),
            ]),
            PyValue::List(vec![]),
        ]);
        let jobs = read_journal(&journal);
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[0],
            Job {
                agent: 7,
                mission: 999_501,
                accepted: true,
                pocket: Some(Pocket {
                    system: 39_500_001,
                    position: [1.0, 2.0, 3.0]
                })
            }
        );
        assert!(!jobs[1].accepted && jobs[1].pocket.is_none());
    }

    fn docked_in(group: i64, guns: bool) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: Some(escort::RIFTER),
        });
        o.hangar = vec![ItemRow {
            item_id: SHIP,
            type_id: escort::RIFTER,
            owner_id: 0,
            location_id: STATION,
            flag_id: crate::FLAG_HANGAR,
            quantity: 1,
            group_id: group,
            category_id: courier::CATEGORY_SHIP,
            singleton: true,
        }];
        if guns {
            // Two civilian guns (capacity 0): armed, with no rounds to load.
            o.guns = (0..escort::WANT_GUNS as i64)
                .map(|i| brain::GunInfo {
                    item_id: 2 + i,
                    type_id: escort::GUN,
                    flag_id: 27 + i,
                    online: true,
                    active: false,
                    target: None,
                    rounds: None,
                    capacity: 0,
                    range: 0.0,
                    reload: Duration::ZERO,
                    kind: crate::brain::GunKind::Weapon,
                    reach: 0.0,
                })
                .collect();
        }
        o
    }

    /// The ship's level is its hull's, and 0 with no gun or an unlisted hull.
    #[test]
    fn the_ships_level_needs_a_gun_and_a_listed_hull() {
        let (group, level) = HULL_LEVELS[0];
        assert_eq!(ship_level(&docked_in(group, true)), level);
        assert_eq!(ship_level(&docked_in(group, false)), 0);
        assert_eq!(ship_level(&docked_in(ODD_GROUP, true)), 0);
    }

    /// Where the escort would undock, the missioner goes to the desk with
    /// its agent and its ship's level; after a failed visit it says
    /// `MISSING` and waits first.
    #[test]
    fn docked_and_ready_it_goes_to_the_desk_instead_of_undocking() {
        let (group, level) = HULL_LEVELS[0];
        let mut m = Memory::new("Missioner1", Policy::fast());
        m.policy.agent = Some(3_500_001);
        let mut o = docked_in(group, true);
        let errand = Act::Mission {
            agent: Some(3_500_001),
            level,
        };
        assert_eq!(docked(&mut m, &o), vec![errand.clone()]);
        o.failures.insert(ActKind::Mission, 1);
        assert_eq!(
            docked(&mut m, &o),
            vec![
                Act::Missing(MISSING_AGENT_ACT),
                Act::Wait(brain::MISSING_WAIT),
                errand
            ]
        );
    }

    /// In space, the site runner's "next anomaly" becomes the field visit;
    /// a failed one sends the bot home.
    #[test]
    fn in_space_the_next_site_is_the_pocket_and_a_failure_goes_home() {
        let mut m = Memory::new("Missioner1", Policy::fast());
        let mut o = Observation::empty(Instant::now());
        o.failures.insert(ActKind::Mission, 1);
        let _ = in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::Recovery));
    }

    #[test]
    fn the_report_names_what_was_paid() {
        let r = Report {
            steps: vec![
                Step::Completed {
                    agent: 7,
                    mission: Some(1),
                    isk: 10.0,
                    lp: 3,
                },
                Step::Undocked,
            ],
        };
        assert_eq!(r.paid(), (1, 10.0, 3));
        assert!(r.summary().contains("completed mission 1"));
    }
}

/// **Over a real socket** (the row's harness test): an in-process server
/// whose pool is two invented security agents at the pilot's station -- a
/// level-1 agent with one invented kill mission, and a level-2 agent with
/// another -- against an invented pirate whose only ship is the server's
/// no-bundle rat (`npc::FIXTURE_RAT`). No client data; no `[A]` value is an
/// expected value (the level-2 agent is "one above `HULL_LEVELS[0]`'s").
///
/// Both ends marshal with `eve-marshal`, so this proves behaviour, not
/// bytes.
#[cfg(test)]
mod harness {
    use super::*;
    use crate::driver::{self, BotConfig, DEV_TOKEN, Pace, Verdict};
    use eve_server::mission_pool::{self as pool, Agent, Mission, Table};
    use eve_server::model::Character;
    use eve_server::npc_data::{self, Needs, NpcTypeRow, Tier};
    use eve_server::server::{Config, Server};
    use std::net::SocketAddr;
    use std::sync::Arc;

    const PILOT: i64 = 90_000_001;
    const NAME: &str = "Missioner1";
    /// Below the no-bundle station agent's id (`service::STATION_AGENT_ID`,
    /// a security agent at every station whose one offer is a courier), so
    /// the desk talks to these first.
    const AGENT: i64 = 3_000_501;
    const HARD_AGENT: i64 = 3_000_502;
    const KILL: i64 = 995_001;
    const HARD_KILL: i64 = 995_002;
    /// Invented: the agents' corporation and faction, and the pirate.
    const CORP: i64 = 1_995_001;
    const FACTION: i64 = 598_501;
    const PIRATE: i64 = 599_501;
    const DUNGEON: i64 = 999_591;

    /// The default character's station and system: the pilot's home.
    fn home() -> (i64, i64) {
        let c = Character::default();
        (
            c.station_id.expect("docked"),
            c.solar_system_id.expect("in a system"),
        )
    }

    fn table() -> Table {
        let (station, system) = home();
        let hull = HULL_LEVELS[0].1;
        let agent = |id, level| Agent {
            id,
            agent_type: AGENT_TYPE_BASIC,
            division: DIVISION_SECURITY,
            level,
            corp: CORP,
            faction: FACTION,
            station: Some(station),
            system: Some(system),
            ..Agent::default()
        };
        let kill = |id, level| Mission {
            id,
            name_id: id + 1_000,
            template: "BasicKill".into(),
            family: "kill".into(),
            level: Some(level),
            enemy_faction: Some(PIRATE),
            dungeon: Some(DUNGEON),
            ..Mission::default()
        };
        Table::new(
            vec![agent(AGENT, hull), agent(HARD_AGENT, hull + 1)],
            vec![kill(KILL, hull), kill(HARD_KILL, hull + 1)],
            pool::Policy::TRANQUILITY,
        )
    }

    /// The pirate, whose one ship is the no-bundle rat.
    fn npcs() -> npc_data::Table {
        let rat = eve_server::npc::FIXTURE_RAT;
        let mut t = npc_data::Table::default();
        t.types.insert(
            rat.type_id,
            NpcTypeRow {
                type_id: rat.type_id,
                group_id: rat.group_id,
                faction_id: PIRATE,
                context: eve_server::npc_gen::ASTEROID.to_owned(),
                class: "Frigate".to_owned(),
                tier: Tier::Base,
                alias_of: None,
                needs: Needs::TURRET,
                role: npc_data::Role::Dps,
            },
        );
        t
    }

    fn start() -> SocketAddr {
        let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
        config.sandbox.characters = vec![Character {
            character_id: PILOT,
            name: NAME.to_owned(),
            ship_id: 100_000_001,
            ..Character::default()
        }];
        let mut content = eve_server::content::Content::fixture();
        content.insert("mission_pool", table());
        content.insert("npc_data", npcs());
        config.content = Some(Arc::new(content));
        let server = Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = server.run();
        });
        addr
    }

    fn login(addr: SocketAddr) -> Conn {
        let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
        c.handshake_as(DEV_TOKEN).expect("handshake");
        c.select_character_named(NAME).expect("selects");
        c
    }

    /// **The desk, by hand**: a pilot whose ship runs level 1 is not sent
    /// to the level-2 agent; at the station's own agents it finds the
    /// level-1 one, takes its kill mission and undocks; the journal then
    /// holds the accepted mission with its pocket in the home system.
    #[test]
    fn the_desk_refuses_a_kill_above_the_hull_and_takes_one_it_runs() {
        let addr = start();
        let mut c = login(addr);
        let level = HULL_LEVELS[0].1;
        let hard = Errand {
            agent: Some(HARD_AGENT),
            level,
            trips: None,
        };
        match desk(&mut c, &hard) {
            Err(Error::Refused(why)) => assert!(why.contains("skipped agent"), "{why}"),
            other => panic!("a level-2 agent for a level-1 hull: {other:?}"),
        }
        assert!(journal(&mut c).expect("reads").iter().all(|j| !j.accepted));

        let r = desk(
            &mut c,
            &Errand {
                agent: None,
                level,
                trips: None,
            },
        )
        .expect("the desk takes the kill");
        assert_eq!(
            r.steps,
            vec![
                Step::Accepted {
                    agent: AGENT,
                    mission: Some(KILL)
                },
                Step::Undocked
            ]
        );
        let jobs = journal(&mut c).expect("reads");
        let job = jobs.iter().find(|j| j.accepted).expect("accepted");
        assert_eq!((job.agent, job.mission), (AGENT, KILL));
        assert_eq!(job.pocket.map(|p| p.system), Some(home().1));
    }

    /// **PROG-PVE, the promotion over the socket**: the same pilot, a ship
    /// that runs level 2, and the same two agents (levels 1 and 2). With no
    /// standing the desk takes the level-1 agent's mission; once the
    /// agents' corporation likes the pilot at the client's threshold for
    /// level 2 (`mission_pool::can_use_agent`), the desk goes to the level-2
    /// agent first and takes its mission.
    #[test]
    fn the_desk_moves_up_a_level_when_the_standings_open_it() {
        let level = HULL_LEVELS[0].1 + 1;
        let errand = Errand {
            agent: None,
            level,
            trips: None,
        };
        let r = desk(&mut login(start()), &errand).expect("the desk takes the easy one");
        assert_eq!(
            r.steps.first(),
            Some(&Step::Accepted {
                agent: AGENT,
                mission: Some(KILL)
            }),
            "no standing: the level-1 agent, not the level-2 one: {:?}",
            r.steps
        );

        let mut c = login(start());
        let said = crate::gm::slash(&mut c, &format!("/setstanding {CORP} me 1.0")).expect("sent");
        assert!(said.refusal().is_none(), "{said:?}");
        let r = desk(&mut c, &errand).expect("the desk takes the harder one");
        assert_eq!(
            r.steps.first(),
            Some(&Step::Accepted {
                agent: HARD_AGENT,
                mission: Some(HARD_KILL)
            }),
            "a standing of 1.0 opens level 2: {:?}",
            r.steps
        );
    }

    /// **The row's test, the whole loop**: the bot arms itself (GM setup),
    /// takes the kill mission at the desk, warps to the pocket, kills the
    /// rats, flies home, and turns the mission in for ISK.
    #[test]
    fn a_missioner_takes_a_kill_clears_the_pocket_and_is_paid() {
        let addr = start();
        let config = BotConfig::new(NAME, addr, Pace::Fast)
            .with_token(DEV_TOKEN)
            .with_role(&ROLE);
        let (h, t) = driver::spawn(config);
        let paid = |e: &driver::Entry| {
            e.kind == ActKind::Mission
                && e.verdict == Verdict::Ok
                && e.what.contains(&format!("completed mission {KILL}"))
        };
        let deadline = Instant::now() + Duration::from_secs(240);
        while Instant::now() < deadline && !h.log().iter().any(paid) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let log = h.log();
        h.stop();
        let _ = t.join();
        let dump = || {
            log.iter()
                .map(|e| format!("  {:?} {} {:?}", e.kind, e.what, e.verdict))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let at = |want: &dyn Fn(&driver::Entry) -> bool| log.iter().position(want);
        let accepted = at(&|e| {
            e.kind == ActKind::Mission && e.what.contains(&format!("accepted kill mission {KILL}"))
        });
        let warped = at(&|e| e.kind == ActKind::Mission && e.what.contains("warped to agent"));
        let fired = at(&|e| e.kind == ActKind::Fire && e.verdict == Verdict::Ok);
        let completed = at(&paid);
        assert!(
            [accepted, warped, fired, completed]
                .iter()
                .all(Option::is_some)
                && warped > accepted
                && fired > warped
                && completed > fired,
            "accept, warp, fight, then complete:\n{}",
            dump()
        );
        let isk = log[completed.expect("checked")].what.clone();
        assert!(!isk.contains("+0.00 ISK"), "the mission paid: {isk}");
    }
}
