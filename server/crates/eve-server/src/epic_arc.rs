//! Epic arcs: the hand-written mission graphs some agents walk a pilot along.
//!
//! W16 packet CP41 (`docs/content/plan.md` §6.2), spec C4 §7 and packet K6.
//! The graph is CP13's `agents/epic-arcs.tsv` (`docs/content/schema.md`), one
//! row per (arc, mission): the mission's agent, its `nextMissions`, its
//! `failMissionID` and the arc's own columns repeated.
//!
//! # The rules
//!
//! - **An arc is valid** when its restart interval is above 1 minute, the
//!   client's own `IsValidArc` (`epicArcController.py:41-42`) [V]. An invalid
//!   arc (the Pilot Certification loops) is never offered, never listed and
//!   never advanced.
//! - **Its starts** are its missions that are no mission's `nextMissions`,
//!   the client's `get_all_start_missions` (`epic_arcs_data.py:78`) [V].
//! - **The frontier** is what the arc offers a character now [A] (C4 §7.3):
//!   the mission they have accepted, if any; else, with nothing done, every
//!   start; else the `nextMissions` of the mission they completed last, less
//!   any they have completed. When those are several the arc **branches**:
//!   each next agent offers its mission, and accepting one withdraws the
//!   others. An arc whose end mission (no `nextMissions`) is complete offers
//!   nothing until its **replay**, `completed_at + restart_minutes` (minutes
//!   [I], C4 §7.1), and then its starts again.
//! - **The offer is the mission conversation's own.** [`prime`] runs before
//!   every `agentMgr` arm that reads an agent's mission (`service/agents.rs`)
//!   and puts the frontier mission at that agent on offer through
//!   `mission::set_offer`, or withdraws an arc offer the frontier has left;
//!   accept, complete and quit are `mission::converse_at`'s as for any
//!   mission. Arc agents are not pool agents, so once an arc has nothing for
//!   one it has no mission (`NoMission1`).
//!
//! # State
//!
//! **Persisted as deltas** (plan §4.2): one [`KIND`] row per (character,
//! arc) the character has completed a mission of, `char arc_id -> state
//! completed_at log`: `open` or `done`, the end's completion time (0 while
//! open), and every completed mission as `mission:accepted:completed`, in
//! completion order. The third column is ours [A]: the plan's
//! `state completed_at` has nowhere for the per-mission dates
//! `GetMyEpicArcStatus` serves. Offered and accepted missions are not
//! duplicated here; they are read from the `mission` rows.
//!
//! [`on_complete`] is the `mission_hooks::HOOKS` row on `Change::Completed`
//! that appends to the log, closes the arc on an end mission and pushes
//! `OnEpicJournalChange()` (no arguments, `epicArcSvc.py:42`) [V].
//!
//! # The calls (C4 §7.2) [V]
//!
//! - [`status`] is `agentMgr.GetMyEpicArcStatus()`, `{arcID: {missionID:
//!   KeyVal(acceptedDate, completedDate, quitDate, nameID)}}`. The client
//!   reads a mission as offered when `nameID` is truthy, accepted on
//!   `acceptedDate` and complete on `completedDate`
//!   (`epicArcController.py:227-239`) [V].
//! - [`messages`] is `agentMgr.GetMessagesForEpicArcMissions()`,
//!   `{slot: {missionID: messageID}}` over the `messages.epicMission.*`
//!   slots, read with `.get(slot, {}).get(missionID, None)`
//!   (`epicArcController.py:65-66`) [V].
//! - [`has_missions`] is `epicArcStatus.AgentHasEpicMissionsForCharacter`.
//! - [`replay_at`] is the bound `GetReplayTimestamp(characterID)`.
//!
//! # Not done
//!
//! `failMissionID` (quit or fail offers it, C4 §7.3) and `quitDate` are not
//! served: a quit leaves the mission on offer, as for any agent, and no
//! valid arc is known to need more [A]. `lastActionInfo['missionCantReplay']`
//! is `mission.rs`'s.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::mission::{self, State};
use crate::mission_hooks::MissionEvent;
use crate::pyutil::{bstr, keyval};
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "epic_arc",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The delta kind this module owns: `char arc_id -> state completed_at log`.
pub const KIND: &str = "arc";

/// CP13's file and its header line, copied from `schema.md`.
pub const ARCS: (&str, &str) = (
    "agents/epic-arcs.tsv",
    "#arc_id\tmission_id\tagent_id\tnext_missions\tfail_mission_id\trestart_minutes\tfaction_id\tname_id",
);

/// CP13's long-form message file and its header, from `schema.md`.
pub const MESSAGES: (&str, &str) = (
    "agents/mission-messages.tsv",
    "#mission_id\tslot\tmessage_id",
);

