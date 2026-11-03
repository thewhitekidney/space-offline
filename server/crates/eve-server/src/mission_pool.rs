//! Agent rows and the mission pool: which mission an agent offers.
//!
//! W11 packet CP19 (`docs/content/plan.md` §6.2), spec C4 §3 and packet K2.
//! Pure: no I/O past [`Table::from_bundle`], no clock, no hash order. The
//! offer is a function of `(world seed, character, agent, offer_seq,
//! history)` plus the loaded rows, so a restart reproduces it (C4 §3.5). It
//! draws from `seed::stream(seed, Kind::Offer, ..)`, the one RNG every
//! generator shares (IP1, amendment CA5).
//!
//! What this module does **not** own: the home pirate (`npc_data`, CP20; the
//! caller passes the lookup in), courier destinations and kill sites (C4
//! §3.7, the mission state, CP26a), and rewards (CP26b).

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::seed;
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "mission_pool",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The bundle's rows, or the fixture with no bundle (client data, class B).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::fixture(),
    }))
}

/// `agents/agents.tsv`'s header, from `docs/content/schema.md` (X1).
pub const AGENTS_HEADER: &str = "#agent_id\tagent_type_id\tdivision_id\tlevel\tcorp_id\tfaction_id\tstation_id\tsystem_id\tis_locator\tstoryline_missions";
/// `agents/missions.tsv`'s header, from `docs/content/schema.md` (X1).
pub const MISSIONS_HEADER: &str = "#mission_id\tname_id\ttemplate\tfamily\tlevel\tlevel_src\toffer_faction_id\tenemy_faction_id\tdungeon_id\tobj_type_id\tobj_qty\tobj_volume_total\tobj_group_id\tdrop_item_type_id\texpire_minutes\tagent_type_id\tcorp_id\tchain_key\tchain_index\tchain_len\thas_next_slot\tgift_type_id\tgift_qty\treward_type_id\treward_qty\tbonus_type_id\tbonus_qty\tbonus_minutes\tfixed_lp\thas_standing_rewards\textra_standings\tcontent_tags\tmessages";

/// `appConst.agentType*` [V] (`appConst.py:105-117`, C4 §2.1).
pub const TYPE_BASIC: i64 = 2;
pub const TYPE_RESEARCH: i64 = 4;
pub const TYPE_CONCORD: i64 = 5;
pub const TYPE_EVENT: i64 = 8;
pub const TYPE_FACWAR: i64 = 9;
pub const TYPE_AURA: i64 = 11;
pub const TYPE_HERALDRY: i64 = 13;

/// [A] The highest pool level per kind; an agent above it draws from it
/// (C4 §3.2). Falsified by a capture of an L5 distribution or mining offer.
pub const MAX_LEVEL_SECURITY: i64 = 5;
pub const MAX_LEVEL_OTHER: i64 = 4;
/// [A] How many recent families a new offer avoids (C4 §3.5).
pub const RECENT_FAMILIES: usize = 3;
/// [A] Variant weights (C4 §3.5): the home pirate, Rogue Drones, the rest.
/// Falsified by a Tranquility offer census per agent faction.
pub const W_HOME_PIRATE: u64 = 4;
pub const W_DRONES: u64 = 2;
pub const W_OTHER: u64 = 1;

/// One `agents.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Agent {
    pub id: i64,
    pub agent_type: i64,
    pub division: i64,
    pub level: i64,
    pub corp: i64,
    pub faction: i64,
    pub station: Option<i64>,
    pub system: Option<i64>,
    pub is_locator: bool,
    pub storyline: Vec<i64>,
}

