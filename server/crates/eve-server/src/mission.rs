//! Agent missions: what every character is doing with every agent.
//!
//! W13 packet CP26a (`docs/content/plan.md` §6.2), spec C4 §2.7, §3.7-§3.9
//! and packet K3. The offer comes from the pool (`crate::mission_pool`,
//! CP19); this module owns the rest: the per-(character, agent) state, the
//! conversation, the five things C4 §3.7 makes ours (courier destination,
//! kill site, cargo, expiry, reward), the standing gate, and the pocket.
//!
//! # State
//!
//! **Persisted as deltas** (plan §4.2): one `mission` row per (character,
//! agent) who has spoken, `char agent -> mission_id state offer_seq
//! accepted_at expires_at history` ([`Progress`]). Nothing else holds it;
//! every read parses the row. The pocket is **not** persisted (C7 §3.1):
//! NPC balls die with the process, so it lives in the [`SLOT`] beside the
//! derived NPC types, and a restart leaves an accepted kill mission with no
//! pocket, which a quit and re-accept respawns.
//!
//! An offer is drawn **lazily**: a (character, agent) with no row would be
//! offered `pool.offer(seed, character, agent, offer_seq = 0, [])`, so a read
//! (briefing, keywords) shows the same mission the next `DoAction` stores.
//!
//! # The station agent with no pool row
//!
//! The unbound `agentMgr` arms (`service/agents.rs`, CP26b-1's file) do not
//! yet pass the agent they were bound to, so they all speak to [`AGENT_ID`].
//! With a bundle that agent is a pool row like any other. **With no bundle**
//! the fixture pool does not hold it, and it offers [`starter`], the one
//! courier the no-bundle server always served, whose numbers
//! `service/agents.rs`'s, `service/tests.rs`'s and `reprocess.rs`'s tests
//! name. CP26b-1 binds the agent and can retire both.
//!
//! # Not one word of the prose is ours
//!
//! A mission's own message ids where it has the slot; otherwise CCP's generic
//! `UI/Agents/DefaultMessages/*` label in the `(label, {key: id})` form
//! `ProcessMessage` resolves (C4 §2.6, `agents-missions.md` §2.3) **[V]**.

use crate::dungeon::{self, Pocket};
use crate::mission_pool::{self as pool, Mission};
use crate::npc::NpcType;
use crate::pyutil::{bstr, keyval, pydict};
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kind (`delta_kinds::KINDS`).
pub const KIND: &str = "mission";

/// The world slot: pockets and derived pocket types, never persisted.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "mission",
    make: |_| Box::new(Live::default()),
};

/// The agent the unbound `agentMgr` arms speak to; must match
/// `service::STATION_AGENT_ID`. CP26b-1 passes the bound agent instead.
pub const AGENT_ID: i64 = 3_010_110;

/// [`AGENT_ID`]'s solar system, where the starter's legs are.
pub const AGENT_SOLAR_SYSTEM_ID: i64 = 30_000_001;

/// The starter courier's `missionID` (see the module docs).
pub const STARTER_ID: i64 = 4832;
/// The starter's cargo: 4 x 60 m3.
pub const CARGO_TYPE_ID: i64 = 2665;
pub const CARGO_QUANTITY: i64 = 4;
/// The starter's destination, in [`AGENT_SOLAR_SYSTEM_ID`], and its owner.
pub const DESTINATION_ID: i64 = 60_014_437;
const DESTINATION_OWNER: i64 = 1_000_163;
/// `[A]` The station type on a courier leg: the destination's own type is
/// not in the bundle. Falsified by a leg whose icon or name renders wrong.
const STATION_TYPE_ID: i64 = 2502;

/// `[A]` NPCs in a kill pocket: the dungeon table never says what a spawn
/// holds. Falsified by CP22's squad fill, which replaces it.
pub const POCKET_NPCS: usize = 3;

/// `[A]` (C4 §3.7) How long an offer stands, and an accepted mission with no
/// `expire_minutes`.
pub const OFFER_LIFETIME: i64 = 7 * crate::seed::DAY;
pub const ACCEPTED_LIFETIME: i64 = 7 * crate::seed::DAY;

/// [`OFFER_LIFETIME`] at the game clock's pace (lane `clock`): an offer made
/// at scale N stands `OFFER_LIFETIME / N`. The expiry is stored, so a restart
/// at another scale keeps it. An accepted mission's lifetime (its
/// `expire_minutes`, else [`ACCEPTED_LIFETIME`]) is paced the same way.
fn offer_lifetime(w: &Registry) -> i64 {
    crate::game_clock::span(w, OFFER_LIFETIME)
}

/// `[A]` How many further `offer_seq` values a draw tries when it lands on a
/// mission this server cannot run (a kill with no NPC rows, a mining row).
pub const REDRAWS: u64 = 16;

/// Generic lines, by message id (C4 §2.6, §3.3) **[V]**.
const MSG_TOO_LOW_STANDINGS: i64 = 236_691;
const MSG_NO_MISSION: i64 = 236_798;

/// The `CmdWarpToStuff` subject this server uses for "my mission pocket";
/// ours, not one of the client's thirteen.
pub const WARP_SUBJECT_MISSION: &[u8] = b"agentmission";

/// "Warp Fleet to" an agent bookmark: a **fleet-mate's** pocket with the
/// agent, for a member who is not on the mission. Ours, like
/// [`WARP_SUBJECT_MISSION`]; only `fleet_warp::follow_if` asks for it, so a
/// pilot's own `WarpToLocation` never reaches another pilot's pocket.
pub const WARP_SUBJECT_MISSION_FLEET: &[u8] = b"agentmissionfleet";

/// Our action ids; the client echoes them. The **button** each maps to is the
/// client's (`appConst`).
pub const ACTION_ACCEPT: i64 = 2;
pub const ACTION_DECLINE: i64 = 3;
pub const ACTION_COMPLETE: i64 = 4;
pub const ACTION_QUIT: i64 = 5;

mod button {
    pub const ACCEPT: i64 = 3;
    pub const DECLINE: i64 = 9;
    pub const COMPLETE: i64 = 6;
    pub const QUIT: i64 = 11;
}

/// `appConst.agentMissionState*`, **[V]** `appConst.py:1926-1934`.
mod mission_state {
    pub const ALLOCATED: i64 = 0;
    pub const OFFERED: i64 = 1;
    pub const ACCEPTED: i64 = 2;
}

/// Where a character is with an agent's current mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Offered,
    Accepted,
}

/// One `mission` delta row's value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Progress {
    /// The current mission; `None` between a turn-in or decline and the next
    /// conversation, which draws the next.
    pub mission_id: Option<i64>,
    pub state: State,
    /// How many offers this agent has drawn for this character: the next
    /// draw uses it, the current offer used one less.
    pub offer_seq: u64,
    pub accepted_at: i64,
    pub expires_at: i64,
    /// The last [`pool::RECENT_FAMILIES`] missions completed or declined,
    /// oldest first (C4 §3.5).
    pub history: Vec<i64>,
}

impl Progress {
    pub fn to_value(&self) -> Vec<String> {
        let id = self.mission_id.map_or(String::new(), |m| m.to_string());
        let state = match self.state {
            State::Offered => "offered",
            State::Accepted => "accepted",
        };
        let history: Vec<String> = self.history.iter().map(i64::to_string).collect();
        vec![
            id,
            state.to_owned(),
            self.offer_seq.to_string(),
            self.accepted_at.to_string(),
            self.expires_at.to_string(),
            history.join(","),
        ]
    }

    /// `None` for a row this build cannot read, which then reads as no row.
    pub fn from_value(v: &[String]) -> Option<Progress> {
        let [id, state, seq, at, exp, hist] = v else {
            return None;
        };
        Some(Progress {
            mission_id: if id.is_empty() {
                None
            } else {
                Some(id.parse().ok()?)
            },
            state: match state.as_str() {
                "offered" => State::Offered,
                "accepted" => State::Accepted,
                _ => return None,
            },
            offer_seq: seq.parse().ok()?,
            accepted_at: at.parse().ok()?,
            expires_at: exp.parse().ok()?,
            history: hist
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?,
        })
    }

    fn remember(&mut self, mission: i64) {
        self.history.push(mission);
        let over = self.history.len().saturating_sub(pool::RECENT_FAMILIES);
        self.history.drain(..over);
    }
}

/// What the [`SLOT`] holds.
#[derive(Default)]
pub struct Live {
    /// `(character, agent) -> pocket`. Kept across a quit, so a re-accept
    /// sends the pilot back to the NPCs still there; dropped on completion.
    pockets: BTreeMap<(i64, i64), Pocket>,
    /// NPC types derived from `npc_data` rows, leaked once per type: `Npc`
    /// holds a `&'static NpcType`.
    rats: BTreeMap<i64, &'static NpcType>,
}

fn key(character: i64, agent: i64) -> Vec<String> {
    vec![character.to_string(), agent.to_string()]
}

/// The stored row, or the default for a pair that never spoke.
pub fn progress(w: &Registry, character: i64, agent: i64) -> Progress {
    w.deltas()
        .of(KIND)
        .get(&key(character, agent))
        .and_then(|v| Progress::from_value(v))
        .unwrap_or_default()
}

fn save(w: &mut Registry, character: i64, agent: i64, p: &Progress) {
    w.deltas_mut()
        .put(KIND, key(character, agent), p.to_value());
}

/// Store `mission` as `agent`'s standing offer to `character`, made by a
/// trigger outside the conversation (CP48's storyline offer): the next
/// conversation with `agent` shows it, as it would a drawn offer.
pub fn put_offer(w: &mut Registry, character: i64, agent: i64, mission: i64, now: i64) {
    let mut p = progress(w, character, agent);
    p.mission_id = Some(mission);
    p.state = State::Offered;
    p.offer_seq += 1;
    p.expires_at = now + offer_lifetime(w);
    save(w, character, agent, &p);
}

/// Put `mission` on offer from `agent` to `character`, or withdraw the
/// standing offer with `None`: the seam for offers that do not come from the
/// pool (CP41's epic arcs). An accepted mission is never touched (`false`).
/// The same mission still on offer is left as it is; a new or expired one
/// stands until `now + OFFER_LIFETIME`. The `mission` delta stays this
/// module's alone (review A6).
pub fn set_offer(
    w: &mut Registry,
    character: i64,
    agent: i64,
    mission: Option<i64>,
    now: i64,
) -> bool {
    let mut p = progress(w, character, agent);
    if p.state == State::Accepted {
        return false;
    }
    if p.mission_id == mission && (mission.is_none() || p.expires_at > now) {
        return true;
    }
    p.mission_id = mission;
    p.state = State::Offered;
    p.expires_at = now + offer_lifetime(w);
    save(w, character, agent, &p);
    true
}

/// Whether `actor` has accepted [`AGENT_ID`]'s mission. `Offered` for no actor.
pub fn state(w: &Registry, actor: Option<i64>) -> State {
    actor.map_or(State::Offered, |id| progress(w, id, AGENT_ID).state)
}

fn pool_table(w: &Registry) -> Option<&pool::Table> {
    w.content().try_get::<pool::Table>()
}

/// The agent's row; [`AGENT_ID`] with no pool row is the starter's agent.
fn agent_row(w: &Registry, agent: i64) -> Option<pool::Agent> {
    if let Some(a) = pool_table(w).and_then(|t| t.agent(agent)) {
        return Some(a.clone());
    }
    (agent == AGENT_ID).then(|| pool::Agent {
        id: AGENT_ID,
        agent_type: pool::TYPE_BASIC,
        division: 22,
        level: 1,
        corp: crate::model::STATION_AGENT_CORP,
        system: Some(AGENT_SOLAR_SYSTEM_ID),
        ..pool::Agent::default()
    })
}

/// The starter courier (module docs): 4832's own message ids **[V]**.
pub fn starter() -> Mission {
    let slots = [
        ("messages.mission.briefing", 134_916),
        ("messages.mission.offered.agentsays", 134_918),
        ("messages.mission.accepted.agentsays", 134_919),
        ("messages.mission.declined.agentsays", 134_921),
        ("messages.mission.completed.agentsays", 134_920),
        ("messages.mission.extrainfo.header", 134_917),
        ("messages.mission.extrainfo.body", 134_915),
    ];
    Mission {
        id: STARTER_ID,
        name_id: 56_901,
        template: "BasicCourier".into(),
        family: "courier".into(),
        level: Some(1),
        obj_type: Some(CARGO_TYPE_ID),
        obj_qty: Some(CARGO_QUANTITY),
        obj_volume_total: Some(240.0),
        messages: slots.iter().map(|(s, id)| ((*s).into(), *id)).collect(),
        ..Mission::default()
    }
}

