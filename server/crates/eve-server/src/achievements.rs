//! **Career Program achievements** (#205, AIR NPE scene 2.5.01).
//!
//! The Opportunities window's Career Program cards and the AIR tutorial read
//! a character's achievements through the **public gateway** (package
//! `eve_public.achievement`, `achievement_system/client/repository/
//! message_bus`), not machoNet. Scene 2.5.01 (client graph 683) waits on the
//! `IsAnyCareerAgentMissionCompleted` condition, which scans the career
//! root category for an NPE career achievement that has reached a milestone
//! **[V]** (`nodegraph/client/conditions/achievement.py`, its
//! `validate`). With no achievement state the tutorial cannot leave 2.5.01.
//!
//! This module serves the Career Program achievements that count agent
//! missions, from the bundle's `achievements/agent-missions.tsv` ([`TABLE`];
//! `tools/content_achievements.py` reads them from the client's cached CDN
//! definitions). With no bundle file there are none, and every request keeps
//! the gateway's empty answer.
//!
//! **Off by default.** The gateway arm and the mission hook run only on a
//! server started with the AIR tutorial's `--air-npe` ([`switched_on`]);
//! without it every request keeps the gateway's empty answer, nothing is
//! recorded and nothing is paid.
//!
//! - **Progress.** Each achievement counts the *distinct* missions completed
//!   for agents of its divisions (and levels, when it names any). Definitions
//!   1.7.3 say so (a `SET` of value type `mission` **[V client
//!   definitions]**); 1.8.0 moved the career-agent achievements to a
//!   cumulative counter of completions **[V-TQ data.bin 1.8.0, read
//!   20NOV2026]**. **[A]** the two agree, as a career agent offers each
//!   mission once; falsified by a career achievement on Tranquility that
//!   advances on a repeated mission. The [`on_complete`] mission hook
//!   ([`crate::mission_hooks::HOOKS`]) records one; a milestone is reached
//!   when the count reaches its target.
//! - **What the gateway answers** (`GetRequest`, the category and showcase
//!   scores, `GetUnclaimed`, `Claim`) is shaped as the TQ recording
//!   20261117_122753 (17NOV2026, build 3707965) shows it **[V-TQ]**: 0.3 s
//!   after Enforcer career mission 1 (mission 3120) was turned in,
//!   `GetResponse` held the reached milestone with its 10 category points,
//!   `not_completed`, and the checklist `{mission: ["3120"]}`; the unclaimed
//!   reward expired 730 days after the milestone; the claim paid 25,000 ISK.
//! - **Notices** go out through `gateway_out` on every change: the
//!   achievement's `ProgressedNotice`, then a milestone's `ReachedNotice`
//!   and the category's `ProgressedNotice`, then `CompletedNotice` at the
//!   last milestone (`notice_listener.py` subscribes to all four **[V]**).
//! - **Rewards.** ISK is paid as journal ref 198,
//!   `refAchievementMilestoneReward` **[V]** (`appConst`), with no tax skim
//!   (no `derivedTransactionParentMapping` pair). An item lands in the
//!   active ship's cargo when the pilot is in space, and in the current
//!   station when docked ([`give_item`]).
//!   **[A]** SP and title rewards are not paid yet: logged at the claim.
//!   **Owner decision pending:** these ISK and item rewards add currency and
//!   items to a closed economy. They are paid only with `--air-npe`.
//!
//! **Open:** a V24.02 client with the 1.8.0 definitions in its cache draws
//! the other achievement groups but leaves the career group, and every one of
//! its subcategories, EMPTY, even with 62 rows served (MEASURED 23NOV2026 on a
//! second PC). Why is not known; the career pages may be built from what
//! `category.api.Get` answers rather than from the definitions alone.
//!
//! **[A]** Achievements outside the table keep the gateway's empty answer,
//! as before. A request about one in the table that the pilot has no
//! progress on answers `not_completed` with nothing reached. The showcase
//! score counts these achievements' points only.

use crate::content::{Boxed, Sources, TableSpec};
use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, get_bytes, get_uint, put_bytes, put_varint};
use crate::mission_hooks::MissionEvent;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind (`delta_kinds::KINDS`, owner `achievements`).
pub const KIND: &str = "ach";
/// `appConst.refAchievementMilestoneReward` **[V]**.
pub const REF_ACHIEVEMENT_MILESTONE_REWARD: i64 = 198;
/// An unclaimed reward expires this long after its milestone **[V-TQ]**
/// (730.0 days in the 17NOV2026 recording).
pub const UNCLAIMED_SECS: u64 = 730 * 24 * 3600;
/// The Career Program root category (`AchievementCategoryRoot.CAREER_PROGRAM`)
/// **[V client]**: its score is every career category's.
pub const CAREER_ROOT: [u8; 16] = [
    0xad, 0xf4, 0xa5, 0x91, 0x3a, 0xac, 0x50, 0x8f, 0x94, 0x56, 0xb7, 0x3e, 0xc8, 0x4d, 0x25, 0x3f,
];

