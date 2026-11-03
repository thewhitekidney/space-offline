//! Mission rewards: ISK, the time bonus, LP, items and standing gains, from
//! our own multipliers over a base computed from the client's data.
//!
//! W14 packet CP26b-2 (`docs/content/plan.md` §6.2), spec C4 §4, C5 §5.4 and
//! §5.6, ruling 9 and errata 11 and 13. The multipliers are the authored
//! table `content/agents/rewards.tsv` ([`TABLE`], key `(scale, level)`),
//! **our numbers, not CCP's and not EvEJS's** (ruling 9). Everything else is
//! derived from the client's own data:
//!
//! - **The ISK base of a level** ([`isk_base`]) is the median, over that
//!   level's kill missions (`agents/missions.tsv`), of the bounty sum
//!   (`npc/bounty.tsv`) of the squads CP22's R5 fill puts on each mission's
//!   dungeon spawns (`dungeons/spawns.tsv`, [`crate::npc_gen::dungeon_squads`]).
//!   Erratum 11: a spawn's entity id is not a type id, so the squad fill is
//!   the only way from a spawn to a bounty.
//! - **The reward** ([`reward`]) is `k_isk × base` paid with ref 33, plus a
//!   time bonus of `k_bonus × reward` with ref 34 when the mission is turned
//!   in inside `bonus_minutes` of its acceptance. A mission that carries its
//!   own `missionRewards` (C4 §4: "a mission's own `missionRewards` wins")
//!   pays exactly those, ISK as `typeID 29` and anything else as items.
//! - **LP** ([`Reward::lp`]) is the mission's `fixed_lp` when set (the Omega
//!   value, erratum 13, [A]), else `k_lp × value / ISK_PER_LP` for a Basic
//!   mission (C5 §5.6), where an item's value is its `P_ref`
//!   ([`crate::econ_prices::p_ref`]). Nothing credits LP yet: the wallet is
//!   CP37b's, which reads this number.
//!
//! `mission.rs` calls [`pay`] on a completion and builds the objective
//! pane's `normalRewards`, `bonusRewards` and `loyaltyPoints` from
//! [`Reward`] (its coordinator hunk).

use crate::content::{Boxed, Sources, TableSpec};
use crate::mission_pool::{Agent, Mission};
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "agents/rewards.tsv";

/// Its header, frozen in `docs/content/schema.md` (`content/agents/rewards.tsv`).
pub const HEADER: &str =
    "scale\tlevel\tk_isk\tk_bonus\tk_lp\tbonus_minutes\tagent_gain\tcorp_gain\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/agents/rewards.tsv");

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "mission_rewards",
    load: boxed_load,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let rows = file.parse_rows(parse_row);
    Ok(Box::new(
        Table::new(rows).map_err(|e| format!("{FILE}: {e}"))?,
    ))
}

/// `appConst.refAgentMissionReward`, **[V]** `eve/common/lib/appConst.pyc`
/// (`appConst.py:857`).
pub const REF_AGENT_MISSION_REWARD: i64 = 33;

/// `appConst.refAgentMissionTimeBonusReward`, **[V]**
/// `eve/common/lib/appConst.pyc` (read with `tools/pyc27.py`).
pub const REF_AGENT_MISSION_TIME_BONUS_REWARD: i64 = 34;

/// `typeID 29`, *"Credits"*: how a mission carries ISK (C4 §4) **[V]**.
pub const TYPE_CREDITS: i64 = 29;

/// `R`, the ISK one LP stands for (C5 §5.6). **[A]**: falsified by players
/// converting LP at well over `R` (then cut it).
pub const ISK_PER_LP: f64 = 1_000.0;

/// The levels a row exists for (C4 §2.3: agent levels 1-5).
pub const LEVELS: std::ops::RangeInclusive<i64> = 1..=5;

/// `standingUtil.MAX_STANDING`, **[V]** `standingUtil.py:15`.
const MAX_STANDING: f64 = 10.0;

/// 100-nanosecond ticks in a minute.
const TICKS_PER_MINUTE: i64 = 60 * 10_000_000;