fn mission_row(w: &Registry, id: i64) -> Option<Mission> {
    match pool_table(w).and_then(|t| t.mission(id)) {
        Some(m) => Some(m.clone()),
        None => (id == STARTER_ID).then(starter),
    }
}

/// A kill mission, by the row's own `family` (`agents/missions.tsv`, from
/// the `…Kill` template): `BasicKill`, the storyline and career kills and
/// `EpicArcKill` alike (FX5).
fn is_kill(m: &Mission) -> bool {
    m.family == "kill"
}

/// A courier mission, by the row's own `family`: every `…Courier` template.
/// `[A]` The `trade` family (`BasicTrade`, the storyline and epic-arc trades)
/// runs as a courier too, as `BasicTrade` always has here: the agent hands
/// over the goods and the pilot hauls them. Falsified by a trade mission on
/// Tranquility that gives no cargo and asks for the items at the agent.
fn is_courier(m: &Mission) -> bool {
    matches!(m.family.as_str(), "courier" | "trade")
}

/// Whether a kill pocket can be spawned for `m`: `npc_data` is loaded and
/// has a pocket type for the row's enemy faction (CA9).
fn pocket_possible(w: &Registry, m: &Mission) -> bool {
    let data = w.content().try_get::<crate::npc_data::Table>();
    m.enemy_faction
        .zip(data)
        .is_some_and(|(e, d)| !pocket_types(d, e).is_empty())
}

/// The enemy's pocket types from `npc_data`, best first: frigates, then base
/// tier, then the lowest id `[A]`. Aliases are left out.
pub fn pocket_types(data: &crate::npc_data::Table, enemy: i64) -> Vec<i64> {
    let mut rows: Vec<_> = data
        .types
        .values()
        .filter(|t| t.faction_id == enemy && t.alias_of.is_none())
        .collect();
    rows.sort_by_key(|t| {
        (
            t.class != "Frigate",
            t.tier != crate::npc_data::Tier::Base,
            t.type_id,
        )
    });
    rows.into_iter().map(|t| t.type_id).collect()
}

/// Whether this server can run `m`: a courier always; a kill only with
/// `npc_data` loaded and a pocket type for its enemy (CA9); nothing else
/// yet `[A]`.
fn servable(w: &Registry, m: &Mission) -> bool {
    is_courier(m) || (is_kill(m) && pocket_possible(w, m))
}

/// The next offer from `seq` on, with the seq it used. `None` is `NoMission1`.
fn draw(w: &Registry, character: i64, agent: i64, p: &Progress) -> Option<(i64, u64)> {
    let Some(t) = pool_table(w).filter(|t| t.agent(agent).is_some()) else {
        return (agent == AGENT_ID).then_some((STARTER_ID, p.offer_seq));
    };
    // An agent with a fixed list offers it, never the pool (CP48, C4 §8).
    match crate::storyline::fixed_offer(w, character, agent) {
        crate::storyline::Fixed::Next(id) => return Some((id, p.offer_seq)),
        crate::storyline::Fixed::Done => return None,
        crate::storyline::Fixed::NotFixed => {}
    }
    let a = t.agent(agent)?;
    if !t.candidates(a).iter().any(|m| servable(w, m)) {
        return None;
    }
    let data = w.content().try_get::<crate::npc_data::Table>();
    let home = |f: i64| data.and_then(|d| d.home_pirate(f));
    let seed = w.world().seed;
    (p.offer_seq..p.offer_seq + REDRAWS).find_map(|seq| {
        let id = t.offer(seed, character, agent, seq, &p.history, &home)?;
        servable(w, t.mission(id)?).then_some((id, seq))
    })
}

/// The mission a (character, agent) is on or would be offered, with the
/// progress it would be stored as. Reads only.
struct Current {
    p: Progress,
    m: Mission,
    agent: pool::Agent,
    /// Not yet in the delta map.
    fresh: bool,
}

fn current(w: &Registry, character: i64, agent: i64, now: i64) -> Option<Current> {
    let agent_row = agent_row(w, agent)?;
    let mut p = progress(w, character, agent);
    let stale = p.state == State::Offered && p.expires_at <= now;
    let mut fresh = false;
    if p.mission_id.is_none() || stale {
        let (id, seq) = draw(w, character, agent, &p)?;
        p.mission_id = Some(id);
        p.state = State::Offered;
        p.offer_seq = seq + 1;
        p.expires_at = now + offer_lifetime(w);
        fresh = true;
    }
    let m = mission_row(w, p.mission_id?)?;
    Some(Current {
        p,
        m,
        agent: agent_row,
        fresh,
    })
}

/// A courier's destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dest {
    pub station: i64,
    pub system: i64,
    pub owner: i64,
}

/// C4 §3.7's courier destination `[A]`: another pool agent's station, in the
/// agent's system for L1-2, its constellation for L3 and its region above,
/// widening outward when that ring is empty; picked with the offer's own
/// stream. Jumps are approximated by those rings until a gate graph is
/// loaded; falsified by a Tranquility offer census of jump counts.
fn destination(w: &Registry, c: &Current) -> Option<Dest> {
    if c.m.id == STARTER_ID && pool_table(w).and_then(|t| t.agent(c.agent.id)).is_none() {
        return Some(Dest {
            station: DESTINATION_ID,
            system: AGENT_SOLAR_SYSTEM_ID,
            owner: DESTINATION_OWNER,
        });
    }
    let (t, home) = (pool_table(w)?, c.agent.system?);
    let uni = w.content().try_get::<crate::celestial::Universe>();
    let ring = |sys: i64| -> i64 {
        let (a, b) = (
            uni.and_then(|u| u.system(home)),
            uni.and_then(|u| u.system(sys)),
        );
        match (a, b) {
            _ if sys == home => 0,
            (Some(a), Some(b)) if a.constellation_id == b.constellation_id => 1,
            (Some(a), Some(b)) if a.region_id == b.region_id => 2,
            _ => 3,
        }
    };
    let want = match c.m.level.unwrap_or(1) {
        ..=2 => 0,
        3 => 1,
        _ => 2,
    };
    let mut by_station: BTreeMap<i64, Dest> = BTreeMap::new();
    for a in t.agents() {
        if let (Some(station), Some(system)) = (a.station, a.system)
            && Some(station) != c.agent.station
        {
            by_station.entry(station).or_insert(Dest {
                station,
                system,
                owner: a.corp,
            });
        }
    }
    let best = by_station
        .values()
        .map(|d| (ring(d.system) - want).abs())
        .min()?;
    let picks: Vec<&Dest> = by_station
        .values()
        .filter(|d| (ring(d.system) - want).abs() == best)
        .collect();
    let seq = c.p.offer_seq.saturating_sub(1);
    let mut rng = crate::seed::stream(
        w.world().seed,
        crate::seed::Kind::Offer,
        &[c.agent.id as u64, seq, u64::MAX],
    );
    picks
        .get(rng.range(0, picks.len() as u64) as usize)
        .map(|d| **d)
}

/// The kill site: the agent's own system `[A]` (C4 §3.7 allows 0 jumps).
fn site(c: &Current) -> i64 {
    c.agent.system.unwrap_or(AGENT_SOLAR_SYSTEM_ID)
}

/// What the client must be told after an action, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Accepted,
    Declined,
    Completed,
    Quit,
    /// A pocket's last NPC died ([`npc_killed`]).
    Modified,
}

impl Change {
    /// **A string**: compared against `appConst.agentMission*` literals
    /// (`appConst.py:1909-1920`) **[V]**.
    pub fn as_str(self) -> &'static str {
        match self {
            Change::Accepted => "accepted",
            Change::Declined => "declined",
            Change::Completed => "completed",
            Change::Quit => "quit",
            Change::Modified => "modified",
        }
    }
}

/// `[A]` (C4 §3.8) The mood suffix of the generic lines, by standing toward
/// the agent: below 0, 2, 5, else high.
fn mood(standing: f64) -> &'static str {
    match standing {
        s if s < 0.0 => "Low",
        s if s < 2.0 => "LowMed",
        s if s < 5.0 => "MedHigh",
        _ => "High",
    }
}

/// `messages.mission.<slot>`, if the mission has it.
fn slot(m: &Mission, name: &str) -> Option<i64> {
    let full = format!("messages.mission.{name}");
    m.messages.iter().find(|(k, _)| *k == full).map(|(_, v)| *v)
}

/// The label form `ProcessMessage` resolves: `(label, {key: message id})`.
fn generic(name: &str, args: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Tuple(vec![
        bstr(&format!("UI/Agents/DefaultMessages/{name}")),
        pydict(args),
    ])
}

/// The mission's own line, or the generic one.
fn line(m: &Mission, name: &str, fallback: impl FnOnce() -> PyValue) -> PyValue {
    slot(m, name).map_or_else(fallback, PyValue::Int)
}

/// The offer: `offered`, or `MissionOfferN` around the briefing.
fn offer_line(m: &Mission, variant: u64) -> PyValue {
    line(m, "offered.agentsays", || {
        let briefing = slot(m, "briefing").map(PyValue::Int);
        generic(
            &format!("MissionOffer{}", 1 + variant % 6),
            briefing
                .map(|b| vec![("missionBriefingText", b)])
                .unwrap_or_default(),
        )
    })
}

/// `lastActionInfo`: **all five keys are subscripted**, so all five ship.
fn last_action_info(completed: bool, declined: bool, quit: bool) -> PyValue {
    pydict(vec![
        ("missionCompleted", PyValue::Int(completed as i64)),
        ("missionDeclined", PyValue::Int(declined as i64)),
        ("missionQuit", PyValue::Int(quit as i64)),
        ("missionCantReplay", PyValue::Int(0)),
        ("loyaltyPoints", PyValue::Int(0)),
    ])
}

/// `((agentSaysMsg, availableActions), lastActionInfo)`, where
/// `agentSaysMsg` is `(msgInfo, contentID)` (`_, mission_id = agent_says`).
fn reply(says: PyValue, content: PyValue, actions: Vec<(i64, i64)>, info: PyValue) -> PyValue {
    let actions = actions
        .into_iter()
        .map(|(a, b)| PyValue::Tuple(vec![PyValue::Int(a), PyValue::Int(b)]))
        .collect();
    PyValue::Tuple(vec![
        PyValue::Tuple(vec![
            PyValue::Tuple(vec![says, content]),
            PyValue::List(actions),
        ]),
        info,
    ])
}

/// A refusal with no mission: `(message id, None)` and no buttons.
fn refusal(msg: i64) -> PyValue {
    reply(
        PyValue::Int(msg),
        PyValue::None,
        vec![],
        last_action_info(false, false, false),
    )
}

/// What one `DoAction` answers, before anything touches the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub answer: PyValue,
    pub change: Option<Change>,
}

/// The conversation (C4 §3.8), pure. `done` is whether the accepted
/// mission's objective is met; `mood` and `variant` pick generic lines.
pub fn do_action(
    m: &Mission,
    state: State,
    done: bool,
    action: Option<i64>,
    mood: &str,
    variant: u64,
) -> Turn {
    let id = PyValue::Int(m.id);
    let plain = || last_action_info(false, false, false);
    let turn = |answer, change| Turn { answer, change };
    match (state, action) {
        (State::Offered, None) => turn(
            reply(
                offer_line(m, variant),
                id,
                vec![
                    (ACTION_ACCEPT, button::ACCEPT),
                    (ACTION_DECLINE, button::DECLINE),
                ],
                plain(),
            ),
            None,
        ),
        (State::Offered, Some(ACTION_ACCEPT)) => {
            let says = line(m, "accepted.agentsays", || {
                generic(&format!("MissionAccepted{mood}1"), vec![])
            });
            turn(reply(says, id, vec![], plain()), Some(Change::Accepted))
        }
        // No standing penalty or decline timer: we have neither number.
        (State::Offered, Some(ACTION_DECLINE)) => {
            let says = line(m, "declined.agentsays", || {
                generic(&format!("MissionDeclined{}", 1 + variant % 5), vec![])
            });
            let info = last_action_info(false, true, false);
            turn(reply(says, id, vec![], info), Some(Change::Declined))
        }
        // **No Complete before the objective is met**; Quit is always there.
        (State::Accepted, None) => {
            let mut actions = Vec::new();
            if done {
                actions.push((ACTION_COMPLETE, button::COMPLETE));
            }
            actions.push((ACTION_QUIT, button::QUIT));
            turn(reply(offer_line(m, variant), id, actions, plain()), None)
        }
        (State::Accepted, Some(ACTION_COMPLETE)) if done => {
            let says = line(m, "completed.agentsays", || {
                generic(&format!("MissionCompleted{mood}1"), vec![])
            });
            let info = last_action_info(true, false, false);
            turn(reply(says, id, vec![], info), Some(Change::Completed))
        }
        (State::Accepted, Some(ACTION_QUIT)) => {
            let says = line(m, "quitted.agentsays", || {
                generic(&format!("MissionQuitted{}", 1 + variant % 3), vec![])
            });
            let info = last_action_info(false, false, true);
            turn(reply(says, id, vec![], info), Some(Change::Quit))
        }
        // Anything else: the offer text, no buttons; inert, not wrong.
        _ => turn(reply(offer_line(m, variant), id, vec![], plain()), None),
    }
}