/// One `missions.tsv` row, every column (CP26a and CP26b read it here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mission {
    pub id: i64,
    pub name_id: i64,
    pub template: String,
    pub family: String,
    pub level: Option<i64>,
    pub level_src: String,
    pub offer_faction: Option<i64>,
    pub enemy_faction: Option<i64>,
    pub dungeon: Option<i64>,
    pub obj_type: Option<i64>,
    pub obj_qty: Option<i64>,
    pub obj_volume_total: Option<f64>,
    pub obj_group: Option<i64>,
    pub drop_item_type: Option<i64>,
    pub expire_minutes: Option<i64>,
    pub agent_type: Option<i64>,
    pub corp: Option<i64>,
    pub chain_key: Option<String>,
    pub chain_index: Option<i64>,
    pub chain_len: Option<i64>,
    pub has_next_slot: bool,
    pub gift: (Option<i64>, Option<i64>),
    pub reward: (Option<i64>, Option<i64>),
    pub bonus: (Option<i64>, Option<i64>),
    pub bonus_minutes: Option<i64>,
    pub fixed_lp: i64,
    pub has_standing_rewards: bool,
    pub extra_standings: Vec<(i64, f64)>,
    pub content_tags: Vec<String>,
    pub messages: Vec<(String, i64)>,
}

impl Mission {
    /// `(chain_key, k, n)` for a well-formed "(k of n)" row.
    fn chain(&self) -> Option<(&str, i64, i64)> {
        let (k, n) = (self.chain_index?, self.chain_len?);
        (1 <= k && k <= n).then_some((self.chain_key.as_deref()?, k, n))
    }

    /// [A] The family key. C4 §3.2 groups by the title text, which the
    /// bundle does not carry: a chain is one family per series key, and
    /// anything else is keyed by `name_id`. Falsified if CP13's `check`
    /// finds variants of one title under distinct `name_id`s.
    fn family_key(&self) -> (String, i64) {
        match self.chain() {
            Some((key, _, _)) => (key.to_owned(), 0),
            None => (String::new(), self.name_id),
        }
    }
}

/// The pool an agent draws from (C4 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Security,
    Distribution,
    Mining,
}

/// `(kind, level)`.
pub type Key = (Kind, i64);

/// The faction rules C4 §3.4-§3.5 need, as data so a fixture can invent
/// ids. `Table::from_bundle` uses [`Policy::TRANQUILITY`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    /// `(vassal, parent)`.
    pub parents: &'static [(i64, i64)],
    /// The everywhere-enemy (Rogue Drones).
    pub drones: i64,
}

impl Policy {
    /// [A] Ammatar and Khanid are Amarr's vassals; Rogue Drones is 500021
    /// (C4 §2.4, §3.4). Falsified by a vassal agent offered a mission
    /// against its parent.
    pub const TRANQUILITY: Policy = Policy {
        parents: &[(500_007, 500_003), (500_008, 500_003)],
        drones: 500_021,
    };

    /// The faction's bloc: its parent, or itself.
    pub fn root(&self, faction: i64) -> i64 {
        self.parents
            .iter()
            .find(|(v, _)| *v == faction)
            .map_or(faction, |(_, p)| *p)
    }
}

/// The agents, the missions and the index built over them (C4 §3.2).
#[derive(Debug, Clone)]
pub struct Table {
    agents: BTreeMap<i64, Agent>,
    missions: BTreeMap<i64, Mission>,
    policy: Policy,
    pool: BTreeMap<Key, Vec<i64>>,
    chain_next: BTreeMap<i64, Vec<i64>>,
}