/// The slots `GetMessagesForEpicArcMissions` serves: every `messages` key
/// under this prefix (`epic_arcs_data.py`'s `MESSAGE_*` constants) [V].
pub const SLOT_PREFIX: &str = "messages.epicMission.";

/// One blue FILETIME minute: 100 ns ticks.
pub const MINUTE: i64 = 60 * 10_000_000;

/// `IsValidArc`'s bound: an arc whose restart interval is not above it is
/// hidden (`epicArcController.py:41-42`) [V].
pub const VALID_ABOVE: i64 = 1;

/// The bundle's rows, or empty with no bundle: invented arcs served to a
/// client would be a lie (the `loyalty` rule).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::default(),
    }))
}

/// One mission of an arc.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    pub agent: i64,
    pub next: Vec<i64>,
    pub fail: Option<i64>,
}

/// One arc: its columns and its missions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arc {
    pub id: i64,
    pub restart_minutes: i64,
    pub faction: Option<i64>,
    pub name_id: i64,
    pub missions: BTreeMap<i64, Node>,
}

impl Arc {
    /// The client's `IsValidArc` [V].
    pub fn is_valid(&self) -> bool {
        self.restart_minutes > VALID_ABOVE
    }

    /// The missions no mission leads to, ascending (`get_all_start_missions`)
    /// [V].
    pub fn starts(&self) -> Vec<i64> {
        let led: BTreeSet<i64> = self
            .missions
            .values()
            .flat_map(|n| n.next.iter().copied())
            .collect();
        self.missions
            .keys()
            .copied()
            .filter(|m| !led.contains(m))
            .collect()
    }

    /// `m`'s `nextMissions`; empty for an end mission or one not in the arc.
    pub fn next(&self, m: i64) -> &[i64] {
        self.missions.get(&m).map_or(&[], |n| &n.next[..])
    }
}

/// Every arc, the arc of each mission, and the journal messages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    arcs: BTreeMap<i64, Arc>,
    arc_of: BTreeMap<i64, i64>,
    /// `slot -> mission -> message`, the `messages.epicMission.*` slots only.
    messages: BTreeMap<String, BTreeMap<i64, i64>>,
}

impl Table {
    /// `agents/epic-arcs.tsv`, and the messages from
    /// `agents/mission-messages.tsv` or, while CP13 does not write that file,
    /// the same pairs in `agents/missions.tsv`'s `messages` column (CP13's
    /// `check` holds the two equal). An absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let arcs = b.data_lines(ARCS.0, ARCS.1)?;
        let pairs = if b.path(MESSAGES.0).is_some() {
            parse_messages(&b.data_lines(MESSAGES.0, MESSAGES.1)?)?
        } else {
            match b.path("agents/missions.tsv") {
                Some(p) => {
                    let text =
                        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    crate::mission_pool::parse_missions(&text)?
                        .into_iter()
                        .flat_map(|m| {
                            let id = m.id;
                            m.messages.into_iter().map(move |(s, msg)| (id, s, msg))
                        })
                        .collect()
                }
                None => Vec::new(),
            }
        };
        Table::parse(&arcs, pairs)
    }

    /// The data lines of `epic-arcs.tsv` and `(mission, slot, message)`
    /// pairs. A bad line is an error naming it; so is a mission in two arcs
    /// or one arc's columns that disagree between its rows.
    pub fn parse<S: AsRef<str>>(
        arc_lines: &[S],
        messages: Vec<(i64, String, i64)>,
    ) -> Result<Table, String> {
        let mut t = Table::default();
        for (i, line) in arc_lines.iter().enumerate() {
            let line = line.as_ref();
            let bad = |why: &str| format!("{}: data line {}: {why}: {line:?}", ARCS.0, i + 1);
            let (head, mission, node) = parse_arc(line).ok_or_else(|| bad("unreadable"))?;
            let arc = t.arcs.entry(head.id).or_insert_with(|| head.clone());
            if (arc.restart_minutes, arc.faction, arc.name_id)
                != (head.restart_minutes, head.faction, head.name_id)
            {
                return Err(bad("the arc's columns differ from its first row"));
            }
            if t.arc_of.insert(mission, head.id).is_some() {
                return Err(bad("the mission is in two rows"));
            }
            arc.missions.insert(mission, node);
        }
        for (mission, slot, message) in messages {
            if slot.starts_with(SLOT_PREFIX) {
                t.messages.entry(slot).or_default().insert(mission, message);
            }
        }
        Ok(t)
    }

    /// Invented rows (ids in the test bands), never a client value.
    ///
    /// - Arc 9901, valid: 999501 at 3999401 branches to 999502 (3999402)
    ///   and 999503 (3999403); both lead to the end, 999504 (3999404).
    /// - Arc 9902, hidden (restart 1 minute): 999511 at 3999411 leads to
    ///   999512 at the same agent, which fails back to 999511.
    pub fn fixture() -> Table {
        let lines = [
            "9901\t999501\t3999401\t999502,999503\t\t1000\t599001\t999920",
            "9901\t999502\t3999402\t999504\t\t1000\t599001\t999920",
            "9901\t999503\t3999403\t999504\t\t1000\t599001\t999920",
            "9901\t999504\t3999404\t\t\t1000\t599001\t999920",
            "9902\t999511\t3999411\t999512\t\t1\t\t999921",
            "9902\t999512\t3999411\t\t999511\t1\t\t999921",
        ];
        let mut messages = vec![(
            999_501,
            format!("{SLOT_PREFIX}journalText.chapterTitle"),
            999_930,
        )];
        for (k, m) in (999_501..=999_504).chain([999_511, 999_512]).enumerate() {
            let k = k as i64;
            messages.push((
                m,
                format!("{SLOT_PREFIX}journalText.inProgressMessage"),
                999_940 + k,
            ));
            messages.push((
                m,
                format!("{SLOT_PREFIX}journalText.completedMessage"),
                999_960 + k,
            ));
            messages.push((m, "messages.mission.briefing".into(), 999_980 + k));
        }
        Table::parse(&lines, messages).unwrap_or_else(|e| unreachable!("the fixture parses: {e}"))
    }

    pub fn arc(&self, id: i64) -> Option<&Arc> {
        self.arcs.get(&id)
    }

    pub fn arcs(&self) -> impl Iterator<Item = &Arc> {
        self.arcs.values()
    }

    /// The arc `mission` belongs to.
    pub fn arc_of(&self, mission: i64) -> Option<&Arc> {
        self.arc(*self.arc_of.get(&mission)?)
    }
}

