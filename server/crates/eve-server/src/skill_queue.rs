//! Skill training (W8 lane E) -- the queue, training in real time, and the
//! level-up push.
//!
//! Moved verbatim out of `service/character.rs` by IP38-b; the arms that
//! serve it stay there.
//!
//! This module is the skill-training lane's. The model -- the queue, the
//! client's own rate and time formulas, the two pushes and the world-file
//! records -- is in `crate::skill` and knows nothing of `Registry`; this
//! module is the glue. The arms that call into it
//! (`GetSkillQueueAndFreePoints`, `SaveNewQueue`, `AbortTraining`) sit in
//! `service/character.rs`'s `dispatch`, because every arm in that file does,
//! and the world clock calls [`skill_training_tick`]. `docs/lanes/skill-training.md` has every
//! call shape with its citation, the `[A]` list, and the coordinator edits.

use crate::service::{
    ATTR_CHARISMA, ATTR_INTELLIGENCE, ATTR_MEMORY, ATTR_PERCEPTION, ATTR_WILLPOWER, Registry,
    Request, ServiceError,
};
use eve_marshal::PyValue;
use std::time::Duration;

/// The five character attributes a new pilot starts with, by attribute id.
///
/// **What is [V].** The client's own remap rules, `appConst.pyc:1850`-`:1852`:
/// each attribute is between `respecMinimumAttributeValue` 17 and
/// `respecMaximumAttributeValue` 27, and the five sum to
/// `respecTotalRespecPoints` **99**. The attributes window counts
/// `unspentPts = 99 - sum(attributes)` (`attributes.Load`, `:121`-`:159`), so
/// the five 20.0s this used to answer (100) were a remap window one point
/// *overspent*.
///
/// **What is [A].** Which attribute is the 19: charisma, as retail gives a new
/// character (20/20/20/20/19). The client data has no rule for it. The
/// `bloodlines.fsdbinary` table does carry five per-bloodline attribute
/// columns (`bloodlinesLoader.pyd`, read with `tools/dogma.py derive`), but
/// they are the pre-remap bonuses: every playable bloodline's five sum to 30,
/// not 99 or 14, and no client code reads them (the only `willpower` readers
/// are the character's own dogma item and the protobuf stubs). So a
/// bloodline-derived start would be invented arithmetic, and every character
/// starts the same.
pub(crate) const STARTING_ATTRIBUTES: [(i64, f64); 5] = [
    (ATTR_CHARISMA, 19.0),
    (ATTR_INTELLIGENCE, 20.0),
    (ATTR_MEMORY, 20.0),
    (ATTR_PERCEPTION, 20.0),
    (ATTR_WILLPOWER, 20.0),
];

/// `respecMinimumAttributeValue` (`appConst.pyc:1850`) **[V]**: no character
/// attribute is ever below it, so it is the answer for an id that is not one
/// of the five and for a call with no character. Never zero: every training
/// estimate divides by an attribute.
pub(crate) const STARTING_ATTRIBUTE_FLOOR: f64 = 17.0;

/// One character's attribute, for the training rate.
///
/// **The one function both readers share**: `GetAttributes` answers it, the
/// training clock trains with it, and `godma`'s character row is primed from
/// it, so the server's clock and the client's countdown cannot drift apart --
/// a disagreement there is a skill the client shows as done that the server
/// has not trained, or the reverse.
///
/// The same start for everyone ([`STARTING_ATTRIBUTES`]) **plus what the
/// head's implants and the active boosters add** ([`crate::implants::attribute_bonus`]):
/// the client's attribute window subtracts the implants' own bonus from the
/// answer to find the remappable base (`attributes.py:130-142`), so the answer
/// includes them **[V]**. A remap (`respec_character`) replaces the start with
/// its stored base, read in [`base_attributes`].
pub(crate) fn skill_attribute(reg: &Registry, character_id: i64, attribute_id: i64) -> f64 {
    base_attributes(reg, character_id)(attribute_id)
        + crate::implants::attribute_bonus(reg, character_id, attribute_id)
}

/// The base attributes of one character, read once (its remap row, else
/// [`STARTING_ATTRIBUTES`]), as a lookup by attribute id.
fn base_attributes(reg: &Registry, character_id: i64) -> impl Fn(i64) -> f64 + use<> {
    let row = remap_row(reg, character_id);
    move |attribute_id| match (&row, REMAP_ORDER.iter().position(|a| *a == attribute_id)) {
        (Some(r), Some(i)) => r.attributes[i] as f64,
        _ => STARTING_ATTRIBUTES
            .iter()
            .find(|(id, _)| *id == attribute_id)
            .map_or(STARTING_ATTRIBUTE_FLOOR, |(_, v)| *v),
    }
}

// ===========================================================================
// Neural remap (lane gap-remap, EVEJS-GAP svcD §3.5)
// ===========================================================================

/// The delta kind: `char -> per cha int mem wil lastRemapFILETIME bonusRemaps`.
/// A character with no row has never remapped.
pub const REMAP_KIND: &str = "remap";

/// The order of the five attributes in a [`REMAP_KIND`] row.
const REMAP_ORDER: [i64; 5] = [
    ATTR_PERCEPTION,
    ATTR_CHARISMA,
    ATTR_INTELLIGENCE,
    ATTR_MEMORY,
    ATTR_WILLPOWER,
];

/// `respecTotalRespecPoints` **[V]** (`appConst.pyc`, as for
/// [`STARTING_ATTRIBUTES`]): the five base attributes sum to it.
pub(crate) const REMAP_TOTAL: i64 = 99;
/// `respecMinimumAttributeValue` **[V]**.
pub(crate) const REMAP_MIN: i64 = 17;
/// `respecMaximumAttributeValue` **[V]**.
pub(crate) const REMAP_MAX: i64 = 27;
/// `respecTimeInterval`, 365 days in FILETIME ticks **[V]** (the client's
/// `attributes.py` compares `nextTimedRespec` with the wall clock).
pub(crate) const REMAP_INTERVAL: i64 = 365 * 86_400 * 10_000_000;
/// The bonus remaps a character that never remapped holds. **[A]** retail
/// gives a new character one; falsifier: the owner's memory of retail, or a
/// TQ `GetRespecInfo` of a new character.
pub(crate) const INITIAL_BONUS_REMAPS: i64 = 1;

/// One character's stored remap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemapRow {
    /// In [`REMAP_ORDER`].
    attributes: [i64; 5],
    /// FILETIME of the last timed remap; 0 for none yet.
    last: i64,
    bonus: i64,
}

fn remap_row(reg: &Registry, character_id: i64) -> Option<RemapRow> {
    let v = reg
        .deltas()
        .of(REMAP_KIND)
        .get(&vec![character_id.to_string()])?;
    let n: Vec<i64> = v.iter().filter_map(|s| s.parse().ok()).collect();
    (n.len() == 7).then(|| RemapRow {
        attributes: [n[0], n[1], n[2], n[3], n[4]],
        last: n[5],
        bonus: n[6],
    })
}

fn put_remap_row(reg: &mut Registry, character_id: i64, r: RemapRow) {
    let mut v: Vec<String> = r.attributes.iter().map(i64::to_string).collect();
    v.push(r.last.to_string());
    v.push(r.bonus.to_string());
    reg.deltas_mut()
        .put(REMAP_KIND, vec![character_id.to_string()], v);
}

fn created(reg: &Registry, character_id: i64) -> i64 {
    reg.character(character_id)
        .map_or(0, |c| c.create_date_time)
}

