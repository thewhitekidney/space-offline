//! The storyline trigger and the agents with a fixed mission list.
//!
//! W16 packet CP48 (`docs/content/plan.md` §6.2), spec C4 §6, §8 and packet
//! K7. Two rules, both outside the pool (`crate::mission_pool`, CP19):
//!
//! - **The storyline trigger** (C4 §6). Every [`STORYLINE_EVERY`]-th `Basic*`
//!   mission a character completes for agents of faction F at level L puts
//!   a `GenericStoryline*` offer of level L at the nearest storyline agent
//!   (type [`TYPE_GENERIC_STORYLINE`] or [`TYPE_STORYLINE`]) of faction F,
//!   and pushes `OnAgentMissionChange('offered', agentID)`. The counter is
//!   the [`KIND`] delta, keyed (character, F, L). [`on_complete`] is a
//!   `mission_hooks::HOOKS` row on `Change::Completed`.
//! - **Fixed lists** (C4 §8). An agent whose row carries
//!   `storyline_missions` (career agents, type [`TYPE_CAREER`]; 367 event
//!   agents; one storyline agent) offers that list, never the pool: the
//!   lowest list entry the character has not completed, in play order.
//!   `mission::draw` asks [`fixed_offer`] first. A completed list mission is
//!   a [`DONE_KIND`] row and is never offered again [A].
//!
//! The [`SLOT`] is an index built from the pool's rows alone (CA11): the
//! storyline agents by faction, the `GenericStoryline*` rows by level, and
//! each fixed list in play order. Everything per character is a delta.
//!
//! What this module does **not** do: storyline rewards (C4 §6 says faction
//! standing and no LP; `mission_rewards` pays what it pays any row), the
//! `Important…` briefing labels, and running a storyline row's objective
//! (`mission::objective_done` treats a non-`Basic*` row as met, its own [A]).

use crate::content::{Boxed, Content};
use crate::mission_hooks::MissionEvent;
use crate::mission_pool::{self as pool, Agent, Mission};
use crate::service::Registry;
use crate::world_ext::SlotSpec;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Built from content only (never from deltas, CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "storyline",
    make: |c| Box::new(Index::build(c)) as Boxed,
};

/// The trigger's counter (plan §4.2): `char faction level -> count`. A row
/// is deleted when its count reaches [`STORYLINE_EVERY`].
pub const KIND: &str = "story";

/// A fixed-list mission a character has completed: `char mission_id ->
/// completed_at`. Never expires.
pub const DONE_KIND: &str = "listdone";

/// **[V2]** (C4 §6) How many `Basic*` completions for agents of one faction
/// at one level earn one storyline offer of that level.
pub const STORYLINE_EVERY: u32 = 16;

/// `appConst.agentType*` [V] (`appConst.py:105-117`, C4 §2.1): the two
/// storyline agent types, and the career agents.
pub const TYPE_GENERIC_STORYLINE: i64 = 6;
pub const TYPE_STORYLINE: i64 = 7;
#[cfg(test)]
pub const TYPE_CAREER: i64 = 12;

/// The pool rows the trigger draws from (C4 §2.2, source 4).
pub const GENERIC_PREFIX: &str = "GenericStoryline";

/// `OnAgentMissionChange`'s first argument for a new offer **[V]**
/// (`agents-missions.md` §4.1).
pub const OFFERED: &str = "offered";

/// The slot: what the rows say, indexed once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    /// Faction -> its storyline agents with a solar system, `(agent,
    /// system)` by agent id.
    storyline_agents: BTreeMap<i64, Vec<(i64, i64)>>,
    /// Level -> the `GenericStoryline*` rows of that level, by id.
    generic: BTreeMap<i64, Vec<i64>>,
    /// Agent -> its fixed list in play order ([`play_order`]).
    lists: BTreeMap<i64, Vec<i64>>,
}

impl Index {
    fn build(c: &Content) -> Index {
        c.try_get::<pool::Table>()
            .map_or_else(Index::default, Index::from_table)
    }