fn parse_arc(line: &str) -> Option<(Arc, i64, Node)> {
    let f: Vec<&str> = line.split('\t').collect();
    let [arc, mission, agent, next, fail, restart, faction, name] = f[..] else {
        return None;
    };
    let int = |s: &str| s.parse::<i64>().ok();
    let opt = |s: &str| {
        if s.is_empty() {
            Some(None)
        } else {
            int(s).map(Some)
        }
    };
    let next = next
        .split(',')
        .filter(|s| !s.is_empty())
        .map(int)
        .collect::<Option<Vec<_>>>()?;
    let head = Arc {
        id: int(arc)?,
        restart_minutes: int(restart)?,
        faction: opt(faction)?,
        name_id: int(name)?,
        missions: BTreeMap::new(),
    };
    let node = Node {
        agent: int(agent)?,
        next,
        fail: opt(fail)?,
    };
    Some((head, int(mission)?, node))
}

fn parse_messages<S: AsRef<str>>(lines: &[S]) -> Result<Vec<(i64, String, i64)>, String> {
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line = line.as_ref();
            let f: Vec<&str> = line.split('\t').collect();
            match f[..] {
                [m, slot, msg] => m
                    .parse()
                    .ok()
                    .zip(msg.parse().ok())
                    .map(|(m, msg)| (m, slot.to_owned(), msg)),
                _ => None,
            }
            .ok_or_else(|| format!("{}: data line {}: {line:?}", MESSAGES.0, i + 1))
        })
        .collect()
}

/// One completed mission in a [`KIND`] row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    pub mission: i64,
    pub accepted_at: i64,
    pub completed_at: i64,
}

/// One [`KIND`] row's value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// The end mission is complete.
    pub done: bool,
    /// When it was; 0 while open.
    pub completed_at: i64,
    /// Every completed mission, in completion order.
    pub log: Vec<Done>,
}

impl Progress {
    pub fn to_value(&self) -> Vec<String> {
        let log: Vec<String> = self
            .log
            .iter()
            .map(|d| format!("{}:{}:{}", d.mission, d.accepted_at, d.completed_at))
            .collect();
        vec![
            if self.done { "done" } else { "open" }.to_owned(),
            self.completed_at.to_string(),
            log.join(","),
        ]
    }

    /// `None` for a row this build cannot read, which then reads as no row.
    pub fn from_value(v: &[String]) -> Option<Progress> {
        let [state, at, log] = v else {
            return None;
        };
        let log = log
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|e| {
                let mut p = e.split(':').map(str::parse::<i64>);
                let d = Done {
                    mission: p.next()?.ok()?,
                    accepted_at: p.next()?.ok()?,
                    completed_at: p.next()?.ok()?,
                };
                p.next().is_none().then_some(d)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Progress {
            done: match state.as_str() {
                "done" => true,
                "open" => false,
                _ => return None,
            },
            completed_at: at.parse().ok()?,
            log,
        })
    }