/// `(nextTimedRespec, freeRespecs)`: a year after the last timed remap, or the
/// character's creation if it never remapped (a timed remap has been open
/// since birth), and the bonus remaps left.
fn respec_state(reg: &Registry, character_id: i64) -> (i64, i64) {
    match remap_row(reg, character_id) {
        Some(r) if r.last > 0 => (r.last + REMAP_INTERVAL, r.bonus),
        Some(r) => (created(reg, character_id), r.bonus),
        None => (created(reg, character_id), INITIAL_BONUS_REMAPS),
    }
}

/// NES-BACKEND: `n` more bonus remaps for `character_id` (the store's
/// service product). A character with no remap row gets one holding its
/// starting attributes and the [`INITIAL_BONUS_REMAPS`] it is owed, plus `n`.
pub(crate) fn grant_bonus_remaps(reg: &mut Registry, character_id: i64, n: i64) {
    let row = match remap_row(reg, character_id) {
        Some(mut r) => {
            r.bonus += n;
            r
        }
        None => {
            let base = base_attributes(reg, character_id);
            RemapRow {
                attributes: REMAP_ORDER.map(|a| base(a) as i64),
                last: 0,
                bonus: INITIAL_BONUS_REMAPS + n,
            }
        }
    };
    put_remap_row(reg, character_id, row);
    reg.mark_dirty();
}

/// `GetRespecInfo` for this character.
pub(crate) fn respec_info(reg: &Registry, actor: Option<i64>) -> PyValue {
    let (next, free) = actor.map_or((0, 0), |c| respec_state(reg, c));
    crate::pyutil::pydict(vec![
        ("nextTimedRespec", PyValue::Int(next)),
        ("freeRespecs", PyValue::Int(free)),
    ])
}

fn remap_error(name: &str, extra: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(name, crate::pyutil::pypairs(extra)))
}

/// `RespecCharacter({attributeID: value})` **[V]** for the shape
/// (`attributes.py:43-48, 232-263`): five base values, 17-27, sum 99. A
/// timed remap is used when it is due, else a bonus one, else
/// `RespecTooSoon`. The training head is settled at the old rate and
/// restarted at the new one ([`freeze_training`] / [`thaw_training`]).
/// The error names are the spec's "the client's keys" **[A]**; falsifier: the
/// client's `localization` message ids for them.
pub(crate) fn respec_character(
    reg: &mut Registry,
    actor: Option<i64>,
    arg: Option<&PyValue>,
) -> Result<PyValue, ServiceError> {
    let (Some(id), Some(PyValue::Dict(entries))) = (actor, arg) else {
        return Ok(PyValue::None);
    };
    let misallocated = || remap_error("RespecAttributesMisallocated", vec![]);
    let mut wanted = [None; 5];
    for (value, key) in entries {
        let v = match value {
            PyValue::Int(i) => *i,
            PyValue::Real(r) if r.fract() == 0.0 => *r as i64,
            _ => return Err(misallocated()),
        };
        let slot = match key {
            PyValue::Int(k) => REMAP_ORDER.iter().position(|a| a == k),
            _ => None,
        };
        let Some(slot) = slot else {
            return Err(misallocated());
        };
        wanted[slot] = Some(v);
    }
    let [Some(a), Some(b), Some(c), Some(d), Some(e)] = wanted else {
        return Err(misallocated());
    };
    let attributes = [a, b, c, d, e];
    if attributes.iter().any(|v| *v < REMAP_MIN) {
        return Err(remap_error("RespecAttributesTooLow", vec![]));
    }
    if attributes.iter().any(|v| *v > REMAP_MAX) {
        return Err(remap_error("RespecAttributesTooHigh", vec![]));
    }
    if attributes.iter().sum::<i64>() != REMAP_TOTAL {
        return Err(misallocated());
    }
    let now = crate::pyutil::filetime_now();
    let (next, free) = respec_state(reg, id);
    let timed = next <= now;
    if !timed && free <= 0 {
        return Err(remap_error(
            "RespecTooSoon",
            vec![("nextTime", PyValue::Int(next))],
        ));
    }
    let frozen = freeze_training(reg, id);
    let last = remap_row(reg, id).map_or(0, |r| r.last);
    let row = RemapRow {
        attributes,
        last: if timed { now } else { last },
        bonus: if timed { free } else { free - 1 },
    };
    put_remap_row(reg, id, row);
    reg.mark_dirty();
    thaw_training(reg, id, frozen);
    reg.push(crate::skill::ForcedRefresh);
    crate::brain::announce(reg, id);
    Ok(PyValue::None)
}

/// **The training pace**: the clock's `--training-scale` pace times the
/// world rule `training_mult` (1.0 unless a module sets it, so by default the
/// clock's pace unchanged).
fn training_pace(reg: &Registry) -> f64 {
    crate::game_clock::of(reg).training_pace() * reg.config().rules.training_mult
}

/// **The training attributes at the game clock's pace** (lane `clock`):
/// [`skill_attribute`] times the clock's scale. The training rate
/// (`crate::skill::sp_per_minute`, `primary + secondary / 2`) is linear in
/// both attributes, so this is exactly the rate times N: every end time,
/// every settle and every pause credit the queue derives runs N times
/// faster, and none of them is stored, so a restart at another scale
/// re-derives the head skill's end from its stored start at the new rate.
/// Only the queue reads it; the character sheet still shows
/// [`skill_attribute`]. **[A]** the client's own countdown is the server's
/// row times (`GetSkillQueueAndFreePoints`); falsified by a client that
/// recomputes the end from the sheet's attributes and shows it N times
/// longer. Scale 1: [`skill_attribute`] itself.
///
/// The implants are read **once**, here, not per attribute.
pub(crate) fn paced_attribute(reg: &Registry, character_id: i64) -> impl Fn(i64) -> f64 + use<> {
    let pace = training_pace(reg);
    let bonus = crate::implants::training_bonuses(reg, character_id);
    let base = base_attributes(reg, character_id);
    move |a| (base(a) + bonus.get(&a).copied().unwrap_or(0.0)) * pace
}

/// **What the client is told its attributes are** (lane `bot-ctl`):
/// [`skill_attribute`] at the training pace, the numbers [`paced_attribute`]
/// trains with. The client recomputes every queue entry after the head
/// from `GetAttributes` (`skillQueueSvc.UpdateQueueTimestamps` 161-178,
/// `GetTimeForTraining` 1524-1543, `GetPlayerAttributeDict` 1093-1101 =
/// `skillsvc.GetCharacterAttributes` 246-252, which is
/// `GetSkillHandler().GetAttributes()`) **[V]**; only the head keeps the
/// server's row end. So a scaled server must serve scaled attributes or the
/// queue view disagrees with the clock. Pace 1 (every normal run): exactly
/// [`skill_attribute`].
pub(crate) fn client_attribute(reg: &Registry, character_id: i64, attribute_id: i64) -> f64 {
    skill_attribute(reg, character_id, attribute_id) * training_pace(reg)
}

/// **What [`freeze_training`] found**, for [`thaw_training`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frozen {
    was_training: bool,
    credited: Vec<i64>,
}