/// C4 §4's two reward scales.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scale {
    /// Kill missions.
    Security,
    /// Courier, mining and trade missions.
    Distribution,
}

impl Scale {
    fn parse(s: &str) -> Result<Scale, String> {
        match s {
            "security" => Ok(Scale::Security),
            "distribution" => Ok(Scale::Distribution),
            _ => Err(format!("scale {s:?} is not security or distribution")),
        }
    }

    /// The scale a mission pays on, by its family; `None` for the families
    /// that pay only what the mission itself carries (`talk`, `other`).
    pub fn of(m: &Mission) -> Option<Scale> {
        match m.family.as_str() {
            "kill" => Some(Scale::Security),
            "courier" | "mining" | "trade" => Some(Scale::Distribution),
            _ => None,
        }
    }
}

/// One `rewards.tsv` row's numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub k_isk: f64,
    pub k_bonus: f64,
    pub k_lp: f64,
    pub bonus_minutes: i64,
    pub agent_gain: f64,
    pub corp_gain: f64,
}

fn parse_row(r: &crate::authored::Row) -> Result<(Scale, i64, Row), String> {
    let level = r.i64(1)?;
    if !LEVELS.contains(&level) {
        return Err(format!("level {level} is not 1-5"));
    }
    let row = Row {
        k_isk: r.f64(2)?,
        k_bonus: r.f64(3)?,
        k_lp: r.f64(4)?,
        bonus_minutes: r.i64(5)?,
        agent_gain: r.f64(6)?,
        corp_gain: r.f64(7)?,
    };
    let finite = [
        row.k_isk,
        row.k_bonus,
        row.k_lp,
        row.agent_gain,
        row.corp_gain,
    ];
    if finite.iter().any(|x| !x.is_finite() || *x < 0.0) || row.bonus_minutes < 0 {
        return Err("a multiplier, gain or window is negative or not finite".into());
    }
    Ok((Scale::parse(r.field(0))?, level, row))
}

/// Every `(scale, level)` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    rows: BTreeMap<(Scale, i64), Row>,
}

impl Table {
    /// The rows, checked.
    ///
    /// # Errors
    ///
    /// A `(scale, level)` listed twice, or one of the ten missing: a
    /// mission at that level would otherwise pay nothing, silently.
    pub fn new(rows: Vec<(Scale, i64, Row)>) -> Result<Table, String> {
        let mut out = BTreeMap::new();
        for (scale, level, row) in rows {
            if out.insert((scale, level), row).is_some() {
                return Err(format!("{scale:?} level {level} is listed twice"));
            }
        }
        for scale in [Scale::Security, Scale::Distribution] {
            for level in LEVELS {
                if !out.contains_key(&(scale, level)) {
                    return Err(format!("{scale:?} level {level} has no row"));
                }
            }
        }
        Ok(Table { rows: out })
    }

    /// Invented multipliers, never the shipped file's.
    pub fn fixture() -> Table {
        let mut rows = Vec::new();
        for (scale, k) in [(Scale::Security, 1.25), (Scale::Distribution, 0.4)] {
            for level in LEVELS {
                let l = level as f64;
                rows.push((
                    scale,
                    level,
                    Row {
                        k_isk: k * l,
                        k_bonus: 0.3,
                        k_lp: 0.7,
                        bonus_minutes: 45 * level,
                        agent_gain: 0.07 * l,
                        corp_gain: 0.03 * l,
                    },
                ));
            }
        }
        Table::new(rows).unwrap_or_else(|e| unreachable!("the fixture is complete: {e}"))
    }

    /// The row for `scale` at `level`, clamped into 1-5.
    pub fn row(&self, scale: Scale, level: i64) -> Row {
        let level = level.clamp(*LEVELS.start(), *LEVELS.end());
        self.rows[&(scale, level)]
    }
}

