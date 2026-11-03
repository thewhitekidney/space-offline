//! **Access groups** (`ownerGroupManager`; lane GAP-ACCESS, spec
//! `handback/evejs-gap-svcA/report.md` B7): the lists the Access Lists
//! window edits and the bookmark folder window picks from.
//!
//! The wire is in `service/owner_groups.rs`; this file is the model.
//!
//! - A group is `groupID creatorID name description` plus members. A member
//!   is a character, a corporation or an alliance, with a type: MEMBER 0,
//!   MANAGER 1, ADMIN 2, EXCLUDED 3 (`ownergroups/ownergroupConst.py`) [V].
//!   Member id [`PUBLIC_MEMBER_ID`] (0) means everyone [V].
//! - A pilot's **effective** type is the best of the rows that match the
//!   character, its corporation, its alliance and the public row; an EXCLUDED
//!   row on the character or its corporation or alliance wins over them all
//!   [A: EVE's usual deny-wins; falsified by a retail excluded corp member
//!   who still sees the list].
//! - Limits [V]: 1000 groups to manage, 10000 entries per group; the name is
//!   at most 100 characters and the description 3900 [V as numbers].
//! - Who may do what is ours [A]: an admin everything, a manager renames
//!   and adds, retypes or removes members and exclusions (never a manager or
//!   an admin), a member only reads. The last admin cannot be removed or
//!   demoted, so a group is never left without one.
//!
//! Stored as delta kind [`KIND`]:
//! `"g" group -> creator name description`,
//! `"m" group member -> type`, `"next" -> id` (an id is never reused).

use crate::notify::Push;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "ogroup";

pub const MEMBER: i64 = 0;
pub const MANAGER: i64 = 1;
pub const ADMIN: i64 = 2;
pub const EXCLUDED: i64 = 3;
/// "Not a member".
pub const NONE: i64 = -1;
/// A member row for everyone.
pub const PUBLIC_MEMBER_ID: i64 = 0;

pub const MAX_GROUPS_TO_MANAGE: usize = 1000;
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_DESC_CHARS: usize = 3900;

/// `OnOwnerGroupsUpdated` change codes [V] (`accessGroupsController.py:292`).
pub const GROUP_CREATED: i64 = 1;
pub const MEMBER_ADDED: i64 = 2;
pub const MEMBERTYPE_CHANGED: i64 = 3;
pub const MEMBER_REMOVED: i64 = 4;
pub const NAME_CHANGED: i64 = 5;
pub const DESCRIPTION_CHANGED: i64 = 6;
pub const DELETED: i64 = 7;
pub const CHANGED_FOR_LISTENER: i64 = 8;

/// A group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub group_id: i64,
    pub creator_id: i64,
    pub name: String,
    pub description: String,
}

/// Why a call was refused; the wire turns it into words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NoSuchGroup,
    NotAllowed,
    BadName,
    TooMany,
}

/// A failed member of a batch call: `(memberID, reason)`.
pub type Failure = (i64, &'static str);

fn gkey(gid: i64) -> Vec<String> {
    vec!["g".to_owned(), gid.to_string()]
}

fn mkey(gid: i64, member: i64) -> Vec<String> {
    vec!["m".to_owned(), gid.to_string(), member.to_string()]
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn group_from(k: &[String], v: &[String]) -> Option<Group> {
    let [_, gid] = k else { return None };
    Some(Group {
        group_id: gid.parse().ok()?,
        creator_id: v.first()?.parse().ok()?,
        name: v.get(1).cloned().unwrap_or_default(),
        description: v.get(2).cloned().unwrap_or_default(),
    })
}

/// One group.
pub fn group(reg: &Registry, gid: i64) -> Option<Group> {
    let k = gkey(gid);
    reg.deltas()
        .of(KIND)
        .get(&k)
        .and_then(|v| group_from(&k, v))
}

/// Every group, by id.
pub fn all(reg: &Registry) -> Vec<Group> {
    let mut out: Vec<Group> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.first().map(String::as_str) == Some("g"))
        .filter_map(|(k, v)| group_from(k, v))
        .collect();
    out.sort_by_key(|g| g.group_id);
    out
}

/// The members of a group, `member -> type`.
pub fn members(reg: &Registry, gid: i64) -> BTreeMap<i64, i64> {
    let g = gid.to_string();
    reg.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| match k.as_slice() {
            [t, gg, m] if t == "m" && *gg == g => Some((m.parse().ok()?, v.first()?.parse().ok()?)),
            _ => None,
        })
        .collect()
}