    /// When the arc may be started again; `None` while it is open. Lane
    /// clock: `restart_minutes` at the game clock's pace, derived from the
    /// stored completion (a restart at another scale re-derives it).
    pub fn replay_at(&self, arc: &Arc, clock: crate::game_clock::GameClock) -> Option<i64> {
        self.done
            .then(|| self.completed_at + clock.span(arc.restart_minutes.saturating_mul(MINUTE)))
    }
}

/// A marshal dict from logical `(key, value)` pairs with any keys: the
/// wire order is value-then-key (`pyutil::pydict`).
fn dict(kv: Vec<(PyValue, PyValue)>) -> PyValue {
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, k)).collect())
}

fn key(ch: i64, arc: i64) -> Vec<String> {
    vec![ch.to_string(), arc.to_string()]
}

/// `ch`'s row for `arc`, or `None` when they never completed one of its
/// missions (or the row is unreadable).
pub fn progress(reg: &Registry, ch: i64, arc: i64) -> Option<Progress> {
    reg.deltas()
        .of(KIND)
        .get(&key(ch, arc))
        .and_then(|v| Progress::from_value(v))
}

fn table(reg: &Registry) -> Option<&Table> {
    reg.content().try_get::<Table>()
}

/// The arc mission `ch` holds with `m`'s agent, as `(state, accepted_at)`,
/// when that agent's current mission is `m` and it has not expired unaccepted.
fn live(reg: &Registry, ch: i64, arc: &Arc, m: i64, now: i64) -> Option<(State, i64)> {
    let node = arc.missions.get(&m)?;
    let p = mission::progress(reg, ch, node.agent);
    (p.mission_id == Some(m) && (p.state == State::Accepted || p.expires_at > now))
        .then_some((p.state, p.accepted_at))
}

/// The arc mission `ch` has accepted, if any.
fn accepted(reg: &Registry, ch: i64, arc: &Arc) -> Option<i64> {
    arc.missions.iter().find_map(|(m, n)| {
        let p = mission::progress(reg, ch, n.agent);
        (p.mission_id == Some(*m) && p.state == State::Accepted).then_some(*m)
    })
}

/// What `arc` offers `ch` at `now` (module doc), ascending.
pub fn frontier(reg: &Registry, ch: i64, arc: &Arc, now: i64) -> Vec<i64> {
    if !arc.is_valid() {
        return Vec::new();
    }
    if let Some(m) = accepted(reg, ch, arc) {
        return vec![m];
    }
    match progress(reg, ch, arc.id) {
        None => arc.starts(),
        Some(p) if p.done => match p.replay_at(arc, crate::game_clock::of(reg)) {
            Some(at) if now >= at => arc.starts(),
            _ => Vec::new(),
        },
        Some(p) => {
            let did: BTreeSet<i64> = p.log.iter().map(|d| d.mission).collect();
            match p.log.last() {
                None => arc.starts(),
                Some(last) => {
                    let mut next: Vec<i64> = arc
                        .next(last.mission)
                        .iter()
                        .copied()
                        .filter(|m| !did.contains(m))
                        .collect();
                    next.sort_unstable();
                    next.dedup();
                    next
                }
            }
        }
    }
}

/// Before `ch` speaks to `agent`: put the lowest frontier mission at that
/// agent on offer [A], or withdraw an arc offer no frontier holds any more.
/// A start offered after a replay clears the old run's row. An accepted
/// mission is never touched (`mission::set_offer`).
pub fn prime(reg: &mut Registry, ch: i64, agent: i64, now: i64) {
    let Some(t) = table(reg) else {
        return;
    };
    let mut want: Option<(i64, i64)> = None;
    for arc in t.arcs().filter(|a| a.is_valid()) {
        let here = frontier(reg, ch, arc, now)
            .into_iter()
            .filter(|m| arc.missions.get(m).is_some_and(|n| n.agent == agent))
            .min();
        if let Some(m) = here
            && want.is_none_or(|(w, _)| m < w)
        {
            want = Some((m, arc.id));
        }
    }
    let holds_arc = {
        let p = mission::progress(reg, ch, agent);
        p.state == State::Offered && p.mission_id.is_some_and(|m| t.arc_of(m).is_some())
    };
    match want {
        Some((m, arc)) => {
            if progress(reg, ch, arc).is_some_and(|p| p.done) {
                reg.deltas_mut().del(KIND, &key(ch, arc));
            }
            mission::set_offer(reg, ch, agent, Some(m), now);
        }
        None if holds_arc => {
            mission::set_offer(reg, ch, agent, None, now);
        }
        None => {}
    }
}

/// `AgentHasEpicMissionsForCharacter(agentID)`: whether `agent` has an arc
/// mission for `ch`, on offer or accepted (`agentUtil.py:43`, `:68`).
pub fn has_missions(reg: &Registry, ch: i64, agent: i64, now: i64) -> bool {
    table(reg).is_some_and(|t| {
        t.arcs().filter(|a| a.is_valid()).any(|arc| {
            frontier(reg, ch, arc, now)
                .iter()
                .any(|m| arc.missions.get(m).is_some_and(|n| n.agent == agent))
        })
    })
}