/// `CanUseAgent` (C4 §2.7) over the stored standings, unadjusted for skills
/// `[A]`.
fn can_use(w: &Registry, character: i64, a: &pool::Agent) -> bool {
    let s = pool::Standings {
        faction: w.npc_standing(character, a.faction),
        corp: w.npc_standing(character, a.corp),
        agent: w.npc_standing(character, a.id),
    };
    pool::can_use_agent(a.agent_type, a.level, s, false)
}

/// Whether `character`'s accepted mission `c` has met its objective, which
/// is what puts Complete on the dialogue and `completionStatus` in the
/// objective document (`agentDialogueUtil.py:409`) **[V]**.
///
/// - A kill: its pocket is cleared. `[A]` A kill row that can get no pocket
///   ([`pocket_possible`]: no enemy faction, or no NPC rows for it; a
///   storyline or epic-arc offer does not go through [`servable`]) is met
///   on talking, as before FX5, so it can never get stuck; falsified by
///   such a row on Tranquility spawning a site.
/// - A courier (`docs/port-tracker.md` §72.2): [`delivered`].
/// - Anything else (mining, talk, other): met. A talk mission is met by
///   talking; mining is never offered ([`servable`]) `[A]`, falsified by a
///   chain that hands out a mining row.
fn objective_done(w: &Registry, character: i64, c: &Current) -> bool {
    if is_kill(&c.m) {
        match w.ext::<Live>().pockets.get(&(character, c.agent.id)) {
            Some(p) => dungeon::cleared(w, p),
            None => !pocket_possible(w, &c.m),
        }
    } else if is_courier(&c.m) {
        delivered(w, character, c)
    } else {
        true
    }
}

/// A courier is delivered when its whole cargo, `obj_qty` units of
/// `obj_type`, sits in [`destination`]'s hangar (`FLAG_HANGAR`) under
/// `character`: the drop-off leg of the `'transport'` objective
/// (`agents-missions.md` §4.5, `mission-model.md` §4) **[V]**. Partial cargo
/// is not a delivery, and neither is the cargo in a ship's hold docked
/// there: the bot and the player unload, as the briefing says.
///
/// No destination means it cannot be delivered. A courier row with no cargo
/// is delivered by definition `[A]` (nothing to carry, and docking is not
/// ours to check); falsified by such a row on Tranquility asking for a
/// visit.
fn delivered(w: &Registry, character: i64, c: &Current) -> bool {
    let Some(d) = destination(w, c) else {
        return false;
    };
    let (Some(type_id), Some(qty)) = (c.m.obj_type, c.m.obj_qty) else {
        return true;
    };
    let there: i64 = w
        .store()
        .items()
        .iter()
        .filter(|i| {
            i.type_id == type_id
                && i.owner_id == character
                && i.location_id == d.station
                && i.flag_id == crate::model::FLAG_HANGAR
        })
        .map(|i| i.quantity)
        .sum();
    there >= qty
}

/// [`converse_at`] with [`AGENT_ID`], for the unbound arms.
pub fn converse(
    w: &mut Registry,
    actor: Option<i64>,
    action: Option<i64>,
) -> (PyValue, Option<Change>) {
    converse_at(w, actor, AGENT_ID, action)
}

/// Run one `DoAction` with `agent` against the world.
///
/// Accepting a courier puts its cargo in the agent's station hangar (the
/// character's station when the agent has none); accepting a kill mission
/// puts its pocket in space unless one is still alive; completing pays, raises
/// the standings and drops the pocket, then offers a chain's next part.
/// **It pushes no `OnAgentMissionChange`**: the caller does, last, with
/// [`tell_journal_at`]. **No character means answer, store nothing**: a
/// refusal would blank the window.
pub fn converse_at(
    w: &mut Registry,
    actor: Option<i64>,
    agent: i64,
    action: Option<i64>,
) -> (PyValue, Option<Change>) {
    let now = crate::pyutil::filetime_now();
    let ch = actor.filter(|id| w.character(*id).is_some());
    let Some(c) = current(w, ch.unwrap_or(0), agent, now) else {
        return (refusal(MSG_NO_MISSION), None);
    };
    let Some(ch) = ch else {
        let t = do_action(&c.m, State::Offered, false, action, "LowMed", 0);
        return (t.answer, None);
    };
    if c.p.state == State::Offered && !can_use(w, ch, &c.agent) {
        return (refusal(MSG_TOO_LOW_STANDINGS), None);
    }
    let done = c.p.state == State::Accepted && objective_done(w, ch, &c);
    let standing = w.npc_standing(ch, agent);
    let variant = crate::seed::stream(
        w.world().seed,
        crate::seed::Kind::Offer,
        &[ch as u64, agent as u64, c.p.offer_seq, now as u64],
    )
    .next();
    let turn = do_action(&c.m, c.p.state, done, action, mood(standing), variant);
    let mut p = c.p.clone();
    match turn.change {
        Some(Change::Accepted) => {
            p.state = State::Accepted;
            p.accepted_at = now;
            p.expires_at = now
                + crate::game_clock::span(
                    w,
                    c.m.expire_minutes
                        .map_or(ACCEPTED_LIFETIME, |n| n * 60 * 10_000_000),
                );
            if is_kill(&c.m) {
                spawn_pocket(w, ch, &c);
            } else if is_courier(&c.m) {
                spawn_cargo(w, ch, &c);
            }
        }
        Some(Change::Declined) => {
            p.remember(c.m.id);
            p.mission_id = None;
        }
        Some(Change::Completed) => {
            crate::mission_rewards::pay(w, ch, &c.agent, &c.m, c.p.accepted_at, now);
            w.ext_mut::<Live>().pockets.remove(&(ch, agent));
            p.remember(c.m.id);
            p.state = State::Offered;
            p.mission_id = None;
            let data = w.content().try_get::<crate::npc_data::Table>();
            let home = |f: i64| data.and_then(|d| d.home_pirate(f));
            let next = pool_table(w).and_then(|t| t.next_in_chain(agent, c.m.id, &home));
            if let Some(next) = next {
                p.mission_id = Some(next);
                p.offer_seq += 1;
                p.expires_at = now + offer_lifetime(w);
            }
        }
        Some(Change::Quit) => p.state = State::Offered,
        Some(Change::Modified) | None => {}
    }
    if turn.change.is_some() || c.fresh {
        save(w, ch, agent, &p);
    }
    // What else the change sets in motion (`mission_hooks::HOOKS`), after
    // the store so a hook reads the new progress.
    if let Some(change) = turn.change {
        let ev = crate::mission_hooks::MissionEvent {
            character_id: ch,
            agent_id: agent,
            mission_id: c.m.id,
            change,
        };
        crate::mission_hooks::fire(&crate::module::plan().mission, w, &ev);
    }
    (turn.answer, turn.change)
}

/// The pocket's NPC type for the mission's enemy, from `npc_data`: the hand
/// table or the world's typedata first, then derived once and kept.
fn rat(w: &mut Registry, enemy: i64) -> Option<&'static NpcType> {
    let data = w.content().try_get::<crate::npc_data::Table>()?;
    let types = pocket_types(data, enemy);
    for type_id in types {
        if let Some(t) = crate::npc::find(type_id) {
            return Some(t);
        }
        if let Some(t) = w.ext::<Live>().rats.get(&type_id) {
            return Some(t);
        }
        let derived = (|| {
            let data = w.content().try_get::<crate::npc_data::Table>()?;
            let td = w.content().try_get::<crate::typedata::Installed>()?.table();
            crate::npc::derive_loaded(td.get(type_id)?, td, data)
        })();
        if let Some(t) = derived {
            let t: &'static NpcType = Box::leak(Box::new(t));
            w.ext_mut::<Live>().rats.insert(type_id, t);
            return Some(t);
        }
    }
    None
}

fn spawn_pocket(w: &mut Registry, ch: i64, c: &Current) {
    let k = (ch, c.agent.id);
    if w.ext::<Live>()
        .pockets
        .get(&k)
        .is_some_and(|p| dungeon::alive(w, p))
    {
        return;
    }
    let Some(kind) = c.m.enemy_faction.and_then(|e| rat(w, e)) else {
        return;
    };
    if let Some(pocket) = dungeon::spawn(w, kind, site(c), POCKET_NPCS) {
        w.ext_mut::<Live>().pockets.insert(k, pocket);
    }
}

/// The courier's `obj_type x obj_qty`, topped up to the quantity: accepting
/// twice must not double it.
fn spawn_cargo(w: &mut Registry, ch: i64, c: &Current) {
    let (Some(type_id), Some(qty)) = (c.m.obj_type, c.m.obj_qty) else {
        return;
    };
    let Some(station) = c
        .agent
        .station
        .or_else(|| w.character(ch).and_then(|x| x.station_id))
    else {
        return;
    };
    let have: i64 = w
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id == type_id && i.owner_id == ch)
        .map(|i| i.quantity)
        .sum();
    if have >= qty {
        return;
    }
    if let Ok((ids, _)) = w.spawn_items(&type_id.to_string(), qty - have, ch, station) {
        w.mark_dirty();
        for id in ids {
            if let Some(item) = w.item(id).cloned() {
                let row = w.item_row_value(&item);
                w.push(crate::notify::ItemChange::spawned(row));
            }
        }
        w.push(crate::notify::InvalidateLocations {
            location_ids: vec![station],
        });
    }
}

/// [`tell_journal_at`] with [`AGENT_ID`].
pub fn tell_journal(w: &mut Registry, change: Option<Change>) {
    tell_journal_at(w, AGENT_ID, change);
}

/// Push the `OnAgentMissionChange` a [`converse_at`] owes, **last**: the
/// journal's handler calls straight back for the journal and the hangar.
pub fn tell_journal_at(w: &mut Registry, agent: i64, change: Option<Change>) {
    if let Some(change) = change {
        w.push(AgentMissionChange {
            change: change.as_str(),
            agent_id: agent,
        });
    }
}

/// GAP-LOOT: the character whose accepted mission's pocket holds `npc_id`;
/// its wreck is theirs (EvEJS `combatDestruction.js:120-224`, `[A]`).
pub fn pocket_owner(w: &Registry, npc_id: i64) -> Option<i64> {
    w.ext::<Live>()
        .pockets
        .iter()
        .find(|(_, p)| p.npc_ids.contains(&npc_id))
        .map(|((ch, _), _)| *ch)
}

/// **An NPC died**: every (character, agent) whose accepted kill mission's
/// pocket held it and is now cleared is sent `OnAgentMissionChange
/// ('modified', agentID)`, so an open dialogue re-runs `InteractWithAgent` and
/// the Complete button appears (`agentDialogueWindow.py:124-129`) **[V]**.
pub fn npc_killed(w: &mut Registry, npc_id: i64) {
    let hit: Vec<(i64, i64)> = w
        .ext::<Live>()
        .pockets
        .iter()
        .filter(|(_, p)| p.npc_ids.contains(&npc_id) && dungeon::cleared(w, p))
        .map(|(k, _)| *k)
        .collect();
    for (ch, agent) in hit {
        if progress(w, ch, agent).state == State::Accepted {
            w.schedule(
                std::time::Duration::ZERO,
                crate::notify::Addressed {
                    to: Some(vec![ch]),
                    inner: Box::new(AgentMissionChange {
                        change: Change::Modified.as_str(),
                        agent_id: agent,
                    }),
                },
            );
        }
    }
}