/// The bundle file and the header `schema.md` freezes.
pub const FILE: (&str, &str) = (
    "achievements/agent-missions.tsv",
    "#achievement\tname\tcategory\tdivisions\tlevels\tmilestone\tmilestone_name\ttarget\tpoints\treward\tkind\tamount\ttype_id",
);

const PKG: &str = "eve_public.achievement.";
const N_PROGRESSED: &str = "eve_public.achievement.api.ProgressedNotice";
const N_COMPLETED: &str = "eve_public.achievement.api.CompletedNotice";
const N_REACHED: &str = "eve_public.achievement.milestone.api.ReachedNotice";
const N_CATEGORY: &str = "eve_public.achievement.category.api.ProgressedNotice";

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "achievements",
    load,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// The definitions are client data (class B): no shipped copy. No bundle
/// file, no achievements.
fn load(src: &Sources) -> Result<Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Table::default()));
    };
    if b.path(FILE.0).is_none() {
        crate::outln!(
            "[achievements] the bundle has no {}; no achievements",
            FILE.0
        );
    }
    Ok(Box::new(parse(&b.data_lines(FILE.0, FILE.1)?)?))
}

/// What a reward gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Give {
    Isk(i64),
    Item {
        type_id: i64,
        qty: i64,
    },
    /// A kind this server does not pay yet (`SP`, `TITLE`) **[A]**.
    Unpaid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reward {
    pub uuid: [u8; 16],
    pub give: Give,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Milestone {
    pub uuid: [u8; 16],
    pub target: usize,
    pub points: u32,
    pub rewards: Vec<Reward>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Achievement {
    pub uuid: [u8; 16],
    pub divisions: Vec<i64>,
    /// Empty: any level.
    pub levels: Vec<i64>,
    pub category: [u8; 16],
    /// In target order.
    pub milestones: Vec<Milestone>,
}

impl Achievement {
    /// Whether a mission of an agent of `division` and `level` counts.
    pub fn counts(&self, division: i64, level: i64) -> bool {
        self.divisions.contains(&division)
            && (self.levels.is_empty() || self.levels.contains(&level))
    }
}

/// Every achievement the bundle lists, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub list: Vec<Achievement>,
}

impl Table {
    pub fn find(&self, uuid: &[u8]) -> Option<&Achievement> {
        self.list.iter().find(|a| a.uuid[..] == *uuid)
    }
}

/// A dashed or bare hex uuid.
pub fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn hex(u: &[u8; 16]) -> String {
    u.iter().map(|b| format!("{b:02x}")).collect()
}

fn list_i64(s: &str) -> Result<Vec<i64>, ()> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|x| x.parse().map_err(|_| ()))
        .collect()
}

/// The bundle file's data lines, one per reward (a milestone with none has
/// one `NONE` row), grouped into achievements and milestones in file order.
/// A malformed line is an error naming it.
pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
    let mut out: Vec<Achievement> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let l = l.as_ref();
        let bad = || format!("{} data line {}: {l:?}", FILE.0, i + 1);
        let f: Vec<&str> = l.split('\t').collect();
        let [
            ach,
            _name,
            category,
            divisions,
            levels,
            ms,
            _ms_name,
            target,
            points,
            reward,
            kind,
            amount,
            type_id,
        ] = f[..]
        else {
            return Err(bad());
        };
        let ach = parse_uuid(ach).ok_or_else(bad)?;
        let ms = parse_uuid(ms).ok_or_else(bad)?;
        let amount: i64 = amount.parse().map_err(|_| bad())?;
        let reward = match kind {
            "NONE" => None,
            "ISK" => Some(Give::Isk(amount)),
            "ITEM" => Some(Give::Item {
                type_id: type_id.parse().map_err(|_| bad())?,
                qty: amount,
            }),
            other => Some(Give::Unpaid(other.to_owned())),
        }
        .map(|give| {
            Ok::<_, String>(Reward {
                uuid: parse_uuid(reward).ok_or_else(bad)?,
                give,
            })
        })
        .transpose()?;
        let a = match out.iter_mut().position(|a| a.uuid == ach) {
            Some(i) => &mut out[i],
            None => {
                out.push(Achievement {
                    uuid: ach,
                    divisions: list_i64(divisions).map_err(|_| bad())?,
                    levels: list_i64(levels).map_err(|_| bad())?,
                    category: parse_uuid(category).ok_or_else(bad)?,
                    milestones: Vec::new(),
                });
                out.last_mut().ok_or_else(bad)?
            }
        };
        let m = match a.milestones.iter().position(|m| m.uuid == ms) {
            Some(i) => &mut a.milestones[i],
            None => {
                a.milestones.push(Milestone {
                    uuid: ms,
                    target: target.parse().map_err(|_| bad())?,
                    points: points.parse().map_err(|_| bad())?,
                    rewards: Vec::new(),
                });
                a.milestones.last_mut().ok_or_else(bad)?
            }
        };
        m.rewards.extend(reward);
    }
    for a in &mut out {
        a.milestones.sort_by_key(|m| m.target);
    }
    Ok(Table { list: out })
}