/// The median of `v`, the mean of the middle two for an even count; `None`
/// when empty.
pub fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// The spawn bounty sum of each kill mission at `level`, in mission order:
/// the summed bounty of every squad [`crate::npc_gen::dungeon_squads`] fills
/// the mission's dungeon with, under the world seed. A mission whose dungeon
/// is unknown, or whose faction has no pool (a sum of 0), is left out: it
/// says nothing about what the level's rats pay.
pub fn spawn_bounty_sums(reg: &Registry, level: i64) -> Vec<f64> {
    let c = reg.content();
    let (Some(pool), Some(dungeons), Some(npc)) = (
        c.try_get::<crate::mission_pool::Table>(),
        c.try_get::<crate::dungeon_data::Table>(),
        c.try_get::<crate::npc_data::Table>(),
    ) else {
        return Vec::new();
    };
    let seed = reg.world().seed;
    let mut by_dungeon: BTreeMap<i64, f64> = BTreeMap::new();
    let mut sums = Vec::new();
    for m in pool.missions() {
        let (true, Some(dungeon)) = (m.family == "kill" && m.level == Some(level), m.dungeon)
        else {
            continue;
        };
        let sum = *by_dungeon.entry(dungeon).or_insert_with(|| {
            dungeons.dungeons.get(&dungeon).map_or(0.0, |d| {
                crate::npc_gen::dungeon_squads(d, npc, seed, &[])
                    .iter()
                    .map(|s| s.squad.bounty)
                    .sum()
            })
        });
        if sum > 0.0 {
            sums.push(sum);
        }
    }
    sums
}

/// **The ISK base of `level`** (C5 §5.4): the median of
/// [`spawn_bounty_sums`]. **[A]** with no such mission (no bundle, or a
/// level with no fillable pocket), R5's whole-dungeon budget for that
/// difficulty, which every squad sum stays under. Falsified by a level whose
/// fallback pays visibly off its neighbours.
pub fn isk_base(reg: &Registry, level: i64) -> f64 {
    median(spawn_bounty_sums(reg, level))
        .unwrap_or_else(|| crate::npc_gen::dungeon_budget(Some(level)))
}

/// What a mission pays, before the bonus window is known.
#[derive(Debug, Clone, PartialEq)]
pub struct Reward {
    /// `(typeID, quantity)`, ISK as [`TYPE_CREDITS`].
    pub normal: Vec<(i64, i64)>,
    /// The same, paid only inside [`Self::bonus_minutes`].
    pub bonus: Vec<(i64, i64)>,
    pub bonus_minutes: i64,
    pub lp: i64,
    pub agent_gain: f64,
    pub corp_gain: f64,
}

impl Reward {
    /// The ISK paid, with or without the bonus.
    #[cfg(test)]
    pub fn isk(&self, in_time: bool) -> i64 {
        let credits = |v: &[(i64, i64)]| -> i64 {
            v.iter()
                .filter(|(t, _)| *t == TYPE_CREDITS)
                .map(|(_, q)| *q)
                .sum()
        };
        credits(&self.normal) + if in_time { credits(&self.bonus) } else { 0 }
    }

    /// `normalRewards`: `[(typeID, quantity, extra)]` **[V]**
    /// (`agentDialogueUtil.py:499`).
    pub fn normal_rewards(&self) -> PyValue {
        PyValue::List(
            self.normal
                .iter()
                .map(|(t, q)| {
                    PyValue::Tuple(vec![PyValue::Int(*t), PyValue::Int(*q), PyValue::None])
                })
                .collect(),
        )
    }

    /// `bonusRewards`: `[(timeRemaining, typeID, quantity, extra,
    /// timeBonusIntervalMin)]` **[V]** (`agentDialogueUtil.py:546`). The
    /// client says "time passed" at `timeRemaining <= 0` and still lists the
    /// reward. `timeRemaining` is in 100-ns ticks **[I]** (the label's
    /// `timeinterval` parameter); before acceptance it is the whole window.
    pub fn bonus_rewards(&self, accepted_at: Option<i64>, now: i64) -> PyValue {
        let window = self.bonus_minutes * TICKS_PER_MINUTE;
        let left = accepted_at.map_or(window, |at| window - (now - at));
        PyValue::List(
            self.bonus
                .iter()
                .map(|(t, q)| {
                    PyValue::Tuple(vec![
                        PyValue::Int(left),
                        PyValue::Int(*t),
                        PyValue::Int(*q),
                        PyValue::None,
                        PyValue::Int(self.bonus_minutes),
                    ])
                })
                .collect(),
        )
    }
}