impl Table {
    /// Index the rows. Excluded from the pool (C4 §3.2): FW and Heraldry
    /// rows, kill rows with no dungeon, rows with no level, chain parts past
    /// the first, and chain starts whose chain does not complete.
    pub fn new(agents: Vec<Agent>, missions: Vec<Mission>, policy: Policy) -> Table {
        let agents = agents.into_iter().map(|a| (a.id, a)).collect();
        let missions: BTreeMap<i64, Mission> = missions.into_iter().map(|m| (m.id, m)).collect();
        let mut parts: BTreeMap<(&str, i64, i64), Vec<&Mission>> = BTreeMap::new();
        for m in missions.values() {
            if let Some(c) = m.chain() {
                parts.entry(c).or_default().push(m);
            }
        }
        let mut chain_next = BTreeMap::new();
        for m in missions.values() {
            let Some((key, k, n)) = m.chain().filter(|(_, k, n)| k < n) else {
                continue;
            };
            let kill = |x: &Mission| x.family == "kill";
            let next: Vec<i64> = parts
                .get(&(key, k + 1, n))
                .into_iter()
                .flatten()
                .filter(|x| !(kill(m) && kill(x)) || x.level == m.level)
                .map(|x| x.id)
                .collect();
            chain_next.insert(m.id, next);
        }
        let mut t = Table {
            agents,
            missions,
            policy,
            pool: BTreeMap::new(),
            chain_next,
        };
        let mut pool: BTreeMap<Key, Vec<i64>> = BTreeMap::new();
        for m in t.missions.values() {
            let Some(kind) = kind_of(&m.template) else {
                continue;
            };
            let excluded = matches!(m.agent_type, Some(TYPE_FACWAR | TYPE_HERALDRY))
                || (kind == Kind::Security && m.dungeon.is_none())
                || m.chain()
                    .is_some_and(|(_, k, _)| k > 1 || !t.completes(m.id));
            if let (false, Some(level)) = (excluded, m.level) {
                pool.entry((kind, level)).or_default().push(m.id);
            }
        }
        t.pool = pool;
        t
    }

    /// Whether a chain from `id` reaches its last part.
    fn completes(&self, id: i64) -> bool {
        let Some((_, k, n)) = self.missions.get(&id).and_then(Mission::chain) else {
            return false;
        };
        k == n
            || self
                .chain_next
                .get(&id)
                .into_iter()
                .flatten()
                .any(|&x| self.completes(x))
    }

    pub fn agent(&self, id: i64) -> Option<&Agent> {
        self.agents.get(&id)
    }

    pub fn agents(&self) -> impl Iterator<Item = &Agent> {
        self.agents.values()
    }

    /// Every mission row, pooled or not, by id (CP26b-2's ISK base).
    pub fn missions(&self) -> impl Iterator<Item = &Mission> {
        self.missions.values()
    }

    pub fn mission(&self, id: i64) -> Option<&Mission> {
        self.missions.get(&id)
    }

    /// The agent's pool, or `None` when it is not a pool agent (C4 §3.3):
    /// types Basic and CONCORD, or Event without a fixed list, in divisions
    /// 24, 22 and 23 [V].
    pub fn key(&self, a: &Agent) -> Option<Key> {
        let pool_type = matches!(a.agent_type, TYPE_BASIC | TYPE_CONCORD)
            || (a.agent_type == TYPE_EVENT && a.storyline.is_empty());
        let (kind, max) = match a.division {
            24 => (Kind::Security, MAX_LEVEL_SECURITY),
            22 => (Kind::Distribution, MAX_LEVEL_OTHER),
            23 => (Kind::Mining, MAX_LEVEL_OTHER),
            _ => return None,
        };
        pool_type.then_some((kind, a.level.min(max)))
    }

    /// C4 §3.4. [A] Rules 2 and 3 are read as one: no agent fights its own
    /// bloc (its faction, its parent, or the parent's vassals). CONCORD
    /// agents keep only the rows with no offering faction.
    fn admits(&self, a: &Agent, m: &Mission) -> bool {
        let bloc = self.policy.root(a.faction);
        let offer = m
            .offer_faction
            .is_none_or(|o| a.agent_type != TYPE_CONCORD && (o == a.faction || o == bloc));
        let enemy = m.enemy_faction.is_none_or(|e| self.policy.root(e) != bloc);
        offer && enemy && m.corp.is_none_or(|c| c == a.corp)
    }

    /// Every mission the agent may offer, in id order.
    pub fn candidates(&self, a: &Agent) -> Vec<&Mission> {
        let Some(key) = self.key(a) else {
            return Vec::new();
        };
        let ids = self.pool.get(&key).map_or(&[][..], Vec::as_slice);
        ids.iter()
            .map(|id| &self.missions[id])
            .filter(|m| self.admits(a, m))
            .collect()
    }