/// **Before anything changes this character's training attributes** (an
/// implant plugged, destroyed or swapped): settle the queue and credit the
/// head skill with what it trained at the *old* rate, and stop the clock.
///
/// The queue stores no rate, only when the head started and the points the
/// skill held then, and derives every end from the attributes of the moment
/// (`Queue::rows_in`), so a rate that changed under a running head would
/// re-price the time already trained. Crediting first and restarting at
/// [`thaw_training`] makes the head's remaining work run at the new rate and
/// its finished work stay finished. The same credit a pause makes
/// ([`crate::skill::Queue::pause_in`]).
pub(crate) fn freeze_training(reg: &mut Registry, character_id: i64) -> Frozen {
    let now = crate::pyutil::filetime_now();
    let mut credited = settle_training(reg, character_id, now);
    let attribute = paced_attribute(reg, character_id);
    let was_training = reg
        .skill_queues
        .get(character_id)
        .is_some_and(crate::skill::Queue::is_training);
    if was_training && let Some(skills) = reg.skills.get_mut(&character_id) {
        let q = reg.skill_queues.entry(character_id);
        if let Some(t) = q.pause_in(&reg.content, skills, now, &attribute) {
            credited.push(t);
        }
        reg.mark_dirty();
    }
    Frozen {
        was_training,
        credited,
    }
}

/// **After the attributes changed**: start the head again from now at the new
/// rate, and tell the client what the freeze credited
/// (`OnServerSkillsChanged`; the queue itself is re-read by the client's own
/// `OnServerImplantsChanged` handler, which refreshes it when there is one).
pub(crate) fn thaw_training(reg: &mut Registry, character_id: i64, frozen: Frozen) {
    restart_head(reg, character_id, &frozen);
    if let Some(push) = skills_changed_push(reg, character_id, &frozen.credited) {
        reg.push(push);
    }
}

/// [`thaw_training`]'s restart without its push: the head starts again from
/// now if it was training when [`freeze_training`] ran.
fn restart_head(reg: &mut Registry, character_id: i64, frozen: &Frozen) {
    if frozen.was_training {
        let q = reg.skill_queues.entry(character_id);
        if !q.entries.is_empty() {
            q.start = Some(crate::pyutil::filetime_now());
        }
        reg.mark_dirty();
    }
}

impl Frozen {
    /// Add skills the caller changed while frozen, so [`thaw_training`] tells
    /// the client about them in its `OnServerSkillsChanged`.
    pub(crate) fn note(&mut self, type_ids: &[i64]) {
        for t in type_ids {
            if !self.credited.contains(t) {
                self.credited.push(*t);
            }
        }
    }
}

/// **Change something that sets the training rate for every running queue**
/// (the `training_mult` rule), without re-pricing time already trained.
///
/// Each training queue is frozen at the old rate ([`freeze_training`]), then
/// `apply` runs, then each head restarts from now at the new rate. The
/// clock's side, not a payload turn: nothing is pushed on the acting
/// session; each pilot's `OnServerSkillsChanged` and `OnNewSkillQueueSaved`
/// go through [`Registry::schedule`] (an offline pilot has no session, so the
/// push goes nowhere and the next login reads the queue). A paused queue has
/// no rate to re-price and is left alone.
pub(crate) fn retime_all(reg: &mut Registry, apply: impl FnOnce(&mut Registry)) {
    let frozen: Vec<(i64, Frozen)> = reg
        .skill_queues
        .training()
        .into_iter()
        .map(|id| (id, freeze_training(reg, id)))
        .collect();
    apply(reg);
    for (id, f) in frozen {
        restart_head(reg, id, &f);
        if let Some(push) = skills_changed_push(reg, id, &f.credited) {
            reg.schedule(Duration::ZERO, push);
        }
        let rows = queue_rows(reg, id);
        reg.schedule(
            Duration::ZERO,
            crate::skill::QueueSaved {
                character_id: id,
                rows,
            },
        );
    }
}

/// Settle one character's queue at `now`: every level whose time has come
/// is trained. The type ids whose level moved; the world is marked dirty if
/// the queue or a skill changed.
fn settle_training(reg: &mut Registry, character_id: i64, now: i64) -> Vec<i64> {
    let attribute = paced_attribute(reg, character_id);
    let Some(skills) = reg.skills.get_mut(&character_id) else {
        return Vec::new();
    };
    let q = reg.skill_queues.entry(character_id);
    let before = q.clone();
    let trained = q.settle_in(&reg.content, skills, now, &attribute);
    if *q != before {
        reg.mark_dirty();
    }
    trained
}

/// `OnServerSkillsChanged` for these skills of this character, or `None`
/// when there is nothing to say.
fn skills_changed_push(
    reg: &Registry,
    character_id: i64,
    type_ids: &[i64],
) -> Option<crate::skill::SkillsChanged> {
    let skills = reg.skills.get(&character_id)?;
    let entries: Vec<crate::skill::SkillEntry> = type_ids
        .iter()
        .filter_map(|t| skills.get(*t).cloned())
        .collect();
    (!entries.is_empty()).then(|| crate::skill::SkillsChanged {
        character_id,
        entries,
        // Taken when the push is built, which is when it is sent to within a
        // tick. Must increase across pushes: see `SkillsChanged`.
        time_stamp: crate::pyutil::filetime_now(),
    })
}

/// **The world clock's hook**: every level whose end time has passed is
/// trained, and its pilot told.
///
/// Scheduled rather than pushed, because there is no payload turn to ride:
/// a skill finishing is the server speaking first, which is what
/// `Registry::schedule` is for. A pilot who is offline has no session, so
/// the push goes nowhere and the next login reads the trained level from
/// `GetSkills` -- training does not need anyone connected.
pub(crate) fn skill_training_tick(reg: &mut Registry, now: i64) {
    for character_id in reg.skill_queues.training() {
        let trained = settle_training(reg, character_id, now);
        if let Some(push) = skills_changed_push(reg, character_id, &trained) {
            reg.schedule(Duration::ZERO, push);
            // A new level changes what the skill gives the client's dogma.
            crate::brain::announce_later(reg, character_id);
        }
    }
}

/// `GetSkillQueueAndFreePoints`' first half: the queue as `KeyVal` rows.
///
/// Settles first and tells the client about anything that finished, so a
/// read can never hand back a queue whose head is already trained while the
/// skill caches say otherwise.
pub(crate) fn skill_queue_rows(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    let Some(id) = actor else {
        return PyValue::List(vec![]);
    };
    let trained = settle_training(reg, id, crate::pyutil::filetime_now());
    if let Some(push) = skills_changed_push(reg, id, &trained) {
        reg.push(push);
        crate::brain::announce(reg, id);
    }
    queue_rows(reg, id)
}

/// The queue rows as the client reads them, with no settle and no push.
fn queue_rows(reg: &Registry, id: i64) -> PyValue {
    let (Some(skills), Some(q)) = (reg.skills.get(&id), reg.skill_queues.get(id)) else {
        return PyValue::List(vec![]);
    };
    PyValue::List(
        q.rows_in(reg.content(), skills, &paced_attribute(reg, id))
            .iter()
            .map(crate::skill::QueueRow::to_pyvalue)
            .collect(),
    )
}