/// Whether a mission accepted at `accepted_at` and turned in at `now` earns
/// its bonus: inside the window, the window's last tick included.
pub fn bonus_due(accepted_at: i64, now: i64, bonus_minutes: i64) -> bool {
    now >= accepted_at && now - accepted_at <= bonus_minutes * TICKS_PER_MINUTE
}

/// A mission's own `missionRewards` line, when it carries a positive one.
fn own(type_id: Option<i64>, qty: Option<i64>) -> Option<(i64, i64)> {
    match (type_id, qty) {
        (Some(t), Some(q)) if q > 0 => Some((t, q)),
        _ => None,
    }
}

/// What `m`, offered by `agent`, pays. The level is the mission's, else the
/// agent's.
pub fn reward(reg: &Registry, agent: &Agent, m: &Mission) -> Reward {
    let level = m.level.unwrap_or(agent.level);
    let scale = Scale::of(m);
    let row = scale.map(|s| reg.content().get::<Table>().row(s, level));
    let own_normal = own(m.reward.0, m.reward.1);
    let own_bonus = own(m.bonus.0, m.bonus.1);
    let (normal, bonus, bonus_minutes) = if own_normal.is_some() || own_bonus.is_some() {
        (
            own_normal.into_iter().collect(),
            own_bonus.into_iter().collect(),
            m.bonus_minutes
                .or(row.map(|r| r.bonus_minutes))
                .unwrap_or(0),
        )
    } else if let Some(r) = row {
        let isk = (r.k_isk * isk_base(reg, level)).round() as i64;
        let bonus = (r.k_bonus * isk as f64).round() as i64;
        (
            vec![(TYPE_CREDITS, isk)],
            vec![(TYPE_CREDITS, bonus)]
                .into_iter()
                .filter(|(_, q)| *q > 0)
                .collect(),
            r.bonus_minutes,
        )
    } else {
        (Vec::new(), Vec::new(), 0)
    };
    let lp = if m.fixed_lp > 0 {
        m.fixed_lp
    } else {
        match row {
            Some(r) if m.template.starts_with("Basic") => {
                (r.k_lp * value(reg, &normal) / ISK_PER_LP).round() as i64
            }
            _ => 0,
        }
    };
    Reward {
        normal,
        bonus,
        bonus_minutes,
        lp,
        agent_gain: row.map_or(0.0, |r| r.agent_gain),
        corp_gain: row.map_or(0.0, |r| r.corp_gain),
    }
}

/// What `lines` are worth in ISK: credits at face value, an item at its
/// `P_ref` (0 when unpriced).
pub fn value(reg: &Registry, lines: &[(i64, i64)]) -> f64 {
    lines
        .iter()
        .map(|&(t, q)| {
            let each = if t == TYPE_CREDITS {
                1.0
            } else {
                crate::econ_prices::p_ref(reg, t).unwrap_or(0.0)
            };
            each * q as f64
        })
        .sum()
}

/// Pay a completion: the reward (ref 33), the bonus inside its window (ref
/// 34), items into the agent's station hangar (the character's station when
/// the agent has none), and the standing gains toward the agent and its
/// corporation. LP is not credited here (CP37b's wallet reads
/// [`Reward::lp`]).
pub fn pay(w: &mut Registry, ch: i64, agent: &Agent, m: &Mission, accepted_at: i64, now: i64) {
    let r = reward(w, agent, m);
    grant(w, ch, agent, m.id, &r.normal, REF_AGENT_MISSION_REWARD);
    if bonus_due(accepted_at, now, r.bonus_minutes) {
        grant(
            w,
            ch,
            agent,
            m.id,
            &r.bonus,
            REF_AGENT_MISSION_TIME_BONUS_REWARD,
        );
    }
    for (from, gain) in [(agent.id, r.agent_gain), (agent.corp, r.corp_gain)] {
        if gain > 0.0 {
            let now = w.npc_standing(ch, from);
            w.set_npc_standing(ch, from, (now + gain).min(MAX_STANDING));
        }
    }
}