    fn weight(&self, a: &Agent, m: &Mission, home: &dyn Fn(i64) -> Option<i64>) -> u64 {
        let pirate = home(a.faction).or_else(|| home(self.policy.root(a.faction)));
        match m.enemy_faction {
            Some(e) if Some(e) == pirate => W_HOME_PIRATE,
            Some(e) if e == self.policy.drones => W_DRONES,
            _ => W_OTHER,
        }
    }

    /// The next offer (C4 §3.5): uniform over families, minus the
    /// [`RECENT_FAMILIES`] most recent in `history` (oldest first; all of
    /// them when that leaves none), then weighted over the family's
    /// variants. `None` is `NoMission1`. `home` is `npc_data::home_pirate`.
    pub fn offer(
        &self,
        seed: u64,
        character: i64,
        agent: i64,
        offer_seq: u64,
        history: &[i64],
        home: &dyn Fn(i64) -> Option<i64>,
    ) -> Option<i64> {
        let a = self.agents.get(&agent)?;
        let cands = self.candidates(a);
        let recent: BTreeSet<_> = history
            .iter()
            .rev()
            .take(RECENT_FAMILIES)
            .filter_map(|id| self.missions.get(id).map(Mission::family_key))
            .collect();
        let all: BTreeSet<_> = cands.iter().map(|m| m.family_key()).collect();
        let fresh: Vec<_> = all.iter().filter(|f| !recent.contains(*f)).collect();
        let fams = if fresh.is_empty() {
            all.iter().collect()
        } else {
            fresh
        };
        let mut rng = seed::stream(
            seed,
            seed::Kind::Offer,
            &[character as u64, agent as u64, offer_seq],
        );
        let fam = fams.get(rng.range(0, fams.len() as u64) as usize)?;
        let vars: Vec<_> = cands
            .into_iter()
            .filter(|m| m.family_key() == **fam)
            .collect();
        let total: u64 = vars.iter().map(|m| self.weight(a, m, home)).sum();
        let mut r = rng.range(0, total);
        vars.into_iter()
            .find(|m| {
                let w = self.weight(a, m, home);
                let hit = r < w;
                r = r.saturating_sub(w);
                hit
            })
            .map(|m| m.id)
    }

    /// The part after `done` (C4 §3.6): the admitted candidate of highest
    /// weight, then the same enemy as `done`, then the lowest id. `None` at
    /// the end of the chain.
    pub fn next_in_chain(
        &self,
        agent: i64,
        done: i64,
        home: &dyn Fn(i64) -> Option<i64>,
    ) -> Option<i64> {
        let (a, prev) = (self.agents.get(&agent)?, self.missions.get(&done)?);
        self.chain_next
            .get(&done)?
            .iter()
            .map(|id| &self.missions[id])
            .filter(|m| self.admits(a, m))
            .max_by_key(|m| {
                (
                    self.weight(a, m, home),
                    m.enemy_faction == prev.enemy_faction,
                    -m.id,
                )
            })
            .map(|m| m.id)
    }