/// The ids that stand for `ch` in a member list: itself, its corporation, its
/// alliance and everyone.
fn identities(reg: &Registry, ch: i64) -> Vec<i64> {
    let mut ids = vec![ch, PUBLIC_MEMBER_ID];
    if let Some(c) = reg.character(ch) {
        ids.push(c.corporation_id);
        ids.extend(c.alliance_id);
    }
    ids
}

fn rank(t: i64) -> i64 {
    match t {
        ADMIN => 3,
        MANAGER => 2,
        MEMBER => 1,
        _ => 0,
    }
}

/// The effective type of `ch` in a group's member map, [`NONE`] when absent
/// or excluded.
pub fn effective_in(reg: &Registry, mem: &BTreeMap<i64, i64>, ch: i64) -> i64 {
    let mut best = NONE;
    for id in identities(reg, ch) {
        match mem.get(&id) {
            Some(&EXCLUDED) => return NONE,
            Some(&t) if rank(t) > rank(best) => best = t,
            _ => {}
        }
    }
    best
}

/// The effective type of `ch` in group `gid`.
pub fn effective(reg: &Registry, gid: i64, ch: i64) -> i64 {
    effective_in(reg, &members(reg, gid), ch)
}

/// The groups `ch` belongs to, with its type in each (`GetMyGroups`).
pub fn mine(reg: &Registry, ch: i64) -> Vec<(Group, i64)> {
    all(reg)
        .into_iter()
        .filter_map(|g| {
            let t = effective(reg, g.group_id, ch);
            (t >= MEMBER).then_some((g, t))
        })
        .collect()
}

fn group_value(g: &Group, membership: i64) -> PyValue {
    crate::pyutil::keyval(vec![
        ("groupID", PyValue::Int(g.group_id)),
        ("creatorID", PyValue::Int(g.creator_id)),
        ("name", PyValue::Unicode(g.name.clone())),
        ("description", PyValue::Unicode(g.description.clone())),
        ("membershipType", PyValue::Int(membership)),
    ])
}

/// A group as `ch` sees it: a KeyVal (the client assigns to `.name`).
pub fn group_value_for(reg: &Registry, g: &Group, ch: i64) -> PyValue {
    group_value(g, effective(reg, g.group_id, ch))
}

/// `GetMyGroups`.
pub fn mine_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::List(
        mine(reg, ch)
            .iter()
            .map(|(g, t)| group_value(g, *t))
            .collect(),
    )
}

/// `GetMembers`: `[KeyVal(memberID, membershipType)]`.
pub fn members_value(mem: &BTreeMap<i64, i64>) -> PyValue {
    PyValue::List(
        mem.iter()
            .map(|(m, t)| {
                crate::pyutil::keyval(vec![
                    ("memberID", PyValue::Int(*m)),
                    ("membershipType", PyValue::Int(*t)),
                ])
            })
            .collect(),
    )
}

fn set_member(reg: &mut Registry, gid: i64, member: i64, t: i64) {
    reg.deltas_mut()
        .put(KIND, mkey(gid, member), vec![t.to_string()]);
}

/// `OnOwnerGroupsUpdated({groupID: [codes]})` for one character.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GroupsUpdated {
    pub to: i64,
    pub changed: Vec<(i64, Vec<i64>)>,
}