/// The loaded table; empty when the content has none.
pub fn defs(reg: &Registry) -> Table {
    reg.content()
        .try_get::<Table>()
        .cloned()
        .unwrap_or_default()
}

// ---- state --------------------------------------------------------------------------

/// `[ch, achievement] -> [mission ids, comma-separated, ascending]`.
fn set_key(ch: i64, a: &[u8; 16]) -> Vec<String> {
    vec![ch.to_string(), hex(a)]
}

/// `[ch, achievement, milestone] -> [reached unix secs, claimed 0|1]`.
fn ms_key(ch: i64, a: &[u8; 16], m: &[u8; 16]) -> Vec<String> {
    vec![ch.to_string(), hex(a), hex(m)]
}

/// The distinct missions counted toward `a`, ascending.
pub fn missions(d: &Deltas, ch: i64, a: &[u8; 16]) -> Vec<i64> {
    d.of(KIND)
        .get(&set_key(ch, a))
        .and_then(|v| v.first())
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default()
}

/// When milestone `m` of `a` was reached, and whether its rewards are claimed.
pub fn reached(d: &Deltas, ch: i64, a: &[u8; 16], m: &[u8; 16]) -> Option<(u64, bool)> {
    let v = d.of(KIND).get(&ms_key(ch, a, m))?;
    let at = v.first()?.parse().ok()?;
    Some((at, v.get(1).is_some_and(|c| c == "1")))
}

/// The points `ch` holds in `category` (every achievement's for
/// [`CAREER_ROOT`] or `None`).
pub fn score(d: &Deltas, t: &Table, ch: i64, category: Option<&[u8]>) -> u32 {
    t.list
        .iter()
        .filter(|a| match category {
            None => true,
            Some(c) => *c == CAREER_ROOT[..] || *c == a.category[..],
        })
        .flat_map(|a| {
            a.milestones
                .iter()
                .filter(move |m| reached(d, ch, &a.uuid, &m.uuid).is_some())
        })
        .map(|m| m.points)
        .sum()
}

// ---- encoding -----------------------------------------------------------------------

fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// An `Identifier { 1 bytes uuid }`.
fn ident(u: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, u);
    out
}

/// `google.protobuf.Timestamp { seconds }`.
fn stamp(secs: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_uint(&mut out, 1, secs);
    out
}

/// `ChecklistProgress { values: [ValueEntry { "mission", ids }] }`.
fn checklist(ids: &[i64]) -> Vec<u8> {
    let mut entry = Vec::new();
    put_bytes(&mut entry, 1, b"mission");
    for id in ids {
        put_bytes(&mut entry, 2, id.to_string().as_bytes());
    }
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &entry);
    out
}

/// `achievement.api.GetResponse` for `a`, fields in the order TQ sent them
/// (5 reached milestones, then the completion, then 4 the checklist).
pub fn get_response(d: &Deltas, ch: i64, a: &Achievement) -> Vec<u8> {
    let mut out = Vec::new();
    let mut last = None;
    let mut all = true;
    for m in &a.milestones {
        match reached(d, ch, &a.uuid, &m.uuid) {
            Some((at, _)) => {
                let mut r = Vec::new();
                put_bytes(&mut r, 1, &ident(&m.uuid));
                put_bytes(&mut r, 2, &stamp(at));
                put_uint(&mut r, 3, u64::from(m.points));
                put_bytes(&mut out, 5, &r);
                last = Some(at);
            }
            None => all = false,
        }
    }
    match last.filter(|_| all) {
        Some(at) => put_bytes(&mut out, 1, &stamp(at)),
        None => put_uint(&mut out, 2, 1),
    }
    let ids = missions(d, ch, &a.uuid);
    if !ids.is_empty() {
        put_bytes(&mut out, 4, &checklist(&ids));
    }
    out
}