    /// The bundle's rows. An absent file is an empty table (plan §1.3); a
    /// file that cannot be read, or a header or row the schema does not
    /// allow, is an error naming the file, which stops the server with the
    /// binary's normal message, because a stale bundle must fail loudly
    /// (`schema.md` §1.1).
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let read = |rel: &str| match std::fs::read_to_string(b.dir.join(rel)) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{rel}: {e}")),
        };
        let agents = read("agents/agents.tsv")?.map_or(Ok(Vec::new()), |t| parse_agents(&t))?;
        let missions =
            read("agents/missions.tsv")?.map_or(Ok(Vec::new()), |t| parse_missions(&t))?;
        Ok(Table::new(agents, missions, Policy::TRANQUILITY))
    }

    /// Invented rows (ids from `schema.md` §1.1's unused ranges), never a
    /// client value. Factions: 599001-599004 empires, 599007 a vassal of
    /// 599003, 599011-599012 pirates, 599021 drones.
    pub fn fixture() -> Table {
        let agent = |id, division, level, faction| Agent {
            id,
            agent_type: TYPE_BASIC,
            division,
            level,
            corp: 1_999_001,
            faction,
            station: Some(69_000_001),
            system: Some(39_000_001),
            ..Agent::default()
        };
        let kill = |id, name_id, level, offer, enemy| Mission {
            id,
            name_id,
            template: "BasicKill".into(),
            family: "kill".into(),
            level: Some(level),
            offer_faction: offer,
            enemy_faction: Some(enemy),
            dungeon: Some(999_101),
            ..Mission::default()
        };
        let courier = |id, level| Mission {
            id,
            name_id: 990_000 + id,
            template: "BasicCourier".into(),
            family: "courier".into(),
            level: Some(level),
            ..Mission::default()
        };
        let part = |m: Mission, key: &str, k, n| Mission {
            chain_key: Some(key.into()),
            chain_index: Some(k),
            chain_len: Some(n),
            ..m
        };
        let mut missions = vec![
            kill(999_001, 999_901, 1, None, 599_021),
            kill(999_002, 999_902, 1, None, 599_011),
            kill(999_003, 999_903, 1, None, 599_001),
            kill(999_004, 999_904, 1, Some(599_003), 599_012),
            kill(999_005, 999_905, 1, Some(599_001), 599_003),
            kill(999_006, 999_906, 1, None, 599_007),
            part(
                kill(999_020, 999_920, 1, None, 599_011),
                "fixture series",
                1,
                2,
            ),
            part(courier(999_021, 1), "fixture series", 2, 2),
            part(
                kill(999_022, 999_922, 1, None, 599_012),
                "fixture orphan",
                1,
                3,
            ),
            courier(999_030, 4),
            courier(999_031, 5),
        ];
        // One family of many variants, which a mission-uniform draw would favour.
        missions.extend((0..12).map(|i| kill(999_040 + i, 999_940, 1, None, 599_011 + i % 2)));
        let agents = vec![
            agent(3_999_001, 24, 1, 599_001),
            agent(3_999_002, 24, 1, 599_007),
            agent(3_999_003, 22, 5, 599_003),
        ];
        const FIXTURE: Policy = Policy {
            parents: &[(599_007, 599_003)],
            drones: 599_021,
        };
        Table::new(agents, missions, FIXTURE)
    }
}

/// C4 §3.2: which pool a template feeds.
fn kind_of(template: &str) -> Option<Kind> {
    match template {
        "BasicKill" => Some(Kind::Security),
        "BasicCourier" | "BasicTrade" => Some(Kind::Distribution),
        "BasicMining" => Some(Kind::Mining),
        _ => None,
    }
}

/// The three standings `CanUseAgent` takes, each already skill-adjusted.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standings {
    pub faction: f64,
    pub corp: f64,
    pub agent: f64,
}

/// `eveCfg.CanUseAgent` [V] (`eveCfg.py:157-193`, C4 §2.7). Aura agents
/// always; level 1 with `no_l1_check`; else the max standing is at least
/// `m = (level − 1)·2 − 1` and the min is above −2, and a research agent
/// also needs corp standing at least `m − 2`.
pub fn can_use_agent(agent_type: i64, level: i64, s: Standings, no_l1_check: bool) -> bool {
    if agent_type == TYPE_AURA || (level == 1 && agent_type != TYPE_RESEARCH && no_l1_check) {
        return true;
    }
    let m = ((level - 1) * 2 - 1) as f64;
    let (hi, lo) = (
        s.faction.max(s.corp).max(s.agent),
        s.faction.min(s.corp).min(s.agent),
    );
    hi >= m && lo > -2.0 && (agent_type != TYPE_RESEARCH || s.corp >= m - 2.0)
}

