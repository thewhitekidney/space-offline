//! **Boosters** (lane boosters, #79; packets B1 and B2): consuming one,
//! reading the active set, what it does to the sheet and the training
//! rate, its side effects, and its end.
//!
//! A booster is an ordinary item until it is taken. Taken, one is consumed
//! off its stack and the character holds a `booster` delta row instead
//! ([`KIND`]), so there is one source of truth and the item store, the
//! clones and the killmail stay untouched.
//!
//! # What the client does, and so what the server answers **[V]**
//!
//! Citations are the client's, build 3662168, by `co_lnotab`.
//!
//! - **The offer.** `itemCheckers.py:323 OfferConsumeBooster`: group 303, a
//!   non-zero `boosterness` (1087, **the slot**), in the pilot's location,
//!   the slot not already held by one of `GetBoosters()`.
//! - **The call.** `invItemFunctions.py:113 ConsumeBooster`: `godma.
//!   GetDogmaLM().UseBooster(itemID, locationID)`, result discarded; the
//!   "consumed" notification is made by the client, so the server sends none.
//!   An exception shows to the player: a refusal is a `UserError`.
//! - **The read.** `skillsvc.py:1126 GetBoosters()`, cached on `is None`: a
//!   dict keyed by `boosterTypeID` of settable `KeyVal`s with `boosterTypeID`,
//!   `boosterSlot`, `boosterDuration` (ms; falsy means no timer),
//!   `expiryTime` (an absolute FILETIME) and `sideEffectIDs` (a list, there
//!   even when empty); `godma.py:2290` does `setattr(booster, 'typeID', ...)`,
//!   so a `typeID` and an `itemID` ride along.
//! - **The push.** `skillsvc.py:1103 OnServerBoostersChanged(*args)` re-reads
//!   the attributes, the boosters and the implants and refreshes the queue.
//!   It is also the only way the client learns a booster ran out.
//! - **The attributes.** `attributes.py` subtracts accelerator bonuses from
//!   `GetAttributes`, so the answer **includes** them, like an implant's.
//!
//! # The rules
//!
//! - **Duration and chance are the engine's values** on the booster's node
//!   (`dogma::booster_stats`), never the raw type row: Biology raises the
//!   duration, Neurotoxin Recovery and Control move the chances and
//!   penalties through ordinary dogma modifiers.
//! - **A side effect is an effect with a `fittingUsageChanceAttributeID`.**
//!   One die per such effect is rolled when the booster is taken and the
//!   outcome is stored, so a side effect is on for the whole life of the
//!   booster or not at all. A table without that column (a bundle from before
//!   B0) refuses the call, because every penalty would be permanent.
//! - **Expiry** is `now + duration` in FILETIME. A type with no duration but
//!   an age limit (`boosterMaxCharAgeHours`, 1647: the cerebral accelerators)
//!   lasts until the character is that old.
//! - **The clock** ([`tick`]) removes a row at its time, re-bases the
//!   training queue, tells the client and, for a pilot in a ship, the ship's
//!   changed attributes and top speed. A world loaded after the time has
//!   passed drops the row at its first tick; [`inputs_of`] already ignores it.
//! - **Death and clones.** A podding and a jump drop the rows whose type is
//!   destructible (`nonDestructible` 1890 is 0) and, for a jump, that does
//!   not follow jump clones (`followsJumpClones` 1916 is 0).
//!
//! # `[A]`
//!
//! - **No item stays at `flagBooster` (88).** The client uses the flag for
//!   badging and a panel refresh `OnServerBoostersChanged` already covers.
//!   Falsifier: a client path that needs the item.
//! - **Taking one during training is allowed**, as for implants
//!   (`implants.rs`). Falsifier: TQ refusing `FailedBoosterConsume`.
//! - **A chance attribute is a probability in `0..=1`** (unit 105).
//!   Falsifier: a booster whose chance reads above 1.
//! - **Expired types**: `boosterLastInjectionDatetime` (2422) is days since
//!   1970 and a value in the past refuses with `CantConsumeExpiredBooster`.
//!   Falsifier: a current booster refused.
//! - **Age-limited types** use `create_date_time + 1647 h` as the expiry,
//!   with `boosterDuration` 0. Falsifier: a TQ capture.
//! - **The `OnlyOneBoosterActive` arguments**: `typeName` the one being
//!   taken, `typeName2` the one in the slot. Falsifier: the text reading
//!   backwards.
//! - **A required skill** is refused with `ImplantHasSkillPrerequisites`
//!   (the client has no booster-specific key).
//! - **A killmail loses nothing** here; a booster in a left-behind clone is
//!   not kept.

use crate::dogma::{BoosterInput, Inputs};
use crate::pyutil::keyval;
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The delta kind: `char slot -> type expiry_filetime duration_ms side,side,...`
/// (`-` for no side effect).
pub const KIND: &str = "booster";

/// `groupBooster` (303) **[V]**.
pub const GROUP_BOOSTER: i64 = 303;
/// `boosterness` (1087) **[V]**: the slot.
pub const ATTR_BOOSTERNESS: i64 = 1087;
/// `boosterMaxCharAgeHours` (1647) **[V]**.
pub const ATTR_MAX_CHAR_AGE_HOURS: i64 = 1647;
/// `boosterLastInjectionDatetime` (2422) **[V]**.
pub const ATTR_LAST_INJECTION: i64 = 2422;
/// `followsJumpClones` (1916) **[V]**.
pub const ATTR_FOLLOWS_JUMP_CLONES: i64 = 1916;