/// `SaveNewQueue(queueInfo, activate=activate)`.
///
/// `skillQueueSvc.CommitTransaction`, `skillQueueSvc.py:224`-`:227` **[V]**:
/// `queueInfo = {idx: (x.trainingTypeID, x.trainingToLevel) for idx, x in
/// enumerate(self.skillQueue)}`, one positional **dict keyed by position**,
/// and `activate` as a **keyword**. The return is discarded. What matters is
/// what it raises: anything but three named errors rolls the player's edit
/// back (`:227`'s `except`), which is right for a queue we refuse and is
/// why a stale level is dropped rather than refused (`Queue::save`).
///
/// No `OnNewSkillQueueSaved` back. The client keeps its own optimistic copy
/// on commit, and every caller of `CommitTransaction` in the queue panel
/// opens a fresh transaction afterwards (`ApplySkillQueue`'s `finally`),
/// which re-reads `GetSkillQueueAndFreePoints` -- so the server's times
/// arrive on the very next call anyway.
pub(crate) fn save_new_queue(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(id) = req.actor else {
        return Ok(PyValue::None);
    };
    let wanted = queue_info(req)?;
    // `CommitTransaction(self, activate=True)`: absent means True.
    let activate = match crate::pyutil::pairs_get(req.kwargs, b"activate") {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        Some(PyValue::None) => false,
        _ => true,
    };
    let now = crate::pyutil::filetime_now();
    let mut changed = settle_training(reg, id, now);
    let levels_trained = !changed.is_empty();
    let attribute = paced_attribute(reg, id);
    let Some(skills) = reg.skills.get_mut(&id) else {
        return Err(queue_refusal(crate::skill::QueueError::NotInjected(0), req));
    };
    let credited = reg
        .skill_queues
        .entry(id)
        .save_in(&reg.content, skills, &wanted, activate, now, &attribute)
        .map_err(|e| queue_refusal(e, req))?;
    reg.mark_dirty();
    if let Some(t) = credited.filter(|t| !changed.contains(t)) {
        changed.push(t);
    }
    if let Some(push) = skills_changed_push(reg, id, &changed) {
        reg.push(push);
    }
    if levels_trained {
        crate::brain::announce(reg, id);
    }
    Ok(PyValue::None)
}

/// `AbortTraining()`: pause.
///
/// `skillsvc.AbortTrain` (`skillsvc.py:918`, result `POP_TOP`) **[V]**, called
/// only from the queue panel's pause button, which then saves with
/// `activate=False` (`skillQueuePanelNew.PauseTraining`) **[V]**. The head
/// skill keeps what it trained so far, and the client is told both that its
/// points moved and that the queue stopped.
pub(crate) fn abort_training(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(id) = req.actor else {
        return Ok(PyValue::None);
    };
    let now = crate::pyutil::filetime_now();
    let mut changed = settle_training(reg, id, now);
    let levels_trained = !changed.is_empty();
    let attribute = paced_attribute(reg, id);
    let Some(skills) = reg.skills.get_mut(&id) else {
        return Ok(PyValue::None);
    };
    let q = reg.skill_queues.entry(id);
    if !q.is_training() {
        return Ok(PyValue::None);
    }
    if let Some(t) = q.pause_in(&reg.content, skills, now, &attribute) {
        changed.push(t);
    }
    reg.mark_dirty();
    if let Some(push) = skills_changed_push(reg, id, &changed) {
        reg.push(push);
    }
    if levels_trained {
        crate::brain::announce(reg, id);
    }
    reg.push(crate::skill::QueuePaused { character_id: id });
    Ok(PyValue::None)
}

/// `queueInfo` as `(typeID, toLevel)` in position order.
///
/// A marshal dict is `(value, key)` pairs and its order is the client's hash
/// order, not the queue's, so it is sorted by the key.
fn queue_info(req: &Request<'_>) -> Result<Vec<(i64, i64)>, ServiceError> {
    let bad = |why: &str| ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    };
    let Some(PyValue::Dict(pairs)) = req.args.first() else {
        return Err(bad(
            "queueInfo must be a dict {position: (typeID, toLevel)}",
        ));
    };
    let mut entries = Vec::with_capacity(pairs.len());
    for (value, key) in pairs {
        let PyValue::Int(position) = key else {
            return Err(bad("a queue position is not an integer"));
        };
        let (PyValue::Tuple(pair) | PyValue::List(pair)) = value else {
            return Err(bad("a queue entry is not a (typeID, toLevel) pair"));
        };
        let [PyValue::Int(type_id), PyValue::Int(level)] = pair.as_slice() else {
            return Err(bad("a queue entry is not a (typeID, toLevel) pair"));
        };
        entries.push((*position, *type_id, *level));
    }
    entries.sort_unstable_by_key(|e| e.0);
    Ok(entries.into_iter().map(|(_, t, l)| (t, l)).collect())
}

/// A refused queue, in the client's own words where it has them.
///
/// The two labels are the ones `CheckCanInsertSkillAtPosition` raises for
/// the same conditions, with no keywords, so the client already renders
/// them (`skillQueueSvc.py`, the `checkedErrors` tuple) **[V]**. The other two
/// are only reachable by a client that skipped its own checks.
fn queue_refusal(e: crate::skill::QueueError, req: &Request<'_>) -> ServiceError {
    use crate::skill::QueueError;
    let label = match e {
        QueueError::NotInjected(_) => "QueueSkillNotUploaded",
        QueueError::OutOfOrder(..) => "QueueCannotPlaceSkillLevelsOutOfOrder",
        QueueError::BeforeRequirements(..) => "QueueCannotPlaceSkillBeforeRequirements",
        QueueError::BadLevel(..) | QueueError::TooLong(_) => {
            return ServiceError::BadArguments {
                service: req.service.to_owned(),
                method: req.method.to_owned(),
                why: format!("{e:?}"),
            };
        }
    };
    ServiceError::Raise(eve_proto::user_error(label, vec![]))
}