/// Every reached milestone of `ch` whose rewards are unclaimed, in table
/// order, with when it was reached.
pub fn unclaimed<'t>(
    d: &Deltas,
    t: &'t Table,
    ch: i64,
) -> Vec<(&'t Achievement, &'t Milestone, u64)> {
    let mut out = Vec::new();
    for a in &t.list {
        for m in &a.milestones {
            if let Some((at, false)) = reached(d, ch, &a.uuid, &m.uuid) {
                out.push((a, m, at));
            }
        }
    }
    out
}

/// `reward.api.GetUnclaimedResponse`: one `Reward` per unclaimed milestone
/// (its reward ids, its expiry, its source, in TQ's field order), and an
/// empty `next_page`.
pub fn unclaimed_response(d: &Deltas, t: &Table, ch: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for (a, m, at) in unclaimed(d, t, ch) {
        let mut r = Vec::new();
        for rw in &m.rewards {
            put_bytes(&mut r, 3, &ident(&rw.uuid));
        }
        put_bytes(&mut r, 4, &stamp(at.saturating_add(UNCLAIMED_SECS)));
        let mut src = Vec::new();
        put_bytes(&mut src, 1, &ident(&a.uuid));
        put_bytes(&mut src, 2, &ident(&m.uuid));
        put_bytes(&mut r, 1, &src);
        put_bytes(&mut out, 1, &r);
    }
    put_bytes(&mut out, 2, &[]);
    out
}

// ---- progress -----------------------------------------------------------------------

fn notify(reg: &mut Registry, ch: i64, ty: &str, payload: Vec<u8>) {
    crate::gateway_out::queue(reg, ty, payload, Target::Character(ch));
}

/// `ch` completed `mission` for an agent of `division` and `level` at `now`
/// (unix seconds): counts toward every achievement that takes such a
/// mission and has not seen it, reaches the milestones it now meets, and
/// tells the client. Returns the milestones reached by this call.
pub fn record(
    reg: &mut Registry,
    ch: i64,
    division: i64,
    level: i64,
    mission: i64,
    now: u64,
) -> Vec<[u8; 16]> {
    let t = defs(reg);
    let mut hit = Vec::new();
    for a in t.list.iter().filter(|a| a.counts(division, level)) {
        let mut ids = missions(reg.deltas(), ch, &a.uuid);
        if ids.contains(&mission) {
            continue;
        }
        ids.push(mission);
        ids.sort_unstable();
        let csv = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        reg.deltas_mut().put(KIND, set_key(ch, &a.uuid), vec![csv]);
        let mut prog = Vec::new();
        put_bytes(&mut prog, 1, &ident(&a.uuid));
        put_bytes(&mut prog, 3, &checklist(&ids));
        notify(reg, ch, N_PROGRESSED, prog);
        let mut reached_now = false;
        for ms in &a.milestones {
            if ms.target > ids.len() || reached(reg.deltas(), ch, &a.uuid, &ms.uuid).is_some() {
                continue;
            }
            reg.deltas_mut().put(
                KIND,
                ms_key(ch, &a.uuid, &ms.uuid),
                vec![now.to_string(), "0".to_owned()],
            );
            hit.push(ms.uuid);
            reached_now = true;
            let mut n = Vec::new();
            put_bytes(&mut n, 1, &ident(&a.uuid));
            put_bytes(&mut n, 2, &ident(&ms.uuid));
            notify(reg, ch, N_REACHED, n);
            let mut c = Vec::new();
            put_bytes(&mut c, 1, &ident(&a.category));
            put_uint(
                &mut c,
                2,
                u64::from(score(reg.deltas(), &t, ch, Some(&a.category))),
            );
            notify(reg, ch, N_CATEGORY, c);
        }
        let done = a
            .milestones
            .iter()
            .all(|ms| reached(reg.deltas(), ch, &a.uuid, &ms.uuid).is_some());
        if done && reached_now {
            let mut c = Vec::new();
            put_bytes(&mut c, 1, &ident(&a.uuid));
            notify(reg, ch, N_COMPLETED, c);
        }
    }
    hit
}

/// Whether achievements run: only with the AIR tutorial's `--air-npe`
/// ([`crate::rules::Rules::air_npe`]). Off, the mission hook records nothing
/// and the gateway arm answers nothing, so no reward is paid.
pub fn switched_on(reg: &Registry) -> bool {
    reg.config().rules.air_npe
}

/// **The `mission_hooks::HOOKS` row**: a mission turned in counts toward the
/// achievements of its agent's division and level, with `--air-npe` only.
pub fn on_complete(reg: &mut Registry, ev: &MissionEvent) {
    if !switched_on(reg) {
        return;
    }
    let Some((division, level)) = reg
        .content()
        .try_get::<crate::mission_pool::Table>()
        .and_then(|t| t.agent(ev.agent_id))
        .map(|a| (a.division, a.level))
    else {
        return;
    };
    let now = crate::game_clock::clock::unix_secs();
    record(reg, ev.character_id, division, level, ev.mission_id, now);
}