fn grant(
    w: &mut Registry,
    ch: i64,
    agent: &Agent,
    mission: i64,
    lines: &[(i64, i64)],
    ref_id: i64,
) {
    for &(type_id, qty) in lines {
        if type_id == TYPE_CREDITS {
            // CORP-W: the corporation's tax comes off, and the row is the
            // character's own (payer the agent, payee the pilot).
            crate::service::corp_wallet::credit_taxed(w, ch, qty as f64, ref_id, agent.id, mission);
            continue;
        }
        let Some(station) = agent
            .station
            .or_else(|| w.character(ch).and_then(|x| x.station_id))
        else {
            continue;
        };
        if let Ok((ids, _)) = w.spawn_items(&type_id.to_string(), qty, ch, station) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_pool as pool;

    const ME: i64 = 90_000_001;
    const PIRATE: i64 = 599_011;
    const AGENT: i64 = 3_999_201;
    const CORP: i64 = 1_999_201;

    fn agent(level: i64) -> Agent {
        Agent {
            id: AGENT,
            agent_type: pool::TYPE_BASIC,
            division: 24,
            level,
            corp: CORP,
            faction: 599_001,
            station: Some(69_000_001),
            system: Some(39_000_001),
            ..Agent::default()
        }
    }

    fn kill(id: i64, level: i64, dungeon: i64) -> Mission {
        Mission {
            id,
            name_id: id + 1000,
            template: "BasicKill".into(),
            family: "kill".into(),
            level: Some(level),
            enemy_faction: Some(PIRATE),
            dungeon: Some(dungeon),
            ..Mission::default()
        }
    }

    fn courier(id: i64, level: i64) -> Mission {
        Mission {
            id,
            name_id: id + 1000,
            template: "BasicCourier".into(),
            family: "courier".into(),
            level: Some(level),
            ..Mission::default()
        }
    }

    /// Invented dungeons of the fixture pirate (whose only fillable type is
    /// in `npc_data`'s fixture), at two difficulties and spawn counts.
    fn dungeons() -> crate::dungeon_data::Table {
        let spawn = |object| crate::dungeon_data::Spawn {
            room: 1,
            object,
            ..Default::default()
        };
        let d = |id, difficulty, spawns: usize| crate::dungeon_data::Dungeon {
            id,
            archetype: 99,
            faction: Some(PIRATE),
            difficulty: Some(difficulty),
            spawns: (0..spawns as i64).map(|i| spawn(10 + i)).collect(),
            ..Default::default()
        };
        let mut t = crate::dungeon_data::Table::default();
        for dg in [
            d(990_700_001, 1, 1),
            d(990_700_002, 1, 2),
            d(990_700_003, 1, 3),
        ] {
            t.dungeons.insert(dg.id, dg);
        }
        // A dungeon of a faction with no pool: its sum is 0, left out.
        let mut empty = d(990_700_009, 1, 2);
        empty.faction = Some(599_099);
        t.dungeons.insert(empty.id, empty);
        t
    }

    fn missions() -> Vec<Mission> {
        vec![
            kill(999_501, 1, 990_700_001),
            kill(999_502, 1, 990_700_002),
            kill(999_503, 1, 990_700_003),
            kill(999_504, 1, 990_700_009),
            kill(999_505, 2, 990_700_003),
            courier(999_510, 1),
        ]
    }

    fn world() -> Registry {
        let mut c = crate::content::Content::fixture();
        c.insert(
            "mission_pool",
            pool::Table::new(
                vec![agent(1)],
                missions(),
                pool::Policy {
                    parents: &[],
                    drones: 599_021,
                },
            ),
        );
        c.insert("dungeon_data", dungeons());
        c.insert("npc_data", crate::npc_data::Table::fixture());
        c.insert("mission_rewards", Table::fixture());
        Registry::new(crate::model::SandboxConfig::default()).with_content(std::sync::Arc::new(c))
    }

    fn balance(w: &Registry) -> f64 {
        w.character(ME).map(|c| c.balance).unwrap()
    }

    fn row(scale: Scale, level: i64) -> Row {
        Table::fixture().row(scale, level)
    }

    #[test]
    fn the_shipped_file_parses_into_a_complete_table() {
        let (rows, skipped) = crate::authored::parse(SHIPPED, HEADER).unwrap();
        assert!(skipped.is_empty(), "{skipped:?}");
        let parsed: Vec<_> = rows.iter().map(|r| parse_row(r).unwrap()).collect();
        assert!(Table::new(parsed).is_ok());
    }

    #[test]
    fn a_missing_or_doubled_row_is_refused() {
        let one = |level| (Scale::Security, level, row(Scale::Security, 1));
        assert!(Table::new(vec![one(1)]).is_err());
        let mut all: Vec<_> = Table::fixture()
            .rows
            .iter()
            .map(|(&(s, l), r)| (s, l, *r))
            .collect();
        all.push(one(1));
        assert!(Table::new(all).is_err());
    }

    #[test]
    fn the_median_takes_the_middle_or_the_mean_of_the_middle_two() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    /// The base is the median of the fixture level's spawn bounty sums, each
    /// summed here from the squad fill itself, never typed in.
    #[test]
    fn the_isk_base_is_the_median_of_the_levels_spawn_bounty_sums() {
        let w = world();
        let npc = crate::npc_data::Table::fixture();
        let d = dungeons();
        let seed = w.world().seed;
        let expected: Vec<f64> = missions()
            .iter()
            .filter(|m| m.family == "kill" && m.level == Some(1))
            .map(|m| {
                crate::npc_gen::dungeon_squads(&d.dungeons[&m.dungeon.unwrap()], &npc, seed, &[])
                    .iter()
                    .map(|s| s.squad.bounty)
                    .sum::<f64>()
            })
            .filter(|s| *s > 0.0)
            .collect();
        assert_eq!(expected.len(), 3, "the pool-less dungeon is left out");
        assert_eq!(spawn_bounty_sums(&w, 1), expected);
        assert_eq!(isk_base(&w, 1), median(expected).unwrap());
        assert!(isk_base(&w, 1) > 0.0);
    }

    #[test]
    fn a_level_with_no_fillable_pocket_falls_back_to_the_dungeon_budget() {
        let w = world();
        assert!(spawn_bounty_sums(&w, 4).is_empty());
        assert_eq!(isk_base(&w, 4), crate::npc_gen::dungeon_budget(Some(4)));
    }

    #[test]
    fn the_reward_is_k_isk_times_the_base_for_a_fixture_row() {
        let w = world();
        for m in missions() {
            let scale = Scale::of(&m).unwrap();
            let level = m.level.unwrap();
            let r = reward(&w, &agent(1), &m);
            let isk = (row(scale, level).k_isk * isk_base(&w, level)).round() as i64;
            assert_eq!(r.normal, vec![(TYPE_CREDITS, isk)]);
            let bonus = (row(scale, level).k_bonus * isk as f64).round() as i64;
            assert_eq!(r.bonus, vec![(TYPE_CREDITS, bonus)]);
            assert_eq!(r.bonus_minutes, row(scale, level).bonus_minutes);
            assert_eq!(r.isk(true), isk + bonus);
            let lp = (row(scale, level).k_lp * isk as f64 / ISK_PER_LP).round() as i64;
            assert_eq!(r.lp, lp);
        }
    }

    #[test]
    fn a_missions_own_rewards_and_fixed_lp_win() {
        let w = world();
        let item = 990_001; // econ_prices' fixture prices it.
        let m = Mission {
            reward: (Some(item), Some(3)),
            bonus: (Some(TYPE_CREDITS), Some(500)),
            bonus_minutes: Some(7),
            ..courier(999_520, 1)
        };
        let r = reward(&w, &agent(1), &m);
        assert_eq!(r.normal, vec![(item, 3)]);
        assert_eq!(r.bonus, vec![(TYPE_CREDITS, 500)]);
        assert_eq!(r.bonus_minutes, 7);
        let p = crate::econ_prices::p_ref(&w, item).unwrap();
        let k = row(Scale::Distribution, 1).k_lp;
        assert_eq!(
            r.lp,
            (k * 3.0 * p / ISK_PER_LP).round() as i64,
            "items at P_ref"
        );
        let fixed = Mission {
            fixed_lp: 77,
            ..m.clone()
        };
        assert_eq!(reward(&w, &agent(1), &fixed).lp, fixed.fixed_lp);
    }

    #[test]
    fn a_talk_mission_pays_only_what_it_carries() {
        let w = world();
        let m = Mission {
            template: "EpicArcTalkToAgent".into(),
            family: "talk".into(),
            ..courier(999_530, 1)
        };
        let r = reward(&w, &agent(1), &m);
        assert!(r.normal.is_empty() && r.bonus.is_empty());
        assert_eq!((r.lp, r.agent_gain, r.corp_gain), (0, 0.0, 0.0));
    }

    #[test]
    fn the_bonus_applies_only_inside_bonus_minutes() {
        let window = 30 * TICKS_PER_MINUTE;
        assert!(bonus_due(1_000, 1_000, 30));
        assert!(bonus_due(1_000, 1_000 + window, 30));
        assert!(!bonus_due(1_000, 1_000 + window + 1, 30));
        assert!(!bonus_due(1_000, 999, 30), "turned in before acceptance");

        for (late, paid_bonus) in [(0, true), (1, false)] {
            let mut w = world();
            let m = missions()[0].clone();
            let r = reward(&w, &agent(1), &m);
            let before = balance(&w);
            let at = 5_000;
            let now = at + r.bonus_minutes * TICKS_PER_MINUTE + late;
            pay(&mut w, ME, &agent(1), &m, at, now);
            assert_eq!(balance(&w), before + r.isk(paid_bonus) as f64);
        }
    }

    #[test]
    fn the_reward_is_journalled_as_ref_33_and_the_bonus_as_ref_34() {
        let mut w = world();
        let m = missions()[0].clone();
        let r = reward(&w, &agent(1), &m);
        let n = w.journal().entries().len();
        pay(&mut w, ME, &agent(1), &m, 0, 0);
        let rows: Vec<(i64, i64, f64)> = w.journal().entries()[n..]
            .iter()
            .map(|e| (e.entry_type_id, e.reference_id, e.amount))
            .collect();
        assert_eq!(
            rows,
            vec![
                (33, m.id, r.isk(false) as f64),
                (34, m.id, (r.isk(true) - r.isk(false)) as f64),
            ]
        );
        let row = row(Scale::Security, 1);
        assert_eq!(w.npc_standing(ME, AGENT), row.agent_gain);
        assert_eq!(w.npc_standing(ME, CORP), row.corp_gain);
    }

    #[test]
    fn the_bonus_rewards_tuple_counts_down_the_window() {
        let w = world();
        let r = reward(&w, &agent(1), &missions()[0]);
        let window = r.bonus_minutes * TICKS_PER_MINUTE;
        let first = |v: PyValue| match v {
            PyValue::List(l) => match &l[0] {
                PyValue::Tuple(t) => t.clone(),
                o => panic!("a tuple: {o:?}"),
            },
            o => panic!("a list: {o:?}"),
        };
        let t = first(r.bonus_rewards(None, 0));
        assert_eq!(t.len(), 5);
        assert_eq!(t[0], PyValue::Int(window));
        assert_eq!(t[4], PyValue::Int(r.bonus_minutes));
        let t = first(r.bonus_rewards(Some(100), 100 + window + 1));
        assert_eq!(t[0], PyValue::Int(-1), "past the window: <= 0");
        let PyValue::List(n) = r.normal_rewards() else {
            panic!("a list")
        };
        assert_eq!(
            n,
            vec![PyValue::Tuple(vec![
                PyValue::Int(TYPE_CREDITS),
                PyValue::Int(r.isk(false)),
                PyValue::None
            ])]
        );
    }
}