/// The data lines of a bundle file, split, after checking its header byte
/// for byte (`schema.md` §1.1). Errors name the file and the line.
fn rows<'a>(file: &str, text: &'a str, header: &str) -> Result<Vec<Vec<&'a str>>, String> {
    let mut lines = text.lines();
    let found = lines.next().unwrap_or("");
    if found != header {
        return Err(format!(
            "{file}: expected header {header:?}, found {found:?}"
        ));
    }
    let width = header.split('\t').count();
    lines
        .enumerate()
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .map(|(n, l)| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() == width).then_some(f).ok_or_else(|| {
                format!(
                    "{file}:{}: {} columns, expected {width}",
                    n + 2,
                    l.split('\t').count()
                )
            })
        })
        .collect()
}

fn opt(s: &str) -> Result<Option<i64>, String> {
    if s.is_empty() {
        Ok(None)
    } else {
        s.parse()
            .map(Some)
            .map_err(|_| format!("bad integer {s:?}"))
    }
}
fn int(s: &str) -> Result<i64, String> {
    opt(s)?.ok_or_else(|| "missing integer".to_owned())
}
fn flag(s: &str) -> Result<bool, String> {
    match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("bad bool {s:?}")),
    }
}
fn text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}
fn pairs(s: &str) -> impl Iterator<Item = Result<(&str, &str), String>> {
    s.split(';').filter(|p| !p.is_empty()).map(|p| {
        p.split_once('=')
            .ok_or_else(|| format!("bad map entry {p:?}"))
    })
}

