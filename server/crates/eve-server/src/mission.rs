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

/// `[A]` NPCs in a fallback kill pocket, used only when the mission has no
/// dungeon or its dungeon has no squads: a mission with a dungeon spawns that
/// dungeon's own rooms and squads through `site_spawn`. Falsified by a kill
/// mission whose dungeon has squads and still gets the ring of rats.
pub const POCKET_NPCS: usize = 3;

/// The site ids of mission pockets: 8.075e9 up to the population site band
/// (8.1e9), carved from the abyss band's top (`abyss::SITE_END`). One id per
/// live (character, agent) pocket; they wrap and skip a live one. Not a ball
/// id: an instance's balls are in `site::INSTANCE_BASE`'s band.
pub const SITE_BASE: i64 = 8_075_000_000;
/// One past the band.
pub const SITE_END: i64 = crate::population::SITE_BASE;

const _: () = assert!(SITE_BASE < SITE_END);

/// Whether a site id is a mission pocket's. `site.rs` and `site_spawn.rs`
/// leave these instances to the mission that owns them.
pub fn is_mission_site(site_id: i64) -> bool {
    (SITE_BASE..SITE_END).contains(&site_id)
}

/// `[A]` Two pockets' anchors are this far apart along z, metres, so the
/// rooms of two pilots' missions in one system never overlap. Falsified by a
/// pocket that lands on a body, or two that touch.
const POCKET_SPACING: f64 = 1.0e8;
/// Anchors before the spacing wraps.
const POCKET_SLOTS: i64 = 1_000;

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
    /// `(character, agent) -> site id` for a pocket that is the mission's own
    /// dungeon, run by `site_spawn`. Its `pockets` entry holds only where it
    /// is (the bookmark and the warp aim), no NPCs.
    sites: BTreeMap<(i64, i64), i64>,
    /// Pockets a GM cleared (`/missioncomplete`): the objective is met though
    /// the site never counted the kills.
    gm_done: std::collections::BTreeSet<(i64, i64)>,
    next_site: u64,
    /// The mission each pocket (a ring or a dungeon) was made for. A pocket
    /// is reused only by the same mission; another is dropped first.
    pocket_mission: BTreeMap<(i64, i64), i64>,
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

/// A heraldry mission (EM-1): "bring these built items, get fixed Evermarks".
/// By the row's template, since the bundle's family for it is `other`.
fn is_heraldry(m: &Mission) -> bool {
    m.template == "Heraldry"
}

/// What `character` can hand a heraldry agent: `(item id, units, assembled)`
/// packaged stacks first, then assembled units, each by id (the smallest ids
/// are taken first `[A]`), of the row's
/// `obj_type` in the agent's own station hangar. A packaged stack counts its
/// quantity. `[A]` An assembled (singleton) item counts one unit, so a hull
/// `/createitem` made can be turned in; the pilot's active ship and a hull
/// with anything inside it never count, and neither does an insured one
/// (turning it in would forfeit the premium paid; skipping it keeps the
/// pilot from losing that by accident). Falsified by a Tranquility heraldry
/// agent refusing an unpackaged hull. Empty when the agent has no station or
/// the row names no item.
fn heraldry_stock(w: &Registry, character: i64, c: &Current) -> Vec<(i64, i64, bool)> {
    let (Some(station), Some(type_id)) = (c.agent.station, c.m.obj_type) else {
        return Vec::new();
    };
    let active = w
        .character(character)
        .and_then(|ch| crate::fitting::active_ship_of(w.store(), ch))
        .map(|s| s.0);
    let items = w.store().items();
    // One pass: every location something sits in.
    let holders: std::collections::BTreeSet<i64> = items.iter().map(|k| k.location_id).collect();
    let now = crate::pyutil::filetime_now();
    let mut stacks: Vec<(i64, i64, bool)> = items
        .iter()
        .filter(|i| {
            i.type_id == type_id
                && i.owner_id == character
                && i.location_id == station
                && i.flag_id == crate::model::FLAG_HANGAR
                && i.quantity > 0
                && (!i.singleton
                    || (Some(i.item_id) != active
                        && !holders.contains(&i.item_id)
                        && crate::service::insurance::contract(w, i.item_id, now).is_none()))
        })
        .map(|i| {
            (
                i.item_id,
                if i.singleton { 1 } else { i.quantity },
                i.singleton,
            )
        })
        .collect();
    stacks.sort_unstable_by_key(|s| (s.2, s.0));
    stacks
}

