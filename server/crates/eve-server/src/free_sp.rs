//! **Unallocated skill points** (lane ws-1-free-sp, `docs/reference/skill-system.md`).
//!
//! CCP behaviour, so core: a pool of skill points per character that the
//! pilot spends on skills they already hold. The client UI is complete (the
//! queue banner, the apply window, the right-click and the skill-button
//! paths); the server owes a persisted pool and five calls.
//!
//! - **The pool**: one `freesp` delta row per character, `char -> points`
//!   ([`KIND`]); no row is 0. [`grant`] is the one writer of new points (the
//!   world rule `start_sp` through `start::fund_new_character`; an injector
//!   will call it too), [`apply_many`] the one spender.
//! - **Reads**: `GetSkillQueueAndFreePoints`' second item and
//!   `GetFreeSkillPoints` answer [`of`]; `GetFreeSkillPointsAppliedToQueue`
//!   and `...ToSkills` answer the preview ([`applied_to_queue`],
//!   [`applied_to_skills`]), a `{typeID: points}` dict.
//! - **Writes** `ApplyFreeSkillPoints(typeID, points)` ([`apply_one`]),
//!   `ApplyFreeSkillPointsToQueue()` and `ApplyFreeSkillPointsToSkills(list)`
//!   ([`apply_to_queue`], [`apply_to_skills`]). The client discards the
//!   return of the last two and `BeginTransaction` only sets its pool when
//!   it is above 0, so every apply ends with an `OnFreeSkillPointsChanged`
//!   push, even to 0.
//!
//! **[A] The allocation rule is not in the client**: queue entries in order,
//! each filled up to `sp_for_level(rank, level)`, the training head
//! included. Falsifier: the apply window's numbers differ from what lands.
//!
//! **[A] A skill with no points is not funded while a prerequisite is unmet**
//! (`QueueCannotPlaceSkillBeforeRequirements` from [`apply_one`]; skipped
//! in the two list paths, so a window never promises what it cannot
//! deliver). The client's own injection path makes no prerequisite check.
//! Falsifier: the owner says TQ allows it.

use crate::service::{Registry, Request, ServiceError};
use crate::skill::{MAX_LEVEL, level_for_points, sp_for_level};
use eve_marshal::PyValue;
use std::time::Duration;

/// The delta kind: `char -> points`.
pub const KIND: &str = "freesp";

fn key(character_id: i64) -> Vec<String> {
    vec![character_id.to_string()]
}