/// FILETIME ticks (100 ns) in a millisecond, hour and day.
const MS: i64 = 10_000;
const HOUR: i64 = 3_600 * 10_000_000;
const DAY: i64 = 24 * HOUR;
/// 1601 to 1970, in FILETIME ticks.
const UNIX_EPOCH: i64 = 11_644_473_600 * 10_000_000;

/// One active booster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    pub slot: i64,
    pub type_id: i64,
    /// An absolute FILETIME.
    pub expiry: i64,
    pub duration_ms: i64,
    /// The side effects that were rolled in.
    pub side: Vec<i64>,
}

fn key(character_id: i64, slot: i64) -> Vec<String> {
    vec![character_id.to_string(), slot.to_string()]
}

fn parse(k: &[String], v: &[String]) -> Option<Active> {
    let side = match v.get(3).map(String::as_str) {
        None | Some("-") | Some("") => Vec::new(),
        Some(s) => s.split(',').filter_map(|e| e.parse().ok()).collect(),
    };
    Some(Active {
        slot: k.get(1)?.parse().ok()?,
        type_id: v.first()?.parse().ok()?,
        expiry: v.get(1)?.parse().ok()?,
        duration_ms: v.get(2)?.parse().ok()?,
        side,
    })
}

/// Every row of `character_id`, expired ones included, by slot.
fn rows(reg: &Registry, character_id: i64) -> Vec<Active> {
    let all = reg.deltas().of(KIND);
    if all.is_empty() {
        return Vec::new();
    }
    let c = character_id.to_string();
    let mut out: Vec<Active> = all
        .range(vec![c.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&c))
        .filter_map(|(k, v)| parse(k, v))
        .collect();
    out.sort_by_key(|a| a.slot);
    out
}

/// The boosters that have not run out, by slot.
pub fn active(reg: &Registry, character_id: i64) -> Vec<Active> {
    let now = crate::pyutil::filetime_now();
    rows(reg, character_id)
        .into_iter()
        .filter(|a| a.expiry > now)
        .collect()
}

/// The engine's view of [`active`]: ascending by type, no write.
pub fn inputs_of(reg: &Registry, character_id: i64) -> Vec<BoosterInput> {
    if reg.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    let mut v: Vec<BoosterInput> = active(reg, character_id)
        .into_iter()
        .map(|a| BoosterInput {
            type_id: a.type_id,
            side_effects: a.side,
        })
        .collect();
    v.sort();
    v
}