/// Take the heraldry row's `obj_qty` of `obj_type` out of the station hangar
/// (`Edit::Consume`; an assembled item or a stack taken to zero is
/// destroyed). `false`, with nothing taken, when the hangar is short or the
/// edit is refused.
fn take_heraldry(w: &mut Registry, character: i64, c: &Current) -> bool {
    let Some(qty) = c.m.obj_qty.filter(|q| *q > 0) else {
        return false;
    };
    let mut left = qty;
    let mut edits = Vec::new();
    for (item_id, units, assembled) in heraldry_stock(w, character, c) {
        if left <= 0 {
            break;
        }
        let take = units.min(left);
        edits.push(if assembled {
            crate::item::Edit::Destroy { item_id }
        } else {
            crate::item::Edit::Consume {
                item_id,
                quantity: take,
            }
        });
        left -= take;
    }
    left <= 0 && w.apply_items(&edits).is_ok()
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
    is_courier(m) || is_heraldry(m) || (is_kill(m) && pocket_possible(w, m))
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
    // A heraldry turn-in is at the agent itself.
    if is_heraldry(&c.m) {
        return Some(Dest {
            station: c.agent.station?,
            system: c.agent.system?,
            owner: c.agent.corp,
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
/// - A heraldry row (EM-1): `obj_qty` of `obj_type` in the agent's own
///   station hangar ([`heraldry_stock`]); [`take_heraldry`] consumes it on Complete.
/// - Anything else (mining, talk, other): met. A talk mission is met by
///   talking; mining is never offered ([`servable`]) `[A]`, falsified by a
///   chain that hands out a mining row.
fn objective_done(w: &Registry, character: i64, c: &Current) -> bool {
    if is_kill(&c.m) {
        let key = (character, c.agent.id);
        let live = w.ext::<Live>();
        if live.gm_done.contains(&key) {
            return true;
        }
        // The mission's own dungeon: `site_spawn`'s completion over every
        // room, including the rooms behind gates.
        if let Some(site) = live.sites.get(&key) {
            return crate::site_spawn::is_completed(w, *site);
        }
        match live.pockets.get(&key) {
            Some(p) => dungeon::cleared(w, p),
            None => !pocket_possible(w, &c.m),
        }
    } else if is_heraldry(&c.m) {
        // `obj_qty` of `obj_type` in the agent's own station; a row naming
        // no item or quantity is never complete (it would pay for nothing).
        c.m.obj_qty.filter(|q| *q > 0).is_some_and(|q| {
            heraldry_stock(w, character, c)
                .iter()
                .map(|s| s.1)
                .sum::<i64>()
                >= q
        })
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
    if c.fresh {
        // A new offer replaces the old mission: its pocket is nobody's now.
        drop_pocket(w, (ch, agent));
    } else if c.p.state == State::Accepted
        && is_kill(&c.m)
        && !w.ext::<Live>().pockets.contains_key(&(ch, agent))
    {
        // A restart emptied space: the accepted mission gets its pocket again.
        spawn_pocket(w, ch, &c);
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
            drop_pocket(w, (ch, agent));
        }
        Some(Change::Completed) => {
            // A heraldry turn-in hands the items over; `done` was met a
            // moment ago, so a refusal here is a race and pays nothing.
            if is_heraldry(&c.m) && !take_heraldry(w, ch, &c) {
                return (refusal(MSG_NO_MISSION), None);
            }
            crate::mission_rewards::pay(w, ch, &c.agent, &c.m, c.p.accepted_at, now);
            drop_pocket(w, (ch, agent));
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
    if w.ext::<Live>().pocket_mission.get(&k) == Some(&c.m.id) && pocket_alive(w, k) {
        return;
    }
    // A pocket left behind (another mission's, cleared, or its instance gone)
    // is let go first.
    drop_pocket(w, k);
    match spawn_dungeon_pocket(w, k, c) {
        Ok(()) => return,
        Err(why) => crate::outln!(
            "[mission] mission {} for {ch}: {why}; falling back to {POCKET_NPCS} generic rats",
            c.m.id
        ),
    }
    let Some(kind) = c.m.enemy_faction.and_then(|e| rat(w, e)) else {
        return;
    };
    if let Some(pocket) = dungeon::spawn(w, kind, site(c), POCKET_NPCS) {
        let live = w.ext_mut::<Live>();
        live.pockets.insert(k, pocket);
        live.pocket_mission.insert(k, c.m.id);
    }
}

/// Whether `k`'s pocket still stands and still has a fight in it: the ring of
/// rats with one alive, or the dungeon's instance, not yet cleared.
fn pocket_alive(w: &Registry, k: (i64, i64)) -> bool {
    let live = w.ext::<Live>();
    if let Some(site) = live.sites.get(&k) {
        return w.ext::<crate::site::Sites>().get(*site).is_some()
            && !crate::site_spawn::is_completed(w, *site);
    }
    live.pockets.get(&k).is_some_and(|p| dungeon::alive(w, p))
}

/// The next free site id of the mission band.
fn next_site_id(w: &mut Registry) -> Option<i64> {
    let span = (SITE_END - SITE_BASE) as u64;
    for _ in 0..64 {
        let live = w.ext_mut::<Live>();
        let id = SITE_BASE + (live.next_site % span) as i64;
        live.next_site += 1;
        let taken = live.sites.values().any(|s| *s == id)
            || w.ext::<crate::site::Sites>().get(id).is_some();
        if !taken {
            return Some(id);
        }
    }
    None
}

/// The anchor of anchor slot `n`: [`dungeon::POCKET_OFFSET`] moved along z.
fn slot_anchor(n: i64) -> [f64; 3] {
    let mut at = dungeon::POCKET_OFFSET;
    at[2] += n as f64 * POCKET_SPACING;
    at
}

/// The lowest anchor slot, from 1, that no live mission instance in `system`
/// holds. Slot 0 is the fallback ring's own spot ([`dungeon::POCKET_OFFSET`]).
fn free_slot(w: &Registry, system: i64) -> Option<i64> {
    let used: std::collections::BTreeSet<i64> = w
        .ext::<crate::site::Sites>()
        .iter()
        .filter(|i| i.system == system && is_mission_site(i.site_id))
        .map(|i| ((i.anchor[2] - dungeon::POCKET_OFFSET[2]) / POCKET_SPACING).round() as i64)
        .collect();
    (1..POCKET_SLOTS).find(|n| !used.contains(n))
}

/// **The mission's own dungeon as a per-mission instance** (`site`, then
/// `site_spawn`): its entry room's objects, its R5 squads, its triggers,
/// respawn and gates, the same machinery a combat anomaly runs. The reward is
/// priced from this dungeon's squad fill (`mission_rewards`), so the fight and
/// the pay agree. `Err` says why not, and the caller falls back.
fn spawn_dungeon_pocket(w: &mut Registry, k: (i64, i64), c: &Current) -> Result<(), String> {
    let Some(did) = c.m.dungeon else {
        return Err("the mission names no dungeon".to_owned());
    };
    let archetype = w
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&did))
        .map(|d| d.archetype)
        .ok_or_else(|| format!("dungeon {did} is not in the catalogue"))?;
    let id = next_site_id(w).ok_or("the mission site band is full")?;
    let system = site(c);
    let anchor =
        slot_anchor(free_slot(w, system).ok_or("every anchor slot in the system is taken")?);
    let s = crate::population::Site {
        system,
        row: 0,
        k: 0,
        slot: 0,
        generation: 0,
        site_id: id,
        dungeon_id: did,
        archetype,
        entry: crate::dungeon_data::Entry::Anomaly,
        anchor,
        target_id: format!("MSN-{}", id - SITE_BASE),
        state: crate::population::State::Live,
        at: crate::pyutil::filetime_now(),
    };
    if !crate::site::instantiate(w, &s) {
        return Err(format!("dungeon {did} could not be instantiated"));
    }
    let (squads, ships) = w
        .ext::<crate::site::Sites>()
        .get(id)
        .map(|inst| {
            let groups = crate::site_spawn::open(w, inst).groups;
            let ships: usize = groups
                .iter()
                .filter(|g| !g.gated)
                .map(|g| g.members.len())
                .sum();
            (groups.len(), ships)
        })
        .unwrap_or((0, 0));
    if squads == 0 {
        crate::site_spawn::discard(w, id);
        return Err(format!("dungeon {did} has no squads in its entry room"));
    }
    let live = w.ext_mut::<Live>();
    live.sites.insert(k, id);
    live.pocket_mission.insert(k, c.m.id);
    live.gm_done.remove(&k);
    live.pockets.insert(
        k,
        Pocket {
            solar_system_id: system,
            position: anchor,
            npc_ids: Vec::new(),
        },
    );
    crate::outln!(
        "[mission] mission {} for {}: its own dungeon {did} as site {id}, {squads} squads, {ships} ships on arrival",
        c.m.id,
        k.0
    );
    Ok(())
}

/// Let `k`'s pocket go: its record, and a dungeon pocket's NPCs and instance.
/// The ring of rats' NPCs are left as they stood (their wrecks are loot).
fn drop_pocket(w: &mut Registry, k: (i64, i64)) {
    let live = w.ext_mut::<Live>();
    let ring = live.pockets.remove(&k);
    live.gm_done.remove(&k);
    live.pocket_mission.remove(&k);
    let site = live.sites.remove(&k);
    if let Some(p) = ring {
        // A ring's NPCs still standing leave with it (a cleared one has none).
        crate::npc::despawn_ids(w, p.solar_system_id, &p.npc_ids);
    }
    if let Some(site) = site {
        crate::site_spawn::discard(w, site);
    }
}

/// **Give every accepted kill mission of `actor` its pocket back** after a
/// restart (space is never persisted): the bookmark and the objective work
/// again without a Quit and a re-accept. Called by the journal read.
pub fn revive(w: &mut Registry, actor: Option<i64>) {
    let Some(ch) = actor.filter(|id| w.character(*id).is_some()) else {
        return;
    };
    let now = crate::pyutil::filetime_now();
    let me = ch.to_string();
    let agents: Vec<i64> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| match k.as_slice() {
            [c, a] if *c == me && Progress::from_value(v)?.state == State::Accepted => {
                a.parse().ok()
            }
            _ => None,
        })
        .collect();
    for agent in agents {
        if w.ext::<Live>().pockets.contains_key(&(ch, agent)) {
            continue;
        }
        if let Some(c) = current(w, ch, agent, now).filter(|c| is_kill(&c.m)) {
            spawn_pocket(w, ch, &c);
        }
    }
}

/// `site_spawn` found mission site `site_id` cleared: the owner's open dialogue
/// is told 'modified' so Complete shows (as [`npc_killed`] does for a ring).
pub fn site_completed(w: &mut Registry, site_id: i64) {
    let hit: Vec<(i64, i64)> = w
        .ext::<Live>()
        .sites
        .iter()
        .filter(|(_, s)| **s == site_id)
        .map(|(k, _)| *k)
        .collect();
    for (ch, agent) in hit {
        if progress(w, ch, agent).state == State::Accepted {
            tell_modified(w, ch, agent);
        }
    }
}

fn tell_modified(w: &mut Registry, ch: i64, agent: i64) {
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
    let live = w.ext::<Live>();
    live.pockets
        .iter()
        .find(|(_, p)| p.npc_ids.contains(&npc_id))
        .map(|((ch, _), _)| *ch)
        .or_else(|| {
            let site = crate::site_spawn::site_of_npc(w, npc_id)?;
            live.sites
                .iter()
                .find(|(_, s)| **s == site)
                .map(|((ch, _), _)| *ch)
        })
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

/// GM (dev): every accepted kill mission of `character` with `agent` (or with
/// every agent): its pocket's NPCs leave space and the client is told
/// 'modified', so the Complete button shows. The reward is still the normal
/// turn-in. Returns how many pockets it cleared.
pub fn gm_clear_pocket(w: &mut Registry, character: i64, agent: Option<i64>) -> usize {
    let hit: Vec<((i64, i64), i64, Vec<i64>)> = w
        .ext::<Live>()
        .pockets
        .iter()
        .filter(|((ch, a), _)| *ch == character && agent.is_none_or(|x| x == *a))
        .filter(|((ch, a), _)| progress(w, *ch, *a).state == State::Accepted)
        .map(|(k, p)| (*k, p.solar_system_id, p.npc_ids.clone()))
        .collect();
    for ((ch, a), system, ids) in &hit {
        if let Some(site) = w.ext::<Live>().sites.get(&(*ch, *a)).copied() {
            // A dungeon pocket: it and its rooms go, and the objective is
            // met by the GM's say-so.
            crate::site_spawn::discard(w, site);
            w.ext_mut::<Live>().gm_done.insert((*ch, *a));
        } else {
            crate::npc::despawn_ids(w, *system, ids);
        }
        tell_modified(w, *ch, *a);
    }
    hit.len()
}

/// `/missioncomplete <agentID|all>` (`--dev` only: skipping the fight and
/// keeping the reward is value from nowhere).
pub fn complete_slash(
    w: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    if !w.config().dev {
        return Ok(bstr("dev only: start the server with --dev"));
    }
    let Some(actor) = actor else {
        return Ok(bstr("no character to change"));
    };
    let agent = match words {
        [a] if a.eq_ignore_ascii_case("all") => None,
        [a] => match a.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => return Ok(bstr("usage: /missioncomplete <agentID|all>")),
        },
        _ => return Ok(bstr("usage: /missioncomplete <agentID|all>")),
    };
    let n = gm_clear_pocket(w, actor, agent);
    Ok(bstr(&if n == 0 {
        "no kill pocket to clear (courier missions: deliver the cargo)".to_owned()
    } else {
        format!("cleared {n} mission pocket(s); open the agent to complete")
    }))
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
            drop_pocket(w, (ch, agent));
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
                (
                    "location",
                    dungeon_location(agent, site(&c), w.ext::<Live>().pockets.get(&(ch, agent))),
                ),
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

/// The `location` of a kill mission's `dungeons` entry in
/// `GetMissionObjectiveInfo`. A live server's carries, beside the three keys
/// `LocationWrapper` subscripts, `coords` (the pocket's x, y, z),
/// `locationType` `'dungeon'`, `agentID`, `locationNumber` 0 and `shipTypeID`
/// None **[V-TQ]** (a recorded Enforcer career mission 1, dungeon 2055). With
/// the system alone, the agent window's objective location has nothing to
/// warp to and the pilot never finds the pocket. With no pocket
/// (none spawned yet, or gone) it is the system alone, as before.
fn dungeon_location(agent: i64, system: i64, pocket: Option<&Pocket>) -> PyValue {
    let Some(pk) = pocket else {
        return dungeon::location_wrap(system);
    };
    pydict(vec![
        ("solarsystemID", PyValue::Int(pk.solar_system_id)),
        ("typeID", PyValue::Int(dungeon::TYPE_SOLAR_SYSTEM)),
        ("locationID", PyValue::Int(pk.solar_system_id)),
        ("shipTypeID", PyValue::None),
        ("locationNumber", PyValue::Int(0)),
        ("agentID", PyValue::Int(agent)),
        (
            "coords",
            PyValue::List(pk.position.iter().map(|v| PyValue::Real(*v)).collect()),
        ),
        ("locationType", bstr("dungeon")),
    ])
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
    let r = crate::service::args::as_float(arg).unwrap_or(0.0);
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
        // No dungeon catalogue: a kill mission takes the fallback ring of
        // rats. The tests of a mission's own dungeon (`own_dungeon`) bring one.
        c.insert("dungeon_data", crate::dungeon_data::Table::default());
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

    /// A kill mission's objective location names its pocket the way a live
    /// server does: coords, `locationType` `'dungeon'`, the agent, location
    /// number 0 and no ship type, beside the system keys **[V-TQ]**; with no
    /// pocket it is the system alone.
    #[test]
    fn a_kill_objective_location_carries_its_pocket_coords() {
        let pk = Pocket {
            solar_system_id: 30_990_001,
            position: [1.0e11, -2.0e10, 3.5e11],
            npc_ids: Vec::new(),
        };
        let v = dungeon_location(3_990_001, 30_990_001, Some(&pk));
        let get = |k: &str| crate::pyutil::dict_get(&v, k.as_bytes()).cloned();
        assert_eq!(
            get("coords"),
            Some(PyValue::List(vec![
                PyValue::Real(1.0e11),
                PyValue::Real(-2.0e10),
                PyValue::Real(3.5e11)
            ]))
        );
        assert_eq!(get("locationType"), Some(bstr("dungeon")));
        assert_eq!(get("agentID"), Some(PyValue::Int(3_990_001)));
        assert_eq!(get("locationNumber"), Some(PyValue::Int(0)));
        assert_eq!(get("shipTypeID"), Some(PyValue::None));
        assert_eq!(get("solarsystemID"), Some(PyValue::Int(30_990_001)));
        let bare = dungeon_location(3_990_001, 30_990_001, None);
        assert_eq!(bare, dungeon::location_wrap(30_990_001));
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

    const HERALD_AGENT: i64 = 981_000_101;
    const HERALD_CORP: i64 = 981_000_001;
    const HERALD_STATION: i64 = 69_000_021;
    const HERALD: i64 = 981_001;
    const HERALD_LP: i64 = 40;

    /// The invented heraldry agent and its only row: `qty` of `type_id` for
    /// 40 LP (`herald_world`: 5 of `CARGO_TYPE_ID`).
    fn herald_world() -> Registry {
        herald_world_for(CARGO_TYPE_ID, 5)
    }

    fn herald_world_for(type_id: i64, qty: i64) -> Registry {
        herald_world_row(Some(type_id), Some(qty))
    }

    fn herald_world_row(type_id: Option<i64>, qty: Option<i64>) -> Registry {
        let base = table();
        let mut agents: Vec<pool::Agent> = base.agents().cloned().collect();
        let mut missions: Vec<Mission> = base.missions().cloned().collect();
        agents.push(pool::Agent {
            id: HERALD_AGENT,
            agent_type: pool::TYPE_HERALDRY,
            division: 37,
            level: 1,
            corp: HERALD_CORP,
            faction: 599_001,
            station: Some(HERALD_STATION),
            system: Some(SYSTEM),
            ..pool::Agent::default()
        });
        missions.push(Mission {
            id: HERALD,
            name_id: 981_900,
            template: "Heraldry".into(),
            family: "other".into(),
            agent_type: Some(pool::TYPE_HERALDRY),
            corp: Some(HERALD_CORP),
            obj_type: type_id,
            obj_qty: qty,
            fixed_lp: HERALD_LP,
            ..Mission::default()
        });
        let t = pool::Table::new(
            agents,
            missions,
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        );
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", t);
        c.insert("npc_data", npc_rows());
        Registry::new(crate::model::SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    /// `n` packaged units of the cargo type in `station`'s hangar, mine.
    fn stock(w: &mut Registry, station: i64, n: i64) {
        w.spawn_items(&CARGO_TYPE_ID.to_string(), n, ME, station)
            .expect("the stock spawns");
    }

    fn held(w: &Registry, station: i64) -> i64 {
        w.store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == CARGO_TYPE_ID
                    && i.owner_id == ME
                    && i.location_id == station
                    && i.flag_id == crate::model::FLAG_HANGAR
            })
            .map(|i| i.quantity)
            .sum()
    }

    /// EM-1: the heraldry agent offers its row, accepting puts no cargo in
    /// the hangar, 3 of 5 cannot be turned in and nothing moves, 5 are
    /// consumed (a sixth stays) and the row's LP goes to the agent's corp.
    #[test]
    fn a_heraldry_turn_in_takes_the_items_and_pays_the_agents_corp_lp() {
        let mut w = herald_world();
        let (_, change) = act(&mut w, HERALD_AGENT, None);
        assert_eq!(change, None);
        assert_eq!(progress(&w, ME, HERALD_AGENT).mission_id, Some(HERALD));
        assert_eq!(
            act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT)).1,
            Some(Change::Accepted)
        );
        assert_eq!(cargo(&w), 0, "heraldry hands out no cargo");

        stock(&mut w, HERALD_STATION, 3);
        let (a, _) = act(&mut w, HERALD_AGENT, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only with 3 of 5");
        assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(progress(&w, ME, HERALD_AGENT).state, State::Accepted);
        assert_eq!(held(&w, HERALD_STATION), 3, "a short turn-in moves nothing");
        assert_eq!(crate::loyalty::balance(&w, ME, HERALD_CORP), 0);

        stock(&mut w, HERALD_STATION, 3);
        assert_eq!(held(&w, HERALD_STATION), 6);
        let (a, _) = act(&mut w, HERALD_AGENT, None);
        assert_eq!(unwrap_reply(&a).1, 2, "Complete and Quit with 6");
        assert_eq!(
            act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
        assert_eq!(held(&w, HERALD_STATION), 1, "exactly obj_qty is consumed");
        assert_eq!(crate::loyalty::balance(&w, ME, HERALD_CORP), HERALD_LP);
    }

    /// Items elsewhere than the agent's station do not count.
    #[test]
    fn heraldry_items_in_another_station_do_not_count() {
        let mut w = herald_world();
        act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
        stock(&mut w, HERALD_STATION + 1, 5);
        assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
        assert_eq!(held(&w, HERALD_STATION + 1), 5);
    }

    /// An assembled (singleton) hull counts one unit and is destroyed on
    /// turn-in; a spare stays.
    #[test]
    fn heraldry_assembled_hulls_count_one_each_and_a_spare_stays() {
        const HULL: i64 = 587;
        let mut w = herald_world_for(HULL, 2);
        act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
        let (ids, _) = w
            .spawn_items(&HULL.to_string(), 1, ME, HERALD_STATION)
            .unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
        w.spawn_items(&HULL.to_string(), 2, ME, HERALD_STATION)
            .unwrap();
        let hulls = |w: &Registry| {
            w.store()
                .items()
                .iter()
                .filter(|i| i.type_id == HULL && i.location_id == HERALD_STATION)
                .count()
        };
        assert_eq!(hulls(&w), 3);
        assert_eq!(
            act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
        assert_eq!(hulls(&w), 1);
        assert_eq!(crate::loyalty::balance(&w, ME, HERALD_CORP), HERALD_LP);
    }

    /// F1: a hull with something inside it and the boarded ship (with its
    /// pod inside) are not for sale: Complete is not offered, both survive.
    #[test]
    fn heraldry_never_takes_a_fitted_hull_or_the_boarded_ship() {
        use crate::item::{Edit, Slot};
        const HULL: i64 = 587;
        let mut w = herald_world_for(HULL, 2);
        act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
        let (ids, _) = w
            .spawn_items(&HULL.to_string(), 3, ME, HERALD_STATION)
            .unwrap();
        // ids[2] is a plain hull: the one honest unit of the two asked for.
        let (fitted, boarded) = (ids[0], ids[1]);
        // (a) something inside `fitted`.
        let (inner, _) = w
            .spawn_items(&CARGO_TYPE_ID.to_string(), 1, ME, HERALD_STATION)
            .unwrap();
        w.apply_items(&[Edit::Move {
            item_id: inner[0],
            to: Slot::new(ME, fitted, crate::fitting::FLAG_CARGO),
        }])
        .unwrap();
        // (b) `boarded` holds the pilot's pod.
        let pod = {
            let ch = w.character(ME).unwrap();
            crate::fitting::capsule_of(w.store(), ch).map(|p| p.item_id)
        }
        .expect("the pilot has a pod");
        w.apply_items(&[Edit::Move {
            item_id: pod,
            to: Slot::new(ME, boarded, crate::fitting::FLAG_CAPSULE),
        }])
        .unwrap();
        let (a, _) = act(&mut w, HERALD_AGENT, None);
        assert_eq!(unwrap_reply(&a).1, 1, "Quit only");
        assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
        assert!(ids.iter().all(|id| w.store().get(*id).is_some()));
    }

    /// F2: packaged units go before an older assembled hull.
    #[test]
    fn heraldry_takes_packaged_stacks_before_an_assembled_unit() {
        const HULL: i64 = 587;
        let mut w = herald_world_for(HULL, 5);
        act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
        let (ids, _) = w
            .spawn_items(&HULL.to_string(), 6, ME, HERALD_STATION)
            .unwrap();
        for id in &ids[1..] {
            w.apply_items(&[crate::item::Edit::Repackage { item_id: *id }])
                .expect("repackage");
        }
        assert!(w.store().get(ids[0]).is_some_and(|i| i.singleton));
        assert_eq!(
            act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
        let left: Vec<_> = w
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == HULL && i.location_id == HERALD_STATION)
            .map(|i| (i.item_id, i.singleton))
            .collect();
        assert_eq!(left, vec![(ids[0], true)], "the 5 packaged went");
    }

    /// F3: a row naming no item or no quantity is never complete, pays nothing.
    #[test]
    fn a_heraldry_row_without_item_or_quantity_never_completes() {
        for (t, q) in [(None, Some(1)), (Some(CARGO_TYPE_ID), None), (None, None)] {
            let mut w = herald_world_row(t, q);
            act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
            stock(&mut w, HERALD_STATION, 3);
            let (a, _) = act(&mut w, HERALD_AGENT, None);
            assert_eq!(unwrap_reply(&a).1, 1, "Quit only for {t:?} {q:?}");
            assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
            assert_eq!(crate::loyalty::balance(&w, ME, HERALD_CORP), 0);
        }
    }

    /// F4: an insured assembled hull does not count (its premium would be
    /// lost).
    #[test]
    fn heraldry_does_not_take_an_insured_hull() {
        const HULL: i64 = 587;
        let mut w = herald_world_for(HULL, 2);
        act(&mut w, HERALD_AGENT, Some(ACTION_ACCEPT));
        let (ids, _) = w
            .spawn_items(&HULL.to_string(), 2, ME, HERALD_STATION)
            .unwrap();
        let now = crate::pyutil::filetime_now();
        // A running contract row: `[owner, type, fraction, start, end]`.
        w.deltas_mut().put(
            crate::service::insurance::KIND,
            vec![ids[0].to_string()],
            vec![
                ME.to_string(),
                HULL.to_string(),
                "0.5".to_string(),
                now.to_string(),
                (now + 10_000_000_000).to_string(),
            ],
        );
        assert_eq!(act(&mut w, HERALD_AGENT, Some(ACTION_COMPLETE)).1, None);
        assert!(ids.iter().all(|id| w.store().get(*id).is_some()));
    }

    /// `/missioncomplete` (dev): the pocket's NPCs leave space, the cleared
    /// test holds, and the agent window is told 'modified'.
    #[test]
    fn gm_clear_pocket_clears_an_accepted_kill_pocket() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let pocket = pocket(&w, KILLER).expect("spawned");
        assert!(!dungeon::cleared(&w, &pocket));
        let _ = w.take_pending();
        assert_eq!(gm_clear_pocket(&mut w, ME, Some(KILLER)), 1);
        assert!(dungeon::cleared(&w, &pocket));
        let told: Vec<_> = w
            .take_due(std::time::Instant::now() + std::time::Duration::from_secs(600))
            .into_iter()
            .filter(|p| p.to_push().0 == "OnAgentMissionChange")
            .map(|p| (p.audience(), p.to_push().1))
            .collect();
        assert_eq!(
            told,
            vec![(Some(vec![ME]), vec![bstr("modified"), PyValue::Int(KILLER)])]
        );
        assert_eq!(gm_clear_pocket(&mut w, ME, None), 1, "all: still accepted");
        assert_eq!(gm_clear_pocket(&mut w, ME, Some(KILLER + 1)), 0);
        assert_eq!(
            act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
            Some(Change::Completed)
        );
    }

    #[test]
    fn complete_slash_refuses_without_dev_and_leaves_the_pocket() {
        let mut w = world();
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        let pocket = pocket(&w, KILLER).expect("spawned");
        let said = |r: Result<PyValue, crate::service::ServiceError>| match r {
            Ok(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        };
        let id = KILLER.to_string();
        assert!(said(complete_slash(&mut w, Some(ME), &[&id])).starts_with("dev only"));
        assert!(!dungeon::cleared(&w, &pocket), "the NPCs remain");
        assert_eq!(w.npcs().len(), POCKET_NPCS);
    }

    #[test]
    fn complete_slash_in_dev_says_what_it_did() {
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", table());
        c.insert("npc_data", npc_rows());
        let mut w = Registry::new(crate::model::SandboxConfig {
            dev: true,
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(c));
        let said = |r: Result<PyValue, crate::service::ServiceError>| match r {
            Ok(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        };
        assert!(said(complete_slash(&mut w, Some(ME), &["all"])).starts_with("no kill pocket"));
        act(&mut w, KILLER, Some(ACTION_ACCEPT));
        assert!(said(complete_slash(&mut w, Some(ME), &["all"])).starts_with("cleared 1"));
        assert!(said(complete_slash(&mut w, Some(ME), &["x"])).starts_with("usage"));
    }

    /// **Kill missions run their own dungeon**: invented rows only. The
    /// mission's dungeon is [`DUNGEON`], which the mission row names.
    mod own_dungeon {
        use super::*;
        use crate::dungeon_data::{Dungeon, Object, Room, Spawn};
        use crate::npc_data::{Bounty, Needs, NpcTypeRow, Role, Tier};
        use std::time::Instant;

        /// The dungeon `table()`'s kill mission names.
        const DUNGEON: i64 = 999_101;
        const STRUCTURE: i64 = 990_450;
        const BLUE: i64 = 10_000_000;

        fn spawn(object: i64, entity: i64) -> Spawn {
            Spawn {
                room: 1,
                object,
                entity,
                ..Spawn::default()
            }
        }

        /// Room 1 with two structures and `spawns` ungated spawns on them.
        fn dungeon(spawns: Vec<Spawn>) -> Dungeon {
            let obj = |id, x| Object {
                room: 1,
                id,
                type_id: STRUCTURE,
                pos: [x, 0.0, 0.0],
                radius: Some(2_000.0),
                ..Object::default()
            };
            Dungeon {
                id: DUNGEON,
                archetype: 24,
                faction: Some(ENEMY),
                difficulty: Some(1),
                entry_object: Some(1),
                rooms: vec![Room {
                    id: 1,
                    ..Room::default()
                }],
                objects: vec![obj(1, 0.0), obj(2, 20_000.0)],
                spawns,
                ..Dungeon::default()
            }
        }

        /// The enemy's one type is the hand hull, with a bounty to budget.
        fn rows() -> crate::npc_data::Table {
            let mut t = crate::npc_data::Table::default();
            let id = crate::npc::ANGEL_HIJACKER.type_id;
            t.types.insert(
                id,
                NpcTypeRow {
                    type_id: id,
                    group_id: 99_600,
                    faction_id: ENEMY,
                    context: crate::npc_gen::ASTEROID.to_owned(),
                    class: "Frigate".to_owned(),
                    tier: Tier::Base,
                    alias_of: None,
                    needs: Needs::TURRET,
                    role: Role::Dps,
                },
            );
            t.bounty.insert(
                id,
                Bounty {
                    bounty: 1.0,
                    ..Bounty::default()
                },
            );
            t
        }

        fn world_with(d: Dungeon) -> Registry {
            let mut dungeons = crate::dungeon_data::Table::fixture();
            dungeons.dungeons.insert(d.id, d);
            let mut c = crate::content::Content::fixture();
            c.insert("mission_pool", table());
            c.insert("npc_data", rows());
            c.insert("dungeon_data", dungeons);
            Registry::new(crate::model::SandboxConfig::default())
                .with_content(std::sync::Arc::new(c))
        }

        fn two_squads() -> Registry {
            world_with(dungeon(vec![spawn(1, 71), spawn(2, 72)]))
        }

        fn site_id(w: &Registry) -> i64 {
            *w.ext::<Live>().sites.get(&(ME, KILLER)).expect("a site")
        }

        const ME2: i64 = 90_000_002;

        /// A world with a second pilot.
        fn two_pilots() -> Registry {
            let mut dungeons = crate::dungeon_data::Table::fixture();
            let d = dungeon(vec![spawn(1, 71), spawn(2, 72)]);
            dungeons.dungeons.insert(d.id, d);
            let mut c = crate::content::Content::fixture();
            c.insert("mission_pool", table());
            c.insert("npc_data", rows());
            c.insert("dungeon_data", dungeons);
            let who = |id, ship| crate::model::Character {
                character_id: id,
                name: format!("Pilot {id}"),
                ship_id: ship,
                ..crate::model::Character::default()
            };
            Registry::new(crate::model::SandboxConfig {
                characters: vec![who(ME, 100_000_001), who(ME2, 100_000_002)],
                ..crate::model::SandboxConfig::default()
            })
            .with_content(std::sync::Arc::new(c))
        }

        fn step(w: &mut Registry, at: i64) {
            crate::site_spawn::tick(w, Instant::now(), at * BLUE);
        }

        /// What `npc_gen::dungeon_squads` says the dungeon holds, drawn as
        /// `mission_rewards` draws it (`&[]`): the same fill for every site.
        fn want(w: &Registry, _site: i64) -> usize {
            let d = &w.content().get::<crate::dungeon_data::Table>().dungeons[&DUNGEON];
            let data = w.content().get::<crate::npc_data::Table>();
            crate::npc_gen::dungeon_squads(d, data, w.world().seed, &[])
                .iter()
                .map(|s| s.squad.members.len())
                .sum()
        }

        fn kill_all(w: &mut Registry, site: i64, at: i64) {
            for id in crate::site_spawn::npcs_of(w, site) {
                w.roster_mut().leave(id);
                let ev = crate::kill_hooks::KillEvent {
                    victim_character: id,
                    victim_ship: id,
                    ship_type: crate::npc::ANGEL_HIJACKER.type_id,
                    killer: Some(ME),
                    system: w.roster().get(ME).map_or(SYSTEM, |p| p.solar_system_id),
                    npc: Some(&crate::npc::ANGEL_HIJACKER),
                    when: at * BLUE,
                    park: Vec::new(),
                };
                crate::site_spawn::npc_killed(w, &ev);
                w.npcs_mut().remove(id);
            }
        }

        #[test]
        fn a_kill_mission_spawns_its_dungeons_squads_not_a_ring() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            assert!(is_mission_site(site));
            assert_eq!(w.npcs().len(), 0, "the squads come on the site's next step");
            step(&mut w, 1);
            let expected = want(&w, site);
            assert!(expected > 0);
            assert_eq!(w.npcs().len(), expected, "the dungeon's own squad fill");
            let held = crate::site_spawn::npcs_of(&w, site);
            assert_eq!(held.len(), expected);
            // The bookmark and the warp aim are the instance's anchor.
            let pk = pocket(&w, KILLER).expect("where it is");
            assert!(pk.npc_ids.is_empty());
            assert_eq!(
                w.ext::<crate::site::Sites>().get(site).map(|i| i.anchor),
                Some(pk.position)
            );
            assert_eq!(pocket_owner(&w, held[0]), Some(ME));
        }

        #[test]
        fn a_mission_with_no_dungeon_row_falls_back_to_the_ring() {
            // The catalogue has no such dungeon: the old ring of rats.
            let mut w = world();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            assert!(w.ext::<Live>().sites.is_empty());
            assert_eq!(w.npcs().len(), POCKET_NPCS);
        }

        #[test]
        fn a_dungeon_with_no_squads_falls_back_to_the_ring() {
            // Known, but it has no spawn rows: nothing to fill.
            let mut w = world_with(dungeon(Vec::new()));
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            assert!(w.ext::<Live>().sites.is_empty());
            assert!(w.ext::<crate::site::Sites>().iter().next().is_none());
            assert_eq!(w.npcs().len(), POCKET_NPCS);
        }

        #[test]
        fn the_objective_holds_over_the_whole_spawned_set() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            let c = current(&w, ME, KILLER, crate::pyutil::filetime_now()).expect("current");
            assert!(!objective_done(&w, ME, &c));
            // All but one: not met.
            let all = crate::site_spawn::npcs_of(&w, site);
            let (_, rest) = all.split_last().expect("ships");
            for id in rest {
                w.roster_mut().leave(*id);
                w.npcs_mut().remove(*id);
            }
            step(&mut w, 2);
            assert!(!objective_done(&w, ME, &c), "one is still standing");
            kill_all(&mut w, site, 3);
            step(&mut w, 4);
            assert!(objective_done(&w, ME, &c));
            assert_eq!(
                act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
                Some(Change::Completed)
            );
        }

        #[test]
        fn turning_the_mission_in_takes_the_pocket_and_its_instance_away() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            kill_all(&mut w, site, 2);
            step(&mut w, 3);
            assert_eq!(
                act(&mut w, KILLER, Some(ACTION_COMPLETE)).1,
                Some(Change::Completed)
            );
            assert!(w.ext::<Live>().sites.is_empty());
            assert!(pocket(&w, KILLER).is_none());
            assert!(w.ext::<crate::site::Sites>().get(site).is_none());
            assert!(crate::site_spawn::npcs_of(&w, site).is_empty());
        }

        #[test]
        fn an_expired_mission_takes_the_standing_pocket_away_with_its_npcs() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            assert_ne!(w.npcs().len(), 0);
            on_rollover(&mut w, 1_000_000);
            assert!(w.ext::<Live>().sites.is_empty());
            assert!(w.ext::<crate::site::Sites>().get(site).is_none());
            assert_eq!(w.npcs().len(), 0, "the NPCs left with it");
        }

        #[test]
        fn an_abandoned_pocket_is_not_despawned_for_being_vacant() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            let n = w.npcs().len();
            let later = 1 + crate::site_spawn::SITE_VACANT / BLUE + 10;
            step(&mut w, later);
            step(&mut w, later + 5);
            assert_eq!(w.npcs().len(), n);
            assert!(w.ext::<crate::site::Sites>().get(site).is_some());
        }

        #[test]
        fn a_reaccept_reuses_the_standing_pocket_of_the_same_mission() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let first = site_id(&w);
            act(&mut w, KILLER, Some(ACTION_QUIT));
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            assert_eq!(site_id(&w), first);
            assert_eq!(w.ext::<crate::site::Sites>().iter().count(), 1);
        }

        #[test]
        fn two_pilots_get_two_instances_that_never_share_an_anchor() {
            let mut w = two_pilots();
            converse_at(&mut w, Some(ME), KILLER, Some(ACTION_ACCEPT));
            converse_at(&mut w, Some(ME2), KILLER, Some(ACTION_ACCEPT));
            let sites = w.ext::<Live>().sites.clone();
            assert_eq!(sites.len(), 2);
            let anchors: Vec<[f64; 3]> = sites
                .values()
                .map(|s| w.ext::<crate::site::Sites>().get(*s).expect("live").anchor)
                .collect();
            assert_ne!(anchors[0], anchors[1]);
            for a in &anchors {
                assert_ne!(*a, dungeon::POCKET_OFFSET, "the ring's own spot");
            }
            // A freed slot is taken again; a held one never.
            let held = free_slot(&w, pocket(&w, KILLER).unwrap().solar_system_id);
            assert_eq!(held, Some(3));
            drop_pocket(&mut w, (ME, KILLER));
            let again = free_slot(&w, pocket(&w, KILLER).map_or(SYSTEM, |p| p.solar_system_id));
            assert!(again.is_some());
        }

        /// A mission's pocket is its mission's: another mission at the same
        /// agent gets its own dungeon, and the first one's NPCs leave.
        #[test]
        fn another_mission_at_the_same_agent_gets_its_own_pocket() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let first = site_id(&w);
            step(&mut w, 1);
            act(&mut w, KILLER, Some(ACTION_QUIT));
            // The pool redrew another mission for this agent.
            w.ext_mut::<Live>()
                .pocket_mission
                .insert((ME, KILLER), KILL + 77);
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let second = site_id(&w);
            assert_ne!(first, second);
            assert!(w.ext::<crate::site::Sites>().get(first).is_none());
            assert_eq!(w.ext::<crate::site::Sites>().iter().count(), 1);
            assert!(crate::site_spawn::npcs_of(&w, first).is_empty());
        }

        #[test]
        fn quit_then_decline_leaves_no_pocket_behind() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            step(&mut w, 1);
            assert_ne!(w.npcs().len(), 0);
            act(&mut w, KILLER, Some(ACTION_QUIT));
            assert_eq!(
                act(&mut w, KILLER, Some(ACTION_DECLINE)).1,
                Some(Change::Declined)
            );
            assert!(w.ext::<Live>().sites.is_empty());
            assert!(w.ext::<crate::site::Sites>().iter().next().is_none());
            assert_eq!(w.npcs().len(), 0, "no orphan NPCs");
        }

        #[test]
        fn a_redrawn_offer_drops_the_old_pocket() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            step(&mut w, 1);
            act(&mut w, KILLER, Some(ACTION_QUIT));
            // The offer ran out: the next talk redraws.
            let mut p = progress(&w, ME, KILLER);
            p.expires_at = 1;
            save(&mut w, ME, KILLER, &p);
            act(&mut w, KILLER, None);
            assert!(w.ext::<Live>().sites.is_empty());
            assert_eq!(w.npcs().len(), 0, "no orphan NPCs");
        }

        #[test]
        fn a_restart_gives_an_accepted_mission_its_pocket_back() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            // A restart: space and the live state are gone, the row is not.
            crate::site_spawn::discard(&mut w, site);
            *w.ext_mut::<Live>() = Live::default();
            assert!(journal_bookmarks(&w).is_empty());
            revive(&mut w, Some(ME));
            assert_eq!(journal_bookmarks(&w).len(), 1, "the bookmark is back");
            let site = site_id(&w);
            step(&mut w, 1);
            let c = current(&w, ME, KILLER, crate::pyutil::filetime_now()).expect("current");
            assert!(!objective_done(&w, ME, &c));
            kill_all(&mut w, site, 2);
            step(&mut w, 3);
            assert!(objective_done(&w, ME, &c));
        }

        fn journal_bookmarks(w: &Registry) -> Vec<PyValue> {
            let PyValue::List(rows) = journal(w, Some(ME), None) else {
                panic!("a list")
            };
            let PyValue::List(missions) = &rows[0] else {
                panic!("missions")
            };
            missions
                .iter()
                .flat_map(|m| match m {
                    PyValue::Tuple(t) => match &t[6] {
                        PyValue::List(b) => b.clone(),
                        _ => vec![],
                    },
                    _ => vec![],
                })
                .collect()
        }

        /// What the reward prices is what spawns: the same squads (types
        /// and bounty), and no respawn adds to it.
        #[test]
        fn the_spawned_bounty_is_the_priced_bounty() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            let d = w.content().get::<crate::dungeon_data::Table>().dungeons[&DUNGEON].clone();
            let data = w.content().get::<crate::npc_data::Table>().clone();
            let priced: f64 = crate::npc_gen::dungeon_squads(&d, &data, w.world().seed, &[])
                .iter()
                .map(|s| s.squad.bounty)
                .sum();
            // One entry per level-1 kill mission, each naming this dungeon.
            let sums = crate::mission_rewards::spawn_bounty_sums(&w, 1);
            assert!(
                !sums.is_empty() && sums.iter().all(|s| *s == priced),
                "{sums:?}"
            );
            let pocket = w
                .ext::<crate::site_spawn::SiteSpawns>()
                .get(site)
                .cloned()
                .expect("a pocket");
            let spawned: f64 = pocket
                .groups
                .iter()
                .flat_map(|g| g.members.iter())
                .map(|t| data.bounty[t].bounty)
                .sum();
            assert_eq!(spawned, priced);
            // Every squad of the pocket is one the `&[]` draw made.
            let want: Vec<Vec<i64>> =
                crate::npc_gen::dungeon_squads(&d, &data, w.world().seed, &[])
                    .iter()
                    .map(|s| s.squad.members.clone())
                    .collect();
            for g in &pocket.groups {
                assert_eq!(g.members, want[g.spawn]);
            }
            // A squad killed does not come back, whatever its window says.
            assert_eq!(crate::site_spawn::squad_parts(site), Vec::<u64>::new());
            assert_eq!(crate::site_spawn::squad_parts(1), vec![1]);
        }

        /// A squad that waits on a trigger is part of the objective: the
        /// pocket is not clear until it has come and died too.
        #[test]
        fn a_gated_squad_must_come_and_die_before_the_objective_is_met() {
            let gated = Spawn {
                trigger_spawn: true,
                ..spawn(2, 72)
            };
            let mut w = world_with(dungeon(vec![spawn(1, 71), gated]));
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            let c = current(&w, ME, KILLER, crate::pyutil::filetime_now()).expect("current");
            let first = crate::site_spawn::npcs_of(&w, site);
            assert_ne!(first.len(), 0);
            kill_all(&mut w, site, 2);
            assert!(!objective_done(&w, ME, &c), "the gated squad has not come");
            step(&mut w, 3);
            step(&mut w, 4);
            let second = crate::site_spawn::npcs_of(&w, site);
            assert_ne!(second.len(), 0, "the trigger fired");
            assert!(!objective_done(&w, ME, &c));
            kill_all(&mut w, site, 5);
            step(&mut w, 6);
            assert!(objective_done(&w, ME, &c));
        }

        #[test]
        fn a_gm_clear_meets_the_objective_and_removes_the_instance() {
            let mut w = two_squads();
            act(&mut w, KILLER, Some(ACTION_ACCEPT));
            let site = site_id(&w);
            step(&mut w, 1);
            assert_eq!(gm_clear_pocket(&mut w, ME, Some(KILLER)), 1);
            assert_eq!(w.npcs().len(), 0);
            assert!(w.ext::<crate::site::Sites>().get(site).is_none());
            let c = current(&w, ME, KILLER, crate::pyutil::filetime_now()).expect("current");
            assert!(objective_done(&w, ME, &c));
        }
    }
}