/// The pool: the unallocated points of this character, 0 with no row.
pub fn of(reg: &Registry, character_id: i64) -> i64 {
    reg.deltas()
        .of(KIND)
        .get(&key(character_id))
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn put(reg: &mut Registry, character_id: i64, points: i64) {
    if points > 0 {
        reg.deltas_mut()
            .put(KIND, key(character_id), vec![points.to_string()]);
    } else {
        reg.deltas_mut().del(KIND, &key(character_id));
    }
    reg.mark_dirty();
}

/// Add `points` to the pool (saturating) and tell the pilot if they are
/// online. A non-positive amount changes nothing.
pub fn grant(reg: &mut Registry, character_id: i64, points: i64) {
    if points <= 0 {
        return;
    }
    let now = of(reg, character_id).saturating_add(points);
    put(reg, character_id, now);
    reg.schedule(
        Duration::ZERO,
        crate::skill::FreeSkillPointsChanged {
            character_id,
            points: now,
        },
    );
}

/// **Who gets how many points**, pure.
///
/// `entries` is `(typeID, toLevel)` in order. Each is filled up to
/// `sp_for_level(rank, level)`, a running total kept per skill so a second
/// level of the same skill starts where the first ended, until `free` runs
/// out. `skill` gives a skill's `(points, rank)`; a skill it does not know,
/// or an entry already at or above its level, takes nothing. The result is
/// one `(typeID, points)` per skill, in first-seen order.
pub fn allocate(
    entries: &[(i64, i64)],
    skill: impl Fn(i64) -> Option<(i64, i64)>,
    free: i64,
) -> Vec<(i64, i64)> {
    allocate_with(entries, skill, free, &mut |_, _| true)
}

/// The gate: a skill's type and a reader of every skill's points so far.
type Gate<'a> = dyn FnMut(i64, &dyn Fn(i64) -> i64) -> bool + 'a;

/// [`allocate`] with a gate asked before a skill is funded: its type and a
/// reader of every skill's points as the allocation has them so far.
fn allocate_with(
    entries: &[(i64, i64)],
    skill: impl Fn(i64) -> Option<(i64, i64)>,
    free: i64,
    gate: &mut Gate<'_>,
) -> Vec<(i64, i64)> {
    let mut given: Vec<(i64, i64)> = Vec::new();
    let mut left = free;
    for &(type_id, level) in entries {
        if left <= 0 {
            break;
        }
        let Some((points, rank)) = skill(type_id) else {
            continue;
        };
        if !(1..=MAX_LEVEL).contains(&level) {
            continue;
        }
        let added = given.iter().find(|g| g.0 == type_id).map_or(0, |g| g.1);
        let need = sp_for_level(rank, level) - (points + added);
        if need <= 0 {
            continue;
        }
        let now = |t: i64| {
            skill(t).map_or(0, |(p, _)| p) + given.iter().find(|g| g.0 == t).map_or(0, |g| g.1)
        };
        if !gate(type_id, &now) {
            continue;
        }
        let take = need.min(left);
        left -= take;
        match given.iter_mut().find(|g| g.0 == type_id) {
            Some(g) => g.1 += take,
            None => given.push((type_id, take)),
        }
    }
    given
}

/// The skills and queue as they stand **now**: settled, the training head
/// credited with its elapsed points, finished entries dropped. A clone, so
/// a preview moves nothing.
fn standing(reg: &Registry, id: i64) -> Option<(crate::skill::Skills, crate::skill::Queue)> {
    let mut skills = reg.skills.get(&id)?.clone();
    let mut q = reg.skill_queues.get(id).cloned().unwrap_or_default();
    let now = crate::pyutil::filetime_now();
    let attribute = crate::skill_queue::paced_attribute(reg, id);
    q.settle_in(&reg.content, &mut skills, now, &attribute);
    q.pause_in(&reg.content, &mut skills, now, &attribute);
    q.prune_in(&reg.content, &skills);
    Some((skills, q))
}

/// `(typeID, toLevel)` entries to points, with this pilot's skills, pool and
/// the prerequisite gate. The one place the two previews and the two applies
/// agree.
fn plan(reg: &Registry, id: i64, entries: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let Some((skills, _)) = standing(reg, id) else {
        return Vec::new();
    };
    let info = |t: i64| skills.get(t).map(|s| (s.points, s.rank));
    let mut gate = |type_id: i64, now: &dyn Fn(i64) -> i64| {
        // Only a skill with no points is judged, as the queue's own rule.
        if now(type_id) != 0 {
            return true;
        }
        crate::skill::required_in(&reg.content, type_id)
            .iter()
            .all(|&(req, level)| {
                let rank = skills.get(req).map_or(1, |s| s.rank);
                skills.get(req).is_some() && level_for_points(rank, now(req)) >= level
            })
    };
    allocate_with(entries, info, of(reg, id), &mut gate)
}

fn dict(alloc: &[(i64, i64)]) -> PyValue {
    // `(value, key)` on the wire.
    PyValue::Dict(
        alloc
            .iter()
            .map(|&(t, p)| (PyValue::Int(p), PyValue::Int(t)))
            .collect(),
    )
}

/// `GetFreeSkillPointsAppliedToQueue()`: what [`apply_to_queue`] would do,
/// as `{typeID: points}`.
pub(crate) fn applied_to_queue(reg: &Registry, id: i64) -> Vec<(i64, i64)> {
    let Some((_, q)) = standing(reg, id) else {
        return Vec::new();
    };
    plan(reg, id, &q.entries)
}

/// `GetFreeSkillPointsAppliedToSkills(list)`: the same for the client's own
/// `(typeID, level)` list.
pub(crate) fn applied_to_skills(
    reg: &Registry,
    id: i64,
    entries: &[(i64, i64)],
) -> Vec<(i64, i64)> {
    plan(reg, id, entries)
}

/// **Spend**: add each `(typeID, points)` to its skill and the pool.
///
/// The training head is frozen first, so what it trained until now is
/// credited before it moves (`freeze_training` pauses the queue at the old
/// points), and restarted after, from now, so no training time is lost. A
/// level reached moves `level`, drops the entry from the queue and goes to
/// the client in the thaw's `OnServerSkillsChanged`. Each amount is clamped
/// to what is left of the pool and to level V; the pool is written; the
/// pilot is told the new pool even when it is 0 (the client does not read it
/// back from the apply calls); the dogma is re-announced.
pub(crate) fn apply_many(reg: &mut Registry, id: i64, alloc: &[(i64, i64)]) {
    let mut frozen = crate::skill_queue::freeze_training(reg, id);
    let mut left = of(reg, id);
    let mut moved = Vec::new();
    if let Some(skills) = reg.skills.get_mut(&id) {
        for &(type_id, points) in alloc {
            let Some(s) = skills.get_mut(type_id) else {
                continue;
            };
            let room = sp_for_level(s.rank, MAX_LEVEL) - s.points;
            let add = points.min(room).min(left);
            if add <= 0 {
                continue;
            }
            left -= add;
            s.points += add;
            s.level = s.level.max(level_for_points(s.rank, s.points));
            moved.push(type_id);
        }
        reg.skill_queues.entry(id).prune_in(&reg.content, skills);
    }
    put(reg, id, left);
    frozen.note(&moved);
    crate::skill_queue::thaw_training(reg, id, frozen);
    reg.push(crate::skill::FreeSkillPointsChanged {
        character_id: id,
        points: left,
    });
    crate::brain::announce(reg, id);
}

fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn refuse(label: &str, fields: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(label, crate::pyutil::pypairs(fields)))
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(i)) => Some(*i),
        Some(PyValue::Real(r)) if r.fract() == 0.0 => Some(*r as i64),
        _ => None,
    }
}