impl Push for GroupsUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let dict = self
            .changed
            .iter()
            .map(|(g, codes)| {
                (
                    PyValue::List(codes.iter().map(|c| PyValue::Int(*c)).collect()),
                    PyValue::Int(*g),
                )
            })
            .collect();
        ("OnOwnerGroupsUpdated", vec![PyValue::Dict(dict)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// Tell the characters named in `mem` (never `actor`) that `gid` changed:
/// `code` to all of them, [`CHANGED_FOR_LISTENER`] to each of `own` instead.
fn tell(reg: &mut Registry, actor: i64, gid: i64, code: i64, mem: &[i64], own: &[i64]) {
    let mut seen = Vec::new();
    for &m in mem.iter().chain(own) {
        if m == actor || seen.contains(&m) || reg.character(m).is_none() {
            continue;
        }
        seen.push(m);
        let c = if own.contains(&m) {
            CHANGED_FOR_LISTENER
        } else {
            code
        };
        reg.push(GroupsUpdated {
            to: m,
            changed: vec![(gid, vec![c])],
        });
    }
}

fn mark(reg: &mut Registry) {
    reg.mark_dirty();
}

/// `Create(name, description)`; the actor becomes its first admin.
pub fn create(reg: &mut Registry, actor: i64, name: &str, desc: &str) -> Result<Group, Refusal> {
    let name = clip(name.trim(), MAX_NAME_CHARS);
    if name.is_empty() {
        return Err(Refusal::BadName);
    }
    if all(reg).iter().filter(|g| g.creator_id == actor).count() >= MAX_GROUPS_TO_MANAGE {
        return Err(Refusal::TooMany);
    }
    let next_key = vec!["next".to_owned()];
    let stored = reg
        .deltas()
        .of(KIND)
        .get(&next_key)
        .and_then(|v| v.first())
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
    let used = all(reg).last().map_or(0, |g| g.group_id);
    let gid = stored.max(used) + 1;
    let desc = clip(desc, MAX_DESC_CHARS);
    reg.deltas_mut().put(KIND, next_key, vec![gid.to_string()]);
    reg.deltas_mut().put(
        KIND,
        gkey(gid),
        vec![actor.to_string(), name.clone(), desc.clone()],
    );
    set_member(reg, gid, actor, ADMIN);
    mark(reg);
    Ok(Group {
        group_id: gid,
        creator_id: actor,
        name,
        description: desc,
    })
}

/// A group made by the server for `creator` (a corporation, say), with
/// `members` already in it and no admin: the client treats a group its own
/// corporation created as read-only (`IsGroupCorpOwned`), so nobody needs
/// to hold it. The default docking group of a structure profile is one.
pub fn create_for(
    reg: &mut Registry,
    creator: i64,
    name: &str,
    desc: &str,
    members: &[(i64, i64)],
) -> Group {
    let name = clip(name.trim(), MAX_NAME_CHARS);
    let desc = clip(desc, MAX_DESC_CHARS);
    let next_key = vec!["next".to_owned()];
    let stored = reg
        .deltas()
        .of(KIND)
        .get(&next_key)
        .and_then(|v| v.first())
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);
    let used = all(reg).last().map_or(0, |g| g.group_id);
    let gid = stored.max(used) + 1;
    reg.deltas_mut().put(KIND, next_key, vec![gid.to_string()]);
    reg.deltas_mut().put(
        KIND,
        gkey(gid),
        vec![creator.to_string(), name.clone(), desc.clone()],
    );
    for &(member, t) in members {
        set_member(reg, gid, member, t);
    }
    mark(reg);
    Group {
        group_id: gid,
        creator_id: creator,
        name,
        description: desc,
    }
}

/// `Delete(gid)`: an admin only.
pub fn delete(reg: &mut Registry, actor: i64, gid: i64) -> Result<(), Refusal> {
    group(reg, gid).ok_or(Refusal::NoSuchGroup)?;
    let mem = members(reg, gid);
    if effective_in(reg, &mem, actor) != ADMIN {
        return Err(Refusal::NotAllowed);
    }
    for m in mem.keys() {
        reg.deltas_mut().del(KIND, &mkey(gid, *m));
    }
    reg.deltas_mut().del(KIND, &gkey(gid));
    let ids: Vec<i64> = mem.keys().copied().collect();
    tell(reg, actor, gid, DELETED, &ids, &[]);
    mark(reg);
    Ok(())
}

/// `UpdateName` and `UpdateDescription`: an admin or a manager.
pub fn rename(
    reg: &mut Registry,
    actor: i64,
    gid: i64,
    text: &str,
    description: bool,
) -> Result<(), Refusal> {
    let mut g = group(reg, gid).ok_or(Refusal::NoSuchGroup)?;
    let mem = members(reg, gid);
    if rank(effective_in(reg, &mem, actor)) < rank(MANAGER) {
        return Err(Refusal::NotAllowed);
    }
    let code = if description {
        g.description = clip(text, MAX_DESC_CHARS);
        DESCRIPTION_CHANGED
    } else {
        let n = clip(text.trim(), MAX_NAME_CHARS);
        if n.is_empty() {
            return Err(Refusal::BadName);
        }
        g.name = n;
        NAME_CHANGED
    };
    reg.deltas_mut().put(
        KIND,
        gkey(gid),
        vec![g.creator_id.to_string(), g.name, g.description],
    );
    let ids: Vec<i64> = mem.keys().copied().collect();
    tell(reg, actor, gid, code, &ids, &[]);
    mark(reg);
    Ok(())
}

/// May `actor_t` give or take away a row of type `t`?
fn may_touch(actor_t: i64, t: i64) -> bool {
    match actor_t {
        ADMIN => true,
        MANAGER => t == MEMBER || t == EXCLUDED,
        _ => false,
    }
}

fn admins_left(mem: &BTreeMap<i64, i64>, but: &[i64]) -> bool {
    mem.iter().any(|(m, t)| *t == ADMIN && !but.contains(m))
}

/// `AddMembers(gid, {memberID: type})`; the failures.
pub fn add_members(
    reg: &mut Registry,
    actor: i64,
    gid: i64,
    wanted: &[(i64, i64)],
) -> Result<Vec<Failure>, Refusal> {
    group(reg, gid).ok_or(Refusal::NoSuchGroup)?;
    let mut mem = members(reg, gid);
    let at = effective_in(reg, &mem, actor);
    if rank(at) < rank(MANAGER) {
        return Err(Refusal::NotAllowed);
    }
    let mut failed = Vec::new();
    let mut added = Vec::new();
    for &(m, t) in wanted {
        if !(MEMBER..=EXCLUDED).contains(&t) || m < 0 {
            failed.push((m, "invalid membership"));
        } else if !may_touch(at, t) {
            failed.push((m, "not allowed"));
        } else if mem.contains_key(&m) {
            failed.push((m, "already a member"));
        } else if mem.len() >= MAX_ENTRIES {
            failed.push((m, "the list is full"));
        } else {
            mem.insert(m, t);
            added.push(m);
        }
    }
    for &m in &added {
        set_member(reg, gid, m, mem[&m]);
    }
    let all_m: Vec<i64> = mem.keys().copied().collect();
    tell(reg, actor, gid, MEMBER_ADDED, &all_m, &added);
    if !added.is_empty() {
        mark(reg);
    }
    Ok(failed)
}

/// `UpdateMembershipTypes(gid, ids, type)`; the failures.
pub fn retype(
    reg: &mut Registry,
    actor: i64,
    gid: i64,
    ids: &[i64],
    t: i64,
) -> Result<Vec<Failure>, Refusal> {
    group(reg, gid).ok_or(Refusal::NoSuchGroup)?;
    let mut mem = members(reg, gid);
    let at = effective_in(reg, &mem, actor);
    if rank(at) < rank(MANAGER) {
        return Err(Refusal::NotAllowed);
    }
    let mut failed = Vec::new();
    let mut done = Vec::new();
    for &m in ids {
        let Some(&old) = mem.get(&m) else {
            failed.push((m, "not a member"));
            continue;
        };
        if !(MEMBER..=EXCLUDED).contains(&t) {
            failed.push((m, "invalid membership"));
        } else if !may_touch(at, t) || !may_touch(at, old) {
            failed.push((m, "not allowed"));
        } else if old == ADMIN && t != ADMIN && !admins_left(&mem, &[m]) {
            failed.push((m, "the last administrator"));
        } else {
            mem.insert(m, t);
            done.push(m);
        }
    }
    for &m in &done {
        set_member(reg, gid, m, t);
    }
    let all_m: Vec<i64> = mem.keys().copied().collect();
    tell(reg, actor, gid, MEMBERTYPE_CHANGED, &all_m, &done);
    if !done.is_empty() {
        mark(reg);
    }
    Ok(failed)
}

/// `DeleteMembers(gid, ids)`; the failures. Anyone may leave a group, taking
/// only their own row, unless they are its last admin.
pub fn remove_members(
    reg: &mut Registry,
    actor: i64,
    gid: i64,
    ids: &[i64],
) -> Result<Vec<Failure>, Refusal> {
    group(reg, gid).ok_or(Refusal::NoSuchGroup)?;
    let mut mem = members(reg, gid);
    let at = effective_in(reg, &mem, actor);
    let mut failed = Vec::new();
    let mut done = Vec::new();
    for &m in ids {
        let Some(&old) = mem.get(&m) else {
            failed.push((m, "not a member"));
            continue;
        };
        let leaving = m == actor && at >= MEMBER;
        if !leaving && !may_touch(at, old) {
            failed.push((m, "not allowed"));
        } else if old == ADMIN && !admins_left(&mem, &[m]) {
            failed.push((m, "the last administrator"));
        } else {
            mem.remove(&m);
            done.push(m);
        }
    }
    for &m in &done {
        reg.deltas_mut().del(KIND, &mkey(gid, m));
    }
    let rest: Vec<i64> = mem.keys().copied().collect();
    tell(reg, actor, gid, MEMBER_REMOVED, &rest, &done);
    if !done.is_empty() {
        mark(reg);
    }
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const OPS: i64 = 7;
    const PAL: i64 = 8;
    const FOE: i64 = 9;
    const CORP: i64 = 5_001;

    fn world() -> Registry {
        let ch = |id: i64, corp: i64| Character {
            character_id: id,
            name: format!("P{id}"),
            ship_id: 1_000 + id,
            corporation_id: corp,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![ch(OPS, 1), ch(PAL, CORP), ch(FOE, CORP)],
            ..SandboxConfig::default()
        })
    }

    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn pushes(reg: &mut Registry) -> Vec<Sent> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (n, a) = p.to_push();
                (n, a, p.audience())
            })
            .collect()
    }

    #[test]
    fn the_push_keys_the_dict_by_group_id() {
        let (_, args) = GroupsUpdated {
            to: PAL,
            changed: vec![(7, vec![3, 4])],
        }
        .to_push();
        let kv = args[0].dict_as_kv().expect("a dict");
        assert_eq!(kv.len(), 1);
        assert_eq!(*kv[0].0, PyValue::Int(7));
        assert_eq!(
            *kv[0].1,
            PyValue::List(vec![PyValue::Int(3), PyValue::Int(4)])
        );
    }

    #[test]
    fn create_add_read_remove_with_the_push() {
        let mut reg = world();
        let g = create(&mut reg, OPS, "ops", "").unwrap().group_id;
        assert_eq!(effective(&reg, g, OPS), ADMIN);
        let _ = pushes(&mut reg);
        assert_eq!(add_members(&mut reg, OPS, g, &[(PAL, MEMBER)]), Ok(vec![]));
        let mine = mine(&reg, PAL);
        assert_eq!((mine.len(), mine[0].1), (1, MEMBER));
        let got = pushes(&mut reg);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].2, Some(vec![PAL]));
        assert_eq!(
            rename(&mut reg, PAL, g, "mine", false),
            Err(Refusal::NotAllowed)
        );
        assert_eq!(remove_members(&mut reg, OPS, g, &[PAL]), Ok(vec![]));
        assert!(super::mine(&reg, PAL).is_empty());
        let got = pushes(&mut reg);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "OnOwnerGroupsUpdated");
        assert!(format!("{:?}", got[0].1).contains('8'), "{got:?}");
    }

    #[test]
    fn a_corporation_row_counts_and_exclusion_wins() {
        let mut reg = world();
        let g = create(&mut reg, OPS, "corp", "").unwrap().group_id;
        add_members(&mut reg, OPS, g, &[(CORP, MEMBER), (FOE, EXCLUDED)]).unwrap();
        assert_eq!(effective(&reg, g, PAL), MEMBER, "through the corporation");
        assert_eq!(effective(&reg, g, FOE), NONE, "excluded beats the corp row");
        add_members(&mut reg, OPS, g, &[(PUBLIC_MEMBER_ID, MEMBER)]).unwrap();
        assert_eq!(effective(&reg, g, 12345), MEMBER, "everyone");
    }

    #[test]
    fn managers_stay_below_admins_and_the_last_admin_stays() {
        let mut reg = world();
        let g = create(&mut reg, OPS, "x", "").unwrap().group_id;
        add_members(&mut reg, OPS, g, &[(PAL, MANAGER)]).unwrap();
        assert_eq!(rename(&mut reg, PAL, g, "y", false), Ok(()));
        let f = add_members(&mut reg, PAL, g, &[(FOE, ADMIN), (99, MEMBER)]).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, FOE);
        assert_eq!(remove_members(&mut reg, PAL, g, &[OPS]).unwrap().len(), 1);
        assert_eq!(retype(&mut reg, OPS, g, &[OPS], MEMBER).unwrap().len(), 1);
        assert_eq!(delete(&mut reg, PAL, g), Err(Refusal::NotAllowed));
        assert_eq!(delete(&mut reg, OPS, g), Ok(()));
        assert!(all(&reg).is_empty() && members(&reg, g).is_empty());
        let g2 = create(&mut reg, OPS, "z", "").unwrap().group_id;
        assert!(g2 > g, "ids are not reused");
    }

    #[test]
    fn groups_survive_a_restart() {
        let mut reg = world();
        let g = create(&mut reg, OPS, "keep", "about").unwrap().group_id;
        add_members(&mut reg, OPS, g, &[(PAL, MANAGER)]).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "eve-ogroups-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);
        let mut fresh = world();
        fresh.apply_snapshot(snap);
        let got = group(&fresh, g).unwrap();
        assert_eq!(
            (got.name.as_str(), got.description.as_str()),
            ("keep", "about")
        );
        assert_eq!(effective(&fresh, g, PAL), MANAGER);
        assert_eq!(create(&mut fresh, OPS, "next", "").unwrap().group_id, g + 1);
    }
}