/// The bound `GetReplayTimestamp(characterID)`: when an arc that `agent`
/// starts may be replayed by `ch`, while that is still ahead; `None` (falsy:
/// no cooldown, `station/agents/agents.py:295`) otherwise [A] (falsifier: a
/// client that shows no timer for an int).
pub fn replay_at(reg: &Registry, ch: i64, agent: i64, now: i64) -> Option<i64> {
    let t = table(reg)?;
    t.arcs()
        .filter(|a| a.is_valid())
        .filter(|a| a.starts().iter().any(|m| a.missions[m].agent == agent))
        .filter_map(|a| progress(reg, ch, a.id)?.replay_at(a, crate::game_clock::of(reg)))
        .filter(|at| *at > now)
        .max()
}

/// `GetMyEpicArcStatus()`: per valid arc with anything to say, each mission
/// completed, accepted or on offer, as `KeyVal(acceptedDate, completedDate,
/// quitDate, nameID)`. `{}` for a character who never spoke to an arc agent
/// [V] (C4 §7.2). The next missions of a started arc count as on offer
/// before their agents are spoken to [A].
pub fn status(reg: &Registry, ch: i64, now: i64) -> PyValue {
    let Some(t) = table(reg) else {
        return dict(Vec::new());
    };
    let pool = reg.content().try_get::<crate::mission_pool::Table>();
    let name = |m: i64| {
        pool.and_then(|p| p.mission(m))
            .map_or(PyValue::None, |r| PyValue::Int(r.name_id))
    };
    let date = |d: Option<i64>| d.map_or(PyValue::None, PyValue::Int);
    let mut out = Vec::new();
    for arc in t.arcs().filter(|a| a.is_valid()) {
        // mission -> (accepted, completed)
        let mut seen: BTreeMap<i64, (Option<i64>, Option<i64>)> = BTreeMap::new();
        let row = progress(reg, ch, arc.id);
        if let Some(p) = &row {
            for d in &p.log {
                seen.insert(d.mission, (Some(d.accepted_at), Some(d.completed_at)));
            }
            if !p.done {
                for m in frontier(reg, ch, arc, now) {
                    seen.entry(m).or_insert((None, None));
                }
            }
        }
        for &m in arc.missions.keys() {
            if let Some((state, at)) = live(reg, ch, arc, m, now) {
                let acc = (state == State::Accepted).then_some(at);
                seen.insert(m, (acc, None));
            }
        }
        if seen.is_empty() {
            continue;
        }
        let missions = seen
            .into_iter()
            .map(|(m, (acc, done))| {
                let s = keyval(vec![
                    ("acceptedDate", date(acc)),
                    ("completedDate", date(done)),
                    ("quitDate", PyValue::None),
                    ("nameID", name(m)),
                ]);
                (PyValue::Int(m), s)
            })
            .collect();
        out.push((PyValue::Int(arc.id), dict(missions)));
    }
    dict(out)
}

/// `GetMessagesForEpicArcMissions()`: `{slot: {missionID: messageID}}`.
pub fn messages(reg: &Registry) -> PyValue {
    let Some(t) = table(reg) else {
        return dict(Vec::new());
    };
    dict(
        t.messages
            .iter()
            .map(|(slot, by)| {
                let inner = by
                    .iter()
                    .map(|(m, msg)| (PyValue::Int(*m), PyValue::Int(*msg)))
                    .collect();
                (bstr(slot), dict(inner))
            })
            .collect(),
    )
}

/// `OnEpicJournalChange()` [V], to one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalChange {
    pub character_id: i64,
}

impl crate::notify::Push for JournalChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnEpicJournalChange", Vec::new())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// After a `DoAction` with `agent` that changed something: tell the journal
/// when the mission was an arc's. A completion is [`on_complete`]'s.
pub fn after_turn(reg: &mut Registry, ch: i64, agent: i64, change: Option<mission::Change>) {
    if change.is_none_or(|c| c == mission::Change::Completed) {
        return;
    }
    let m = mission::progress(reg, ch, agent).mission_id;
    if m.is_some_and(|m| table(reg).and_then(|t| t.arc_of(m)).is_some()) {
        reg.push(JournalChange { character_id: ch });
    }
}