/// `ApplyFreeSkillPoints(typeID, points)`: one skill, returns the new pool.
///
/// The client has already clamped to level V and raised the three refusals;
/// the server raises them too, in the client's labels, because a bot or a
/// modified client skips that check. Nothing changes on a refusal.
pub(crate) fn apply_one(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(id) = req.actor else {
        return Ok(PyValue::None);
    };
    let (Some(type_id), Some(points)) = (int(req.args.first()), int(req.args.get(1))) else {
        return Err(bad(req, "(typeID, points)"));
    };
    if points <= 0 {
        return Err(bad(req, "points must be positive"));
    }
    let Some((skills, q)) = standing(reg, id) else {
        return Err(bad(req, "the character has no skill record"));
    };
    let Some(skill) = skills.get(type_id) else {
        return Err(refuse(
            "CannotApplyFreePointsDoNotHaveSkill",
            vec![("skillName", PyValue::Int(type_id))],
        ));
    };
    let training = reg.skill_queues.get(id).is_some_and(|q| q.is_training());
    if training && q.entries.first().is_some_and(|e| e.0 == type_id) {
        return Err(refuse("CannotApplyFreePointsWhileTrainingSkill", vec![]));
    }
    let pool = of(reg, id);
    if points > pool {
        return Err(refuse(
            "CannotApplyFreePointsNotEnoughRemaining",
            vec![
                ("pointsRequested", PyValue::Int(points)),
                ("pointsRemaining", PyValue::Int(pool)),
            ],
        ));
    }
    // [A] see the module note.
    if crate::skill::unmet_requirement(
        &skills,
        &[],
        type_id,
        crate::skill::required_in(&reg.content, type_id),
    )
    .is_some()
    {
        return Err(refuse("QueueCannotPlaceSkillBeforeRequirements", vec![]));
    }
    let room = sp_for_level(skill.rank, MAX_LEVEL) - skill.points;
    apply_many(reg, id, &[(type_id, points.min(room))]);
    Ok(PyValue::Int(of(reg, id)))
}

/// `ApplyFreeSkillPointsToQueue()`: spend on the queue, in order. The return
/// is discarded by the client.
pub(crate) fn apply_to_queue(reg: &mut Registry, req: &Request<'_>) -> PyValue {
    if let Some(id) = req.actor {
        let alloc = applied_to_queue(reg, id);
        apply_many(reg, id, &alloc);
    }
    PyValue::None
}