/// **The rollover step** (plan §4.4 step 5, C4 §3.7): an offer or accepted
/// mission whose expiry has passed by the start of `day` is dropped, with its
/// pocket; `offer_seq` and history stay, so the next offer is a fresh draw.
pub fn on_rollover(w: &mut Registry, day: u32) {
    // Lane clock: the rollover's own time at a `--time-scale` above 1.
    let cutoff = crate::game_clock::day_start(w, day);
    let expired: Vec<(Vec<String>, Progress)> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), Progress::from_value(v)?)))
        .filter(|(_, p)| p.mission_id.is_some() && p.expires_at <= cutoff)
        .collect();
    for (k, mut p) in expired {
        p.mission_id = None;
        p.state = State::Offered;
        if let [ch, agent] = k.as_slice()
            && let (Ok(ch), Ok(agent)) = (ch.parse(), agent.parse())
        {
            w.ext_mut::<Live>().pockets.remove(&(ch, agent));
        }
        w.deltas_mut().put(KIND, k, p.to_value());
    }
}

/// `GetMissionKeywords(contentID)` for the unbound arms: the starter's
/// destination and [`AGENT_ID`]'s kill site together, so neither kind of
/// prose renders a raw token. [`keywords_at`] answers per mission.
pub fn keywords(_content_id: Option<i64>) -> PyValue {
    pydict(vec![
        ("objectiveDestinationID", PyValue::Int(DESTINATION_ID)),
        ("dungeonLocationID", PyValue::Int(AGENT_SOLAR_SYSTEM_ID)),
        ("dungeonSolarSystemID", PyValue::Int(AGENT_SOLAR_SYSTEM_ID)),
    ])
}

fn keywords_of(w: &Registry, c: &Current) -> PyValue {
    if is_kill(&c.m) {
        let s = site(c);
        return pydict(vec![
            ("dungeonLocationID", PyValue::Int(s)),
            ("dungeonSolarSystemID", PyValue::Int(s)),
        ]);
    }
    match destination(w, c) {
        Some(d) => pydict(vec![
            ("objectiveDestinationID", PyValue::Int(d.station)),
            ("objectiveDestinationSystemID", PyValue::Int(d.system)),
        ]),
        None => PyValue::Dict(vec![]),
    }
}

/// `GetMissionKeywords` for `actor`'s mission with `agent`. `{}`, never
/// `None`: a `None` raises outside `ProcessMessage`'s only `try`.
pub fn keywords_at(w: &Registry, actor: Option<i64>, agent: i64) -> PyValue {
    let now = crate::pyutil::filetime_now();
    current(w, actor.unwrap_or(0), agent, now).map_or(PyValue::Dict(vec![]), |c| keywords_of(w, &c))
}

/// `GetMissionBriefingInfo`. `'ContentID'` and `'Mission Keywords'` are both
/// mandatory on a truthy dict; with no mission the answer is `None`, which
/// `if briefingInformation` reads as "no briefing".
pub fn briefing_info_at(w: &Registry, actor: Option<i64>, agent: i64) -> PyValue {
    let now = crate::pyutil::filetime_now();
    let Some(c) = current(w, actor.unwrap_or(0), agent, now) else {
        return PyValue::None;
    };
    let accepted = c.p.state == State::Accepted;
    pydict(vec![
        ("ContentID", PyValue::Int(c.m.id)),
        ("Mission Title ID", PyValue::Int(c.m.name_id)),
        (
            "Mission Briefing ID",
            slot(&c.m, "briefing").map_or(PyValue::None, PyValue::Int),
        ),
        ("Mission Keywords", keywords_of(w, &c)),
        // Interpolated into HTML: the empty string, not `None`.
        ("Mission Image", bstr("")),
        // `-1` is the client's own "unset" for a decline window.
        ("Decline Time", PyValue::Int(-1)),
        ("Expiration Time", PyValue::Int(c.p.expires_at)),
        (
            "AcceptTimestamp",
            if accepted {
                PyValue::Int(c.p.accepted_at)
            } else {
                PyValue::None
            },
        ),
    ])
}

/// `GetMissionJournalInfo` (`agents.py:821-830`): the details window's
/// dict for the bound agent's stored mission, `None` when it has none (the
/// window then closes, which is right). The arguments are ignored: the bound
/// agent's current mission is the one. `objectives` is exactly the
/// `GetMissionObjectiveInfo` dict (`agentDialogueUtil:770`,
/// `activeMissionController:85`); `iconID` is left out (`:744` is guarded).
pub fn journal_info_at(w: &Registry, actor: Option<i64>, agent: i64) -> PyValue {
    let now = crate::pyutil::filetime_now();
    let Some(c) = current(w, actor.unwrap_or(0), agent, now) else {
        return PyValue::None;
    };
    // A draw not yet stored is not in the journal, so it has no details.
    if c.fresh {
        return PyValue::None;
    }
    let accepted = c.p.state == State::Accepted;
    pydict(vec![
        ("missionNameID", PyValue::Int(c.m.name_id)),
        ("contentID", PyValue::Int(c.m.id)),
        (
            "briefingTextID",
            slot(&c.m, "briefing").map_or_else(|| offer_line(&c.m, 0), PyValue::Int),
        ),
        ("missionImage", bstr("")),
        ("expirationTime", PyValue::Int(c.p.expires_at)),
        (
            "missionState",
            PyValue::Int(if accepted {
                mission_state::ACCEPTED
            } else {
                mission_state::OFFERED
            }),
        ),
        ("objectives", objective_info_at(w, actor, agent)),
    ])
}

/// One `{solarsystemID, typeID, locationID}` station leg.
fn leg(system: i64, station: i64) -> PyValue {
    pydict(vec![
        ("solarsystemID", PyValue::Int(system)),
        ("typeID", PyValue::Int(STATION_TYPE_ID)),
        ("locationID", PyValue::Int(station)),
    ])
}

/// `GetMissionObjectiveInfo`: twelve mandatory keys, all subscripted, and
/// `contentID` beside `missionExtra`. A courier is a `'transport'` leg (arity
/// 5); a kill mission an `'agent'` objective (arity 2) plus one `dungeons`
/// entry with `optional`, `objectiveCompleted`, `location` and `dungeonID`
/// **[V]** (`agentDialogueUtil.py:34-35`, `:190-248`).
pub fn objective_info_at(w: &Registry, actor: Option<i64>, agent: i64) -> PyValue {
    let now = crate::pyutil::filetime_now();
    let ch = actor.unwrap_or(0);
    let Some(c) = current(w, ch, agent, now) else {
        return PyValue::Dict(vec![]);
    };
    let accepted = c.p.state == State::Accepted;
    let done = accepted && objective_done(w, ch, &c);
    let reward = crate::mission_rewards::reward(w, &c.agent, &c.m);
    let home_station = c
        .agent
        .station
        .or_else(|| {
            actor
                .and_then(|id| w.character(id))
                .and_then(|x| x.station_id)
        })
        .unwrap_or(0);
    let home_system = c.agent.system.unwrap_or(AGENT_SOLAR_SYSTEM_ID);
    let (objectives, dungeons) = if !accepted {
        (vec![], vec![])
    } else if is_kill(&c.m) {
        (
            vec![PyValue::Tuple(vec![
                bstr("agent"),
                PyValue::Tuple(vec![PyValue::Int(agent), leg(home_system, home_station)]),
            ])],
            vec![pydict(vec![
                ("optional", PyValue::Bool(false)),
                (
                    "objectiveCompleted",
                    if done { PyValue::Int(1) } else { PyValue::None },
                ),
                ("location", dungeon::location_wrap(site(&c))),
                ("dungeonID", PyValue::Int(c.m.dungeon.unwrap_or_default())),
            ])],
        )
    } else {
        let d = destination(w, &c);
        let (station, system, owner) = d.map_or((0, 0, 0), |d| (d.station, d.system, d.owner));
        (
            vec![PyValue::Tuple(vec![
                bstr("transport"),
                PyValue::Tuple(vec![
                    PyValue::Int(c.agent.corp),
                    leg(home_system, home_station),
                    PyValue::Int(owner),
                    leg(system, station),
                    pydict(vec![
                        ("hasCargo", PyValue::Int(i64::from(c.m.obj_type.is_some()))),
                        ("quantity", PyValue::Int(c.m.obj_qty.unwrap_or(0))),
                        ("typeID", PyValue::Int(c.m.obj_type.unwrap_or(0))),
                        // Per unit or total is unknown; the total is the
                        // more useful to a player.
                        ("volume", PyValue::Real(c.m.obj_volume_total.unwrap_or(0.0))),
                    ]),
                ]),
            ])],
            vec![],
        )
    };
    let mut entries = vec![
        // `> 0` is finished (`agentDialogueUtil.py:409`) **[V]**: a
        // cleared pocket or a delivered cargo.
        ("completionStatus", PyValue::Int(i64::from(done))),
        ("missionTitleID", PyValue::Int(c.m.name_id)),
        (
            "missionState",
            PyValue::Int(if accepted {
                mission_state::ACCEPTED
            } else {
                mission_state::ALLOCATED
            }),
        ),
        ("objectives", PyValue::List(objectives)),
        ("dungeons", PyValue::List(dungeons)),
        ("locations", PyValue::List(vec![PyValue::Int(home_system)])),
        ("agentGift", PyValue::List(vec![])),
        ("normalRewards", reward.normal_rewards()),
        (
            "bonusRewards",
            reward.bonus_rewards(accepted.then_some(c.p.accepted_at), now),
        ),
        ("collateral", PyValue::List(vec![])),
        ("loyaltyPoints", PyValue::Int(reward.lp)),
        ("researchPoints", PyValue::Int(0)),
    ];
    if let (Some(h), Some(b)) = (slot(&c.m, "extrainfo.header"), slot(&c.m, "extrainfo.body")) {
        entries.push((
            "missionExtra",
            PyValue::Tuple(vec![PyValue::Int(h), PyValue::Int(b)]),
        ));
    }
    entries.push(("contentID", PyValue::Int(c.m.id)));
    pydict(entries)
}

/// The `'dungeon'` bookmark of an accepted kill mission: a `KeyVal`, because
/// the client writes to it (`journal.py:417`, `bookmarkSvc.py:130`) **[V]**.
fn dungeon_bookmark(agent: i64, pocket: &Pocket) -> PyValue {
    keyval(vec![
        ("agentID", PyValue::Int(agent)),
        ("locationID", PyValue::Int(pocket.solar_system_id)),
        ("locationNumber", PyValue::Int(0)),
        ("locationType", bstr("dungeon")),
        ("itemID", PyValue::Int(pocket.solar_system_id)),
        ("typeID", PyValue::Int(dungeon::TYPE_SOLAR_SYSTEM)),
        ("x", PyValue::Real(pocket.position[0])),
        ("y", PyValue::Real(pocket.position[1])),
        ("z", PyValue::Real(pocket.position[2])),
        ("hint", bstr("UI/Agents/Bookmarks/Encounter/Encounter")),
        ("folderID", PyValue::None),
        ("subfolderID", PyValue::None),
        ("creatorID", PyValue::None),
    ])
}

/// The journal's Type column label, by the row's `family`.
fn type_label(m: &Mission) -> &'static str {
    match m.family.as_str() {
        "kill" => "UI/Agents/MissionTypes/Encounter",
        "mining" => "UI/Agents/MissionTypes/Mining",
        _ => "UI/Agents/MissionTypes/Courier",
    }
}