// ---- rewards ------------------------------------------------------------------------

/// Pay milestone `m` of `a` to `ch`: ISK as ref 198, an item where
/// [`give_item`] puts it. `false` when it is not reached or already claimed.
pub fn claim(reg: &mut Registry, ch: i64, a: &[u8], m: &[u8]) -> bool {
    use crate::service::corp_wallet::{self, Via};
    let t = defs(reg);
    let Some(ach) = t.find(a) else { return false };
    let Some(ms) = ach.milestones.iter().find(|x| x.uuid[..] == *m) else {
        return false;
    };
    let Some((at, false)) = reached(reg.deltas(), ch, &ach.uuid, &ms.uuid) else {
        return false;
    };
    if reg.character(ch).is_none() {
        return false;
    }
    reg.deltas_mut().put(
        KIND,
        ms_key(ch, &ach.uuid, &ms.uuid),
        vec![at.to_string(), "1".to_owned()],
    );
    for rw in &ms.rewards {
        match &rw.give {
            Give::Isk(isk) => {
                corp_wallet::char_post(
                    reg,
                    Via::Clock,
                    ch,
                    *isk as f64,
                    REF_ACHIEVEMENT_MILESTONE_REWARD,
                    1,
                    ch,
                    0,
                    "",
                );
            }
            Give::Item { type_id, qty } => {
                give_item(reg, ch, *type_id, *qty);
            }
            Give::Unpaid(kind) => crate::outln!(
                "[achievements] {ch}: milestone {} reward {} is {kind}, not paid yet",
                hex(&ms.uuid),
                hex(&rw.uuid)
            ),
        }
    }
    true
}

/// Where an item reward lands.
///
/// - **In space: the active ship's cargo** **[V-TQ]**, recording
///   20261119_160210 / 20261119_160229_pg (19NOV2026, build 3720952, a new
///   pilot claiming nine rewards in space in a destroyer): each item arrived
///   as an `OnItemChange` with `{ixLocationID: 0}`, the ship's id as its
///   location, flag 5, and `('Ship', shipID, 'ShipCargo')` as the third
///   argument.
/// - **Docked: the current station's hangar** **[V client]**, "Your items
///   will go to your current station", the Claim All button's tooltip. Not
///   yet seen on the wire; falsified by a docked claim that lands it in the
///   ship.
///
/// Its pushes go on the clock: a gateway call has no machoNet reply to ride.
/// **[A]** nothing is delivered if the cargo refuses it (full); the
/// milestone stays claimed, and the log says so.
fn give_item(reg: &mut Registry, ch: i64, type_id: i64, qty: i64) -> Vec<i64> {
    match reg.docked_station(ch) {
        Some(station) => into_station(reg, ch, station, type_id, qty),
        None => into_cargo(reg, ch, type_id, qty),
    }
}

fn into_station(reg: &mut Registry, ch: i64, station: i64, type_id: i64, qty: i64) -> Vec<i64> {
    let Ok((ids, _)) = reg.spawn_items(&type_id.to_string(), qty, ch, station) else {
        return Vec::new();
    };
    reg.mark_dirty();
    for &id in &ids {
        if let Some(item) = reg.item(id).cloned() {
            let row = reg.item_row_value(&item);
            reg.schedule(
                std::time::Duration::ZERO,
                crate::notify::ItemChange::spawned(row),
            );
        }
    }
    reg.schedule(
        std::time::Duration::ZERO,
        crate::notify::InvalidateLocations {
            location_ids: vec![station],
        },
    );
    ids
}

fn into_cargo(reg: &mut Registry, ch: i64, type_id: i64, qty: i64) -> Vec<i64> {
    use crate::item::{Change, Edit, Slot};
    let Some(c) = reg.character(ch).cloned() else {
        return Vec::new();
    };
    let ship = crate::fitting::active_ship_of(reg.store(), &c).map_or(c.ship_id, |s| s.0);
    let to = Slot::new(ch, ship, crate::fitting::FLAG_CARGO);
    let changes = match reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: qty,
        to,
    }]) {
        Ok(c) => c,
        Err(e) => {
            crate::outln!(
                "[achievements] {ch}: {qty}x {type_id} not delivered to ship {ship}: {e:?}"
            );
            return Vec::new();
        }
    };
    let cargo = PyValue::Tuple(vec![
        PyValue::Bytes(b"Ship".to_vec()),
        PyValue::Int(ship),
        PyValue::Bytes(b"ShipCargo".to_vec()),
    ]);
    let mut ids = Vec::new();
    for ch_ in &changes {
        let id = ch_.item_id();
        let Some(item) = reg.item(id).cloned() else {
            continue;
        };
        let row = reg.item_row_value(&item);
        let push = match ch_ {
            Change::Created { .. } => crate::notify::ItemChange::spawned(row),
            other => crate::notify::ItemChange {
                row,
                change: other.previous_columns(),
                location: PyValue::None,
            },
        };
        reg.schedule(std::time::Duration::ZERO, push.with_location(cargo.clone()));
        ids.push(id);
    }
    reg.schedule(
        std::time::Duration::ZERO,
        crate::notify::InvalidateLocations {
            location_ids: vec![ship],
        },
    );
    ids
}