    /// The index over one pool table.
    pub fn from_table(t: &pool::Table) -> Index {
        let mut ix = Index::default();
        for a in t.agents() {
            if matches!(a.agent_type, TYPE_GENERIC_STORYLINE | TYPE_STORYLINE)
                && let Some(system) = a.system
            {
                ix.storyline_agents
                    .entry(a.faction)
                    .or_default()
                    .push((a.id, system));
            }
            let list = play_order(t, &a.storyline);
            if !list.is_empty() {
                ix.lists.insert(a.id, list);
            }
        }
        for m in t.missions() {
            if let (true, Some(level)) = (m.template.starts_with(GENERIC_PREFIX), m.level) {
                ix.generic.entry(level).or_default().push(m.id);
            }
        }
        ix
    }

    /// `agent`'s fixed list in play order; empty for a pool agent.
    pub fn list(&self, agent: i64) -> &[i64] {
        self.lists.get(&agent).map_or(&[], |v| &v[..])
    }
}

/// C4 §8: a list is stored in hash order, so play order is the `(k of n)`
/// index, then the mission id; a row with no index sorts after every
/// indexed one [A] (falsifier: a list mixing indexed and unindexed rows
/// whose unindexed row plays first). Ids with no mission row are dropped,
/// and so is a repeat.
pub fn play_order(t: &pool::Table, ids: &[i64]) -> Vec<i64> {
    let mut rows: Vec<&Mission> = ids
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|id| t.mission(*id))
        .collect();
    rows.sort_by_key(|m| (m.chain_index.unwrap_or(i64::MAX), m.id));
    rows.into_iter().map(|m| m.id).collect()
}

/// What an agent's fixed list says about the next offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "fired once the module stopped being exported (IP39); a rename is not IP39's"
)]
pub enum Fixed {
    /// No list: the pool decides.
    NotFixed,
    /// The list's next mission for this character.
    Next(i64),
    /// Every list mission is done: no mission (`NoMission1`).
    Done,
}

/// C4 §8 [A]: the lowest list entry `character` has not completed. Career
/// and event list missions are once per character, whichever agent's list
/// held them (falsifier: a Tranquility career agent offering a completed
/// career mission again).
pub fn fixed_offer(w: &Registry, character: i64, agent: i64) -> Fixed {
    let list = w.ext::<Index>().list(agent);
    if list.is_empty() {
        return Fixed::NotFixed;
    }
    list.iter()
        .find(|m| !list_done(w, character, **m))
        .map_or(Fixed::Done, |m| Fixed::Next(*m))
}

/// Whether `character` has completed the fixed-list mission `mission`.
pub fn list_done(w: &Registry, character: i64, mission: i64) -> bool {
    w.deltas()
        .of(DONE_KIND)
        .contains_key(&[character.to_string(), mission.to_string()][..])
}

fn counter_key(character: i64, faction: i64, level: i64) -> Vec<String> {
    vec![
        character.to_string(),
        faction.to_string(),
        level.to_string(),
    ]
}