/// `agentMgr.GetMyJournalDetails`: `[missions, offers]` (`journal.py:413-451`)
/// **[V]**. One 10-tuple per agent with a stored mission, every agent:
/// `(missionState, importantMission, missionTypeLabel, missionNameID, agentID,
/// expirationTime, bookmarks, remoteOfferable, remoteCompletable,
/// contentID)`. Offers are rows in `[0]` with state 1 (C4 §3.9.3) `[A]`.
///
/// `only` is the bound agent: a bound call (`journal.py:431`) is merged into
/// the unbound one's rows, which removes the bound agent's own row and
/// extends `[0]` and `[1]` with the answer (`:434-447`), so a bound answer
/// holds just that agent's rows and an empty `[1]` `[A]` (falsifier: research
/// rows doubling once a bound `[1]` carries them). `None` is every agent.
pub fn journal(w: &Registry, actor: Option<i64>, only: Option<i64>) -> PyValue {
    let mut missions = Vec::new();
    let me = actor.map(|id| id.to_string());
    for (k, v) in w.deltas().of(KIND) {
        let ([ch, agent], Some(p)) = (k.as_slice(), Progress::from_value(v)) else {
            continue;
        };
        let (Some(id), Ok(agent)) = (p.mission_id, agent.parse::<i64>()) else {
            continue;
        };
        if Some(ch) != me.as_ref() || only.is_some_and(|o| o != agent) {
            continue;
        }
        let Some(m) = mission_row(w, id) else {
            continue;
        };
        let accepted = p.state == State::Accepted;
        let bookmarks: Vec<PyValue> = actor
            .and_then(|c| w.ext::<Live>().pockets.get(&(c, agent)))
            .filter(|_| accepted)
            .map(|pk| dungeon_bookmark(agent, pk))
            .into_iter()
            .collect();
        missions.push(PyValue::Tuple(vec![
            PyValue::Int(if accepted {
                mission_state::ACCEPTED
            } else {
                mission_state::OFFERED
            }),
            PyValue::Bool(false),
            bstr(type_label(&m)),
            PyValue::Int(m.name_id),
            PyValue::Int(agent),
            PyValue::Int(p.expires_at),
            PyValue::List(bookmarks),
            PyValue::Bool(false),
            PyValue::Bool(false),
            PyValue::Int(m.id),
        ]));
    }
    PyValue::List(vec![PyValue::List(missions), PyValue::List(vec![])])
}

/// Where `WarpToLocation` sends `actor`: the accepted mission's pocket with
/// `agent`, when in space in its system. `None` otherwise.
pub fn warp_target(w: &Registry, actor: i64, agent: i64) -> Option<crate::model::Target> {
    if progress(w, actor, agent).state != State::Accepted {
        return None;
    }
    let pocket = w.ext::<Live>().pockets.get(&(actor, agent))?;
    let here = w.roster().get(actor)?;
    (here.solar_system_id == pocket.solar_system_id).then(|| pocket.target())
}

/// Where a **fleet warp** sends `actor` (FLEET-MISSION): the pocket with
/// `agent` of a fleet-mate who has accepted that mission, when `actor` is in
/// that pocket's system. A member of the mate's fleet may enter a pocket the
/// mate owns; a pilot outside the fleet gets `None` **[A]** (falsified by TQ
/// refusing the member's warp, or letting a stranger in). With several
/// owners the boss's pocket wins, else the lowest character id.
pub fn fleet_warp_target(w: &Registry, actor: i64, agent: i64) -> Option<crate::model::Target> {
    let fleet = w.fleets().fleet_of(actor)?;
    let here = w.roster().get(actor)?.solar_system_id;
    let mut owners: Vec<(bool, i64)> = fleet
        .members
        .values()
        .filter(|m| m.char_id != actor)
        .map(|m| (!m.is_boss(), m.char_id))
        .collect();
    owners.sort();
    owners.into_iter().find_map(|(_, owner)| {
        if progress(w, owner, agent).state != State::Accepted {
            return None;
        }
        let pocket = w.ext::<Live>().pockets.get(&(owner, agent))?;
        (pocket.solar_system_id == here).then(|| pocket.target())
    })
}

/// `WarpToLocation`'s `warpRange` (`movementFunctions.py:458`) **[V]**: an
/// int, a float or `None`; anything negative or non-finite is `0.0`.
pub fn warp_range(arg: Option<&PyValue>) -> f64 {
    let r = match arg {
        Some(PyValue::Int(n)) => *n as f64,
        Some(PyValue::Real(x)) => *x,
        _ => 0.0,
    };
    if r.is_finite() && r >= 0.0 { r } else { 0.0 }
}

/// `OnAgentMissionChange(change, agentID)`: **the first argument is a
/// string**, and this push is the only journal-cache invalidation short of a
/// session reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentMissionChange {
    pub change: &'static str,
    pub agent_id: i64,
}