/// `agents/agents.tsv`.
pub fn parse_agents(text: &str) -> Result<Vec<Agent>, String> {
    rows("agents/agents.tsv", text, AGENTS_HEADER)?
        .into_iter()
        .map(|f| {
            Ok(Agent {
                id: int(f[0])?,
                agent_type: int(f[1])?,
                division: int(f[2])?,
                level: int(f[3])?,
                corp: int(f[4])?,
                faction: int(f[5])?,
                station: opt(f[6])?,
                system: opt(f[7])?,
                is_locator: flag(f[8])?,
                storyline: f[9]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(int)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<_, String>>()
        .map_err(|e| format!("agents/agents.tsv: {e}"))
}

/// `agents/missions.tsv`.
pub fn parse_missions(t: &str) -> Result<Vec<Mission>, String> {
    rows("agents/missions.tsv", t, MISSIONS_HEADER)?
        .into_iter()
        .map(|f| {
            let volume = match f[11] {
                "" => None,
                s => Some(s.parse().map_err(|_| format!("bad float {s:?}"))?),
            };
            Ok(Mission {
                id: int(f[0])?,
                name_id: int(f[1])?,
                template: f[2].to_owned(),
                family: f[3].to_owned(),
                level: opt(f[4])?,
                level_src: f[5].to_owned(),
                offer_faction: opt(f[6])?,
                enemy_faction: opt(f[7])?,
                dungeon: opt(f[8])?,
                obj_type: opt(f[9])?,
                obj_qty: opt(f[10])?,
                obj_volume_total: volume,
                obj_group: opt(f[12])?,
                drop_item_type: opt(f[13])?,
                expire_minutes: opt(f[14])?.filter(|m| *m > 0),
                agent_type: opt(f[15])?,
                corp: opt(f[16])?,
                chain_key: text(f[17]),
                chain_index: opt(f[18])?,
                chain_len: opt(f[19])?,
                has_next_slot: flag(f[20])?,
                gift: (opt(f[21])?, opt(f[22])?),
                reward: (opt(f[23])?, opt(f[24])?),
                bonus: (opt(f[25])?, opt(f[26])?),
                bonus_minutes: opt(f[27])?,
                fixed_lp: int(f[28])?,
                has_standing_rewards: flag(f[29])?,
                extra_standings: pairs(f[30])
                    .map(|p| {
                        let (k, v) = p?;
                        Ok((int(k)?, v.parse().map_err(|_| format!("bad float {v:?}"))?))
                    })
                    .collect::<Result<_, String>>()?,
                content_tags: f[31]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                messages: pairs(f[32])
                    .map(|p| {
                        let (k, v) = p?;
                        Ok((k.to_owned(), int(v)?))
                    })
                    .collect::<Result<_, String>>()?,
            })
        })
        .collect::<Result<_, String>>()
        .map_err(|e| format!("agents/missions.tsv: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALDARI_LIKE: i64 = 3_999_001; // faction 599001
    const VASSAL: i64 = 3_999_002; // faction 599007, parent 599003
    const L5_COURIER: i64 = 3_999_003;

    fn home(f: i64) -> Option<i64> {
        (f == 599_003).then_some(599_012)
    }

    fn draws(t: &Table, agent: i64, n: u64) -> Vec<i64> {
        (0..n)
            .map(|s| t.offer(7, 90_000_001, agent, s, &[], &home).unwrap())
            .collect()
    }

    #[test]
    fn the_same_inputs_give_the_same_offer() {
        let (a, b) = (Table::fixture(), Table::fixture());
        for seq in 0..64 {
            let hist = [999_001, 999_002];
            assert_eq!(
                a.offer(1, 2, CALDARI_LIKE, seq, &hist, &home),
                b.offer(1, 2, CALDARI_LIKE, seq, &hist, &home)
            );
        }
        assert_ne!(
            draws(&a, CALDARI_LIKE, 64)
                .iter()
                .collect::<BTreeSet<_>>()
                .len(),
            1
        );
    }

    /// Family-uniform: with F families, no family's count over N draws
    /// exceeds N/F by more than five binomial standard deviations, even the
    /// family with twelve variants.
    #[test]
    fn no_family_exceeds_its_uniform_share_by_more_than_the_binomial_bound() {
        let t = Table::fixture();
        let a = t.agent(CALDARI_LIKE).unwrap();
        let fams: BTreeSet<_> = t.candidates(a).iter().map(|m| m.family_key()).collect();
        let n = 10_000u64;
        let mut count: BTreeMap<(String, i64), u64> = BTreeMap::new();
        for id in draws(&t, CALDARI_LIKE, n) {
            *count
                .entry(t.mission(id).unwrap().family_key())
                .or_default() += 1;
        }
        let p = 1.0 / fams.len() as f64;
        let bound = n as f64 * p + 5.0 * (n as f64 * p * (1.0 - p)).sqrt();
        assert_eq!(count.len(), fams.len());
        for (fam, c) in count {
            assert!((c as f64) <= bound, "{fam:?}: {c} > {bound}");
        }
    }

    #[test]
    fn an_agent_never_gets_its_own_faction_or_bloc_as_the_enemy() {
        let t = Table::fixture();
        for agent in [CALDARI_LIKE, VASSAL] {
            let a = t.agent(agent).unwrap();
            for id in draws(&t, agent, 2_000) {
                let e = t.mission(id).unwrap().enemy_faction.unwrap();
                assert_ne!(
                    t.policy.root(e),
                    t.policy.root(a.faction),
                    "{agent} got {id}"
                );
            }
        }
    }

    #[test]
    fn a_vassal_agent_draws_its_parents_locked_rows_and_not_others() {
        let t = Table::fixture();
        let a = t.agent(VASSAL).unwrap();
        let offers: BTreeSet<_> = t
            .candidates(a)
            .iter()
            .filter_map(|m| m.offer_faction)
            .collect();
        assert_eq!(offers, BTreeSet::from([599_003]));
        assert!(draws(&t, VASSAL, 500).contains(&999_004));
    }

    #[test]
    fn an_agent_above_the_top_pool_level_draws_from_the_top_level() {
        let t = Table::fixture();
        let a = t.agent(L5_COURIER).unwrap();
        assert!(a.level > MAX_LEVEL_OTHER);
        assert_eq!(t.key(a), Some((Kind::Distribution, MAX_LEVEL_OTHER)));
        for id in draws(&t, L5_COURIER, 50) {
            assert_eq!(t.mission(id).unwrap().level, Some(MAX_LEVEL_OTHER));
        }
    }

    #[test]
    fn part_two_follows_a_chain_start_and_broken_chains_are_dropped() {
        let t = Table::fixture();
        let a = t.agent(CALDARI_LIKE).unwrap();
        let pool: BTreeSet<_> = t.candidates(a).iter().map(|m| m.id).collect();
        assert!(pool.contains(&999_020));
        assert!(!pool.contains(&999_021), "a part 2 is never a fresh offer");
        assert!(
            !pool.contains(&999_022),
            "a chain with no part 2 is dropped"
        );
        assert_eq!(t.next_in_chain(CALDARI_LIKE, 999_020, &home), Some(999_021));
        assert_eq!(t.next_in_chain(CALDARI_LIKE, 999_021, &home), None);
    }

    #[test]
    fn can_use_agent_thresholds_are_l2_1_l3_3_l4_5_l5_7() {
        let at = |level, v: f64| {
            can_use_agent(
                TYPE_BASIC,
                level,
                Standings {
                    faction: v,
                    ..Default::default()
                },
                false,
            )
        };
        for (level, need) in [(2, 1.0), (3, 3.0), (4, 5.0), (5, 7.0)] {
            assert!(at(level, need) && !at(level, need - 0.01), "L{level}");
        }
        let low = Standings {
            faction: 9.0,
            corp: -2.0,
            agent: 0.0,
        };
        assert!(!can_use_agent(TYPE_BASIC, 2, low, false));
        assert!(can_use_agent(TYPE_AURA, 5, low, false));
        let research = Standings {
            faction: 1.0,
            corp: -1.01,
            agent: 0.0,
        };
        assert!(!can_use_agent(TYPE_RESEARCH, 2, research, false));
    }

    /// The header lines are copied from `docs/content/schema.md`; the rows
    /// are its invented examples.
    #[test]
    fn the_schema_headers_and_example_rows_parse_and_other_headers_refuse() {
        let a = parse_agents(&format!(
            "{AGENTS_HEADER}\n# c\n3999001\t2\t24\t1\t1999001\t599002\t69000001\t39000001\t0\t\n"
        ))
        .unwrap();
        assert_eq!(
            (a[0].faction, a[0].station, a[0].storyline.len()),
            (599_002, Some(69_000_001), 0)
        );
        let row = "999001\t999901\tBasicKill\tkill\t1\tdifficulty\t\t599001\t999101\t\t\t\t\t\t\t\t\t\t\t\t0\t\t\t\t\t\t\t\t0\t0\t\t\tmessages.mission.briefing=999902";
        let m = parse_missions(&format!("{MISSIONS_HEADER}\n{row}\n")).unwrap();
        assert_eq!(
            (m[0].enemy_faction, m[0].offer_faction),
            (Some(599_001), None)
        );
        assert_eq!(
            m[0].messages,
            vec![("messages.mission.briefing".to_owned(), 999_902)]
        );
        assert!(
            parse_agents("#agent_id\tlevel\n1\t2\n")
                .unwrap_err()
                .contains("expected header")
        );
        assert!(parse_missions(&format!("{MISSIONS_HEADER}\n1\t2\n")).is_err());
    }

    #[test]
    fn a_bad_header_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("eve-ip1-pool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let b = Bundle { dir: dir.clone() };
        // No files: an empty table, not an error.
        assert!(Table::from_bundle(&b).is_ok());
        std::fs::write(dir.join("agents/agents.tsv"), "#agent_id\tlevel\n1\t2\n").unwrap();
        let e = Table::from_bundle(&b).err();
        // And through the registered spec, as the server loads it.
        let authored = crate::authored::Dir::compiled_only();
        let spec = (TABLE.load)(&Sources::new(Some(&b), &authored)).err();
        let _ = std::fs::remove_dir_all(&dir);
        let e = e.expect("a bad header is an error");
        assert!(
            e.contains("agents/agents.tsv") && e.contains("header"),
            "{e}"
        );
        assert!(spec.is_some_and(|s| s.contains("agents/agents.tsv")));
    }
}