fn put(reg: &mut Registry, character_id: i64, a: &Active) {
    let side = if a.side.is_empty() {
        "-".to_owned()
    } else {
        a.side
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    reg.deltas_mut().put(
        KIND,
        key(character_id, a.slot),
        vec![
            a.type_id.to_string(),
            a.expiry.to_string(),
            a.duration_ms.to_string(),
            side,
        ],
    );
    reg.mark_dirty();
}

fn del(reg: &mut Registry, character_id: i64, slot: i64) {
    reg.deltas_mut().del(KIND, &key(character_id, slot));
    reg.mark_dirty();
}

// ---------------------------------------------------------------------------
// The read
// ---------------------------------------------------------------------------

/// `skillMgr2.GetBoosters()`: `{boosterTypeID: KeyVal}` (see the module
/// comment). `{}` is true for no booster, not a stand-in.
pub fn get_boosters(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some(actor) = actor else {
        return PyValue::Dict(vec![]);
    };
    PyValue::Dict(
        active(reg, actor)
            .into_iter()
            .map(|a| {
                (
                    // `(value, key)`: marshal dict entries are inverted.
                    keyval(vec![
                        ("itemID", PyValue::Int(0)),
                        ("typeID", PyValue::Int(a.type_id)),
                        ("boosterTypeID", PyValue::Int(a.type_id)),
                        ("boosterSlot", PyValue::Int(a.slot)),
                        ("boosterDuration", PyValue::Int(a.duration_ms)),
                        ("expiryTime", PyValue::Int(a.expiry)),
                        (
                            "sideEffectIDs",
                            PyValue::List(a.side.iter().map(|e| PyValue::Int(*e)).collect()),
                        ),
                    ]),
                    PyValue::Int(a.type_id),
                )
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnServerBoostersChanged(*args)` **[V]**: no arguments are read. To one
/// character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoostersChanged {
    pub character_id: i64,
}

impl crate::notify::Push for BoostersChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnServerBoostersChanged", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// Tell the client its boosters moved, and its brain.
pub(crate) fn announce(reg: &mut Registry, character_id: i64) {
    reg.push(BoostersChanged { character_id });
    crate::brain::announce(reg, character_id);
}

// ---------------------------------------------------------------------------
// The ship, in flight
// ---------------------------------------------------------------------------

/// The ship's inputs before a change, taken with the ship brought up to
/// date: a booster can change the top speed, so the ball is integrated at
/// the old speed first (as `dbuff` does). `None` for no ship.
fn before_change(reg: &mut Registry, character_id: i64) -> Option<(i64, Inputs)> {
    let ship = reg.character(character_id)?.ship_id;
    let inputs = crate::dogma::inputs(reg, ship)?;
    reg.advance_character(character_id);
    Some((ship, inputs))
}

/// What the ship's pilot is told after a change: every attribute of the ship
/// and its modules that moved as `OnModuleAttributeChange`, `SetMaxSpeed`
/// for a changed top speed, the hit points and the shield. Docked, only the
/// attribute pushes. The cached sheet reads its inputs afresh, so the
/// server's own numbers already follow.
fn after_change(reg: &mut Registry, character_id: i64, before: Option<(i64, Inputs)>) {
    let Some((ship, old)) = before else {
        return;
    };
    let Some(new) = crate::dogma::inputs(reg, ship) else {
        return;
    };
    if new == old {
        return;
    }
    let table = crate::typedata::installed_in(reg.content()).table();
    if table
        .get(new.ship_type)
        .is_none_or(|t| t.category_id != crate::typedata::CATEGORY_SHIP)
    {
        return;
    }
    let buffs = crate::dbuff::applied(reg, ship);
    let changes = crate::dbuff::attribute_changes_inputs(table, ship, &old, &new, &buffs);
    let in_space = reg
        .roster()
        .get(character_id)
        .is_some_and(|p| p.ship_id == ship);
    let now = crate::pyutil::filetime_now();
    if in_space {
        let speed = changes
            .iter()
            .any(|c| c.item == ship && c.attribute == crate::dogma::attr::MAX_VELOCITY);
        if speed && let Some(h) = crate::dogma::hull(reg, ship) {
            crate::dbuff::speed_push(reg, character_id, ship, f64::from(h.max_velocity));
        }
        crate::dbuff::refresh_record(reg, ship, character_id, now);
    }
    for c in &changes {
        crate::dbuff::attribute_push(reg, character_id, c, now);
    }
}

// ---------------------------------------------------------------------------
// Taking one
// ---------------------------------------------------------------------------

fn type_row(reg: &Registry, type_id: i64) -> Option<&crate::typedata::TypeRow> {
    crate::typedata::installed_in(reg.content())
        .table()
        .get(type_id)
}

fn attribute(reg: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
    type_row(reg, type_id)?.attribute(attribute_id)
}

fn skills_of(reg: &Registry, character_id: i64) -> Vec<(i64, i64)> {
    reg.skills_for(Some(character_id))
        .map(|s| s.entries().map(|e| (e.type_id, e.level)).collect())
        .unwrap_or_default()
}

fn implant_types(reg: &Registry, character_id: i64) -> Vec<i64> {
    crate::implants::head_types(reg, character_id)
}

/// What taking `type_id` would do for this pilot (`dogma::booster_stats`).
pub fn stats(
    reg: &Registry,
    character_id: i64,
    type_id: i64,
) -> Option<crate::dogma::BoosterStats> {
    let table = crate::typedata::installed_in(reg.content()).table();
    crate::dogma::booster_stats(
        table,
        &skills_of(reg, character_id),
        &implant_types(reg, character_id),
        &inputs_of(reg, character_id),
        type_id,
    )
}

/// `dogmaIM.UseBooster(itemID, locationID)`: take a booster from the pilot's
/// location **[V]**.
///
/// Refused, in this order: not the pilot's item or not in reach
/// (`BadArguments`); not a booster (`CustomNotify`); the slot taken
/// (`OnlyOneBoosterActive`); an expired type (`CantConsumeExpiredBooster`);
/// the character too old (`BoosterPastMaxCharAge`); a skill missing
/// (`ImplantHasSkillPrerequisites`); a table without the side-effect column
/// or a type with no known end (`CustomNotify`). Then, in one plan, one is
/// consumed, the side effects rolled, the row written, the training queue
/// re-based around it, the client and the ship told.
pub(crate) fn use_booster(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    use crate::implants::{bad, notify, type_name, user_error};
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let Some(PyValue::Int(item_id)) = req.args.first() else {
        return Err(bad(req, "(itemID, locationID)"));
    };
    let item = reg
        .store()
        .get(*item_id)
        .filter(|i| i.owner_id == actor && i.quantity >= 1)
        .filter(|i| crate::implants::within_reach(reg, actor, i.location_id))
        .cloned()
        .ok_or_else(|| bad(req, "that is not an item in the pilot's location"))?;
    let row = type_row(reg, item.type_id).filter(|r| r.group_id == GROUP_BOOSTER);
    let slot = row
        .and_then(|r| r.attribute(ATTR_BOOSTERNESS))
        .filter(|s| *s >= 1.0)
        .map(|s| s as i64);
    let Some(slot) = slot else {
        return Err(notify("That item cannot be taken as a booster."));
    };
    let now = crate::pyutil::filetime_now();
    if let Some(held) = active(reg, actor).iter().find(|a| a.slot == slot) {
        return Err(user_error(
            "OnlyOneBoosterActive",
            vec![
                ("typeName", PyValue::Unicode(type_name(reg, item.type_id))),
                ("typeName2", PyValue::Unicode(type_name(reg, held.type_id))),
            ],
        ));
    }
    if let Some(days) = attribute(reg, item.type_id, ATTR_LAST_INJECTION)
        && days > 0.0
        && days * (DAY as f64) + (UNIX_EPOCH as f64) < now as f64
    {
        return Err(user_error(
            "CantConsumeExpiredBooster",
            vec![
                ("boosterType", PyValue::Int(item.type_id)),
                (
                    "expiryDate",
                    PyValue::Int((days * DAY as f64) as i64 + UNIX_EPOCH),
                ),
            ],
        ));
    }
    let born = reg.character(actor).map_or(0, |c| c.create_date_time);
    let age_limit = attribute(reg, item.type_id, ATTR_MAX_CHAR_AGE_HOURS).filter(|h| *h > 0.0);
    if let Some(hours) = age_limit
        && (now - born) as f64 > hours * HOUR as f64
    {
        return Err(user_error("BoosterPastMaxCharAge", vec![]));
    }
    if let Some(refusal) = crate::implants::prerequisite_refusal(reg, actor, item.type_id) {
        return Err(refusal);
    }
    if !crate::typedata::installed_in(reg.content())
        .table()
        .knows_usage_chance()
    {
        return Err(notify(
            "This server's type data predates boosters and must be regenerated.",
        ));
    }
    let Some(stats) = stats(reg, actor, item.type_id) else {
        return Err(notify("That booster is not in this server's type data."));
    };
    let (expiry, duration_ms) = if stats.duration_ms > 0 {
        (now + stats.duration_ms * MS, stats.duration_ms)
    } else if let Some(hours) = age_limit {
        (born + (hours * HOUR as f64) as i64, 0)
    } else {
        return Err(notify("That booster has no known duration."));
    };
    let mut side = Vec::new();
    for &(effect, chance) in &stats.chances {
        let mut die = crate::seed::stream(
            reg.world().seed,
            crate::seed::Kind::Loot,
            &[actor as u64, item.item_id as u64, effect as u64, now as u64],
        );
        if die.chance(chance) {
            side.push(effect);
        }
    }
    let taken = Active {
        slot,
        type_id: item.type_id,
        expiry,
        duration_ms,
        side,
    };
    let old = before_change(reg, actor);
    let freeze = crate::skill_queue::freeze_training(reg, actor);
    let result = reg.apply_items(&[crate::item::Edit::Consume {
        item_id: item.item_id,
        quantity: 1,
    }]);
    if result.is_ok() {
        put(reg, actor, &taken);
        if let Some(at) = reg.ext_mut::<Due>().at.as_mut() {
            *at = (*at).min(taken.expiry);
        }
    }
    crate::skill_queue::thaw_training(reg, actor, freeze);
    result.map_err(|e| bad(req, &format!("taking the booster: {e:?}")))?;
    announce(reg, actor);
    after_change(reg, actor, old);
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// Ending
// ---------------------------------------------------------------------------

/// Delete the given slots' rows: re-base the queue around it, tell the client
/// and (`in_flight`) the ship. `inputs_before` is the ship's inputs **with**
/// the rows still counted, because [`inputs_of`] already ignores an expired
/// one.
fn end_rows(reg: &mut Registry, character_id: i64, ended: &[Active], in_flight: bool) {
    if ended.is_empty() {
        return;
    }
    let old = if in_flight {
        before_change(reg, character_id).map(|(ship, mut inputs)| {
            inputs.boosters = counted(reg, character_id);
            (ship, inputs)
        })
    } else {
        None
    };
    let freeze = crate::skill_queue::freeze_training(reg, character_id);
    for a in ended {
        del(reg, character_id, a.slot);
    }
    crate::skill_queue::thaw_training(reg, character_id, freeze);
    announce(reg, character_id);
    after_change(reg, character_id, old);
}

/// Every stored row as an engine input, expired or not: the "before" of an
/// expiry.
fn counted(reg: &Registry, character_id: i64) -> Vec<BoosterInput> {
    let mut v: Vec<BoosterInput> = rows(reg, character_id)
        .into_iter()
        .map(|a| BoosterInput {
            type_id: a.type_id,
            side_effects: a.side,
        })
        .collect();
    v.sort();
    v
}

/// A podding: the rows whose type dies with the pod go. The ship is gone
/// anyway, so only the client's lists are told.
pub(crate) fn on_podding(reg: &mut Registry, character_id: i64) {
    let ended: Vec<Active> = rows(reg, character_id)
        .into_iter()
        .filter(|a| crate::implants::dies_with_the_pod(reg.content(), a.type_id))
        .collect();
    end_rows(reg, character_id, &ended, false);
}

/// A clone jump: the rows that do not follow jump clones go.
pub(crate) fn on_jump(reg: &mut Registry, character_id: i64) {
    let ended: Vec<Active> = rows(reg, character_id)
        .into_iter()
        .filter(|a| attribute(reg, a.type_id, ATTR_FOLLOWS_JUMP_CLONES).unwrap_or(0.0) == 0.0)
        .collect();
    end_rows(reg, character_id, &ended, false);
}

/// The soonest expiry among all stored rows, `None` for none.
fn next_due(reg: &Registry) -> Option<i64> {
    reg.deltas()
        .of(KIND)
        .values()
        .filter_map(|v| v.get(1)?.parse::<i64>().ok())
        .min()
}

/// **The clock's stage** (`tick::STAGES`, coordinator): end every booster
/// whose time has come. `O(1)` while nothing is due: the soonest expiry is
/// kept in the world and compared with the wall clock; only a tick past it
/// reads the rows.
pub fn tick(w: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    if w.deltas().of(KIND).is_empty() {
        return;
    }
    let now = crate::pyutil::filetime_now();
    let due = w.ext::<Due>().at;
    match due {
        Some(at) if at > now => return,
        _ => {}
    }
    let gone: Vec<(i64, Active)> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse().ok()?, parse(k, v)?)))
        .filter(|(_, a): &(i64, Active)| a.expiry <= now)
        .collect();
    let mut chars: Vec<i64> = gone.iter().map(|(c, _)| *c).collect();
    chars.dedup();
    for c in chars {
        let ended: Vec<Active> = gone
            .iter()
            .filter(|(who, _)| *who == c)
            .map(|(_, a)| a.clone())
            .collect();
        end_rows(w, c, &ended, true);
    }
    let next = next_due(w);
    w.ext_mut::<Due>().at = next;
}

/// The soonest expiry, kept in memory so the stage is one comparison.
/// Derived state only: `None` means "read the rows once". A booster taken
/// pulls it forward.
#[derive(Debug, Default)]
pub struct Due {
    at: Option<i64>,
}

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "booster_due",
    make: |_| Box::new(Due::default()) as crate::world_ext::Boxed,
};

/// Invented boosters for the tests (ids above 990_490_000, none of CCP's), in
/// the shape of the client's rows: `boosterness` (1087), `boosterDuration`
/// (330), passive effects that add to a character attribute, and a side
/// effect whose chance attribute (1089) is on the type.
#[cfg(test)]
pub(crate) mod fixture {
    use crate::content::Content;

    /// Slot 1, +4 memory, side effect -2 perception that always rolls.
    pub(crate) const ALWAYS: i64 = 990_490_011;
    /// The same with a chance of 0.
    pub(crate) const NEVER: i64 = 990_490_012;
    /// Slot 2, +1 willpower, no side effect.
    pub(crate) const OTHER_SLOT: i64 = 990_490_013;
    /// Slot 3, no duration, an age limit of 100 hours.
    pub(crate) const AGE_LIMITED: i64 = 990_490_014;
    /// Slot 4, an old `boosterLastInjectionDatetime`.
    pub(crate) const EXPIRED: i64 = 990_490_015;
    /// Slot 5, needs Gunnery 3.
    pub(crate) const NEEDS_GUNNERY_3: i64 = 990_490_016;
    /// Slot 6, `nonDestructible`.
    pub(crate) const POD_SAFE: i64 = 990_490_017;
    /// Slot 7, `followsJumpClones`.
    pub(crate) const FOLLOWS: i64 = 990_490_018;
    /// Slot 8, nothing special.
    pub(crate) const PLAIN: i64 = 990_490_019;
    /// A skill that lengthens every booster by its level per cent.
    pub(crate) const BIOLOGY: i64 = 990_490_021;
    /// A skill that raises the chance attribute by its level per cent of the type's.
    pub(crate) const RECOVERY: i64 = 990_490_022;
    const MAIN_MEMORY: i64 = 990_490_901;
    const MAIN_WILL: i64 = 990_490_902;
    const SIDE: i64 = 990_490_903;
    const BIOLOGY_LEVEL: i64 = 990_490_904;
    const BIOLOGY_DURATION: i64 = 990_490_905;
    const RECOVERY_LEVEL: i64 = 990_490_906;
    const RECOVERY_CHANCE: i64 = 990_490_907;

    pub(crate) fn type_rows() -> String {
        let b = |id: i64, slot: i64, effects: &str, attrs: &str| {
            format!(
                "type\t{id}\t303\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t{effects}\t\
                 1087={slot}.0;{attrs}\tFixture Booster {id}\n"
            )
        };
        let mut s = String::new();
        s += &b(
            ALWAYS,
            1,
            &format!("{MAIN_MEMORY},{SIDE}"),
            "330=600000.0;177=4.0;178=-2.0;1089=1.0",
        );
        s += &b(
            NEVER,
            1,
            &format!("{MAIN_MEMORY},{SIDE}"),
            "330=600000.0;177=4.0;178=-2.0;1089=0.0",
        );
        s += &b(OTHER_SLOT, 2, &MAIN_WILL.to_string(), "330=60000.0;179=1.0");
        s += &b(
            AGE_LIMITED,
            3,
            &MAIN_MEMORY.to_string(),
            "177=2.0;1647=100.0",
        );
        s += &b(
            EXPIRED,
            4,
            &MAIN_MEMORY.to_string(),
            "330=1000.0;177=1.0;2422=100.0",
        );
        s += &b(
            NEEDS_GUNNERY_3,
            5,
            &MAIN_MEMORY.to_string(),
            "330=1000.0;177=1.0;182=3300.0;277=3.0",
        );
        s += &b(
            POD_SAFE,
            6,
            &MAIN_MEMORY.to_string(),
            "330=1000.0;177=1.0;1890=1.0",
        );
        s += &b(
            FOLLOWS,
            7,
            &MAIN_MEMORY.to_string(),
            "330=1000.0;177=1.0;1890=1.0;1916=1.0",
        );
        s += &b(PLAIN, 8, &MAIN_MEMORY.to_string(), "330=1000.0;177=1.0");
        for (skill, level, bonus) in [
            (BIOLOGY, BIOLOGY_LEVEL, BIOLOGY_DURATION),
            (RECOVERY, RECOVERY_LEVEL, RECOVERY_CHANCE),
        ] {
            s += &format!(
                "type\t{skill}\t255\t16\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t{level},{bonus}\t292=1.0\tFixture Skill {skill}\n"
            );
        }
        // Passive effects; the last column of `SIDE` is its chance attribute.
        for (effect, to, from) in [(MAIN_MEMORY, 166, 177), (MAIN_WILL, 168, 179)] {
            s +=
                &format!("effect\t{effect}\t0\t0\t0\t0\t0\t0\t0\tfixtureBooster{effect}\t\t1\t0\n");
            s += &format!("modifier\t{effect}\tcharID\tItemModifier\t{to}\t{from}\t2\t\t\n");
        }
        s += &format!("effect\t{SIDE}\t0\t0\t0\t0\t0\t0\t0\tfixtureSide\t\t1\t1089\n");
        s += &format!("modifier\t{SIDE}\tcharID\tItemModifier\t167\t178\t2\t\t\n");
        for level in [BIOLOGY_LEVEL, RECOVERY_LEVEL] {
            s += &format!("effect\t{level}\t0\t0\t0\t0\t0\t0\t0\tfixtureLevel{level}\t\t1\t0\n");
            s += &format!("modifier\t{level}\titemID\tItemModifier\t292\t280\t0\t\t\n");
        }
        s += &format!("effect\t{BIOLOGY_DURATION}\t0\t0\t0\t0\t0\t0\t0\tfixtureBiology\t\t1\t0\n");
        s += &format!(
            "modifier\t{BIOLOGY_DURATION}\tcharID\tLocationGroupModifier\t330\t292\t6\t\t303\n"
        );
        s += &format!("effect\t{RECOVERY_CHANCE}\t0\t0\t0\t0\t0\t0\t0\tfixtureRecovery\t\t1\t0\n");
        s += &format!(
            "modifier\t{RECOVERY_CHANCE}\tcharID\tLocationGroupModifier\t1089\t292\t6\t\t303\n"
        );
        s
    }

    pub(crate) fn content() -> Content {
        let text = format!(
            "{}{}{}",
            crate::typedata::FIXTURE,
            crate::implants::fixture::rows(),
            type_rows()
        );
        let table = crate::typedata::Table::parse(&text).expect("the fixture parses");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::implants::fixture::{ALICE, GUNNERY, STATION};
    use crate::model::{Character, SandboxConfig};
    use std::time::Duration;

    fn world() -> Registry {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            station_id: Some(STATION),
            create_date_time: crate::pyutil::filetime_now() - 1_000 * HOUR,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()))
    }

    fn make(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity,
                to: crate::item::Slot::new(ALICE, STATION, crate::model::FLAG_HANGAR),
            }])
            .expect("a type the fixture knows");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("an id")
    }

    fn take(reg: &mut Registry, item: i64) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "dogmaIM",
            method: "UseBooster",
            args: &[PyValue::Int(item), PyValue::Int(STATION)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    }

    fn get(reg: &mut Registry) -> PyValue {
        reg.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method: "GetBoosters",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("answers")
    }

    /// The key of a raised `UserError` and its arguments.
    fn refusal(r: Result<PyValue, ServiceError>) -> (String, Vec<(PyValue, PyValue)>) {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
            panic!("expected a raised UserError, got {r:?}")
        };
        let PyValue::Tuple(h) = *header else {
            panic!("a call header")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("UserError args")
        };
        let PyValue::Bytes(key) = &args[0] else {
            panic!("the key")
        };
        let PyValue::Dict(d) = &args[1] else {
            panic!("the dict")
        };
        (String::from_utf8_lossy(key).into_owned(), d.clone())
    }

    fn events(reg: &mut Registry) -> Vec<&'static str> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push().0)
            .collect()
    }

    fn memory(reg: &Registry) -> f64 {
        crate::skill_queue::skill_attribute(reg, ALICE, 166)
    }

    fn perception(reg: &Registry) -> f64 {
        crate::skill_queue::skill_attribute(reg, ALICE, 167)
    }

    #[test]
    fn taking_one_off_a_stack_leaves_the_rest_and_writes_the_row() {
        let mut reg = world();
        let stack = make(&mut reg, PLAIN, 3);
        let before = crate::pyutil::filetime_now();
        take(&mut reg, stack).expect("taken");
        assert_eq!(reg.store().get(stack).map(|i| i.quantity), Some(2));
        let rows = active(&reg, ALICE);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].slot, rows[0].type_id), (8, PLAIN));
        assert_eq!(rows[0].duration_ms, 1000);
        assert!(rows[0].expiry >= before + 1000 * MS);
        let seen = events(&mut reg);
        assert!(seen.contains(&"OnServerBoostersChanged"), "{seen:?}");
        assert!(seen.contains(&"OnServerBrainUpdated"), "{seen:?}");
    }

    #[test]
    fn a_second_booster_in_a_slot_is_refused_with_both_names() {
        let mut reg = world();
        let (a, b) = (make(&mut reg, ALWAYS, 1), make(&mut reg, NEVER, 1));
        take(&mut reg, a).expect("taken");
        let (key, args) = refusal(take(&mut reg, b));
        assert_eq!(key, "OnlyOneBoosterActive");
        let names: Vec<&PyValue> = args.iter().map(|(v, _)| v).collect();
        assert_eq!(names.len(), 2, "typeName and typeName2");
        assert_eq!(
            reg.store().get(b).map(|i| i.quantity),
            Some(1),
            "not used up"
        );
    }

    #[test]
    fn a_booster_in_another_slot_coexists() {
        let mut reg = world();
        let (a, b) = (make(&mut reg, ALWAYS, 1), make(&mut reg, OTHER_SLOT, 1));
        take(&mut reg, a).expect("taken");
        take(&mut reg, b).expect("taken");
        assert_eq!(active(&reg, ALICE).len(), 2);
    }

    #[test]
    fn get_boosters_is_a_dict_of_settable_keyvals_with_the_five_fields() {
        let mut reg = world();
        assert_eq!(get(&mut reg), PyValue::Dict(vec![]));
        let item = make(&mut reg, NEVER, 1);
        take(&mut reg, item).expect("taken");
        let PyValue::Dict(d) = get(&mut reg) else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1);
        // `(value, key)`: the key is the booster's type.
        assert_eq!(d[0].1, PyValue::Int(NEVER));
        let v = &d[0].0;
        let field = |name: &[u8]| {
            let PyValue::Instance { state, .. } = v else {
                panic!("a KeyVal")
            };
            crate::pyutil::dict_get(state, name).cloned()
        };
        assert_eq!(field(b"boosterTypeID"), Some(PyValue::Int(NEVER)));
        assert_eq!(field(b"boosterSlot"), Some(PyValue::Int(1)));
        assert_eq!(field(b"boosterDuration"), Some(PyValue::Int(600_000)));
        assert!(matches!(field(b"expiryTime"), Some(PyValue::Int(t)) if t > 0));
        assert_eq!(
            field(b"sideEffectIDs"),
            Some(PyValue::List(vec![])),
            "a list even when empty"
        );
        assert_eq!(field(b"typeID"), Some(PyValue::Int(NEVER)));
    }

    #[test]
    fn a_chance_of_zero_never_rolls_and_a_chance_of_one_always_does() {
        let mut reg = world();
        let never = make(&mut reg, NEVER, 1);
        take(&mut reg, never).expect("taken");
        assert!(active(&reg, ALICE)[0].side.is_empty());
        assert_eq!(memory(&reg), 4.0 + base_memory());
        assert_eq!(perception(&reg), base_perception(), "no side effect");
        let mut reg = world();
        let always = make(&mut reg, ALWAYS, 1);
        take(&mut reg, always).expect("taken");
        assert_eq!(active(&reg, ALICE)[0].side.len(), 1);
        assert_eq!(perception(&reg), base_perception() - 2.0, "the side effect");
        assert_eq!(memory(&reg), 4.0 + base_memory(), "the main effect too");
    }

    fn base_memory() -> f64 {
        crate::skill_queue::skill_attribute(&world(), ALICE, 166)
    }

    fn base_perception() -> f64 {
        crate::skill_queue::skill_attribute(&world(), ALICE, 167)
    }

    #[test]
    fn a_skill_changes_the_duration_and_the_chance_through_the_engine() {
        let reg = world();
        let table = crate::typedata::installed_in(reg.content()).table();
        let at = |skills: &[(i64, i64)]| {
            crate::dogma::booster_stats(table, skills, &[], &[], NEVER).expect("stats")
        };
        let plain = at(&[]);
        assert_eq!(plain.duration_ms, 600_000);
        assert_eq!(plain.chances.len(), 1);
        let long = at(&[(BIOLOGY, 5)]);
        assert_eq!(long.duration_ms, 630_000, "5 per cent");
        let always =
            crate::dogma::booster_stats(table, &[(RECOVERY, 5)], &[], &[], ALWAYS).expect("stats");
        assert_eq!(always.chances[0].1, 1.0, "clamped to a probability");
    }

    #[test]
    fn an_attribute_booster_moves_the_training_attribute_and_expiry_returns_it() {
        let mut reg = world();
        let base = memory(&reg);
        let item = make(&mut reg, PLAIN, 1);
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        take(&mut reg, item).expect("taken");
        assert_eq!(memory(&reg), base + 1.0);
        crate::test_clock::step(Duration::from_secs(2));
        assert_eq!(memory(&reg), base, "expired rows stop being inputs");
        crate::test_clock::thaw();
    }

    #[test]
    fn the_clock_removes_a_row_at_its_time_and_tells_the_client() {
        let mut reg = world();
        let item = make(&mut reg, PLAIN, 1);
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        take(&mut reg, item).expect("taken");
        let _ = reg.take_pending();
        let blue = crate::pyutil::filetime_now();
        tick(&mut reg, std::time::Instant::now(), blue);
        assert_eq!(rows(&reg, ALICE).len(), 1, "not yet");
        crate::test_clock::step(Duration::from_secs(2));
        let blue = crate::pyutil::filetime_now();
        tick(&mut reg, std::time::Instant::now(), blue);
        assert!(rows(&reg, ALICE).is_empty());
        let seen = events(&mut reg);
        assert!(seen.contains(&"OnServerBoostersChanged"), "{seen:?}");
        crate::test_clock::thaw();
    }

    #[test]
    fn a_world_loaded_after_the_time_drops_the_row_at_its_first_tick() {
        let mut reg = world();
        crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
        let past = Active {
            slot: 8,
            type_id: PLAIN,
            expiry: crate::pyutil::filetime_now() - HOUR,
            duration_ms: 1000,
            side: vec![],
        };
        put(&mut reg, ALICE, &past);
        assert!(active(&reg, ALICE).is_empty(), "already not an input");
        assert!(inputs_of(&reg, ALICE).is_empty());
        let blue = crate::pyutil::filetime_now();
        tick(&mut reg, std::time::Instant::now(), blue);
        assert!(rows(&reg, ALICE).is_empty());
        crate::test_clock::thaw();
    }

    #[test]
    fn a_podding_keeps_a_nondestructible_row_and_drops_the_rest() {
        let mut reg = world();
        for t in [POD_SAFE, PLAIN] {
            let item = make(&mut reg, t, 1);
            take(&mut reg, item).expect("taken");
        }
        on_podding(&mut reg, ALICE);
        let left: Vec<i64> = rows(&reg, ALICE).iter().map(|a| a.type_id).collect();
        assert_eq!(left, vec![POD_SAFE]);
    }

    #[test]
    fn a_jump_keeps_only_what_follows_jump_clones() {
        let mut reg = world();
        for t in [FOLLOWS, POD_SAFE, PLAIN] {
            let item = make(&mut reg, t, 1);
            take(&mut reg, item).expect("taken");
        }
        on_jump(&mut reg, ALICE);
        let left: Vec<i64> = rows(&reg, ALICE).iter().map(|a| a.type_id).collect();
        assert_eq!(left, vec![FOLLOWS]);
    }

    #[test]
    fn the_refusals() {
        let mut reg = world();
        // Not a booster.
        let junk = make(&mut reg, GUNNERY, 1);
        assert_eq!(refusal(take(&mut reg, junk)).0, "CustomNotify");
        // An expired type.
        let old = make(&mut reg, EXPIRED, 1);
        assert_eq!(refusal(take(&mut reg, old)).0, "CantConsumeExpiredBooster");
        // A skill missing.
        let needs = make(&mut reg, NEEDS_GUNNERY_3, 1);
        assert_eq!(
            refusal(take(&mut reg, needs)).0,
            "ImplantHasSkillPrerequisites"
        );
        assert!(rows(&reg, ALICE).is_empty(), "nothing was written");
    }

    #[test]
    fn an_age_limited_booster_lasts_until_the_character_is_that_old() {
        let mut reg = world();
        let item = make(&mut reg, AGE_LIMITED, 1);
        // Alice is 1000 hours old, the limit is 100.
        assert_eq!(refusal(take(&mut reg, item)).0, "BoosterPastMaxCharAge");
        let born = crate::pyutil::filetime_now() - 10 * HOUR;
        reg.character_mut(ALICE).expect("alice").create_date_time = born;
        take(&mut reg, item).expect("young enough");
        let a = &active(&reg, ALICE)[0];
        assert_eq!((a.duration_ms, a.expiry), (0, born + 100 * HOUR));
    }

    #[test]
    fn a_table_without_the_side_effect_column_refuses() {
        let mut reg = world();
        let old = format!(
            "{}{}",
            crate::typedata::FIXTURE,
            type_rows()
                .lines()
                .map(|l| {
                    if l.starts_with("effect\t") {
                        l.rsplit_once('\t').map_or(l, |x| x.0).to_owned()
                    } else {
                        l.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
        let table = crate::typedata::Table::parse(&old).expect("parses");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        reg = reg.with_content(std::sync::Arc::new(c));
        let item = make(&mut reg, PLAIN, 1);
        assert_eq!(refusal(take(&mut reg, item)).0, "CustomNotify");
        assert_eq!(reg.store().get(item).map(|i| i.quantity), Some(1));
    }

    #[test]
    fn a_row_survives_a_round_trip_through_the_delta_text() {
        let mut reg = world();
        let a = Active {
            slot: 1,
            type_id: ALWAYS,
            expiry: crate::pyutil::filetime_now() + HOUR,
            duration_ms: 600_000,
            side: vec![11, 12],
        };
        put(&mut reg, ALICE, &a);
        let none = Active {
            slot: 2,
            side: vec![],
            ..a.clone()
        };
        put(&mut reg, ALICE, &none);
        assert_eq!(rows(&reg, ALICE), vec![a, none]);
    }

    #[test]
    fn rows_of_a_character_do_not_leak_into_a_longer_id() {
        let mut reg = world();
        let a = Active {
            slot: 1,
            type_id: ALWAYS,
            expiry: crate::pyutil::filetime_now() + HOUR,
            duration_ms: 1,
            side: vec![],
        };
        put(&mut reg, ALICE, &a);
        put(&mut reg, ALICE * 10, &a);
        assert_eq!(rows(&reg, ALICE).len(), 1);
    }
}