/// `dogmaIM.InjectSkillIntoBrain(itemIDs)`: turn skillbooks into skills.
///
/// **Call shape [V]**, `skillsvc.InjectSkillIntoBrain` (`skillsvc.py:892`-
/// `:915`, read with `tools/dis.py`): one positional **list of item ids**
/// (`[skill.itemID for skill in skillX]`, `:897`) on the bound `dogmaIM`
/// (`godma.GetDogmaLM()`), result `POP_TOP`. A `UserError` is re-raised to
/// the player (`:907`-`:908`); success shows the "skill injected"
/// notification and plays the sound itself (`:910`-`:915`). The client has
/// already refused an unpublished type (`ItemNotASkill`, `:903`) and only
/// offers the menu entry for a skill not yet injected
/// (`itemCheckers.OfferInjectSkill`, `:449`-`:460`).
///
/// What the server does: each book is consumed -- one off a stack, or the
/// item -- and the skill is added at level 0 with no points, exactly as
/// `PurchaseSkills` adds one (`Skills::purchase`; `docs/reference/skill-system.md` §5:
/// purchase is injection without the item). Then `OnServerSkillsChanged`
/// with the new entries, which is what fills the client's skill cache
/// inside the 500 ms `invItemFunctions.TrainNow` sleeps before it queues the
/// skill (`:92`) **[V]**. For a skill new to the cache the handler's
/// "levels trained" range is `xrange(1, 0 + 1)`, empty
/// (`_UpdateMySkillsAndReturnLevelsTrained`, `:216`-`:221`) **[V]**, so the
/// push plays no "skill trained" sound for a level-0 skill.
///
/// All or nothing, like a purchase: every id is validated before anything is
/// consumed. Refusals use the client's own labels, both with the type id in
/// `skillName` (`findings/user-errors.tsv`: `[item]skillName.name`):
/// `ItemNotASkill` for a book that is not a skill, `CharacterAlreadyKnowsSkill`
/// for one already injected (or named twice).
///
/// **[A]** a book must be the pilot's own and either in their station (the
/// hangar) or in their ship. That is what the client's `IsInPilotLocation`
/// gate offers; TQ's own check is not in the client. No prerequisite check:
/// the client makes none before injecting, and the queue enforces
/// prerequisites when the skill is trained ([`crate::skill::unmet_requirement`]).
pub(crate) fn inject_skill_into_brain(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let bad = |why: &str| ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    };
    let refuse = |label: &str, type_id: i64| {
        ServiceError::Raise(eve_proto::user_error(
            label,
            crate::pyutil::pypairs(vec![("skillName", PyValue::Int(type_id))]),
        ))
    };
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let ids: Vec<i64> = match req.args.first() {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect::<Option<_>>()
            .ok_or_else(|| bad("an item id that is not an int"))?,
        _ => return Err(bad("(itemIDs,)")),
    };
    let (station, ship) = match reg.character(actor) {
        Some(c) => (c.station_id, c.ship_id),
        None => return Err(bad("no such character")),
    };
    let Some(skills) = reg.skills.get(&actor) else {
        return Err(bad("the character has no skill record"));
    };
    let mut plan = Vec::new();
    let mut types: Vec<i64> = Vec::new();
    for id in ids {
        let item = reg
            .store()
            .get(id)
            .filter(|i| i.owner_id == actor && i.quantity >= 1)
            .filter(|i| Some(i.location_id) == station || i.location_id == ship)
            .ok_or_else(|| bad(&format!("item {id} is not a book in the pilot's location")))?;
        if item.category_id != crate::skill::CATEGORY_SKILL
            || crate::skill::find_in(reg.content(), item.type_id).is_none()
        {
            return Err(refuse("ItemNotASkill", item.type_id));
        }
        if skills.get(item.type_id).is_some() || types.contains(&item.type_id) {
            return Err(refuse("CharacterAlreadyKnowsSkill", item.type_id));
        }
        plan.push(if item.quantity > 1 {
            crate::item::Edit::Consume {
                item_id: id,
                quantity: 1,
            }
        } else {
            crate::item::Edit::Destroy { item_id: id }
        });
        types.push(item.type_id);
    }
    if types.is_empty() {
        return Ok(PyValue::None);
    }
    reg.apply_items(&plan)
        .map_err(|e| bad(&format!("consuming the books: {e:?}")))?;
    // Cannot fail: every type was checked to be a known skill the character
    // does not have, one id each, and nothing ran in between.
    if let Some(skills) = reg.skills.get_mut(&actor) {
        let _ = skills.purchase_in(&reg.content, &types);
    }
    reg.mark_dirty();
    if let Some(push) = skills_changed_push(reg, actor, &types) {
        reg.push(push);
    }
    crate::brain::announce(reg, actor);
    Ok(PyValue::None)
}

#[cfg(test)]
mod skill_training_tests {
    use super::*;
    use crate::model::SandboxConfig;
    use crate::pyutil::bstr;