/// The `mission_hooks::HOOKS` row for `Change::Completed`: log the mission
/// in its arc's row, close the arc on an end mission, and push
/// `OnEpicJournalChange`. A mission of no valid arc, or completed with an
/// agent other than its arc's, is not an arc step.
pub fn on_complete(reg: &mut Registry, ev: &MissionEvent) {
    let Some(arc) = table(reg).and_then(|t| t.arc_of(ev.mission_id)).cloned() else {
        return;
    };
    let ours = arc
        .missions
        .get(&ev.mission_id)
        .is_some_and(|n| n.agent == ev.agent_id);
    if !arc.is_valid() || !ours {
        return;
    }
    let now = crate::pyutil::filetime_now();
    let mut p = match progress(reg, ev.character_id, arc.id) {
        Some(p) if !p.done => p,
        _ => Progress::default(),
    };
    p.log.push(Done {
        mission: ev.mission_id,
        accepted_at: mission::progress(reg, ev.character_id, ev.agent_id).accepted_at,
        completed_at: now,
    });
    if arc.next(ev.mission_id).is_empty() {
        p.done = true;
        p.completed_at = now;
    }
    reg.deltas_mut()
        .put(KIND, key(ev.character_id, arc.id), p.to_value());
    reg.push(JournalChange {
        character_id: ev.character_id,
    });
}