/// `(typeID, level)` pairs from the client's list.
pub(crate) fn pairs(req: &Request<'_>) -> Result<Vec<(i64, i64)>, ServiceError> {
    let Some(PyValue::List(items) | PyValue::Tuple(items)) = req.args.first() else {
        return Err(bad(req, "(skillTypeIDsAndLevels,)"));
    };
    items
        .iter()
        .map(|p| match p {
            PyValue::Tuple(v) | PyValue::List(v) => Some((int(v.first())?, int(v.get(1))?)),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad(req, "an entry that is not (typeID, level)"))
}

/// `GetFreeSkillPointsAppliedToQueue()`'s reply.
pub(crate) fn queue_preview(reg: &Registry, req: &Request<'_>) -> PyValue {
    req.actor
        .map_or(PyValue::Dict(vec![]), |id| dict(&applied_to_queue(reg, id)))
}

/// `GetFreeSkillPointsAppliedToSkills(list)`'s reply.
pub(crate) fn skills_preview(reg: &Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let entries = pairs(req)?;
    Ok(req.actor.map_or(PyValue::Dict(vec![]), |id| {
        dict(&applied_to_skills(reg, id, &entries))
    }))
}

/// `ApplyFreeSkillPointsToSkills(list)`. A skill the pilot does not hold is
/// refused first, as the client does, so nothing is half applied.
pub(crate) fn apply_to_skills(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let entries = pairs(req)?;
    let Some(id) = req.actor else {
        return Ok(PyValue::None);
    };
    for &(type_id, _) in &entries {
        if reg.skills.get(&id).is_none_or(|s| s.get(type_id).is_none()) {
            return Err(refuse(
                "CannotApplyFreePointsDoNotHaveSkill",
                vec![("skillName", PyValue::Int(type_id))],
            ));
        }
    }
    let alloc = applied_to_skills(reg, id, &entries);
    apply_many(reg, id, &alloc);
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    const FRIGATE: i64 = 3329;

    fn actor(reg: &Registry) -> i64 {
        reg.config().characters[0].character_id
    }

    fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        let actor = Some(actor(reg));
        reg.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor,
        })
    }

    /// The label of a user error.
    fn label(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(h) = &**header else {
            panic!("a call header")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("user error args")
        };
        let PyValue::Bytes(b) = &args[0] else {
            panic!("a label")
        };
        String::from_utf8(b.clone()).expect("utf8")
    }

    /// A world whose pilot owns Minmatar Frigate at level 0 and holds
    /// `pool` free points, with nothing pending.
    fn with_pool(pool: i64) -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        call(
            &mut reg,
            "PurchaseSkills",
            &[PyValue::List(vec![PyValue::Int(FRIGATE)])],
        )
        .expect("bought");
        let id = actor(&reg);
        grant(&mut reg, id, pool);
        reg.take_pending();
        reg
    }

    fn queue_frigate(reg: &mut Registry, levels: &[i64]) {
        let info = PyValue::Dict(
            levels
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    (
                        PyValue::Tuple(vec![PyValue::Int(FRIGATE), PyValue::Int(*l)]),
                        PyValue::Int(i as i64),
                    )
                })
                .collect(),
        );
        let kw = [(PyValue::Bool(true), crate::pyutil::bstr("activate"))];
        let actor = Some(actor(reg));
        reg.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method: "SaveNewQueue",
            args: &[info],
            kwargs: &kw,
            bind_params: None,
            actor,
        })
        .expect("saved");
        reg.take_pending();
    }

    fn frigate(reg: &Registry) -> crate::skill::SkillEntry {
        reg.skills[&actor(reg)].get(FRIGATE).expect("owned").clone()
    }

    fn rank(reg: &Registry) -> i64 {
        frigate(reg).rank
    }

    /// The pool the pilot was last told, from the pending pushes.
    fn told(reg: &mut Registry) -> Vec<i64> {
        reg.take_pending()
            .into_iter()
            .filter_map(|(_, p)| match p.to_push() {
                ("OnFreeSkillPointsChanged", a) => match a.as_slice() {
                    [PyValue::Int(n)] => Some(*n),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn level_for_points_walks_each_boundary() {
        for rank in [1, 2, 5] {
            for level in 1..=MAX_LEVEL {
                let sp = sp_for_level(rank, level);
                assert_eq!(level_for_points(rank, sp), level);
                assert_eq!(level_for_points(rank, sp - 1), level - 1);
                assert_eq!(level_for_points(rank, sp + 1), level);
            }
            assert_eq!(level_for_points(rank, 0), 0);
            assert_eq!(level_for_points(rank, i64::MAX), MAX_LEVEL);
        }
    }

    #[test]
    fn allocate_fills_in_order_and_stops_at_the_pool() {
        let skill = |t: i64| match t {
            1 => Some((0, 1)),
            2 => Some((0, 2)),
            _ => None,
        };
        let entries = [(1, 1), (2, 1), (1, 2), (9, 1)];
        let l1 = sp_for_level(1, 1);
        // Pool stops inside the second entry.
        assert_eq!(allocate(&entries, skill, l1 + 7), vec![(1, l1), (2, 7)]);
        // A pool for everything: the second level of skill 1 continues
        // from the first, and an unknown skill takes nothing.
        let all = allocate(&entries, skill, i64::MAX / 2);
        assert_eq!(all, vec![(1, sp_for_level(1, 2)), (2, sp_for_level(2, 1))]);
        // No pool, no allocation; a skill already at the level takes none.
        assert!(allocate(&entries, skill, 0).is_empty());
        let held = |t: i64| (t == 1).then_some((sp_for_level(1, 3), 1));
        assert!(allocate(&[(1, 2), (1, 3)], held, 100).is_empty());
    }

    #[test]
    fn the_pool_is_read_from_the_calls_the_client_makes() {
        let mut reg = with_pool(1234);
        assert_eq!(
            call(&mut reg, "GetFreeSkillPoints", &[]),
            Ok(PyValue::Int(1234))
        );
        let PyValue::Tuple(pair) = call(&mut reg, "GetSkillQueueAndFreePoints", &[]).unwrap()
        else {
            panic!("a pair")
        };
        assert_eq!(pair[1], PyValue::Int(1234));
    }

    #[test]
    fn a_grant_adds_saturates_and_tells_the_pilot() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg);
        assert_eq!(of(&reg, id), 0);
        grant(&mut reg, id, 40);
        grant(&mut reg, id, 2);
        grant(&mut reg, id, 0);
        grant(&mut reg, id, -5);
        assert_eq!(of(&reg, id), 42);
        grant(&mut reg, id, i64::MAX);
        assert_eq!(of(&reg, id), i64::MAX);
        assert_eq!(reg.scheduled_count(), 3);
    }

    #[test]
    fn the_pool_survives_a_world_round_trip() {
        let mut reg = with_pool(777);
        let id = actor(&reg);
        let mut back = Registry::new(SandboxConfig::default());
        back.apply_snapshot(reg.to_snapshot());
        assert_eq!(of(&back, id), 777);
        // Spent to 0, the row goes and the world still reads 0.
        queue_frigate(&mut reg, &[1]);
        call(&mut reg, "ApplyFreeSkillPointsToQueue", &[]).expect("applied");
        assert_ne!(of(&reg, id), 777);
    }

    #[test]
    fn the_previews_are_value_then_key_on_the_wire() {
        let mut reg = with_pool(i64::from(i32::MAX));
        queue_frigate(&mut reg, &[1]);
        let want = sp_for_level(rank(&reg), 1);
        let PyValue::Dict(d) = call(&mut reg, "GetFreeSkillPointsAppliedToQueue", &[]).unwrap()
        else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1, PyValue::Int(FRIGATE));
        let PyValue::Int(points) = d[0].0 else {
            panic!("points")
        };
        assert!(points <= want && want - points < 60, "{points} of {want}");
        // The skills path takes the client's own (typeID, level) list.
        let list = PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(FRIGATE),
            PyValue::Int(2),
        ])]);
        let PyValue::Dict(d) =
            call(&mut reg, "GetFreeSkillPointsAppliedToSkills", &[list]).unwrap()
        else {
            panic!("a dict")
        };
        assert_eq!(d[0].1, PyValue::Int(FRIGATE));
        assert!(matches!(d[0].0, PyValue::Int(p) if p > want));
    }

    #[test]
    fn applying_to_the_head_credits_elapsed_points_reaches_the_level_and_keeps_going() {
        let mut reg = with_pool(i64::from(i32::MAX));
        let id = actor(&reg);
        queue_frigate(&mut reg, &[1, 2]);
        // The head has been training for a while: its points are credited
        // first, so the pool pays only the rest.
        let r = rank(&reg);
        let l1 = sp_for_level(r, 1);
        let before = of(&reg, id);
        let minute = crate::skill::MIN;
        reg.skill_queues.entry(id).start = Some(crate::pyutil::filetime_now() - minute);
        // Fund exactly level I through a one-entry list.
        let list = PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(FRIGATE),
            PyValue::Int(1),
        ])]);
        call(&mut reg, "ApplyFreeSkillPointsToSkills", &[list]).expect("applied");
        let s = frigate(&reg);
        assert_eq!((s.level, s.points), (1, l1));
        let spent = before - of(&reg, id);
        assert!(
            spent > 0 && spent < l1,
            "the minute's points were credited: {spent}"
        );
        // Level I's entry left the queue; level II goes on, training from now.
        let q = reg.skill_queues.get(id).expect("queue");
        assert_eq!(q.entries, vec![(FRIGATE, 2)]);
        assert!(q.is_training());
        assert_eq!(told(&mut reg), vec![of(&reg, id)]);
    }

    #[test]
    fn a_partial_apply_leaves_the_level_and_the_queue_running() {
        let mut reg = with_pool(100);
        let id = actor(&reg);
        queue_frigate(&mut reg, &[1]);
        let l1 = sp_for_level(rank(&reg), 1);
        call(&mut reg, "ApplyFreeSkillPointsToQueue", &[]).expect("applied");
        let s = frigate(&reg);
        assert_eq!(s.level, 0);
        assert!(s.points >= 100 && s.points < l1, "{}", s.points);
        assert_eq!(of(&reg, id), 0);
        let q = reg.skill_queues.get(id).expect("queue");
        assert_eq!(q.entries, vec![(FRIGATE, 1)]);
        assert!(q.is_training());
        // The pool is 0 and the pilot is told so.
        assert_eq!(told(&mut reg), vec![0]);
    }

    #[test]
    fn an_empty_pool_still_pushes_it() {
        let mut reg = with_pool(0);
        queue_frigate(&mut reg, &[1]);
        call(&mut reg, "ApplyFreeSkillPointsToQueue", &[]).expect("applied");
        assert_eq!(told(&mut reg), vec![0]);
        assert_eq!(frigate(&reg).level, 0);
    }

    #[test]
    fn apply_one_spends_clamps_to_level_v_and_returns_the_pool() {
        let mut reg = with_pool(10_000_000);
        let id = actor(&reg);
        let top = sp_for_level(rank(&reg), MAX_LEVEL);
        let got = call(
            &mut reg,
            "ApplyFreeSkillPoints",
            &[PyValue::Int(FRIGATE), PyValue::Int(5_000_000)],
        );
        assert_eq!(got, Ok(PyValue::Int(10_000_000 - top)));
        assert_eq!(of(&reg, id), 10_000_000 - top);
        let s = frigate(&reg);
        assert_eq!((s.level, s.points), (MAX_LEVEL, top));
        assert_eq!(told(&mut reg), vec![10_000_000 - top]);
    }

    #[test]
    fn apply_one_refuses_in_the_clients_labels_and_changes_nothing() {
        let mut reg = with_pool(500);
        let id = actor(&reg);
        let apply = |reg: &mut Registry, t: i64, p: i64| {
            call(
                reg,
                "ApplyFreeSkillPoints",
                &[PyValue::Int(t), PyValue::Int(p)],
            )
        };
        let e = apply(&mut reg, 3300, 501).unwrap_err();
        assert_eq!(label(&e), "CannotApplyFreePointsNotEnoughRemaining");
        let e = apply(&mut reg, 3999, 5).unwrap_err();
        assert_eq!(label(&e), "CannotApplyFreePointsDoNotHaveSkill");
        for bad in [0, -3] {
            assert!(matches!(
                apply(&mut reg, 3300, bad),
                Err(ServiceError::BadArguments { .. })
            ));
        }
        queue_frigate(&mut reg, &[1]);
        let e = apply(&mut reg, FRIGATE, 5).unwrap_err();
        assert_eq!(label(&e), "CannotApplyFreePointsWhileTrainingSkill");
        assert_eq!(of(&reg, id), 500);
        assert_eq!(frigate(&reg).points, 0);
        assert!(told(&mut reg).is_empty());
        // The list path refuses a skill the pilot does not hold, whole.
        let list = PyValue::List(vec![
            PyValue::Tuple(vec![PyValue::Int(FRIGATE), PyValue::Int(1)]),
            PyValue::Tuple(vec![PyValue::Int(3999), PyValue::Int(1)]),
        ]);
        let e = call(&mut reg, "ApplyFreeSkillPointsToSkills", &[list]).unwrap_err();
        assert_eq!(label(&e), "CannotApplyFreePointsDoNotHaveSkill");
        assert_eq!(of(&reg, id), 500);
    }

    /// **[A]** the prerequisite rule: refused for one skill, skipped (never
    /// promised by the preview) in a list, met by an earlier list entry.
    #[test]
    fn a_skill_with_an_unmet_prerequisite_is_not_funded() {
        let text = "skill\t9001\t255\t1\t165\t166\t0.01\t\tFirst Test Skill\n\
            skill\t9002\t255\t1\t165\t166\t0.01\t9001=1\tSecond Test Skill\n";
        let table = crate::skill::Table::parse(text).expect("parses");
        let mut content = crate::content::Content::fixture();
        content.insert(crate::skill::TABLE.name, table);
        let mut reg =
            Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content));
        let id = actor(&reg);
        grant(&mut reg, id, i64::from(i32::MAX));
        let bought = reg
            .skills
            .get_mut(&id)
            .expect("skills")
            .purchase_in(&reg.content, &[9001, 9002]);
        assert!(bought.is_ok());
        let e = call(
            &mut reg,
            "ApplyFreeSkillPoints",
            &[PyValue::Int(9002), PyValue::Int(10)],
        )
        .unwrap_err();
        assert_eq!(label(&e), "QueueCannotPlaceSkillBeforeRequirements");
        // Alone, the dependent takes nothing; after its prerequisite in the
        // same list, it is funded.
        let second = [(9002, 1)];
        assert!(applied_to_skills(&reg, id, &second).is_empty());
        let both = [(9002, 1), (9001, 1), (9002, 1)];
        let got = applied_to_skills(&reg, id, &both);
        assert_eq!(
            got,
            vec![(9001, sp_for_level(1, 1)), (9002, sp_for_level(1, 1))]
        );
    }

    #[test]
    fn retime_all_does_not_reprice_trained_time() {
        let mut reg = with_pool(0);
        let id = actor(&reg);
        queue_frigate(&mut reg, &[1]);
        let rate = {
            let attr = crate::skill_queue::paced_attribute(&reg, id);
            crate::skill::rate_for_in(reg.content(), FRIGATE, &attr).expect("known")
        };
        let full = crate::skill::training_ticks(sp_for_level(rank(&reg), 1), rate);
        let now = crate::pyutil::filetime_now();
        reg.skill_queues.entry(id).start = Some(now - full / 2);
        crate::skill_queue::retime_all(&mut reg, |reg| {
            crate::game_clock::set(reg, crate::game_clock::GameClock::new(2).unwrap());
        });
        // Half was trained at pace 1; the other half now runs at pace 2.
        let q = reg.skill_queues.get(id).expect("queue");
        assert!(q.is_training());
        let attr = crate::skill_queue::paced_attribute(&reg, id);
        let rows = q.rows_in(reg.content(), &reg.skills[&id], &attr);
        let end = rows[0].end.expect("an end");
        let left = end - crate::pyutil::filetime_now();
        let want = full / 4;
        assert!((left - want).abs() < want / 50, "{left} vs {want}");
        let names: Vec<_> = reg
            .take_due(std::time::Instant::now() + Duration::from_secs(1))
            .iter()
            .map(|p| p.to_push().0)
            .collect();
        assert!(names.contains(&"OnNewSkillQueueSaved"), "{names:?}");
        assert!(names.contains(&"OnServerSkillsChanged"), "{names:?}");
    }
}