    /// The tests were written inside `service/character.rs` and called its
    /// `dispatch`; `Registry::dispatch` routes `skillMgr2::skillHandler` to
    /// that one function (no service is claimed by two modules), so this is
    /// the same call made from outside the private `service::character`
    /// module.
    fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        reg.dispatch(req)
    }

    /// `rules.training_mult` multiplies the pace the queue trains at and
    /// the attributes the client is told; the default multiplies by 1.
    #[test]
    fn the_training_mult_rule_scales_the_pace_and_the_told_attributes() {
        let plain = Registry::new(SandboxConfig::default());
        let fast = Registry::new(SandboxConfig {
            rules: crate::rules::Rules {
                training_mult: 4.0,
                ..Default::default()
            },
            ..SandboxConfig::default()
        });
        let clock = crate::game_clock::of(&plain).training_pace();
        assert_eq!(training_pace(&plain), clock);
        assert_eq!(training_pace(&fast), clock * 4.0);
        let id = actor(&plain).expect("a character");
        assert_eq!(
            client_attribute(&fast, id, 165),
            client_attribute(&plain, id, 165) * 4.0
        );
    }

    const MINMATAR_FRIGATE: i64 = 3329;

    fn actor(reg: &Registry) -> Option<i64> {
        Some(reg.config().characters[0].character_id)
    }

    fn call(
        reg: &mut Registry,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        let actor = actor(reg);
        dispatch(
            reg,
            &Request {
                service: "skillMgr2::skillHandler",
                method,
                args,
                kwargs,
                bind_params: None,
                actor,
            },
        )
    }

    /// `{position: (typeID, toLevel)}`, `(value, key)` on the wire.
    fn queue_info(entries: &[(i64, i64)]) -> PyValue {
        PyValue::Dict(
            entries
                .iter()
                .enumerate()
                .map(|(i, (t, l))| {
                    (
                        PyValue::Tuple(vec![PyValue::Int(*t), PyValue::Int(*l)]),
                        PyValue::Int(i as i64),
                    )
                })
                .collect(),
        )
    }

    fn activate(on: bool) -> Vec<(PyValue, PyValue)> {
        vec![(PyValue::Bool(on), bstr("activate"))]
    }

    /// Minmatar Frigate's training rate for the test character, through the
    /// one attribute function.
    fn frigate_rate(reg: &Registry) -> f64 {
        let id = actor(reg).expect("a character");
        crate::skill::rate_for_in(reg.content(), MINMATAR_FRIGATE, &|a| {
            skill_attribute(reg, id, a)
        })
        .expect("known")
    }

    /// A registry whose one character owns Minmatar Frigate at level 0 --
    /// the skill the starter set lacks, bought the way a player buys it.
    fn with_frigate_bought() -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call(
            &mut reg,
            "PurchaseSkills",
            &[PyValue::List(vec![PyValue::Int(MINMATAR_FRIGATE)])],
            &[],
        )
        .expect("answered");
        assert_eq!(got, PyValue::List(vec![PyValue::Int(MINMATAR_FRIGATE)]));
        reg.take_pending();
        reg
    }

    fn queue(reg: &mut Registry) -> Vec<PyValue> {
        let got = call(reg, "GetSkillQueueAndFreePoints", &[], &[]).expect("answered");
        let PyValue::Tuple(pair) = got else {
            panic!("(skillQueue, freeSkillPoints)")
        };
        let PyValue::List(rows) = &pair[0] else {
            panic!("the queue is a list")
        };
        rows.clone()
    }

    fn field<'a>(row: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = row else {
            panic!("a queue row is a KeyVal")
        };
        crate::pyutil::dict_get(state, name.as_bytes()).expect(name)
    }

    #[test]
    fn a_saved_queue_reads_back_with_the_clients_end_time() {
        let mut reg = with_frigate_bought();
        let saved = call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)])],
            &activate(true),
        );
        assert_eq!(saved, Ok(PyValue::None));
        let rows = queue(&mut reg);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            field(&rows[0], "trainingTypeID"),
            &PyValue::Int(MINMATAR_FRIGATE)
        );
        assert_eq!(field(&rows[1], "trainingToLevel"), &PyValue::Int(2));
        let (PyValue::Int(start), PyValue::Int(end)) = (
            field(&rows[0], "trainingStartTime"),
            field(&rows[0], "trainingEndTime"),
        ) else {
            panic!("a training head has FILETIME start and end")
        };
        // 500 SP (rank 2, level 1) at whatever rate the attributes give --
        // the same function `GetAttributes` answers, which is the point.
        assert_eq!(
            end - start,
            crate::skill::training_ticks(500, frigate_rate(&reg))
        );
        assert_eq!(field(&rows[1], "trainingStartTime"), &PyValue::Int(*end));
    }

    /// Lane clock: at scale N the head trains N times sooner, the row the
    /// client reads says so, and the clock trains it at that end.
    #[test]
    fn at_scale_n_a_level_trains_n_times_sooner() {
        let mut reg = with_frigate_bought();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(5).unwrap());
        let id = actor(&reg).expect("a character");
        call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1)])],
            &activate(true),
        )
        .expect("saved");
        let rows = queue(&mut reg);
        let (PyValue::Int(start), PyValue::Int(end)) = (
            field(&rows[0], "trainingStartTime"),
            field(&rows[0], "trainingEndTime"),
        ) else {
            panic!("a training head has FILETIME start and end")
        };
        let real = crate::skill::training_ticks(500, frigate_rate(&reg));
        let paced = crate::skill::training_ticks(500, 5.0 * frigate_rate(&reg));
        assert_eq!(end - start, paced);
        assert!((real - 5 * paced).abs() <= 5, "{real} vs 5 x {paced}");

        skill_training_tick(&mut reg, *end - 1);
        assert_eq!(reg.scheduled_count(), 0, "not yet");
        skill_training_tick(&mut reg, *end);
        assert_eq!(
            reg.skills
                .get(&id)
                .and_then(|s| s.get(MINMATAR_FRIGATE))
                .map(|s| s.level),
            Some(1)
        );
    }

    /// The attributes `GetAttributes` answers, as the client reads them.
    fn served_attributes(reg: &mut Registry) -> std::collections::BTreeMap<i64, f64> {
        let PyValue::Dict(entries) = call(reg, "GetAttributes", &[], &[]).expect("answered") else {
            panic!("a dict of attributes")
        };
        entries
            .into_iter()
            .map(|(v, k)| match (k, v) {
                (PyValue::Int(k), PyValue::Real(v)) => (k, v),
                other => panic!("an attribute row: {other:?}"),
            })
            .collect()
    }

    /// Lane bot-ctl: `--training-scale M` on a clock at `--time-scale N`
    /// trains N x M times sooner; the head's end, the entries after it (which
    /// the client recomputes from `GetAttributes`, `skillQueueSvc.
    /// UpdateQueueTimestamps` [V]) and the clock all agree.
    #[test]
    fn training_scale_multiplies_with_time_scale_and_the_clients_view_agrees() {
        const TIME: u32 = 3;
        const TRAIN: u32 = 7;
        let mut reg = with_frigate_bought();
        let real_rate = frigate_rate(&reg);
        let clock = crate::game_clock::GameClock::new(TIME)
            .unwrap()
            .with_training(TRAIN)
            .unwrap();
        crate::game_clock::set(&mut reg, clock);
        assert!(!clock.is_real() && clock.training() == TRAIN);
        let id = actor(&reg).expect("a character");
        call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)])],
            &activate(true),
        )
        .expect("saved");
        let rows = queue(&mut reg);
        let ints = |row: &PyValue, names: [&str; 4]| -> Vec<i64> {
            names
                .iter()
                .map(|n| match field(row, n) {
                    PyValue::Int(v) => *v,
                    other => panic!("{n}: {other:?}"),
                })
                .collect()
        };
        let names = [
            "trainingStartTime",
            "trainingEndTime",
            "trainingStartSP",
            "trainingDestinationSP",
        ];
        let (first, second) = (ints(&rows[0], names), ints(&rows[1], names));
        let pace = f64::from(TIME * TRAIN);
        assert_eq!(
            first[1] - first[0],
            crate::skill::training_ticks(first[3] - first[2], pace * real_rate)
        );
        // What the client does for the second entry: its own rate from the
        // attributes it was served, over the same points.
        let served = served_attributes(&mut reg);
        let client_rate =
            crate::skill::rate_for_in(reg.content(), MINMATAR_FRIGATE, &|a| served[&a])
                .expect("known");
        assert_eq!(
            second[1] - second[0],
            crate::skill::training_ticks(second[3] - second[2], client_rate),
            "the client's recomputed end is the server's"
        );
        assert_eq!(second[0], first[1]);
        // And the first, which the client takes from the row, is also what
        // its own formula gives from the served attributes.
        assert_eq!(
            first[1] - first[0],
            crate::skill::training_ticks(first[3] - first[2], client_rate)
        );
        assert!(client_rate > pace * real_rate * 0.999 && client_rate < pace * real_rate * 1.001);
        skill_training_tick(&mut reg, first[1]);
        assert_eq!(
            reg.skills
                .get(&id)
                .and_then(|s| s.get(MINMATAR_FRIGATE))
                .map(|s| s.level),
            Some(1)
        );
        // Scale 1 / 1 serves the plain attributes.
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::REAL);
        assert_eq!(
            served_attributes(&mut reg)[&ATTR_PERCEPTION],
            skill_attribute(&reg, id, ATTR_PERCEPTION)
        );
    }

    #[test]
    fn training_scale_parses_within_its_bounds() {
        use crate::game_clock::GameClock as C;
        assert_eq!(C::training_from_flag_or_env(None, None), Ok(1));
        assert_eq!(
            C::training_from_flag_or_env(Some("10000"), None),
            Ok(10_000)
        );
        assert_eq!(C::training_from_flag_or_env(None, Some("25")), Ok(25));
        assert_eq!(C::training_from_flag_or_env(Some("2"), Some("25")), Ok(2));
        for bad in ["0", "10001", "x", "-3", ""] {
            assert!(
                C::training_from_flag_or_env(Some(bad), None).is_err(),
                "{bad}"
            );
        }
        assert_eq!(C::REAL.training_pace(), 1.0);
        assert_eq!(
            C::new(4).unwrap().with_training(5).unwrap().training_pace(),
            20.0
        );
    }

    #[test]
    fn a_queue_level_out_of_order_is_refused_in_the_clients_words() {
        let mut reg = with_frigate_bought();
        let got = call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 2)])],
            &activate(true),
        );
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert!(queue(&mut reg).is_empty());
    }

    /// The clock trains the level and tells the pilot with the push that
    /// carries the new entry.
    #[test]
    fn the_clock_trains_a_level_and_pushes_it() {
        let mut reg = with_frigate_bought();
        let id = actor(&reg).expect("a character");
        call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1)])],
            &activate(true),
        )
        .expect("saved");
        let start = reg
            .skill_queues
            .get(id)
            .and_then(|q| q.start)
            .expect("training");
        let end = start + crate::skill::training_ticks(500, frigate_rate(&reg));

        skill_training_tick(&mut reg, end - 1);
        assert_eq!(reg.scheduled_count(), 0, "not yet");

        skill_training_tick(&mut reg, end);
        let s = reg
            .skills
            .get(&id)
            .and_then(|s| s.get(MINMATAR_FRIGATE))
            .cloned();
        assert_eq!(s.map(|s| (s.level, s.points)), Some((1, 500)));
        let due = reg.take_due(std::time::Instant::now() + Duration::from_secs(5));
        let events: Vec<&str> = due.iter().map(|p| p.to_push().0).collect();
        assert_eq!(
            events,
            vec!["OnServerSkillsChanged", "OnServerBrainUpdated"]
        );
        assert_eq!(due[0].audience(), Some(vec![id]));
        assert!(!reg.skill_queues.get(id).is_some_and(|q| q.is_training()));
    }

    #[test]
    fn a_pause_stops_the_clock_and_keeps_the_queue() {
        let mut reg = with_frigate_bought();
        call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1)])],
            &activate(true),
        )
        .expect("saved");
        assert_eq!(call(&mut reg, "AbortTraining", &[], &[]), Ok(PyValue::None));
        let rows = queue(&mut reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(field(&rows[0], "trainingEndTime"), &PyValue::None);
    }

    #[test]
    fn the_queue_survives_a_restart() {
        let mut reg = with_frigate_bought();
        let id = actor(&reg).expect("a character");
        call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1)])],
            &activate(true),
        )
        .expect("saved");
        assert!(reg.take_dirty());
        let snap = reg.to_snapshot();
        let mut back = Registry::new(SandboxConfig::default());
        back.apply_snapshot(snap);
        assert_eq!(back.skill_queues.get(id), reg.skill_queues.get(id));
        assert!(back.skill_queues.get(id).is_some_and(|q| q.is_training()));
    }

    #[test]
    fn get_skill_points_is_the_trained_total_as_an_int() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg).expect("a character");
        let want = reg
            .skills
            .get(&id)
            .map(|s| s.total_points())
            .expect("starter set");
        assert_eq!(
            call(&mut reg, "GetSkillPoints", &[], &[]),
            Ok(PyValue::Int(want))
        );
    }

    // --- character attributes ---------------------------------------------

    /// The client's own remap accounting, `appConst.pyc:1850`-`:1852`: each
    /// attribute in 17..=27 and the five summing to 99, or the attributes
    /// window shows points over- or under-spent. Which attribute holds which
    /// value is not asserted: that part is `[A]`.
    #[test]
    fn get_attributes_obey_the_clients_remap_rules() {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call(&mut reg, "GetAttributes", &[], &[]).expect("answered");
        let PyValue::Dict(pairs) = got else {
            panic!("a dict")
        };
        let mut ids: Vec<i64> = Vec::new();
        let mut sum = 0.0;
        for (v, k) in &pairs {
            let (PyValue::Real(v), PyValue::Int(k)) = (v, k) else {
                panic!("{{int: float}}")
            };
            assert!((17.0..=27.0).contains(v), "{k} = {v}");
            sum += v;
            ids.push(*k);
        }
        ids.sort_unstable();
        assert_eq!(ids, vec![164, 165, 166, 167, 168]);
        assert_eq!(sum, 99.0);
    }

    /// `GetAttributes` and the training clock read one function.
    #[test]
    fn get_attributes_is_what_the_clock_trains_with() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg).expect("a character");
        let got = call(&mut reg, "GetAttributes", &[], &[]).expect("answered");
        let PyValue::Dict(pairs) = got else {
            panic!("a dict")
        };
        for (v, k) in &pairs {
            let (PyValue::Real(v), PyValue::Int(k)) = (v, k) else {
                panic!("{{int: float}}")
            };
            assert_eq!(*v, skill_attribute(&reg, id, *k));
        }
    }

    // --- skillbook injection -----------------------------------------------

    fn inject(reg: &mut Registry, item_ids: &[i64]) -> Result<PyValue, ServiceError> {
        let actor = actor(reg);
        let args = [PyValue::List(
            item_ids.iter().map(|i| PyValue::Int(*i)).collect(),
        )];
        reg.dispatch(&Request {
            service: "dogmaIM",
            method: "InjectSkillIntoBrain",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor,
        })
    }

    /// A book of `type_id` in the character's station hangar; its item id.
    fn book(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let id = actor(reg).expect("a character");
        let station = reg
            .character(id)
            .and_then(|c| c.station_id)
            .expect("docked");
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity,
                to: crate::item::Slot::new(id, station, crate::model::FLAG_HANGAR),
            }])
            .expect("created");
        reg.take_pending();
        changes[0].item_id()
    }

    /// Whether `e` raises `UserError(label, ...)`. The label is marshal
    /// bytes, so it is matched as bytes.
    fn raises(e: &ServiceError, label: &str) -> bool {
        format!("{e:?}").contains(&format!("Bytes({:?})", label.as_bytes()))
    }

    #[test]
    fn an_injected_book_is_consumed_and_the_skill_arrives_at_level_zero() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg).expect("a character");
        let item = book(&mut reg, MINMATAR_FRIGATE, 1);
        assert_eq!(inject(&mut reg, &[item]), Ok(PyValue::None));
        assert!(reg.store().get(item).is_none(), "the book is gone");
        let s = reg
            .skills
            .get(&id)
            .and_then(|s| s.get(MINMATAR_FRIGATE))
            .cloned();
        assert_eq!(s.map(|s| (s.level, s.points)), Some((0, 0)));
        let events: Vec<&str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        // A destroyed item has no row left, so `Registry::apply_items` tells
        // the hangar list by a junkyard `OnItemChange`, then invalidates.
        assert_eq!(
            events,
            vec![
                "OnItemChange",
                "OnInvalidateCacheForLocations",
                "OnServerSkillsChanged",
                "OnServerBrainUpdated"
            ]
        );
    }

    #[test]
    fn injecting_from_a_stack_takes_one_book() {
        let mut reg = Registry::new(SandboxConfig::default());
        let item = book(&mut reg, MINMATAR_FRIGATE, 3);
        inject(&mut reg, &[item]).expect("injected");
        assert_eq!(reg.store().get(item).map(|i| i.quantity), Some(2));
    }

    #[test]
    fn a_known_skill_is_refused_and_nothing_is_consumed() {
        let mut reg = Registry::new(SandboxConfig::default());
        // Gunnery is in the starter set.
        let item = book(&mut reg, 3300, 1);
        let got = inject(&mut reg, &[item]).expect_err("refused");
        assert!(raises(&got, "CharacterAlreadyKnowsSkill"), "{got:?}");
        assert!(reg.store().get(item).is_some());
    }

    #[test]
    fn the_same_skill_twice_in_one_call_is_refused_whole() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg).expect("a character");
        let a = book(&mut reg, MINMATAR_FRIGATE, 1);
        let b = book(&mut reg, MINMATAR_FRIGATE, 1);
        let got = inject(&mut reg, &[a, b]).expect_err("refused");
        assert!(raises(&got, "CharacterAlreadyKnowsSkill"), "{got:?}");
        assert!(reg.store().get(a).is_some() && reg.store().get(b).is_some());
        assert!(
            reg.skills
                .get(&id)
                .and_then(|s| s.get(MINMATAR_FRIGATE))
                .is_none()
        );
    }

    #[test]
    fn an_item_that_is_not_a_skill_is_refused_in_the_clients_words() {
        let mut reg = Registry::new(SandboxConfig::default());
        let id = actor(&reg).expect("a character");
        let ship = reg.character(id).map(|c| c.ship_id).expect("a ship");
        let got = inject(&mut reg, &[ship]).expect_err("refused");
        assert!(raises(&got, "ItemNotASkill"), "{got:?}");
    }

    #[test]
    fn a_queue_needing_an_untrained_prerequisite_is_refused_in_the_clients_words() {
        let got = queue_refusal(
            crate::skill::QueueError::BeforeRequirements(MINMATAR_FRIGATE, 3327),
            &Request {
                service: "skillMgr2::skillHandler",
                method: "SaveNewQueue",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        );
        assert!(
            raises(&got, "QueueCannotPlaceSkillBeforeRequirements"),
            "{got:?}"
        );
    }

    // ---- neural remap (lane gap-remap) ----

    /// A valid set (17..27, sum 99), in `[per, cha, int, mem, wil]`.
    const REMAP_A: [i64; 5] = [25, 17, 17, 20, 20];
    /// Another valid set.
    const REMAP_B: [i64; 5] = [17, 27, 18, 18, 19];

    fn refused(r: &Result<PyValue, ServiceError>, label: &str) -> bool {
        r.as_ref().err().is_some_and(|e| raises(e, label))
    }

    fn respec_arg(v: [i64; 5]) -> PyValue {
        PyValue::Dict(
            REMAP_ORDER
                .iter()
                .zip(v)
                .map(|(k, v)| (PyValue::Int(v), PyValue::Int(*k)))
                .collect(),
        )
    }

    fn respec(reg: &mut Registry, v: [i64; 5]) -> Result<PyValue, ServiceError> {
        call(reg, "RespecCharacter", &[respec_arg(v)], &[])
    }

    fn respec_info_of(reg: &mut Registry) -> (i64, i64) {
        let info = call(reg, "GetRespecInfo", &[], &[]).expect("answered");
        let (Some(PyValue::Int(n)), Some(PyValue::Int(f))) = (
            crate::pyutil::dict_get(&info, b"nextTimedRespec"),
            crate::pyutil::dict_get(&info, b"freeRespecs"),
        ) else {
            panic!("both keys are integers")
        };
        (*n, *f)
    }

    fn served(reg: &mut Registry) -> Vec<f64> {
        let PyValue::Dict(d) = call(reg, "GetAttributes", &[], &[]).expect("answered") else {
            panic!("a dict")
        };
        REMAP_ORDER
            .iter()
            .map(|k| {
                d.iter()
                    .find_map(|(v, key)| match (v, key) {
                        (PyValue::Real(r), PyValue::Int(i)) if i == k => Some(*r),
                        _ => None,
                    })
                    .expect("present")
            })
            .collect()
    }

    #[test]
    fn an_invalid_remap_is_refused_and_changes_nothing() {
        let mut reg = Registry::new(SandboxConfig::default());
        let before = served(&mut reg);
        let sum_100 = [25, 17, 17, 21, 20];
        let low = [16, 27, 17, 20, 19];
        let high = [28, 17, 17, 18, 19];
        assert!(refused(
            &respec(&mut reg, sum_100),
            "RespecAttributesMisallocated"
        ));
        assert!(refused(&respec(&mut reg, low), "RespecAttributesTooLow"));
        assert!(refused(&respec(&mut reg, high), "RespecAttributesTooHigh"));
        let four = PyValue::Dict(vec![(PyValue::Int(20), PyValue::Int(ATTR_PERCEPTION))]);
        assert!(refused(
            &call(&mut reg, "RespecCharacter", &[four], &[]),
            "RespecAttributesMisallocated"
        ));
        assert_eq!(served(&mut reg), before);
        assert_eq!(respec_info_of(&mut reg).1, INITIAL_BONUS_REMAPS);
    }

    #[test]
    fn a_remap_stores_the_base_and_moves_the_next_timed_date_a_year() {
        let mut reg = Registry::new(SandboxConfig::default());
        let now = crate::pyutil::filetime_now();
        assert!(respec_info_of(&mut reg).0 <= now, "a timed remap is open");
        assert_eq!(respec(&mut reg, REMAP_A), Ok(PyValue::None));
        let id = actor(&reg).expect("a character");
        let want: Vec<f64> = REMAP_A.iter().map(|v| *v as f64).collect();
        // The base plus the implants, which a fresh character has none of.
        for (i, k) in REMAP_ORDER.iter().enumerate() {
            let implants = crate::implants::attribute_bonus(&reg, id, *k);
            assert_eq!(served(&mut reg)[i], want[i] + implants);
        }
        let (next, free) = respec_info_of(&mut reg);
        assert!(next >= now + REMAP_INTERVAL, "a year ahead");
        assert!(next <= crate::pyutil::filetime_now() + REMAP_INTERVAL);
        assert_eq!(free, INITIAL_BONUS_REMAPS);
    }

    #[test]
    fn a_second_remap_uses_the_bonus_and_a_third_is_refused() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(respec(&mut reg, REMAP_A), Ok(PyValue::None));
        for _ in 0..INITIAL_BONUS_REMAPS {
            assert_eq!(respec(&mut reg, REMAP_B), Ok(PyValue::None));
        }
        assert_eq!(respec_info_of(&mut reg).1, 0);
        let want: Vec<f64> = REMAP_B.iter().map(|v| *v as f64).collect();
        let id = actor(&reg).expect("a character");
        for (i, k) in REMAP_ORDER.iter().enumerate() {
            let implants = crate::implants::attribute_bonus(&reg, id, *k);
            assert_eq!(served(&mut reg)[i], want[i] + implants);
        }
        let got = respec(&mut reg, REMAP_A);
        assert!(refused(&got, "RespecTooSoon"), "{got:?}");
    }

    #[test]
    fn a_remap_survives_a_world_round_trip() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(respec(&mut reg, REMAP_A), Ok(PyValue::None));
        let id = actor(&reg).expect("a character");
        let row = remap_row(&reg, id).expect("stored");
        assert_eq!(row.attributes, REMAP_A);
        assert!(
            crate::delta_kinds::KINDS
                .iter()
                .any(|(k, _)| *k == REMAP_KIND)
        );
    }

    /// The SP credited before the remap is the old rate times the time
    /// elapsed, and the end time after it uses the new rate.
    #[test]
    fn a_remap_mid_training_credits_the_old_rate_and_restarts_at_the_new() {
        const TEN_MINUTES: i64 = 10 * 60 * 10_000_000;
        let mut reg = with_frigate_bought();
        let id = actor(&reg).expect("a character");
        let pace = crate::game_clock::of(&reg).training_pace();
        let old_rate = frigate_rate(&reg) * pace;
        let saved = call(
            &mut reg,
            "SaveNewQueue",
            &[queue_info(&[(MINMATAR_FRIGATE, 1)])],
            &activate(true),
        );
        assert_eq!(saved, Ok(PyValue::None));
        let now = crate::pyutil::filetime_now();
        reg.skill_queues.entry(id).start = Some(now - TEN_MINUTES);
        let held = |reg: &Registry| {
            reg.skills
                .get(&id)
                .and_then(|s| s.get(MINMATAR_FRIGATE))
                .map(|e| e.points)
                .expect("owned")
        };
        let before = held(&reg);
        assert_eq!(respec(&mut reg, REMAP_A), Ok(PyValue::None));
        let credited = held(&reg) - before;
        let owed = crate::skill::sp_trained(TEN_MINUTES, old_rate);
        assert!(
            (credited - owed).abs() <= 2,
            "credited {credited}, owed {owed}"
        );
        let new_rate = frigate_rate(&reg) * pace;
        assert_ne!(new_rate, old_rate, "the remap moved the rate");
        let rows = queue(&mut reg);
        let (PyValue::Int(start), PyValue::Int(end)) = (
            field(&rows[0], "trainingStartTime"),
            field(&rows[0], "trainingEndTime"),
        ) else {
            panic!("FILETIMEs")
        };
        assert!(*start >= now, "restarted at the remap");
        let remaining = 500 - held(&reg);
        assert_eq!(
            end - start,
            crate::skill::training_ticks(remaining, new_rate)
        );
    }
}