/// `character`'s `Basic*` completions toward the next storyline offer of
/// `faction` at `level`.
pub fn count(w: &Registry, character: i64, faction: i64, level: i64) -> u32 {
    w.deltas()
        .of(KIND)
        .get(&counter_key(character, faction, level))
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Whether a completion of `m` counts toward the trigger: a `Basic*` row
/// (C4 §6, "Basic missions, any division").
fn counts(m: &Mission) -> bool {
    m.template.starts_with("Basic")
}

/// The `mission_hooks::HOOKS` row for `Change::Completed`.
pub fn on_complete(w: &mut Registry, ev: &MissionEvent) {
    on_complete_at(w, ev, crate::pyutil::filetime_now());
}

/// [`on_complete`] at `now`: mark a fixed-list mission done, then count a
/// `Basic*` completion toward (character, F, L), F the agent's faction and
/// L its level [A] (falsifier: a capture where an L4 agent's L3-capped
/// offers count toward L3). The [`STORYLINE_EVERY`]-th clears the counter
/// and makes the storyline offer; a world with no agent or no row for it
/// makes none [A], and the counter starts again all the same.
pub fn on_complete_at(w: &mut Registry, ev: &MissionEvent, now: i64) {
    let Some(t) = w.content().try_get::<pool::Table>() else {
        return;
    };
    let (Some(agent), Some(m)) = (
        t.agent(ev.agent_id).cloned(),
        t.mission(ev.mission_id).cloned(),
    ) else {
        return;
    };
    let ch = ev.character_id;
    if w.ext::<Index>().list(agent.id).contains(&m.id) {
        w.deltas_mut().put(
            DONE_KIND,
            vec![ch.to_string(), m.id.to_string()],
            vec![now.to_string()],
        );
    }
    if !counts(&m) {
        return;
    }
    let key = counter_key(ch, agent.faction, agent.level);
    let n = count(w, ch, agent.faction, agent.level) + 1;
    if n < STORYLINE_EVERY {
        w.deltas_mut().put(KIND, key, vec![n.to_string()]);
        return;
    }
    w.deltas_mut().del(KIND, &key);
    offer(w, ch, &agent, now);
}

/// Make the storyline offer `from`'s faction and level earned: a
/// `GenericStoryline*` row of `from.level` whose offering faction is empty
/// or `from.faction`, at the [`nearest`] storyline agent of that faction
/// where `character` has no current mission. Returns `(agent, mission)`.
pub fn offer(w: &mut Registry, character: i64, from: &Agent, now: i64) -> Option<(i64, i64)> {
    let t = w.content().try_get::<pool::Table>()?;
    let ix = w.ext::<Index>();
    let rows: Vec<i64> = ix
        .generic
        .get(&from.level)?
        .iter()
        .copied()
        .filter(|id| {
            t.mission(*id)
                .is_some_and(|m| m.offer_faction.is_none_or(|o| o == from.faction))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let free: Vec<(i64, i64)> = ix
        .storyline_agents
        .get(&from.faction)?
        .iter()
        .copied()
        .filter(|(a, _)| {
            crate::mission::progress(w, character, *a)
                .mission_id
                .is_none()
        })
        .collect();
    let agent = nearest(w, from.system?, &free)?;
    let seq = crate::mission::progress(w, character, agent).offer_seq;
    let mut rng = crate::seed::stream(
        w.world().seed,
        crate::seed::Kind::Offer,
        &[character as u64, agent as u64, seq],
    );
    let mission = rows[rng.range(0, rows.len() as u64) as usize];
    crate::mission::put_offer(w, character, agent, mission, now);
    w.push(crate::notify::Addressed {
        to: Some(vec![character]),
        inner: Box::new(crate::mission::AgentMissionChange {
            change: OFFERED,
            agent_id: agent,
        }),
    });
    Some((agent, mission))
}

/// C4 §6 [A]: the agent nearest `home` by gate jumps over the loaded
/// catalogue; then, for systems the gates do not reach (or no catalogue),
/// by ring: the same system, constellation, region, else; then the lowest
/// agent id. Falsified by a Tranquility storyline offer at an agent further
/// than another of the faction.
pub fn nearest(w: &Registry, home: i64, agents: &[(i64, i64)]) -> Option<i64> {
    let jumps = jumps_from(w, home);
    let uni = w.content().try_get::<crate::celestial::Universe>();
    let ring = |sys: i64| -> u8 {
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
    agents
        .iter()
        .min_by_key(|(a, sys)| (jumps.get(sys).copied().unwrap_or(u32::MAX), ring(*sys), *a))
        .map(|(a, _)| *a)
}

/// Gate jumps from `home` to every system the catalogue's gates reach,
/// breadth first. `home` itself is 0, gates or none.
fn jumps_from(w: &Registry, home: i64) -> BTreeMap<i64, u32> {
    let cat = w.celestials();
    let mut next: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for c in cat.served() {
        if let (Some(from), Some((_, to))) = (cat.system_of(c.id), cat.jump_of(c.id))
            && from != 0
        {
            next.entry(from).or_default().push(to);
        }
    }
    let mut dist = BTreeMap::from([(home, 0u32)]);
    let mut queue = VecDeque::from([home]);
    while let Some(s) = queue.pop_front() {
        let d = dist[&s];
        for &n in next.get(&s).into_iter().flatten() {
            if let std::collections::btree_map::Entry::Vacant(e) = dist.entry(n) {
                e.insert(d + 1);
                queue.push_back(n);
            }
        }
    }
    dist
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::{self, Change, State};
    use crate::model::SandboxConfig;
    use eve_marshal::PyValue;
    use std::sync::Arc;

    const ME: i64 = 90_000_001;
    const YOU: i64 = 90_000_002;
    /// Invented factions (`schema.md` §1.1's unused range).
    const F: i64 = 599_001;
    const OTHER_F: i64 = 599_002;
    /// The fixture universe: S1 and S2 share a constellation, S3 is in the
    /// other region.
    const S1: i64 = 39_000_001;
    const S2: i64 = 39_000_002;
    const S3: i64 = 39_000_003;

    const BASIC: i64 = 3_999_201;
    const BASIC_OTHER_F: i64 = 3_999_202;
    const BASIC_L2: i64 = 3_999_203;
    /// Storyline agents of F: type 7 in S2 (near), type 6 in S3 (far).
    const NEAR: i64 = 3_999_211;
    const FAR: i64 = 3_999_212;
    /// A type-6 agent of the other faction, in S1 itself.
    const WRONG_F: i64 = 3_999_213;
    const CAREER: i64 = 3_999_221;

    const COURIER: i64 = 999_301;
    const COURIER_L2: i64 = 999_302;
    const STORY_OK: i64 = 999_311;
    const STORY_F: i64 = 999_312;
    const STORY_OTHER_F: i64 = 999_313;
    const STORY_L2: i64 = 999_314;
    /// A career list, `(k of 3)`, stored out of play order.
    const CAREER_LIST: [i64; 3] = [999_323, 999_321, 999_322];

    fn table() -> pool::Table {
        let agent = |id, agent_type, division, level, faction, system| Agent {
            id,
            agent_type,
            division,
            level,
            corp: 1_999_001,
            faction,
            station: Some(69_000_001),
            system: Some(system),
            ..Agent::default()
        };
        let row = |id, template: &str, level, offer| Mission {
            id,
            name_id: id + 700,
            template: template.into(),
            family: "courier".into(),
            level: Some(level),
            offer_faction: offer,
            ..Mission::default()
        };
        let part = |id, k| Mission {
            chain_key: Some("fixture career".into()),
            chain_index: Some(k),
            chain_len: Some(3),
            ..row(id, "StorylineCourier", 1, None)
        };
        let mut career = agent(CAREER, TYPE_CAREER, 25, 1, F, S1);
        career.storyline = CAREER_LIST.to_vec();
        pool::Table::new(
            vec![
                agent(BASIC, pool::TYPE_BASIC, 22, 1, F, S1),
                agent(BASIC_OTHER_F, pool::TYPE_BASIC, 22, 1, OTHER_F, S1),
                agent(BASIC_L2, pool::TYPE_BASIC, 22, 2, F, S1),
                agent(NEAR, TYPE_STORYLINE, 24, 1, F, S2),
                agent(FAR, TYPE_GENERIC_STORYLINE, 24, 1, F, S3),
                agent(WRONG_F, TYPE_GENERIC_STORYLINE, 24, 1, OTHER_F, S1),
                career,
            ],
            vec![
                row(COURIER, "BasicCourier", 1, None),
                row(COURIER_L2, "BasicCourier", 2, None),
                row(STORY_OK, "GenericStorylineCourier", 1, None),
                row(STORY_F, "GenericStorylineCourier", 1, Some(F)),
                row(STORY_OTHER_F, "GenericStorylineCourier", 1, Some(OTHER_F)),
                row(STORY_L2, "GenericStorylineCourier", 2, None),
                part(999_321, 1),
                part(999_322, 2),
                part(999_323, 3),
            ],
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        )
    }

    fn world_with(cfg: SandboxConfig) -> Registry {
        let mut c = Content::fixture();
        c.insert("mission_pool", table());
        Registry::new(cfg).with_content(Arc::new(c))
    }

    fn world() -> Registry {
        world_with(SandboxConfig::default())
    }

    fn complete(w: &mut Registry, character: i64, agent: i64, mission: i64) {
        let ev = MissionEvent {
            character_id: character,
            agent_id: agent,
            mission_id: mission,
            change: Change::Completed,
        };
        crate::mission_hooks::fire(&[(Change::Completed, on_complete)], w, &ev);
    }

    /// Every `OnAgentMissionChange` pushed, as `(change, agent, audience)`.
    fn told(w: &mut Registry) -> Vec<(PyValue, PyValue, Option<Vec<i64>>)> {
        w.take_pending()
            .into_iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                (name == "OnAgentMissionChange")
                    .then(|| (args[0].clone(), args[1].clone(), p.audience()))
            })
            .collect()
    }

    fn offered_anywhere(w: &Registry, character: i64) -> Vec<i64> {
        [NEAR, FAR, WRONG_F]
            .into_iter()
            .filter(|a| mission::progress(w, character, *a).mission_id.is_some())
            .collect()
    }

    /// The `STORYLINE_EVERY`-th completion for (F, L) creates a
    /// `GenericStoryline*` offer of level L, offered by F or by nobody, at
    /// the nearest type-6 or type-7 agent of F (not the other faction's agent
    /// in the very system), and pushes `'offered'` with that agent to the
    /// character; the counter starts again.
    #[test]
    fn the_storyline_every_th_completion_offers_at_the_nearest_storyline_agent() {
        let mut w = world();
        for i in 1..STORYLINE_EVERY {
            complete(&mut w, ME, BASIC, COURIER);
            assert_eq!(count(&w, ME, F, 1), i);
        }
        assert!(offered_anywhere(&w, ME).is_empty());
        assert!(told(&mut w).is_empty());

        complete(&mut w, ME, BASIC, COURIER);
        assert_eq!(offered_anywhere(&w, ME), vec![NEAR]);
        let p = mission::progress(&w, ME, NEAR);
        assert_eq!(p.state, State::Offered);
        assert!(
            [STORY_OK, STORY_F].contains(&p.mission_id.unwrap()),
            "level 1, offered by F or nobody: {:?}",
            p.mission_id
        );
        assert_eq!(
            told(&mut w),
            vec![(
                crate::pyutil::bstr(OFFERED),
                PyValue::Int(NEAR),
                Some(vec![ME])
            )]
        );
        assert_eq!(count(&w, ME, F, 1), 0, "the counter starts again");

        // The next earned offer skips the agent already holding one.
        for _ in 0..STORYLINE_EVERY {
            complete(&mut w, ME, BASIC, COURIER);
        }
        assert_eq!(offered_anywhere(&w, ME), vec![NEAR, FAR]);
    }

    /// The counter is keyed (character, F, L): another character, another
    /// faction's agent, another level, and a non-`Basic*` row each leave it
    /// alone, so only the `STORYLINE_EVERY`-th (F, L) completion of mine
    /// offers.
    #[test]
    fn the_counter_is_keyed_by_character_faction_and_level() {
        let mut w = world();
        for _ in 1..STORYLINE_EVERY {
            complete(&mut w, ME, BASIC, COURIER);
        }
        complete(&mut w, YOU, BASIC, COURIER);
        complete(&mut w, ME, BASIC_OTHER_F, COURIER);
        complete(&mut w, ME, BASIC_L2, COURIER_L2);
        complete(&mut w, ME, NEAR, STORY_OK);
        assert_eq!(count(&w, ME, F, 1), STORYLINE_EVERY - 1);
        assert_eq!(count(&w, YOU, F, 1), 1);
        assert_eq!(count(&w, ME, OTHER_F, 1), 1);
        assert_eq!(count(&w, ME, F, 2), 1);
        assert!(offered_anywhere(&w, ME).is_empty());
        assert!(offered_anywhere(&w, YOU).is_empty());

        complete(&mut w, ME, BASIC, COURIER);
        assert_eq!(offered_anywhere(&w, ME), vec![NEAR]);
        assert!(offered_anywhere(&w, YOU).is_empty());
    }

    /// The counter and the done list are deltas: a world rebuilt from the
    /// rows reads the same.
    #[test]
    fn the_counter_and_the_done_list_survive_a_restart() {
        let mut w = world();
        complete(&mut w, ME, BASIC, COURIER);
        complete(&mut w, ME, CAREER, 999_321);
        let mut again = world();
        for kind in [KIND, DONE_KIND] {
            for (k, v) in w.deltas().of(kind).clone() {
                again.deltas_mut().put(kind, k, v);
            }
        }
        assert_eq!(count(&again, ME, F, 1), 1);
        assert!(list_done(&again, ME, 999_321));
    }

    /// A career agent offers its fixed list first, in `(k of n)` order
    /// whatever the stored order, one entry after another as each is
    /// completed, and nothing once the list is done; a pool agent has no
    /// list. The conversation shows the list's first entry.
    #[test]
    fn a_career_agent_offers_its_fixed_list_first() {
        let mut w = world();
        assert_eq!(fixed_offer(&w, ME, BASIC), Fixed::NotFixed);
        assert_eq!(fixed_offer(&w, ME, CAREER), Fixed::Next(999_321));

        let _ = mission::converse_at(&mut w, Some(ME), CAREER, None);
        assert_eq!(
            mission::progress(&w, ME, CAREER).mission_id,
            Some(999_321),
            "(1 of 3), not the first stored"
        );

        complete(&mut w, ME, CAREER, 999_321);
        assert_eq!(fixed_offer(&w, ME, CAREER), Fixed::Next(999_322));
        assert_eq!(fixed_offer(&w, YOU, CAREER), Fixed::Next(999_321));
        complete(&mut w, ME, CAREER, 999_322);
        complete(&mut w, ME, CAREER, 999_323);
        assert_eq!(fixed_offer(&w, ME, CAREER), Fixed::Done);
        assert_eq!(count(&w, ME, F, 1), 0, "a storyline row does not count");
    }

    /// With gates loaded, jumps decide before the rings: S3, one gate from
    /// S1, beats S2, which shares S1's constellation but is two gates away.
    #[test]
    fn gate_jumps_decide_before_the_rings() {
        let ring_only = world();
        let both = [(NEAR, S2), (FAR, S3)];
        assert_eq!(nearest(&ring_only, S1, &both), Some(NEAR));

        let gate = |id: i64, from: i64, dest: i64, to: i64| {
            format!("gate\t{id}\t16\t1000\t0\t0\t0\t{from}\t{dest}\t{to}\n")
        };
        let text = [
            gate(59_000_001, S1, 59_000_002, S3),
            gate(59_000_002, S3, 59_000_001, S1),
            gate(59_000_003, S3, 59_000_004, S2),
            gate(59_000_004, S2, 59_000_003, S3),
        ]
        .concat();
        let gated = world_with(SandboxConfig {
            celestials: crate::celestial::Catalogue::parse(&text),
            ..SandboxConfig::default()
        });
        assert_eq!(jumps_from(&gated, S1).get(&S2), Some(&2));
        assert_eq!(nearest(&gated, S1, &both), Some(FAR));
    }
}