impl crate::notify::Push for AgentMissionChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAgentMissionChange",
            vec![bstr(self.change), PyValue::Int(self.agent_id)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;

    const ME: i64 = 90_000_001;
    /// A security pool agent, under the id the unbound arms speak to.
    const KILLER: i64 = AGENT_ID;
    const RUNNER: i64 = 3_999_102;
    const LEVEL_TWO: i64 = 3_999_103;
    const ENEMY: i64 = 599_011;
    const KILL: i64 = 999_201;
    const DROP: i64 = 999_202;
    const SYSTEM: i64 = 39_000_001;
    /// Storyline rows (FX5): never pooled, offered through [`set_offer`].
    const STORY_KILL: i64 = 999_211;
    const STORY_DROP: i64 = 999_212;

    /// Invented rows only: three agents, one kill mission with every slot,
    /// two couriers with only a briefing.
    fn table() -> pool::Table {
        let agent = |id: i64, division, level| pool::Agent {
            id,
            agent_type: pool::TYPE_BASIC,
            division,
            level,
            corp: 1_999_001,
            faction: 599_001,
            station: Some(69_000_001 + id % 10),
            system: Some(SYSTEM),
            ..pool::Agent::default()
        };
        let slots = |names: &[&str], base: i64| -> Vec<(String, i64)> {
            (names.iter().zip(base..))
                .map(|(n, id)| (format!("messages.mission.{n}"), id))
                .collect()
        };
        let all = [
            "briefing",
            "offered.agentsays",
            "accepted.agentsays",
            "declined.agentsays",
            "completed.agentsays",
            "quitted.agentsays",
        ];
        let courier = |id, level| Mission {
            id,
            name_id: id + 700,
            template: "BasicCourier".into(),
            family: "courier".into(),
            level: Some(level),
            obj_type: Some(CARGO_TYPE_ID),
            obj_qty: Some(2),
            obj_volume_total: Some(1.0),
            messages: slots(&["briefing"], 999_850 + id % 10),
            ..Mission::default()
        };
        let kill = Mission {
            id: KILL,
            name_id: 999_901,
            template: "BasicKill".into(),
            family: "kill".into(),
            level: Some(1),
            enemy_faction: Some(ENEMY),
            dungeon: Some(999_101),
            messages: slots(&all, 999_800),
            ..Mission::default()
        };
        let story_kill = Mission {
            id: STORY_KILL,
            name_id: 999_903,
            template: "GenericStorylineKill".into(),
            ..kill.clone()
        };
        let story_drop = Mission {
            template: "GenericStorylineCourier".into(),
            ..courier(STORY_DROP, 1)
        };
        pool::Table::new(
            vec![
                agent(KILLER, 24, 1),
                agent(RUNNER, 22, 1),
                agent(LEVEL_TWO, 22, 2),
            ],
            vec![
                kill,
                courier(DROP, 1),
                courier(DROP + 1, 2),
                story_kill,
                story_drop,
            ],
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        )
    }

    /// The fixture's NPC rows, with the enemy's only type one whose hull the
    /// hand table resolves (no typedata is installed in a test).
    fn npc_rows() -> crate::npc_data::Table {
        let mut d = crate::npc_data::Table::fixture();
        let mut row = d.types[&990_300_001].clone();
        d.types.retain(|_, t| t.faction_id != ENEMY);
        row.type_id = crate::npc::ANGEL_HIJACKER.type_id;
        d.types.insert(row.type_id, row);
        d
    }

    fn world() -> Registry {
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", table());
        c.insert("npc_data", npc_rows());
        Registry::new(crate::model::SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    /// `(agentSays msgInfo, action count, lastActionInfo)`.
    fn unwrap_reply(v: &PyValue) -> (PyValue, usize, PyValue) {
        let PyValue::Tuple(outer) = v else {
            panic!("DoAction is a 2-tuple: {v:?}")
        };
        let PyValue::Tuple(pair) = &outer[0] else {
            panic!("(agentSays, actions)")
        };
        let PyValue::Tuple(says) = &pair[0] else {
            panic!("agentSaysMsg is (msgInfo, contentID)")
        };
        assert_eq!(says.len(), 2, "`_, mission_id = agent_says` needs two");
        let PyValue::List(actions) = &pair[1] else {
            panic!("availableActions is a list")
        };
        (says[0].clone(), actions.len(), outer[1].clone())
    }

    fn act(w: &mut Registry, agent: i64, action: Option<i64>) -> (PyValue, Option<Change>) {
        converse_at(w, Some(ME), agent, action)
    }

    fn balance(w: &Registry) -> f64 {
        w.character(ME).map(|c| c.balance).unwrap()
    }

    fn pocket(w: &Registry, agent: i64) -> Option<Pocket> {
        w.ext::<Live>().pockets.get(&(ME, agent)).cloned()
    }

    fn cargo(w: &Registry) -> i64 {
        w.store()
            .items()
            .iter()
            .filter(|i| i.type_id == CARGO_TYPE_ID && i.owner_id == ME)
            .map(|i| i.quantity)
            .sum()
    }

    /// Offer, accept, complete: the generic offer around the briefing, the
    /// cargo, Complete once met, the payout, and the history.
    #[test]
    fn the_conversation_walks_from_offer_to_completion() {
        let mut w = world();
        let before = balance(&w);
        let (a, change) = act(&mut w, RUNNER, None);
        assert_eq!((unwrap_reply(&a).1, change), (2, None), "Accept, Decline");
        assert_eq!(progress(&w, ME, RUNNER).mission_id, Some(DROP), "stored");

        let (_, change) = act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        assert_eq!(change, Some(Change::Accepted));
        assert_eq!(cargo(&w), 2, "the row's obj_type x obj_qty");
        deliver(&mut w, RUNNER, 2);
        let (a, _) = act(&mut w, RUNNER, None);
        assert_eq!(unwrap_reply(&a).1, 2, "Complete and Quit");

        let (a, change) = act(&mut w, RUNNER, Some(ACTION_COMPLETE));
        assert_eq!(change, Some(Change::Completed));
        let info = unwrap_reply(&a).2;
        assert_eq!(dict_get(&info, b"missionCompleted"), Some(&PyValue::Int(1)));
        let m = table().mission(DROP).cloned().unwrap();
        let r = crate::mission_rewards::reward(&w, table().agent(RUNNER).unwrap(), &m);
        assert_eq!(
            balance(&w),
            before + r.isk(true) as f64,
            "inside the bonus window"
        );
        assert_eq!(w.npc_standing(ME, RUNNER), r.agent_gain);
        assert_eq!(w.npc_standing(ME, 1_999_001), r.corp_gain);
        let p = progress(&w, ME, RUNNER);
        assert_eq!((p.mission_id, p.history), (None, vec![DROP]));
    }

    /// The courier's drop-off station, from the objective's own leg.
    fn dropoff(w: &Registry, agent: i64) -> i64 {
        let c = current(w, ME, agent, crate::pyutil::filetime_now()).expect("a mission");
        destination(w, &c).expect("a destination").station
    }

    /// Move `n` units of my cargo into the drop-off's hangar, as an unload
    /// would.
    fn deliver(w: &mut Registry, agent: i64, mut n: i64) {
        use crate::item::{Edit, Slot};
        let to = Slot::new(ME, dropoff(w, agent), crate::model::FLAG_HANGAR);
        let stacks: Vec<(i64, i64)> = w
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == CARGO_TYPE_ID && i.owner_id == ME && i.slot() != to)
            .map(|i| (i.item_id, i.quantity))
            .collect();
        for (item_id, quantity) in stacks {
            let take = quantity.min(n);
            if take == 0 {
                break;
            }
            let edit = if take == quantity {
                Edit::Move { item_id, to }
            } else {
                Edit::Split {
                    item_id,
                    quantity: take,
                    to,
                }
            };
            w.apply_items(&[edit]).expect("the move applies");
            n -= take;
        }
        assert_eq!(n, 0, "there was enough cargo to move");
    }

    fn completion_status(w: &Registry, agent: i64) -> Option<PyValue> {
        dict_get(&objective_info_at(w, Some(ME), agent), b"completionStatus").cloned()
    }

    /// §72.2: Complete is neither drawn nor honoured while the cargo sits in
    /// the pickup hangar, and is both once it is in the drop-off's hangar.
    #[test]
    fn a_courier_completes_only_after_its_cargo_is_delivered() {
        let mut w = world();
        act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        assert_ne!(
            dropoff(&w, RUNNER),
            table().agent(RUNNER).unwrap().station.unwrap(),
            "a real haul: pickup and drop-off differ"
        );
        let (a, _) = act(&mut w, RUNNER, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only, before delivery");
        assert_eq!(act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(progress(&w, ME, RUNNER).state, State::Accepted);
        assert_eq!(completion_status(&w, RUNNER), Some(PyValue::Int(0)));

        deliver(&mut w, RUNNER, 2);
        assert_eq!(completion_status(&w, RUNNER), Some(PyValue::Int(1)));
        let (a, _) = act(&mut w, RUNNER, None);
        assert_eq!(unwrap_reply(&a).1, 2, "Complete and Quit, once delivered");
        assert_eq!(
            act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
    }

    /// Part of the cargo at the drop-off is not a delivery.
    #[test]
    fn a_partial_delivery_cannot_be_turned_in() {
        let mut w = world();
        act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        deliver(&mut w, RUNNER, 1);
        let (a, _) = act(&mut w, RUNNER, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only");
        assert_eq!(act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(completion_status(&w, RUNNER), Some(PyValue::Int(0)));

        deliver(&mut w, RUNNER, 1);
        assert_eq!(
            act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed),
            "the rest arrives"
        );
    }

    /// No Complete button, and no turn-in, while the pocket has NPCs.
    #[test]
    fn an_unmet_objective_cannot_be_turned_in() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let (a, _) = act(&mut w, KILLER, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only");
        let (_, change) = act(&mut w, KILLER, Some(ACTION_COMPLETE));
        assert_eq!(change, None);
        assert_eq!(progress(&w, ME, KILLER).state, State::Accepted);
    }

    /// Put a non-pool mission on offer at `agent`, as CP48 and CP41 do.
    fn offer(w: &mut Registry, agent: i64, mission: i64) {
        let now = crate::pyutil::filetime_now();
        assert!(set_offer(w, ME, agent, Some(mission), now));
    }

    /// **FX5**: kill is the row's `family`, not the `BasicKill` name. A
    /// storyline kill spawns its pocket on accept and cannot be turned in
    /// until the pocket is cleared.
    #[test]
    fn a_storyline_kill_spawns_its_pocket_and_needs_the_kill() {
        let mut w = world();
        offer(&mut w, KILLER, STORY_KILL);
        assert_eq!(
            act(&mut w, KILLER, Some(ACTION_ACCEPT)).1,
            Some(Change::Accepted)
        );
        let pocket = pocket(&w, KILLER).expect("accepting spawns the pocket");
        let (a, _) = act(&mut w, KILLER, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only");
        assert_eq!(act(&mut w, KILLER, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(progress(&w, ME, KILLER).state, State::Accepted);
        for id in &pocket.npc_ids {
            w.roster_mut().leave(*id);
        }
        assert_eq!(
            act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
    }

    /// **FX5**: a storyline courier puts its cargo and completes only once
    /// it is delivered.
    #[test]
    fn a_storyline_courier_puts_its_cargo_and_needs_the_delivery() {
        let mut w = world();
        offer(&mut w, RUNNER, STORY_DROP);
        assert_eq!(
            act(&mut w, RUNNER, Some(ACTION_ACCEPT)).1,
            Some(Change::Accepted)
        );
        assert_eq!(cargo(&w), 2, "the row's obj_type x obj_qty");
        assert_eq!(act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1, None);
        deliver(&mut w, RUNNER, 2);
        assert_eq!(
            act(&mut w, RUNNER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
    }

    /// Declining remembers the mission and draws the next offer with the
    /// next `offer_seq`.
    #[test]
    fn declining_draws_the_next_offer() {
        let mut w = world();
        let (a, change) = act(&mut w, RUNNER, Some(ACTION_DECLINE));
        assert_eq!(change, Some(Change::Declined));
        let info = unwrap_reply(&a).2;
        assert_eq!(dict_get(&info, b"missionDeclined"), Some(&PyValue::Int(1)));
        let p = progress(&w, ME, RUNNER);
        assert_eq!(
            (p.mission_id, p.offer_seq, p.history),
            (None, 1, vec![DROP])
        );
        act(&mut w, RUNNER, None);
        assert_eq!(progress(&w, ME, RUNNER).offer_seq, 2);
    }

    /// The mission's own line where it has the slot, the generic label where
    /// it has not.
    #[test]
    fn quitting_says_the_mission_line_or_the_generic_one() {
        let t = table();
        let kill = t.mission(KILL).unwrap();
        let turn = do_action(kill, State::Accepted, false, Some(ACTION_QUIT), "Low", 0);
        assert_eq!(unwrap_reply(&turn.answer).0, PyValue::Int(999_805));
        assert_eq!(turn.change, Some(Change::Quit));
        let drop = t.mission(DROP).unwrap();
        let turn = do_action(drop, State::Accepted, true, Some(ACTION_QUIT), "Low", 0);
        let (says, _, info) = unwrap_reply(&turn.answer);
        let PyValue::Tuple(label) = says else {
            panic!("a label tuple")
        };
        let PyValue::Bytes(name) = &label[0] else {
            panic!("a label path")
        };
        assert!(name.starts_with(b"UI/Agents/DefaultMessages/MissionQuitted"));
        assert_eq!(dict_get(&info, b"missionQuit"), Some(&PyValue::Int(1)));
    }

    /// **All five `lastActionInfo` keys ship every time**: subscripted
    /// without `.get`.
    #[test]
    fn last_action_info_always_carries_all_five_keys() {
        let t = table();
        for m in [t.mission(KILL).unwrap(), t.mission(DROP).unwrap()] {
            for (state, done, action) in [
                (State::Offered, false, None),
                (State::Offered, false, Some(ACTION_ACCEPT)),
                (State::Offered, false, Some(ACTION_DECLINE)),
                (State::Accepted, true, None),
                (State::Accepted, false, Some(ACTION_COMPLETE)),
                (State::Accepted, true, Some(ACTION_COMPLETE)),
                (State::Accepted, true, Some(ACTION_QUIT)),
                (State::Accepted, true, Some(ACTION_ACCEPT)),
            ] {
                let info = unwrap_reply(&do_action(m, state, done, action, "High", 3).answer).2;
                for key in [
                    "missionCompleted",
                    "missionDeclined",
                    "missionQuit",
                    "missionCantReplay",
                    "loyaltyPoints",
                ] {
                    assert!(dict_get(&info, key.as_bytes()).is_some(), "{key}");
                }
            }
        }
    }

    /// **CP26a**: a mission with no `offered` slot is offered as
    /// `('UI/Agents/DefaultMessages/MissionOfferN', {'missionBriefingText':
    /// briefing})` (C4 §2.6) [V]; one with the slot says its own id.
    #[test]
    fn a_missing_offered_slot_produces_the_mission_offer_label_tuple() {
        let t = table();
        let drop = t.mission(DROP).unwrap();
        let says = unwrap_reply(&do_action(drop, State::Offered, false, None, "Low", 4).answer).0;
        let PyValue::Tuple(label) = &says else {
            panic!("(label, args): {says:?}")
        };
        assert_eq!(label.len(), 2);
        let PyValue::Bytes(name) = &label[0] else {
            panic!("a label path")
        };
        let n = name
            .strip_prefix(b"UI/Agents/DefaultMessages/MissionOffer")
            .expect("MissionOfferN");
        assert!(matches!(n, [b'1'..=b'6']), "MissionOffer1-6");
        assert_eq!(
            dict_get(&label[1], b"missionBriefingText"),
            slot(drop, "briefing").map(PyValue::Int).as_ref()
        );
        let kill = t.mission(KILL).unwrap();
        let says = unwrap_reply(&do_action(kill, State::Offered, false, None, "Low", 4).answer).0;
        assert_eq!(
            Some(says),
            slot(kill, "offered.agentsays").map(PyValue::Int)
        );
    }

    /// **CP26a**: `CanUseAgent` (C4 §2.7) at level 2 refuses below 1.0 and
    /// accepts at it [V], and a refusal stores nothing.
    #[test]
    fn the_standing_gate_refuses_below_and_accepts_at_the_threshold() {
        let mut w = world();
        // `m = (level - 1) * 2 - 1` at level 2 (`eveCfg.py:164`) [V].
        let threshold = 1.0;
        w.set_npc_standing(ME, LEVEL_TWO, threshold - 0.1);
        let (a, _) = act(&mut w, LEVEL_TWO, None);
        assert_eq!(
            unwrap_reply(&a),
            (
                PyValue::Int(MSG_TOO_LOW_STANDINGS),
                0,
                last_action_info(false, false, false)
            )
        );
        assert!(w.deltas().of(KIND).is_empty(), "a refusal stores nothing");
        w.set_npc_standing(ME, LEVEL_TWO, threshold);
        let (a, _) = act(&mut w, LEVEL_TWO, None);
        assert_eq!(unwrap_reply(&a).1, 2, "offered");
    }

    /// **CP26a**: two agents' missions coexist, each in its own journal row.
    #[test]
    fn two_agents_missions_coexist() {
        let mut w = world();
        act(&mut w, RUNNER, None);
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        assert_eq!(progress(&w, ME, RUNNER).state, State::Offered);
        assert_eq!(progress(&w, ME, KILLER).state, State::Accepted);
        let PyValue::List(parts) = journal(&w, Some(ME), None) else {
            panic!("[missions, offers]")
        };
        let PyValue::List(rows) = &parts[0] else {
            panic!("a list")
        };
        let mut seen: Vec<(PyValue, PyValue)> = rows
            .iter()
            .map(|r| match r {
                PyValue::Tuple(r) if r.len() == 10 => (r[4].clone(), r[0].clone()),
                other => panic!("a 10-tuple: {other:?}"),
            })
            .collect();
        seen.sort_by_key(|(a, _)| format!("{a:?}"));
        assert_eq!(
            seen,
            vec![
                (PyValue::Int(KILLER), PyValue::Int(mission_state::ACCEPTED)),
                (PyValue::Int(RUNNER), PyValue::Int(mission_state::OFFERED)),
            ]
        );
        assert!(
            journal(&w, Some(ME + 1), None) != journal(&w, Some(ME), None),
            "per character"
        );
    }

    /// **CP26a**: the `mission` row round-trips, a restart (a new world on
    /// the same rows) sees the same mission, and an unreadable row is no row.
    #[test]
    fn the_mission_delta_round_trips() {
        let p = Progress {
            mission_id: Some(DROP),
            state: State::Accepted,
            offer_seq: 7,
            accepted_at: 11,
            expires_at: 13,
            history: vec![KILL, DROP],
        };
        assert_eq!(Progress::from_value(&p.to_value()), Some(p));
        assert_eq!(
            Progress::from_value(&Progress::default().to_value()),
            Some(Progress::default())
        );
        assert_eq!(Progress::from_value(&["x".to_owned()]), None);

        let mut w = world();
        act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        let mut again = world();
        for (k, v) in w.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        assert_eq!(progress(&again, ME, RUNNER), progress(&w, ME, RUNNER));
        assert_eq!(journal(&again, Some(ME), None), journal(&w, Some(ME), None));
    }

    /// **CP26a**: the pocket's NPCs are `npc_data` types of the mission's
    /// enemy faction, best first (no hand rat named in this module).
    #[test]
    fn pocket_rats_come_from_npc_data_by_the_enemy_faction() {
        let fixture = crate::npc_data::Table::fixture();
        let order = pocket_types(&fixture, ENEMY);
        assert_eq!(order.first(), Some(&990_300_001), "the base frigate first");
        assert!(pocket_types(&fixture, 599_021).is_empty());

        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let pocket = pocket(&w, KILLER).expect("spawned");
        assert_eq!(pocket.npc_ids.len(), POCKET_NPCS);
        assert_eq!(pocket.solar_system_id, SYSTEM, "the agent's system");
        let allowed = pocket_types(w.content().get::<crate::npc_data::Table>(), ENEMY);
        for id in &pocket.npc_ids {
            let t = w.npcs().get(*id).expect("an NPC").kind.type_id;
            assert!(allowed.contains(&t), "{t} is not an enemy type");
        }
    }

    /// **CP26a, CA9**: with no `npc_data`, no kill template is offered: a
    /// security agent has nothing, a courier agent still offers.
    #[test]
    fn with_no_npc_data_no_kill_template_is_offered() {
        let mut c = crate::content::Content::default();
        c.insert("mission_pool", table());
        c.insert("universe", crate::celestial::Universe::fixture());
        let mut w = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(c));
        let (a, change) = act(&mut w, KILLER, Some(ACTION_ACCEPT));
        assert_eq!(
            (unwrap_reply(&a).0, change),
            (PyValue::Int(MSG_NO_MISSION), None)
        );
        assert!(w.deltas().of(KIND).is_empty());
        act(&mut w, RUNNER, None);
        assert_eq!(progress(&w, ME, RUNNER).mission_id, Some(DROP));
    }

    /// The rollover drops an offer past its expiry, keeps its seq and
    /// history, and keeps one still standing.
    #[test]
    fn the_rollover_drops_an_expired_offer() {
        let mut w = world();
        act(&mut w, RUNNER, None);
        let birth = w.world().birth;
        let now = crate::pyutil::filetime_now();
        on_rollover(&mut w, crate::seed::day_of(birth, now));
        assert_eq!(
            progress(&w, ME, RUNNER).mission_id,
            Some(DROP),
            "still open"
        );
        let later = now + OFFER_LIFETIME + 2 * crate::seed::DAY;
        on_rollover(&mut w, crate::seed::day_of(birth, later));
        let p = progress(&w, ME, RUNNER);
        assert_eq!((p.mission_id, p.offer_seq), (None, 1));
    }

    /// Lane clock: at scale N an offer stands `OFFER_LIFETIME / N`.
    #[test]
    fn at_scale_n_an_offer_stands_n_times_shorter() {
        let mut w = world();
        let before = crate::pyutil::filetime_now();
        act(&mut w, RUNNER, None);
        let real = progress(&w, ME, RUNNER).expires_at - before;
        let mut w = world();
        crate::game_clock::set(&mut w, crate::game_clock::GameClock::new(7).unwrap());
        let before = crate::pyutil::filetime_now();
        act(&mut w, RUNNER, None);
        let after = crate::pyutil::filetime_now();
        let fast = progress(&w, ME, RUNNER).expires_at;
        assert!(fast >= before + OFFER_LIFETIME / 7 && fast <= after + OFFER_LIFETIME / 7);
        assert!(real >= OFFER_LIFETIME);
    }

    /// The briefing's mandatory pair and the keywords its prose needs: a
    /// destination that is not the agent's own station, or the kill site.
    #[test]
    fn the_briefing_carries_its_two_mandatory_keys_and_the_keywords() {
        let w = world();
        let b = briefing_info_at(&w, Some(ME), RUNNER);
        assert_eq!(dict_get(&b, b"ContentID"), Some(&PyValue::Int(DROP)));
        assert_eq!(dict_get(&b, b"Mission Image"), Some(&bstr("")));
        let kw = dict_get(&b, b"Mission Keywords").expect("mandatory");
        let own = table().agent(RUNNER).and_then(|a| a.station).unwrap();
        let dest = dict_get(kw, b"objectiveDestinationID").expect("the courier's key");
        assert_ne!(dest, &PyValue::Int(own));
        let kw = keywords_at(&w, Some(ME), KILLER);
        assert_eq!(
            dict_get(&kw, b"dungeonLocationID"),
            Some(&PyValue::Int(SYSTEM))
        );
        // The unbound arm answers both kinds' keys.
        for key in ["objectiveDestinationID", "dungeonLocationID"] {
            assert!(dict_get(&keywords(None), key.as_bytes()).is_some());
        }
    }

    const MANDATORY: [&str; 12] = [
        "completionStatus",
        "missionTitleID",
        "missionState",
        "objectives",
        "dungeons",
        "locations",
        "agentGift",
        "normalRewards",
        "bonusRewards",
        "collateral",
        "loyaltyPoints",
        "researchPoints",
    ];

    fn first_objective(o: &PyValue) -> Vec<PyValue> {
        let Some(PyValue::List(objectives)) = dict_get(o, b"objectives") else {
            panic!("objectives is a list")
        };
        let PyValue::Tuple(first) = &objectives[0] else {
            panic!("an objective is a tuple")
        };
        first.clone()
    }

    /// The courier objective's keys, and the `'transport'` leg's arity 5.
    #[test]
    fn the_courier_objective_carries_every_subscripted_key() {
        let mut w = world();
        let o = objective_info_at(&w, Some(ME), RUNNER);
        assert_eq!(dict_get(&o, b"objectives"), Some(&PyValue::List(vec![])));
        act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        let o = objective_info_at(&w, Some(ME), RUNNER);
        for key in MANDATORY.iter().chain(&["contentID"]) {
            assert!(dict_get(&o, key.as_bytes()).is_some(), "{key}");
        }
        let first = first_objective(&o);
        assert_eq!(first[0], bstr("transport"));
        let PyValue::Tuple(detail) = &first[1] else {
            panic!("a tuple")
        };
        assert_eq!(detail.len(), 5, "transport has arity 5");
    }

    /// The kill objective: an `'agent'` entry of arity 2 and one dungeon with
    /// its four keys, in progress until the pocket is clear.
    #[test]
    fn the_kill_objective_carries_an_agent_entry_and_a_dungeon() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let o = objective_info_at(&w, Some(ME), KILLER);
        for key in MANDATORY.iter().chain(&["contentID"]) {
            assert!(dict_get(&o, key.as_bytes()).is_some(), "{key}");
        }
        let first = first_objective(&o);
        assert_eq!(first[0], bstr("agent"));
        let Some(PyValue::List(dungeons)) = dict_get(&o, b"dungeons") else {
            panic!("a list")
        };
        for key in ["optional", "objectiveCompleted", "location", "dungeonID"] {
            assert!(dict_get(&dungeons[0], key.as_bytes()).is_some(), "{key}");
        }
        let dungeon = table().mission(KILL).and_then(|m| m.dungeon).unwrap();
        assert_eq!(
            dict_get(&dungeons[0], b"dungeonID"),
            Some(&PyValue::Int(dungeon))
        );
        assert_eq!(
            dict_get(&dungeons[0], b"objectiveCompleted"),
            Some(&PyValue::None)
        );
        for id in pocket(&w, KILLER).unwrap().npc_ids {
            w.roster_mut().leave(id);
        }
        let o = objective_info_at(&w, Some(ME), KILLER);
        let Some(PyValue::List(dungeons)) = dict_get(&o, b"dungeons") else {
            panic!("a list")
        };
        assert_eq!(
            dict_get(&dungeons[0], b"objectiveCompleted"),
            Some(&PyValue::Int(1))
        );
    }

    /// The bookmark carries what the journal, bookmark service and tracker
    /// read, and the `'dungeon'` location type.
    #[test]
    fn the_dungeon_bookmark_carries_what_its_readers_read() {
        let b = dungeon_bookmark(
            KILLER,
            &Pocket {
                solar_system_id: SYSTEM,
                position: [1.0, 2.0, 3.0],
                npc_ids: vec![],
            },
        );
        let PyValue::Instance { class, state } = &b else {
            panic!("a KeyVal: {b:?}")
        };
        assert_eq!(class.as_slice(), crate::pyutil::KEYVAL_CLASS.as_bytes());
        for key in [
            "agentID",
            "locationID",
            "locationType",
            "itemID",
            "typeID",
            "x",
            "y",
            "z",
            "hint",
        ] {
            assert!(dict_get(state, key.as_bytes()).is_some(), "{key}");
        }
        assert_eq!(dict_get(state, b"agentID"), Some(&PyValue::Int(KILLER)));
    }

    /// A kill mission end to end: accept, find the pocket and its bookmark,
    /// fail to turn in while NPCs live, clear them, be paid.
    #[test]
    fn a_kill_mission_is_offered_fought_and_paid() {
        let mut w = world();
        let before = balance(&w);
        let (_, change) = act(&mut w, KILLER, Some(ACTION_ACCEPT));
        assert_eq!(change, Some(Change::Accepted));
        let pocket = pocket(&w, KILLER).expect("accepting spawns it");
        for id in &pocket.npc_ids {
            let p = w.roster().get(*id).expect("each NPC is a ball in space");
            assert_eq!(p.solar_system_id, SYSTEM);
        }
        let PyValue::List(parts) = journal(&w, Some(ME), None) else {
            panic!("[missions, offers]")
        };
        let PyValue::List(rows) = &parts[0] else {
            panic!("a list")
        };
        let PyValue::Tuple(row) = &rows[0] else {
            panic!("a tuple")
        };
        assert_eq!((row.len(), &row[9]), (10, &PyValue::Int(KILL)));
        assert!(
            matches!(&row[6], PyValue::List(b) if b.len() == 1),
            "the bookmark"
        );
        assert_eq!(warp_target(&w, ME, KILLER), None, "docked");

        assert_eq!(act(&mut w, KILLER, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(balance(&w), before, "nothing paid for an unfought fight");
        for id in &pocket.npc_ids {
            w.roster_mut().leave(*id);
        }
        assert_eq!(
            act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
        let m = table().mission(KILL).cloned().unwrap();
        let r = crate::mission_rewards::reward(&w, table().agent(KILLER).unwrap(), &m);
        assert_eq!(balance(&w), before + r.isk(true) as f64);
        assert!(
            w.ext::<Live>().pockets.is_empty(),
            "a finished pocket is forgotten"
        );
    }

    /// **The last kill in the pocket tells the agent window live**: only the
    /// volley that empties it schedules `OnAgentMissionChange('modified',
    /// agentID)` to the pilot (`appConst.py:1914`) [V], with the real agent.
    #[test]
    fn the_last_kill_in_the_pocket_tells_the_agent_window() {
        use crate::damage::{Damage, Shot};
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let pocket = pocket(&w, KILLER).expect("spawned");
        let _ = w.take_pending();
        let changes = |w: &mut Registry| -> Vec<(Option<Vec<i64>>, Vec<PyValue>)> {
            w.take_due(std::time::Instant::now() + std::time::Duration::from_secs(600))
                .into_iter()
                .filter(|p| p.to_push().0 == "OnAgentMissionChange")
                .map(|p| (p.audience(), p.to_push().1))
                .collect()
        };
        let _ = changes(&mut w);
        let t0 = crate::pyutil::filetime_now();
        let last = pocket.npc_ids.len() - 1;
        for (i, id) in pocket.npc_ids.iter().enumerate() {
            let victim = w.roster().get(*id).copied().expect("in space");
            let shot = Shot {
                at: t0,
                attacker_ship_id: 1,
                weapon_item_id: 1,
                weapon_type_id: 3640,
                target_ship_id: *id,
                damage: Damage {
                    em: 1.0e6,
                    explosive: 1.0e6,
                    kinetic: 1.0e6,
                    thermal: 1.0e6,
                },
                hit_quality: crate::combat::HIT_QUALITY,
            };
            crate::combat::fire(&mut w, shot, ME, victim, t0);
            let told = changes(&mut w);
            if i == last {
                assert_eq!(
                    told,
                    vec![(Some(vec![ME]), vec![bstr("modified"), PyValue::Int(KILLER)])]
                );
            } else {
                assert!(told.is_empty(), "NPCs still standing: {told:?}");
            }
        }
        assert_eq!(
            act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
    }

    /// Quit and re-accept goes back to the NPCs still waiting.
    #[test]
    fn a_reaccepted_kill_mission_reuses_its_live_pocket() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let first = pocket(&w, KILLER).expect("spawned");
        assert_eq!(act(&mut w, KILLER, Some(ACTION_QUIT)).1, Some(Change::Quit));
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        assert_eq!(pocket(&w, KILLER), Some(first));
        assert_eq!(w.npcs().len(), POCKET_NPCS, "no second spawn");
    }

    fn undock(w: &mut Registry) {
        use crate::space_state::{Motion, Presence};
        let ship = w.character(ME).map(|c| c.ship_id).unwrap();
        w.roster_mut().arrive(Presence::arriving(
            ME,
            ship,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    }

    /// In space in the pocket's system the bookmark warp resolves to the
    /// pocket; another agent's id does not.
    #[test]
    fn the_bookmark_warp_resolves_to_the_pocket_in_space() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        undock(&mut w);
        let pocket = pocket(&w, KILLER).expect("spawned");
        assert_eq!(warp_target(&w, ME, KILLER), Some(pocket.target()));
        assert_eq!(warp_target(&w, ME, RUNNER), None);
    }

    /// **The wire path** through the unbound arms: `DoAction`, then
    /// `WarpToLocation` with the client's six positionals pushes, and the
    /// journal call answers the row.
    #[test]
    fn warp_to_location_and_the_journal_are_answered_on_the_wire() {
        let mut w = world();
        let call = |w: &mut Registry, method: &str, args: &[PyValue]| {
            w.dispatch(&crate::service::Request {
                service: "agentMgr",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ME),
            })
        };
        call(&mut w, "DoAction", &[PyValue::Int(ACTION_ACCEPT)]).expect("answered");
        assert_eq!(state(&w, Some(ME)), State::Accepted);
        undock(&mut w);
        let _ = w.take_pending();
        let args = [
            bstr("dungeon"),
            PyValue::Int(0),
            PyValue::Real(0.0),
            PyValue::Bool(false),
            PyValue::None,
            PyValue::None,
        ];
        assert_eq!(call(&mut w, "WarpToLocation", &args), Ok(PyValue::None));
        assert!(
            !w.take_pending().is_empty(),
            "a warp pushes its action batch"
        );
        let j = call(&mut w, "GetMyJournalDetails", &[]).expect("answered");
        assert_eq!(j, journal(&w, Some(ME), None));
    }

    // -- FLEET-MISSION: "Warp Fleet to" on a mission bookmark -------------
    // Invented ids. `docs/lanes/fleet-mission.md`.

    const ANN: i64 = 90_000_011;
    const BEN: i64 = 90_000_012;
    const OUTSIDER: i64 = 90_000_013;
    const FAR: i64 = 90_000_014;
    const FAR_SYSTEM: i64 = 39_000_002;

    /// ME (the boss and the mission owner), Ann and Ben in his fleet, an
    /// outsider and a fleet member in another system; all in space but the
    /// outsider's fleet is none. ME has accepted the kill mission.
    fn fleet_world() -> Registry {
        use crate::space_state::{Motion, Presence};
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", table());
        c.insert("npc_data", npc_rows());
        let mut characters = vec![crate::model::Character::default()];
        for (i, id) in [ANN, BEN, OUTSIDER, FAR].into_iter().enumerate() {
            characters.push(crate::model::Character {
                character_id: id,
                name: format!("Pilot {id}"),
                ship_id: 100_000_100 + i as i64,
                ..crate::model::Character::default()
            });
        }
        let mut w = Registry::new(crate::model::SandboxConfig {
            characters,
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(c));
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        for id in [ME, ANN, BEN, OUTSIDER, FAR] {
            let ship = w.character(id).map(|c| c.ship_id).unwrap();
            let system = if id == FAR { FAR_SYSTEM } else { SYSTEM };
            w.roster_mut().arrive(Presence::arriving(
                id,
                ship,
                system,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        }
        let f = w.fleets_mut().create(ME).expect("created");
        w.fleets_mut()
            .init(ME, f, Some(670), None, None)
            .expect("init");
        for who in [ANN, BEN, FAR] {
            w.fleets_mut()
                .invite(ME, f, who, None, None, None)
                .expect("invited");
            w.fleets_mut()
                .accept_invite(who, f, Some(670), None, None)
                .expect("accepted");
        }
        let _ = w.take_pending();
        w
    }

    /// `WarpToLocation` as `who`, with the client's six positionals.
    fn warp_as(
        w: &mut Registry,
        who: i64,
        fleet: PyValue,
    ) -> Result<PyValue, crate::service::ServiceError> {
        let args = [
            bstr("dungeon"),
            PyValue::Int(0),
            PyValue::Real(0.0),
            fleet,
            PyValue::None,
            PyValue::None,
        ];
        let bind = PyValue::Int(KILLER);
        w.dispatch(&crate::service::Request {
            service: "agentMgr",
            method: "WarpToLocation",
            args: &args,
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(who),
        })
    }

    fn warping(w: &Registry, who: i64) -> bool {
        w.roster()
            .get(who)
            .is_some_and(|p| p.motion.warp_commanded())
    }

    #[test]
    fn a_fleet_warp_takes_every_member_in_the_system_into_the_pocket() {
        let mut w = fleet_world();
        assert_eq!(warp_as(&mut w, ME, PyValue::Bool(true)), Ok(PyValue::None));
        for who in [ME, ANN, BEN] {
            assert!(warping(&w, who), "{who} warps with the commander");
        }
        assert!(!warping(&w, OUTSIDER), "a non-member stays");
        assert!(!warping(&w, FAR), "a member in another system is skipped");
    }

    #[test]
    fn without_the_fleet_flag_only_the_caller_warps() {
        let mut w = fleet_world();
        assert_eq!(warp_as(&mut w, ME, PyValue::Bool(false)), Ok(PyValue::None));
        assert!(warping(&w, ME));
        for who in [ANN, BEN, OUTSIDER, FAR] {
            assert!(!warping(&w, who), "{who} stays");
        }
    }

    #[test]
    fn a_non_member_is_refused_the_pocket_and_a_member_may_not_take_the_fleet_in() {
        let mut w = fleet_world();
        // The outsider has no pocket and no fleet: nothing resolves.
        assert_eq!(
            warp_as(&mut w, OUTSIDER, PyValue::Bool(true)),
            Ok(PyValue::None)
        );
        assert!(!warping(&w, OUTSIDER));
        assert!(fleet_warp_target(&w, OUTSIDER, KILLER).is_none());
        // A member's own plain warp is the owner's-only subject: no pocket.
        assert_eq!(warp_as(&mut w, ANN, PyValue::Bool(true)), Ok(PyValue::None));
        assert!(!warping(&w, ANN), "Ann has no pocket of her own");
        assert!(!warping(&w, BEN), "and a member commands nobody");
        // The fleet subject resolves to the owner's pocket for members only.
        let pocket = pocket(&w, KILLER).expect("spawned").target();
        assert_eq!(fleet_warp_target(&w, ANN, KILLER), Some(pocket));
        assert_eq!(fleet_warp_target(&w, FAR, KILLER), None, "other system");
    }

    #[test]
    fn a_member_who_opted_out_of_fleet_warp_stays() {
        let mut w = fleet_world();
        let f = w.fleets().fleet_of(ME).map(|f| f.fleet_id).expect("fleet");
        w.fleets_mut()
            .set_opt_out(BEN, f, "SetTakesFleetWarp", false)
            .expect("opted out");
        assert_eq!(warp_as(&mut w, ME, PyValue::Int(1)), Ok(PyValue::None));
        assert!(warping(&w, ANN));
        assert!(!warping(&w, BEN));
    }

    /// With no bundle the station agent has no pool row and offers the
    /// starter courier.
    #[test]
    fn the_station_agent_with_no_pool_row_offers_the_starter() {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        converse(&mut w, Some(ME), None);
        assert_eq!(progress(&w, ME, AGENT_ID).mission_id, Some(STARTER_ID));
        let kw = keywords_at(&w, Some(ME), AGENT_ID);
        assert_eq!(
            dict_get(&kw, b"objectiveDestinationID"),
            Some(&PyValue::Int(DESTINATION_ID))
        );
    }

    /// A request to `agentMgr`, bound to `bound` when given.
    fn ask(w: &mut Registry, method: &str, args: &[PyValue], bound: Option<i64>) -> PyValue {
        let bind = bound.map(PyValue::Int);
        w.dispatch(&crate::service::Request {
            service: "agentMgr",
            method,
            args,
            kwargs: &[],
            bind_params: bind.as_ref(),
            actor: Some(ME),
        })
        .expect("answered")
    }

    fn journal_agents(v: &PyValue) -> Vec<PyValue> {
        let PyValue::List(parts) = v else {
            panic!("[missions, offers]")
        };
        let PyValue::List(rows) = &parts[0] else {
            panic!("a list")
        };
        rows.iter()
            .map(|r| match r {
                PyValue::Tuple(r) => r[4].clone(),
                other => panic!("a tuple: {other:?}"),
            })
            .collect()
    }

    /// **B1 Spec 1**: a bound `GetMyJournalDetails` answers only the bound
    /// agent's row and an empty offers list; unbound answers every row.
    #[test]
    fn a_bound_journal_holds_only_the_bound_agents_rows() {
        let mut w = world();
        act(&mut w, RUNNER, None);
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let bound = ask(&mut w, "GetMyJournalDetails", &[], Some(RUNNER));
        assert_eq!(journal_agents(&bound), vec![PyValue::Int(RUNNER)]);
        let PyValue::List(parts) = &bound else {
            panic!()
        };
        assert_eq!(parts[1], PyValue::List(vec![]));
        let all = ask(&mut w, "GetMyJournalDetails", &[], None);
        assert_eq!(journal_agents(&all).len(), 2);
    }

    /// **B1 Spec 2**: the details dict carries the seven keys and the
    /// objective document; no mission answers `None`.
    #[test]
    fn the_mission_journal_info_is_the_details_dict() {
        let mut w = world();
        assert_eq!(
            ask(&mut w, "GetMissionJournalInfo", &[], Some(RUNNER)),
            PyValue::None
        );
        act(&mut w, RUNNER, None);
        act(&mut w, RUNNER, Some(ACTION_ACCEPT));
        let got = ask(&mut w, "GetMissionJournalInfo", &[], Some(RUNNER));
        for k in [
            "missionNameID",
            "contentID",
            "briefingTextID",
            "missionImage",
            "expirationTime",
            "missionState",
            "objectives",
        ] {
            assert!(dict_get(&got, k.as_bytes()).is_some(), "{k}");
        }
        assert_eq!(
            dict_get(&got, b"missionState"),
            Some(&PyValue::Int(mission_state::ACCEPTED))
        );
        // The same document, bar the bonus deadline (it moves with the clock).
        let (PyValue::Dict(a), PyValue::Dict(b)) = (
            dict_get(&got, b"objectives").expect("objectives").clone(),
            objective_info_at(&w, Some(ME), RUNNER),
        ) else {
            panic!("dicts")
        };
        assert_eq!(a.len(), b.len());
        assert_eq!(
            dict_get(&PyValue::Dict(a), b"objectives"),
            dict_get(&PyValue::Dict(b), b"objectives")
        );
    }

    /// **B1 Spec 3**: the career trio.
    #[test]
    fn the_career_trio_is_answered() {
        const CAREER: i64 = 3_999_111;
        const PLAIN: i64 = 3_999_112;
        const STEP_A: i64 = 999_221;
        const STEP_B: i64 = 999_222;
        let mission = |id| Mission {
            id,
            name_id: id + 1,
            template: "CareerCourier".into(),
            family: "courier".into(),
            level: Some(1),
            ..Mission::default()
        };
        let agent = |id, storyline| pool::Agent {
            id,
            agent_type: crate::storyline::TYPE_CAREER,
            level: 1,
            corp: 1_999_005,
            faction: 599_001,
            storyline,
            ..pool::Agent::default()
        };
        let t = pool::Table::new(
            vec![agent(CAREER, vec![STEP_A, STEP_B]), agent(PLAIN, vec![])],
            vec![mission(STEP_A), mission(STEP_B)],
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        );
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", t);
        let mut w = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(c));
        let ids = PyValue::List(vec![
            PyValue::Int(CAREER),
            PyValue::Int(PLAIN),
            PyValue::Int(3_999_113),
        ]);
        let done = |w: &mut Registry| {
            ask(
                w,
                "GetCompletedCareerAgentIDs",
                std::slice::from_ref(&ids),
                None,
            )
        };
        let expect = |a: bool| {
            PyValue::Dict(vec![
                (PyValue::Bool(a), PyValue::Int(CAREER)),
                (PyValue::Bool(false), PyValue::Int(PLAIN)),
                (PyValue::Bool(false), PyValue::Int(3_999_113)),
            ])
        };
        assert_eq!(done(&mut w), expect(false));
        for m in [STEP_A, STEP_B] {
            w.deltas_mut().put(
                crate::storyline::DONE_KIND,
                vec![ME.to_string(), m.to_string()],
                vec!["1".into()],
            );
        }
        assert_eq!(done(&mut w), expect(true));

        assert_eq!(
            ask(&mut w, "GetDisabledMissions", &[], None),
            PyValue::List(vec![])
        );
        let gains = ask(
            &mut w,
            "GetStandingGainsForMission",
            &[PyValue::Int(STEP_A)],
            Some(CAREER),
        );
        let PyValue::Dict(rows) = gains else {
            panic!("a dict")
        };
        for (_, k) in &rows {
            assert!(
                [PyValue::Int(CAREER), PyValue::Int(1_999_005)].contains(k),
                "{k:?}"
            );
        }
        assert_eq!(
            ask(
                &mut w,
                "GetStandingGainsForMission",
                &[PyValue::Int(1)],
                Some(CAREER)
            ),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn the_warp_range_reads_int_float_and_none() {
        assert_eq!(warp_range(Some(&PyValue::Int(10_000))), 10_000.0);
        assert_eq!(warp_range(Some(&PyValue::Real(2_500.0))), 2_500.0);
        assert_eq!(warp_range(Some(&PyValue::None)), 0.0);
        assert_eq!(warp_range(None), 0.0);
        assert_eq!(warp_range(Some(&PyValue::Real(-1.0))), 0.0);
    }
}