// ---- the gateway arm ----------------------------------------------------------------

fn reply(status: u32, name: Option<&str>, body: Vec<u8>) -> Reply {
    Reply {
        status,
        payload: name.map(|n| (n.to_owned(), body)),
    }
}

/// The uuid inside field `f`'s `Identifier`.
fn ident_in(body: &[u8], f: u64) -> Option<Vec<u8>> {
    get_bytes(body, f).and_then(|i| get_bytes(&i, 1))
}

/// **The gateway arm** (`gateway_ext::CHAIN`): the achievement requests this
/// module serves. `None` for everything else, which keeps the gateway's
/// empty answer, and for every request without `--air-npe`
/// ([`switched_on`]).
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    let Ground::Server(w) = g else { return None };
    let on = switched_on(&crate::service::world_read_timed(w, "achievements_on"));
    if !on {
        return None;
    }
    match rest {
        "api.GetRequest" => {
            let id = ident_in(body, 1)?;
            let reg = crate::service::world_read_timed(w, "achievements_get");
            let t = defs(&reg);
            let a = t.find(&id)?;
            let out = get_response(reg.deltas(), character, a);
            Some(reply(
                200,
                Some("eve_public.achievement.api.GetResponse"),
                out,
            ))
        }
        "category.api.GetRequest" => {
            let cat = ident_in(body, 1)?;
            let reg = crate::service::world_read_timed(w, "achievements_category");
            let t = defs(&reg);
            if cat != CAREER_ROOT[..] && !t.list.iter().any(|a| a.category[..] == *cat) {
                return None;
            }
            let mut out = Vec::new();
            put_uint(
                &mut out,
                1,
                u64::from(score(reg.deltas(), &t, character, Some(&cat))),
            );
            Some(reply(
                200,
                Some("eve_public.achievement.category.api.GetResponse"),
                out,
            ))
        }
        "showcase.api.GetCharacterScoreRequest" => {
            let ch = get_bytes(body, 1)
                .and_then(|c| get_uint(&c, 1))
                .map_or(character, |c| c as i64);
            let reg = crate::service::world_read_timed(w, "achievements_score");
            let t = defs(&reg);
            let mut out = Vec::new();
            put_uint(&mut out, 1, u64::from(score(reg.deltas(), &t, ch, None)));
            Some(reply(
                200,
                Some("eve_public.achievement.showcase.api.GetCharacterScoreResponse"),
                out,
            ))
        }
        "reward.api.GetUnclaimedRequest" => {
            let reg = crate::service::world_read_timed(w, "achievements_unclaimed");
            let t = defs(&reg);
            let out = unclaimed_response(reg.deltas(), &t, character);
            Some(reply(
                200,
                Some("eve_public.achievement.reward.api.GetUnclaimedResponse"),
                out,
            ))
        }
        "reward.api.ClaimRequest" => {
            let src = get_bytes(body, 1)?;
            let (a, m) = (ident_in(&src, 1)?, ident_in(&src, 2)?);
            let mut reg = crate::service::world_write_timed(w, "achievements_claim");
            defs(&reg).find(&a)?;
            let ok = claim(&mut reg, character, &a, &m);
            let batch = crate::gateway_out::take(&mut reg);
            drop(reg);
            crate::gateway_out::flush(batch);
            Some(if ok {
                reply(
                    200,
                    Some("eve_public.achievement.reward.api.ClaimResponse"),
                    Vec::new(),
                )
            } else {
                reply(404, None, Vec::new())
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Invented definitions: two achievements of invented divisions, the
    // first with two milestones (the second paying an item), the second
    // limited to level 2.
    const A1: &str = "11111111-1111-5111-8111-111111111111";
    const A1_M1: &str = "22222222-2222-5222-8222-222222222222";
    const A1_M2: &str = "33333333-3333-5333-8333-333333333333";
    const A2: &str = "44444444-4444-5444-8444-444444444444";
    const A2_M1: &str = "55555555-5555-5555-8555-555555555555";
    const CAT: &str = "66666666-6666-5666-8666-666666666666";
    const DIV: i64 = 98;
    const DIV2: i64 = 99;
    const ITEM_TYPE: i64 = 1230;

    fn rows() -> Vec<String> {
        let r = |a: &str,
                 divs: &str,
                 lv: &str,
                 m: &str,
                 target: u32,
                 rw: &str,
                 kind: &str,
                 amount: i64,
                 ty: &str| {
            format!(
                "{a}\tinvented\t{CAT}\t{divs}\t{lv}\t{m}\tinvented_m\t{target}\t7\t{rw}\t{kind}\t{amount}\t{ty}"
            )
        };
        vec![
            r(
                A1,
                "98",
                "",
                A1_M1,
                1,
                "77777777-7777-5777-8777-777777777771",
                "ISK",
                1_111,
                "",
            ),
            r(
                A1,
                "98",
                "",
                A1_M2,
                2,
                "77777777-7777-5777-8777-777777777772",
                "ISK",
                2_222,
                "",
            ),
            r(
                A1,
                "98",
                "",
                A1_M2,
                2,
                "77777777-7777-5777-8777-777777777773",
                "ITEM",
                1,
                &ITEM_TYPE.to_string(),
            ),
            r(
                A2,
                "99",
                "2",
                A2_M1,
                1,
                "77777777-7777-5777-8777-777777777774",
                "SP",
                500,
                "",
            ),
        ]
    }

    fn u(s: &str) -> [u8; 16] {
        parse_uuid(s).expect("uuid")
    }

    fn world_with(ch: i64) -> Registry {
        let reg = crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![crate::model::Character {
                character_id: ch,
                owner_user_id: Some(11),
                station_id: Some(60_012_526),
                ..Default::default()
            }];
            cfg.catch_all = None;
        });
        let mut content = crate::content::Content::fixture();
        content.insert(TABLE.name, parse(&rows()).expect("invented rows parse"));
        reg.with_content(Arc::new(content))
    }

    /// The bundle rows group into achievements and milestones, in target
    /// order, and a malformed line is refused.
    #[test]
    fn bundle_rows_group_into_achievements() {
        let t = parse(&rows()).expect("parse");
        assert_eq!(t.list.len(), 2);
        let a1 = t.find(&u(A1)).expect("a1");
        assert_eq!(a1.divisions, vec![DIV]);
        assert!(a1.levels.is_empty());
        assert_eq!(a1.milestones.len(), 2);
        assert_eq!(a1.milestones[1].rewards.len(), 2);
        assert_eq!(
            a1.milestones[1].rewards[1].give,
            Give::Item {
                type_id: ITEM_TYPE,
                qty: 1
            }
        );
        assert_eq!(t.find(&u(A2)).expect("a2").levels, vec![2]);
        assert!(parse(&["x\ty"]).is_err());
    }

    /// A turned-in mission reaches the first milestone once; the same
    /// mission again counts nothing; the level filter holds.
    #[test]
    fn a_career_mission_reaches_its_first_milestone_once() {
        let ch = 90_000_005;
        let mut w = world_with(ch);
        let hit = record(&mut w, ch, DIV, 1, 990_120, 1_794_951_472);
        assert_eq!(hit, vec![u(A1_M1)]);
        assert_eq!(missions(w.deltas(), ch, &u(A1)), vec![990_120]);
        assert_eq!(
            reached(w.deltas(), ch, &u(A1), &u(A1_M1)),
            Some((1_794_951_472, false))
        );
        assert!(record(&mut w, ch, DIV, 1, 990_120, 1_794_951_999).is_empty());
        assert_eq!(score(w.deltas(), &defs(&w), ch, Some(&CAREER_ROOT)), 7);
        assert!(
            record(&mut w, ch, DIV2, 1, 4000, 1).is_empty(),
            "level 1 does not count for A2"
        );
        assert_eq!(record(&mut w, ch, DIV2, 2, 4000, 1), vec![u(A2_M1)]);
        let q = crate::gateway_out::take(&mut w);
        let kinds: Vec<&str> = q.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                N_PROGRESSED,
                N_REACHED,
                N_CATEGORY,
                N_PROGRESSED,
                N_REACHED,
                N_CATEGORY,
                N_COMPLETED
            ]
        );
    }

    /// The `GetResponse` after one mission has TQ's shape (17NOV2026
    /// recording): the reached milestone with its points, `not_completed`,
    /// and the checklist `{mission: [id]}`, field for field in the
    /// recording's order (its timestamps also carried nanoseconds).
    #[test]
    fn the_get_response_has_the_recordings_shape() {
        let ch = 90_000_005;
        let mut w = world_with(ch);
        record(&mut w, ch, DIV, 1, 990_120, 1_794_951_472);
        let t = defs(&w);
        let got = get_response(w.deltas(), ch, t.find(&u(A1)).expect("a1"));
        let mut want = vec![0x2a, 0x1e, 0x0a, 0x12, 0x0a, 0x10];
        want.extend_from_slice(&u(A1_M1));
        want.extend_from_slice(&[0x12, 0x06, 0x08, 0xb0, 0x92, 0xf3, 0xd7, 0x06, 0x18, 0x07]);
        want.extend_from_slice(&[0x10, 0x01]);
        want.extend_from_slice(&[0x22, 0x13, 0x0a, 0x11, 0x0a, 0x07]);
        want.extend_from_slice(b"mission");
        want.extend_from_slice(&[0x12, 0x06]);
        want.extend_from_slice(b"990120");
        assert_eq!(got, want);
    }

    /// A milestone's rewards are claimable once: ISK as ref 198 and the item
    /// into the station; then the unclaimed list is empty.
    #[test]
    fn a_claim_pays_the_milestone_once() {
        let ch = 90_000_005;
        let mut w = world_with(ch);
        record(&mut w, ch, DIV, 1, 990_120, 10);
        record(&mut w, ch, DIV, 1, 990_157, 20);
        let t = defs(&w);
        assert_eq!(unclaimed(w.deltas(), &t, ch).len(), 2);
        let before = w.character(ch).expect("pilot").balance;
        assert!(claim(&mut w, ch, &u(A1), &u(A1_M1)));
        assert!(claim(&mut w, ch, &u(A1), &u(A1_M2)));
        assert_eq!(w.character(ch).expect("pilot").balance - before, 3_333.0);
        assert!(!claim(&mut w, ch, &u(A1), &u(A1_M1)));
        assert!(unclaimed(w.deltas(), &t, ch).is_empty());
        assert_eq!(unclaimed_response(w.deltas(), &t, ch), vec![0x12, 0x00]);
    }

    /// An item reward lands in the pilot's station, owned by them.
    #[test]
    fn an_item_reward_lands_in_the_pilots_station() {
        let ch = 90_000_005;
        let mut w = world_with(ch);
        let station = w.character(ch).and_then(|c| c.station_id).expect("docked");
        let ids = give_item(&mut w, ch, ITEM_TYPE, 1);
        assert_eq!(ids.len(), 1);
        let item = w.item(ids[0]).expect("spawned");
        assert_eq!(
            (item.type_id, item.owner_id, item.location_id),
            (ITEM_TYPE, ch, station)
        );
    }

    /// In space, an item reward lands in the active ship's cargo, flag 5.
    #[test]
    fn in_space_an_item_reward_lands_in_the_ships_cargo() {
        let ch = 90_000_005;
        let mut w = world_with(ch);
        let ship = w.character(ch).expect("pilot").ship_id;
        w.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ch,
                ship,
                30_000_142,
                crate::space_state::Motion::arrived([0.0, 0.0, 0.0], [0.0; 3]),
            ));
        assert_eq!(w.docked_station(ch), None);
        let ids = give_item(&mut w, ch, ITEM_TYPE, 1);
        assert_eq!(ids.len(), 1);
        let item = w.item(ids[0]).expect("spawned");
        assert_eq!(
            (item.type_id, item.owner_id, item.location_id, item.flag_id),
            (ITEM_TYPE, ch, ship, crate::fitting::FLAG_CARGO)
        );
    }

    /// With no definitions every request falls through to the gateway's
    /// empty answer, and nothing is recorded.
    #[test]
    fn no_definitions_means_no_state() {
        let ch = 90_000_005;
        let mut w = crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![crate::model::Character {
                character_id: ch,
                ..Default::default()
            }];
            cfg.catch_all = None;
        });
        assert!(record(&mut w, ch, DIV, 1, 990_120, 1).is_empty());
        assert!(w.deltas().of(KIND).is_empty());
    }

    /// Without `--air-npe` the gateway arm answers nothing, definitions or
    /// not, so the request keeps the gateway's empty answer; with it, the
    /// career root's category is served.
    #[test]
    fn the_gateway_answers_only_with_air_npe() {
        let ch = 90_000_005;
        let w = world_with(ch);
        assert!(!w.config().rules.air_npe, "off by default");
        let mut body = Vec::new();
        put_bytes(&mut body, 1, &ident(&CAREER_ROOT));
        let ty = "eve_public.achievement.category.api.GetRequest";
        let g = Ground::Server(Arc::new(std::sync::RwLock::new(w)));
        assert!(gateway(ty, &body, ch, &g).is_none());
        let Ground::Server(lock) = &g else {
            panic!("a server ground")
        };
        lock.write().expect("world lock").rules_mut().air_npe = true;
        let r = gateway(ty, &body, ch, &g).expect("served with --air-npe");
        assert_eq!(r.status, 200);
    }
}