/// For tests here and in `service/agents.rs`: the fixture content with
/// [`Table::fixture`] and pool rows for its agents and missions (talk
/// missions, which complete on the next conversation), the agents of a type
/// the pool does not serve (C4 §3.3), so an arc agent has no mission once
/// its arc has none.
#[cfg(test)]
pub(crate) fn fixture_content() -> crate::content::Content {
    use crate::mission_pool as pool;
    const NOT_POOL_TYPE: i64 = 10;
    let t = Table::fixture();
    let mut agents: Vec<pool::Agent> = Vec::new();
    let mut missions = Vec::new();
    for arc in t.arcs() {
        for (m, n) in &arc.missions {
            if !agents.iter().any(|a| a.id == n.agent) {
                agents.push(pool::Agent {
                    id: n.agent,
                    agent_type: NOT_POOL_TYPE,
                    division: 24,
                    level: 1,
                    corp: 1_999_001,
                    faction: 599_001,
                    station: Some(69_000_001),
                    system: Some(39_000_001),
                    ..pool::Agent::default()
                });
            }
            missions.push(pool::Mission {
                id: *m,
                name_id: *m + 1_000,
                template: "EpicArcTalkToAgent".into(),
                family: "talk".into(),
                ..pool::Mission::default()
            });
        }
    }
    let mut c = crate::content::Content::fixture();
    c.insert("epic_arc", t);
    c.insert(
        "mission_pool",
        pool::Table::new(agents, missions, pool::Policy::TRANQUILITY),
    );
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;
    use std::sync::Arc as Shared;

    const START: i64 = 999_501;
    const BRANCH_A: i64 = 999_502;
    const BRANCH_B: i64 = 999_503;
    const END: i64 = 999_504;
    const HIDDEN_START: i64 = 999_511;
    const AGENT_START: i64 = 3_999_401;
    const AGENT_A: i64 = 3_999_402;
    const AGENT_B: i64 = 3_999_403;
    const AGENT_END: i64 = 3_999_404;
    const AGENT_HIDDEN: i64 = 3_999_411;

    fn world() -> (Registry, i64) {
        let reg =
            Registry::new(SandboxConfig::default()).with_content(Shared::new(fixture_content()));
        let me = reg.config().characters[0].character_id;
        (reg, me)
    }

    /// One `DoAction` with `agent` as `service/agents.rs` runs it: prime,
    /// the conversation (which fires `mission_hooks::HOOKS`, this module's
    /// row among them), then the journal.
    fn act(
        reg: &mut Registry,
        me: i64,
        agent: i64,
        action: Option<i64>,
    ) -> Option<mission::Change> {
        prime(reg, me, agent, now());
        let (_, change) = mission::converse_at(reg, Some(me), agent, action);
        after_turn(reg, me, agent, change);
        change
    }

    /// Talk, accept and complete `agent`'s mission. Returns the mission.
    fn run(reg: &mut Registry, me: i64, agent: i64) -> i64 {
        assert_eq!(act(reg, me, agent, None), None);
        let m = mission::progress(reg, me, agent)
            .mission_id
            .expect("an offer");
        let accept = act(reg, me, agent, Some(mission::ACTION_ACCEPT));
        assert_eq!(accept, Some(mission::Change::Accepted));
        let done = act(reg, me, agent, Some(mission::ACTION_COMPLETE));
        assert_eq!(done, Some(mission::Change::Completed));
        m
    }

    /// A dict's logical `(key, value)` pairs (the wire stores value first).
    fn entries(v: &PyValue) -> Vec<(PyValue, PyValue)> {
        match v {
            PyValue::Dict(e) => e.iter().map(|(v, k)| (k.clone(), v.clone())).collect(),
            other => panic!("a dict, got {other:?}"),
        }
    }

    /// A `KeyVal`'s attribute.
    fn attr(v: &PyValue, name: &str) -> Option<PyValue> {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes()).cloned()
    }

    fn now() -> i64 {
        crate::pyutil::filetime_now()
    }

    /// schema.md's invented example row parses under its header.
    #[test]
    fn the_schema_example_row_parses() {
        let t = Table::parse(
            &["9901\t999001\t3999001\t999002,999003\t\t1000\t599001\t999920"],
            vec![],
        )
        .expect("parses");
        let arc = t.arc(9901).expect("the arc");
        assert_eq!(arc.next(999_001), &[999_002, 999_003]);
        assert_eq!(arc.missions[&999_001].agent, 3_999_001);
        assert_eq!((arc.faction, arc.name_id), (Some(599_001), 999_920));
        assert!(Table::parse(&["9901\tx\t1\t\t\t1\t\t1"], vec![]).is_err());
        assert!(
            Table::parse(&["1\t2\t3\t\t\t5\t\t7", "1\t2\t3\t\t\t5\t\t7"], vec![]).is_err(),
            "a mission in two rows"
        );
    }

    /// No bundle serves no invented arcs.
    #[test]
    fn no_bundle_loads_no_arcs() {
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = (TABLE.load)(&src).expect("loads");
        assert_eq!(loaded.downcast_ref::<Table>(), Some(&Table::default()));
    }

    /// A row round-trips through its delta value.
    #[test]
    fn a_row_round_trips() {
        let p = Progress {
            done: true,
            completed_at: 7,
            log: vec![
                Done {
                    mission: 1,
                    accepted_at: 2,
                    completed_at: 3,
                },
                Done {
                    mission: 4,
                    accepted_at: 5,
                    completed_at: 7,
                },
            ],
        };
        assert_eq!(Progress::from_value(&p.to_value()), Some(p));
        assert_eq!(
            Progress::from_value(&Progress::default().to_value()),
            Some(Progress::default())
        );
        assert_eq!(
            Progress::from_value(&["x".into(), "0".into(), "".into()]),
            None
        );
    }

    /// `GetMyEpicArcStatus` is `{}` for a new character [V].
    #[test]
    fn status_is_empty_for_a_new_character() {
        let (reg, me) = world();
        assert!(entries(&status(&reg, me, now())).is_empty());
    }

    /// After the start is offered and accepted, the status holds that one
    /// mission, with an `acceptedDate` and its `nameID`.
    #[test]
    fn one_entry_after_the_start_is_accepted() {
        let (mut reg, me) = world();
        act(&mut reg, me, AGENT_START, None);
        assert_eq!(
            mission::progress(&reg, me, AGENT_START).mission_id,
            Some(START)
        );
        act(&mut reg, me, AGENT_START, Some(mission::ACTION_ACCEPT));
        let s = status(&reg, me, now());
        let arcs = entries(&s);
        assert_eq!(arcs.len(), 1);
        assert_eq!(arcs[0].0, PyValue::Int(9901));
        let ms = entries(&arcs[0].1);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].0, PyValue::Int(START));
        let s = &ms[0].1;
        assert!(matches!(attr(s, "acceptedDate"), Some(PyValue::Int(_))));
        assert_eq!(attr(s, "completedDate"), Some(PyValue::None));
        assert_eq!(attr(s, "quitDate"), Some(PyValue::None));
        assert_eq!(attr(s, "nameID"), Some(PyValue::Int(START + 1_000)));
        let journal = reg
            .take_pending()
            .iter()
            .filter(|(_, p)| p.to_push() == ("OnEpicJournalChange", vec![]))
            .count();
        assert_eq!(journal, 1, "the accept tells the epic journal");
    }

    /// Completing a branching mission offers each of its `nextMissions` at
    /// its own agent; accepting one withdraws the other.
    #[test]
    fn a_branch_leads_to_one_of_its_next_missions() {
        let (mut reg, me) = world();
        assert_eq!(run(&mut reg, me, AGENT_START), START);
        let arc = Table::fixture().arc(9901).cloned().expect("arc");
        assert_eq!(frontier(&reg, me, &arc, now()), arc.next(START));
        assert!(has_missions(&reg, me, AGENT_A, now()));
        assert!(has_missions(&reg, me, AGENT_B, now()));
        assert!(!has_missions(&reg, me, AGENT_START, now()));
        // Both branch agents offer; the player takes B.
        act(&mut reg, me, AGENT_A, None);
        assert_eq!(
            mission::progress(&reg, me, AGENT_A).mission_id,
            Some(BRANCH_A)
        );
        let took = run(&mut reg, me, AGENT_B);
        assert_eq!(took, BRANCH_B);
        assert!(arc.next(START).contains(&took));
        // A's offer is gone, and the end is next.
        act(&mut reg, me, AGENT_A, None);
        assert_eq!(mission::progress(&reg, me, AGENT_A).mission_id, None);
        assert_eq!(frontier(&reg, me, &arc, now()), vec![END]);
    }

    /// Lane clock: at scale N the replay comes N times sooner.
    #[test]
    fn at_scale_n_the_replay_comes_n_times_sooner() {
        let (mut reg, me) = world();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(3).unwrap());
        run(&mut reg, me, AGENT_START);
        run(&mut reg, me, AGENT_A);
        run(&mut reg, me, AGENT_END);
        let arc = Table::fixture().arc(9901).cloned().expect("arc");
        let p = progress(&reg, me, 9901).expect("a row");
        let at = replay_at(&reg, me, AGENT_START, p.completed_at).expect("a replay");
        assert_eq!(at, p.completed_at + arc.restart_minutes * MINUTE / 3);
        assert_eq!(frontier(&reg, me, &arc, at), arc.starts());
    }

    /// Completing the end mission closes the arc: nothing is offered, and
    /// the start agent's replay is `restart_minutes` after the completion.
    #[test]
    fn an_end_mission_sets_the_replay() {
        let (mut reg, me) = world();
        run(&mut reg, me, AGENT_START);
        run(&mut reg, me, AGENT_A);
        assert_eq!(run(&mut reg, me, AGENT_END), END);
        let arc = Table::fixture().arc(9901).cloned().expect("arc");
        let p = progress(&reg, me, 9901).expect("a row");
        assert!(p.done);
        assert_eq!(
            p.log.iter().map(|d| d.mission).collect::<Vec<_>>(),
            [START, BRANCH_A, END]
        );
        let at = replay_at(&reg, me, AGENT_START, now()).expect("a replay");
        assert_eq!(at, p.completed_at + arc.restart_minutes * MINUTE);
        assert_eq!(
            replay_at(&reg, me, AGENT_END, now()),
            None,
            "not a start agent"
        );
        assert!(frontier(&reg, me, &arc, now()).is_empty());
        assert!(!has_missions(&reg, me, AGENT_START, now()));
        // Past the replay the start is offered again, and the old run goes.
        assert_eq!(frontier(&reg, me, &arc, at), arc.starts());
        prime(&mut reg, me, AGENT_START, at);
        assert_eq!(progress(&reg, me, 9901), None);
        assert_eq!(
            mission::progress(&reg, me, AGENT_START).mission_id,
            Some(START)
        );
        // The new run's status is the start, on offer.
        let s = status(&reg, me, now());
        let ms = entries(&entries(&s)[0].1);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].0, PyValue::Int(START));
        assert_eq!(attr(&ms[0].1, "acceptedDate"), Some(PyValue::None));
        assert_eq!(attr(&ms[0].1, "nameID"), Some(PyValue::Int(START + 1_000)));
    }

    /// An arc `IsValidArc` hides (restart interval not above
    /// [`VALID_ABOVE`]) is never offered, listed or advanced [V rule].
    #[test]
    fn arcs_hidden_by_is_valid_arc_are_never_offered() {
        let (mut reg, me) = world();
        let t = Table::fixture();
        let hidden = t.arc_of(HIDDEN_START).cloned().expect("arc");
        assert!(hidden.restart_minutes <= VALID_ABOVE);
        assert!(!hidden.is_valid());
        assert!(!hidden.starts().is_empty(), "it has a start to refuse");
        assert!(frontier(&reg, me, &hidden, now()).is_empty());
        assert!(!has_missions(&reg, me, AGENT_HIDDEN, now()));
        act(&mut reg, me, AGENT_HIDDEN, None);
        assert_eq!(mission::progress(&reg, me, AGENT_HIDDEN).mission_id, None);
        on_complete(
            &mut reg,
            &MissionEvent {
                character_id: me,
                agent_id: AGENT_HIDDEN,
                mission_id: HIDDEN_START,
                change: mission::Change::Completed,
            },
        );
        assert_eq!(progress(&reg, me, hidden.id), None);
        assert!(entries(&status(&reg, me, now())).is_empty());
    }

    /// The message map is `{slot: {missionID: messageID}}` over the
    /// `messages.epicMission.*` slots only.
    #[test]
    fn the_message_map_is_a_dict_of_dicts_of_epic_slots() {
        let (reg, _) = world();
        let m = messages(&reg);
        let slots = entries(&m);
        assert!(!slots.is_empty());
        for (slot, by) in &slots {
            let PyValue::Bytes(s) = slot else {
                panic!("a str slot, got {slot:?}")
            };
            assert!(s.starts_with(SLOT_PREFIX.as_bytes()));
            for (k, v) in entries(by) {
                assert!(matches!((k, v), (PyValue::Int(_), PyValue::Int(_))));
            }
        }
    }
}
