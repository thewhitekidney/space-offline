//! **Implants** (lane IMPL): plugging one into the head, reading the head's
//! set, taking one out, what a podding does to them, and the training
//! attributes they raise.
//!
//! An implant is an ordinary item. Plugged in, it is the character's own item
//! at `flagImplant` (89) inside the character (`location == owner ==
//! character`), which is where `dogma::inputs`, the killmail and the pod
//! revive (K-POD) already look. Nothing else is stored: the set is the
//! store's, so it persists, moves and is destroyed with the items.
//!
//! # What the client does, and so what the server must answer **[V]**
//!
//! Citations are the client's own code, build 3619879; line numbers are
//! `co_lnotab`'s (`tools/dis.py`, `tools/call_lines.py`), never the
//! decompiler's.
//!
//! - **The offer.** `menucheckers/itemCheckers.py:701 OfferPlugInImplant`:
//!   the item is an implant (`categoryID == categoryImplant` and a non-zero
//!   `implantness`, `:1342 IsImplant`, `:1478 GetImplantSlot`), it is in the
//!   pilot's location (`baseCheckers.py:565 IsInPilotLocation`, widened at
//!   `itemCheckers.py:1200`: the station, or the active ship's cargo, or a
//!   container in either), and its slot is
//!   not already a key of `skills.GetImplants()` (`:707`, through
//!   `baseCheckers.py:64 getImplants`). So **the slot is
//!   the type's `implantness` (331)** and `GetImplants` is **a dict keyed by
//!   that slot**.
//! - **The call.** `menuSvcExtras/invItemFunctions.py:100 PlugInImplant`:
//!   after a confirm dialog, `godma.GetDogmaLM().InjectImplant(itemID)` for
//!   each selected item, result discarded, then the "implant plugged in"
//!   notification (`implantPluggedInFormatter`, `typeID`). An exception is
//!   shown to the player: the answer to a refusal is a `UserError`.
//! - **The removal.** `control/entries/implant.py:138 RemoveImplant`: after
//!   `ConfirmUnPlugInImplant`, `godma.GetDogmaLM().DestroyImplant(itemID)`
//!   (`:147`). The entry's menu (`:110 GetMenu`) only offers it for a
//!   non-booster with an `itemID`.
//! - **The push.** `services/skillsvc.py:1109 OnServerImplantsChanged(*args)`:
//!   re-reads `GetAttributes` (and so `GetBoosters` and `GetImplants`, both
//!   through `GetCharacterAttributes(renew)`, `:246`), refreshes the queue
//!   when there is one (`if skillQueueSvc.GetQueue()`, which is the client
//!   saying implants may change while a skill trains) and scatters
//!   `OnImplantsChanged`, which redraws the Implants & Boosters panel
//!   (`implantsBoostersPanel.py:38 LoadPanel`) and the home-station
//!   augmentation list (`homestation/client/augmentation.py:66`). **`GetImplants` is cached on
//!   `is None`** (`skillsvc.py:1129`), so a change nobody pushes is invisible
//!   until the session resets.
//! - **The values.** `GetImplants().itervalues()` feeds
//!   `godma.py:2286` (`char.implants`), whose consumers read `.typeID`
//!   (`attributes.py:130`, `probescanning/scanBonusController.py`
//!   `GetBonusFromImplantsByAttributes`, `uix.py:274`, `augmentation.py:33`)
//!   and, on the Unplug entry, `.itemID` (`implant.py:110`). Each value is a `KeyVal` with both.
//! - **The attribute window.** `attributes.py:130-142` takes
//!   `GetAttributes()[attr]` and **subtracts** the implants' own
//!   `charismaBonus`... (175-179, `characterskills/attribute.py`) to find the
//!   remappable base. So `GetAttributes` answers the attribute **with the
//!   implants in it**, and the server's training clock must use the same.
//! - **What a podding keeps.** `homestation/client/prompt.py:35
//!   _is_destructible`: an implant whose `nonDestructible` (1890) is not
//!   zero is left out of "the following implants will be destroyed"
//!   (`AskStationSelfDestructImplants`), and `augmentation.py:78` likewise;
//!   `uix.py:274` skips them in the cost it quotes.
//!   So a podding and a clone self-destruct take only the destructible ones.
//!
//! # The server's rules
//!
//! The refusals use the client's own message keys, found in `findings/
//! user-errors.tsv` (they are raised by the server; no client code names
//! them): `OnlyOneImplantActive(typeName)` for an occupied slot,
//! `ImplantHasSkillPrerequisites(item, skillCount, requiredSkills)` for a
//! missing skill, `CantRemoveImplant` for a removal of what is not plugged
//! in. The required skills are the type's `requiredSkill1..6` and their
//! levels (`dogma::attr::REQUIRED_SKILLS`, `drone_skills::required_skills`).
//!
//! # `[A]`
//!
//! - **No "skill in training" refusal.** `FailedPlugInImplant` ("You cannot
//!   plug in an implant while a skill is being trained") is in the client's
//!   message table (id 892) but nothing in the client names it, and
//!   `OnServerImplantsChanged` refreshes a running queue, which only makes
//!   sense if plugging during training is allowed. So it is allowed, and the
//!   head skill is re-based on the new rate (`skill_queue::freeze_training`).
//!   Falsifier: TQ refusing the plug of an attribute implant while training.
//! - **No prerequisite-implant refusal.** `PrereqImplantMissing(typeName)`
//!   is in the message table too, but no attribute in this build's
//!   `dogmaattributes` names a required implant (the only `implant*`
//!   attributes are `implantness` and the `implantSet*` set-bonus flags), so
//!   there is nothing in the data to drive it. Falsifier: an implant type
//!   with such an attribute.
//! - **Where the item may be**: the pilot's station (hangar) or the active
//!   ship, as for skillbooks (`skill_queue::inject_skill_into_brain`); a
//!   container inside either is allowed. TQ's own check is not in the client.
//! - **The `requiredSkills` text** is "Skill level N, ..." in the
//!   skill table's names; the client only substitutes it into the sentence.
//! - **A plugged implant is a stack of one**, not a singleton: the store
//!   makes a singleton only of ships and blueprints, and nothing in the
//!   client reads the flag of an implant.
//! - **Brain effects.** The client's own fitting window computes with the
//!   character's *brain* (`charInfo[1]` of `GetAllInfo`: `BrainEffect`
//!   objects built server-side, `eve/common/script/dogma/effect.py`), which
//!   this server sends empty for skills as well. An implant's bonus to a
//!   *ship* attribute therefore reaches the server's numbers (the sheet of
//!   `crate::dogma`) and not the client's local fitting window. Falsifier:
//!   the window showing an implanted pilot's bonus. Follow-up: the brain.

use crate::pyutil::{bstr, keyval, pypairs};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// `attributeImplantness` (331) **[V]** `dogma/const.py`: the slot.
pub const ATTR_IMPLANTNESS: i64 = 331;
/// `attributeNonDestructible` (1890) **[V]** `dogma/const.py`: not lost with
/// the clone.
pub const ATTR_NON_DESTRUCTIBLE: i64 = 1890;
/// `categoryImplant` (20) **[V]** `inventorycommon/const.py`.
pub const CATEGORY_IMPLANT: i64 = 20;
/// The five attributes a skill trains with, charisma first: `attributeCharisma`
/// (164), intelligence, memory, perception, willpower (168) **[V]**
/// `dogma/const.py`.
pub const TRAINING_ATTRIBUTES: [i64; 5] = [164, 165, 166, 167, 168];

/// One implant in the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plugged {
    pub item_id: i64,
    pub type_id: i64,
    /// Its `implantness`.
    pub slot: i64,
}

/// The slot of an implant type: its `implantness`, `None` for a type with
/// none (a booster, a skillbook, a hull).
pub fn slot_of(content: &crate::content::Content, type_id: i64) -> Option<i64> {
    crate::typedata::attribute_in(content, type_id, ATTR_IMPLANTNESS)
        .filter(|s| *s >= 1.0)
        .map(|s| s as i64)
}

/// Whether a podding takes this implant: the client's own test
/// (`prompt._is_destructible`, `prompt.py:35`) **[V]**.
pub fn dies_with_the_pod(content: &crate::content::Content, type_id: i64) -> bool {
    crate::typedata::attribute_in(content, type_id, ATTR_NON_DESTRUCTIBLE).unwrap_or(0.0) == 0.0
}

/// The implants plugged into `character_id`, by slot.
pub fn plugged(reg: &Registry, character_id: i64) -> Vec<Plugged> {
    let mut v: Vec<Plugged> = reg
        .store()
        .contents_of(character_id)
        .filter(|i| i.flag_id == crate::dogma::FLAG_IMPLANT && i.owner_id == character_id)
        .map(|i| Plugged {
            item_id: i.item_id,
            type_id: i.type_id,
            slot: slot_of(reg.content(), i.type_id).unwrap_or(0),
        })
        .collect();
    v.sort_by_key(|p| (p.slot, p.item_id));
    v
}

/// The item ids a podding destroys: the head's destructible implants.
pub fn destroyed_in_a_pod(reg: &Registry, character_id: i64) -> Vec<i64> {
    plugged(reg, character_id)
        .into_iter()
        .filter(|p| dies_with_the_pod(reg.content(), p.type_id))
        .map(|p| p.item_id)
        .collect()
}

/// `skillMgr2.GetImplants()`: `{slot: KeyVal(itemID, typeID, slot)}` **[V]**
/// (see the module comment). `{}` for no character or an empty head, which
/// is true, not a stand-in.
pub fn get_implants(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some(actor) = actor else {
        return PyValue::Dict(vec![]);
    };
    PyValue::Dict(
        plugged(reg, actor)
            .into_iter()
            .map(|p| {
                (
                    // `(value, key)`: marshal dict entries are inverted.
                    keyval(vec![
                        ("itemID", PyValue::Int(p.item_id)),
                        ("typeID", PyValue::Int(p.type_id)),
                        ("slot", PyValue::Int(p.slot)),
                    ]),
                    PyValue::Int(p.slot),
                )
            })
            .collect(),
    )
}

/// What the head's implants and the active boosters add to each training
/// attribute, from the dogma engine (`dogma::pilot_bonus`). Empty for an empty
/// head and no booster, which is the common case and costs nothing.
pub fn training_bonuses(reg: &Registry, character_id: i64) -> std::collections::BTreeMap<i64, f64> {
    let head = plugged(reg, character_id);
    let boosters = crate::boosters::inputs_of(reg, character_id);
    if head.is_empty() && boosters.is_empty() {
        return std::collections::BTreeMap::new();
    }
    let types: Vec<i64> = head.iter().map(|p| p.type_id).collect();
    let skills: Vec<(i64, i64)> = reg
        .skills_for(Some(character_id))
        .map(|s| s.entries().map(|e| (e.type_id, e.level)).collect())
        .unwrap_or_default();
    let table = crate::typedata::installed_in(reg.content()).table();
    crate::dogma::pilot_bonus(table, &skills, &types, &boosters, &TRAINING_ATTRIBUTES)
}

/// One attribute's implant bonus, `0.0` for none.
pub fn attribute_bonus(reg: &Registry, character_id: i64, attribute_id: i64) -> f64 {
    training_bonuses(reg, character_id)
        .get(&attribute_id)
        .copied()
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnServerImplantsChanged(*args)` **[V]** (`skillsvc.py:910`): no arguments
/// are read. To one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImplantsChanged {
    pub character_id: i64,
}

impl crate::notify::Push for ImplantsChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnServerImplantsChanged", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// Tell the client its implants moved: the push that makes it re-read
/// `GetImplants` and `GetAttributes`, and the jump-clone panel's cache (a
/// clone's own implants are read from the same state).
pub(crate) fn announce(reg: &mut Registry, character_id: i64) {
    reg.push(ImplantsChanged { character_id });
    // The implants' modifiers are part of the client's brain.
    crate::brain::announce(reg, character_id);
    reg.push(crate::clones::JumpCloneCacheInvalidated { character_id });
}

// ---------------------------------------------------------------------------
// The two calls
// ---------------------------------------------------------------------------

pub(crate) fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

pub(crate) fn user_error(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, pypairs(args)))
}

/// Our words, for a refusal the client has no label for.
pub(crate) fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", bstr(text))])
}

pub(crate) fn type_name(reg: &Registry, type_id: i64) -> String {
    crate::item::find_in(reg.content(), type_id)
        .map_or_else(|| type_id.to_string(), |t| t.name.to_owned())
}

/// The skills `type_id` needs that `actor` does not have, as `(skill,
/// level)`.
pub(crate) fn missing_skills(reg: &Registry, actor: i64, type_id: i64) -> Vec<(i64, i64)> {
    let table = crate::typedata::installed_in(reg.content()).table();
    crate::drone_skills::required_skills(table, type_id)
        .into_iter()
        .filter(|(skill, level)| crate::drone_skills::level_of(reg, actor, *skill) < *level)
        .collect()
}

/// Whether `item` is somewhere the pilot can reach: their station, their
/// active ship, or a container directly in either. **[A]** (module comment).
pub(crate) fn within_reach(reg: &Registry, actor: i64, location_id: i64) -> bool {
    let Some(c) = reg.character(actor) else {
        return false;
    };
    let here = |l: i64| Some(l) == c.station_id || l == c.ship_id;
    here(location_id)
        || reg
            .store()
            .get(location_id)
            .is_some_and(|holder| holder.owner_id == actor && here(holder.location_id))
}

/// `ImplantHasSkillPrerequisites` when `actor` lacks a skill `type_id` needs,
/// else `None`. Boosters use the same check **[A]**: the client has no
/// booster-specific key.
pub(crate) fn prerequisite_refusal(
    reg: &Registry,
    actor: i64,
    type_id: i64,
) -> Option<ServiceError> {
    let missing = missing_skills(reg, actor, type_id);
    if !missing.is_empty() {
        let text: Vec<String> = missing
            .iter()
            .map(|(skill, level)| {
                let name = crate::skill::find_in(reg.content(), *skill)
                    .map_or_else(|| skill.to_string(), |s| s.name.to_owned());
                format!("{name} level {level}")
            })
            .collect();
        return Some(user_error(
            "ImplantHasSkillPrerequisites",
            vec![
                ("item", PyValue::Int(type_id)),
                ("skillCount", PyValue::Int(missing.len() as i64)),
                ("requiredSkills", PyValue::Unicode(text.join(", "))),
            ],
        ));
    }
    None
}

/// `dogmaIM.InjectImplant(itemID)`: plug an implant from the pilot's
/// location into the head **[V]**.
///
/// Refused, in this order: not the pilot's item or not in reach (the client
/// would not have offered it: `BadArguments`); not an implant; the slot
/// taken (`OnlyOneImplantActive`); a skill missing
/// (`ImplantHasSkillPrerequisites`). Then, in one item plan, one is taken
/// off the stack and put in the head, with the training queue re-based on
/// the new attributes around it, and the client told.
pub(crate) fn inject_implant(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let Some(PyValue::Int(item_id)) = req.args.first() else {
        return Err(bad(req, "(itemID,)"));
    };
    let item = reg
        .store()
        .get(*item_id)
        .filter(|i| i.owner_id == actor && i.quantity >= 1)
        .filter(|i| within_reach(reg, actor, i.location_id))
        .filter(|i| i.flag_id != crate::dogma::FLAG_IMPLANT)
        .cloned()
        .ok_or_else(|| bad(req, "that is not an item in the pilot's location"))?;
    let Some(slot) =
        slot_of(reg.content(), item.type_id).filter(|_| item.category_id == CATEGORY_IMPLANT)
    else {
        return Err(notify("That item cannot be plugged in as an implant."));
    };
    if plugged(reg, actor).iter().any(|p| p.slot == slot) {
        return Err(user_error(
            "OnlyOneImplantActive",
            vec![("typeName", PyValue::Unicode(type_name(reg, item.type_id)))],
        ));
    }
    if let Some(refusal) = prerequisite_refusal(reg, actor, item.type_id) {
        return Err(refusal);
    }
    let freeze = crate::skill_queue::freeze_training(reg, actor);
    let result = reg.apply_items(&[crate::item::Edit::Split {
        item_id: item.item_id,
        quantity: 1,
        to: crate::item::Slot::new(actor, actor, crate::dogma::FLAG_IMPLANT),
    }]);
    crate::skill_queue::thaw_training(reg, actor, freeze);
    result.map_err(|e| bad(req, &format!("plugging the implant in: {e:?}")))?;
    announce(reg, actor);
    Ok(PyValue::None)
}

/// `dogmaIM.DestroyImplant(itemID)`: destroy one of the head's implants
/// **[V]**. What is not plugged in is `CantRemoveImplant` (its text: "You
/// can't remove an implant that isn't an implant").
pub(crate) fn destroy_implant(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let Some(PyValue::Int(item_id)) = req.args.first() else {
        return Err(bad(req, "(itemID,)"));
    };
    if !plugged(reg, actor).iter().any(|p| p.item_id == *item_id) {
        return Err(user_error("CantRemoveImplant", vec![]));
    }
    let freeze = crate::skill_queue::freeze_training(reg, actor);
    let result = reg.apply_items(&[crate::item::Edit::Destroy { item_id: *item_id }]);
    crate::skill_queue::thaw_training(reg, actor, freeze);
    result.map_err(|e| bad(req, &format!("destroying the implant: {e:?}")))?;
    announce(reg, actor);
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// Swapping the whole head (jump clones, and a podding)
// ---------------------------------------------------------------------------

/// The implant types in the head, by slot: what a jump clone is left with.
pub fn head_types(reg: &Registry, character_id: i64) -> Vec<i64> {
    plugged(reg, character_id)
        .into_iter()
        .map(|p| p.type_id)
        .collect()
}

/// Replace the head's implants with `wake` (types), in one plan: every
/// implant now in the head is destroyed and one of each `wake` type is made
/// in its place. **Conservation**: a jump moves a set from head to clone and
/// from clone to head, so the types in existence do not change; the item ids
/// do (the store has no item that lives in a clone).
///
/// `Err` names the refusal; nothing was applied then. The training queue is
/// re-based around it and the client told, whether or not the sets differ
/// (an empty swap of empty for empty is a no-op and says nothing).
pub(crate) fn swap_head(reg: &mut Registry, character_id: i64, wake: &[i64]) -> Result<(), String> {
    let now: Vec<Plugged> = plugged(reg, character_id);
    if now.is_empty() && wake.is_empty() {
        return Ok(());
    }
    let mut plan: Vec<crate::item::Edit> = now
        .iter()
        .map(|p| crate::item::Edit::Destroy { item_id: p.item_id })
        .collect();
    plan.extend(wake.iter().map(|&type_id| crate::item::Edit::Create {
        type_id,
        quantity: 1,
        to: crate::item::Slot::new(character_id, character_id, crate::dogma::FLAG_IMPLANT),
    }));
    let freeze = crate::skill_queue::freeze_training(reg, character_id);
    let result = reg.apply_items(&plan);
    crate::skill_queue::thaw_training(reg, character_id, freeze);
    result.map_err(|e| format!("{e:?}"))?;
    announce(reg, character_id);
    Ok(())
}

/// The invented implants every test of this lane uses (ids above 990_480_000,
/// none of CCP's): in the shape of the client's own rows -- `implantness`
/// (331), the `...Bonus` attribute (175-179), a passive effect whose
/// modifier is a `charID` `ItemModifier` with operator 2 (add).
#[cfg(test)]
pub(crate) mod fixture {
    use crate::content::Content;

    pub(crate) const ALICE: i64 = 990_480_001;
    pub(crate) const STATION: i64 = 990_480_100;
    /// Gunnery, in the starter set at level 2.
    pub(crate) const GUNNERY: i64 = 3300;
    /// A memory implant, slot 1, +3 memory.
    pub(crate) const MEMORY_1: i64 = 990_480_011;
    /// A second slot-1 implant.
    pub(crate) const ALSO_SLOT_1: i64 = 990_480_012;
    /// A perception implant, slot 2, +2 perception, needs Gunnery 3.
    pub(crate) const NEEDS_GUNNERY_3: i64 = 990_480_013;
    /// Slot 3, kept through a podding (`nonDestructible`).
    pub(crate) const KEEPER: i64 = 990_480_014;
    /// Slot 4, +4 willpower, needs Gunnery 2.
    pub(crate) const NEEDS_GUNNERY_2: i64 = 990_480_015;
    /// A booster-like type: category 20, no implantness.
    pub(crate) const BOOSTER: i64 = 990_480_016;
    /// Invented effect ids.
    pub(crate) const MEMORY_EFFECT: i64 = 990_480_901;
    pub(crate) const PERCEPTION_EFFECT: i64 = 990_480_902;
    pub(crate) const WILLPOWER_EFFECT: i64 = 990_480_903;

    pub(crate) fn rows() -> String {
        let imp = |id: i64, slot: i64, bonus: &str, effect: i64, extra: &str| {
            format!(
                "type\t{id}\t300\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t{effect}\t\
                 331={slot}.0;{bonus}{extra}\tFixture Implant {id}\n"
            )
        };
        let mut s = String::new();
        s += &imp(MEMORY_1, 1, "177=3.0", MEMORY_EFFECT, "");
        s += &imp(ALSO_SLOT_1, 1, "177=1.0", MEMORY_EFFECT, "");
        s += &imp(
            NEEDS_GUNNERY_3,
            2,
            "178=2.0",
            PERCEPTION_EFFECT,
            ";182=3300.0;277=3.0",
        );
        s += &imp(KEEPER, 3, "177=1.0", MEMORY_EFFECT, ";1890=1.0");
        s += &imp(
            NEEDS_GUNNERY_2,
            4,
            "179=4.0",
            WILLPOWER_EFFECT,
            ";182=3300.0;277=2.0",
        );
        s += &format!(
            "type\t{BOOSTER}\t303\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t1087=1.0\tFixture Booster\n"
        );
        // The shape of the client's own `...ModAdd...LocationChar` effects:
        // passive, a `charID` `ItemModifier`, operator 2 (add), from the
        // implant's own bonus attribute.
        for (effect, to, from) in [
            (MEMORY_EFFECT, 166, 177),
            (PERCEPTION_EFFECT, 167, 178),
            (WILLPOWER_EFFECT, 168, 179),
        ] {
            s += &format!("effect\t{effect}\t0\t0\t0\t0\t0\t0\t0\tfixtureBonus{effect}\t\n");
            s += &format!("modifier\t{effect}\tcharID\tItemModifier\t{to}\t{from}\t2\t\t\n");
        }
        s
    }

    pub(crate) fn content() -> Content {
        let text = format!("{}{}", crate::typedata::FIXTURE, rows());
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
    use crate::model::{Character, SandboxConfig};

    /// Alice, docked at [`STATION`], with one of each fixture implant in the
    /// hangar. The ids of the stacks, in the order of the types.
    fn world() -> (Registry, Vec<i64>) {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            station_id: Some(STATION),
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()));
        let mut ids = Vec::new();
        for t in [
            MEMORY_1,
            ALSO_SLOT_1,
            NEEDS_GUNNERY_3,
            KEEPER,
            NEEDS_GUNNERY_2,
            BOOSTER,
        ] {
            ids.push(make(&mut reg, t, 1));
        }
        (reg, ids)
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

    fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "dogmaIM",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    }

    fn plug(reg: &mut Registry, item: i64) -> Result<PyValue, ServiceError> {
        call(reg, "InjectImplant", &[PyValue::Int(item)])
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

    /// GAP-BRAIN: plugging and unplugging change the client's brain, so each
    /// pushes `OnServerBrainUpdated` beside `OnServerImplantsChanged`.
    #[test]
    fn plugging_and_unplugging_push_the_brain() {
        let (mut reg, ids) = world();
        let _ = reg.take_pending();
        plug(&mut reg, ids[0]).expect("plugged");
        let seen = events(&mut reg);
        assert!(seen.contains(&"OnServerImplantsChanged"), "{seen:?}");
        assert!(seen.contains(&"OnServerBrainUpdated"), "{seen:?}");
        let item = plugged(&reg, ALICE)[0].item_id;
        call(&mut reg, "DestroyImplant", &[PyValue::Int(item)]).expect("destroyed");
        assert!(events(&mut reg).contains(&"OnServerBrainUpdated"));
    }

    #[test]
    fn a_plugged_implant_is_the_characters_item_at_the_implant_flag() {
        let (mut reg, ids) = world();
        plug(&mut reg, ids[0]).expect("plugged");
        let i = reg.store().get(ids[0]).expect("the same item, moved");
        assert_eq!(
            (i.owner_id, i.location_id, i.flag_id),
            (ALICE, ALICE, crate::dogma::FLAG_IMPLANT)
        );
        assert_eq!(plugged(&reg, ALICE).len(), 1);
        assert_eq!(plugged(&reg, ALICE)[0].slot, 1);
    }

    #[test]
    fn one_off_a_stack_goes_in_and_the_rest_stays() {
        let (mut reg, _) = world();
        let stack = make(&mut reg, NEEDS_GUNNERY_2, 3);
        plug(&mut reg, stack).expect("plugged");
        assert_eq!(reg.store().get(stack).map(|i| i.quantity), Some(2));
        assert_eq!(plugged(&reg, ALICE).len(), 1);
        assert_eq!(
            reg.store()
                .get(plugged(&reg, ALICE)[0].item_id)
                .map(|i| i.quantity),
            Some(1)
        );
    }

    #[test]
    fn get_implants_is_a_dict_keyed_by_slot_of_keyvals_with_item_and_type() {
        let (mut reg, ids) = world();
        assert_eq!(get_implants(&reg, Some(ALICE)), PyValue::Dict(vec![]));
        plug(&mut reg, ids[0]).expect("plugged");
        plug(&mut reg, ids[3]).expect("plugged");
        let PyValue::Dict(d) = get_implants(&reg, Some(ALICE)) else {
            panic!("a dict")
        };
        let keys: Vec<&PyValue> = d.iter().map(|(_, k)| k).collect();
        assert_eq!(keys, vec![&PyValue::Int(1), &PyValue::Int(3)]);
        let PyValue::Instance { state, .. } = &d[0].0 else {
            panic!("a KeyVal")
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"typeID"),
            Some(&PyValue::Int(MEMORY_1))
        );
        assert_eq!(
            crate::pyutil::dict_get(state, b"itemID"),
            Some(&PyValue::Int(ids[0]))
        );
    }

    /// One per slot, the slot from the type's attribute.
    #[test]
    fn a_second_implant_for_a_taken_slot_is_refused_by_name() {
        let (mut reg, ids) = world();
        plug(&mut reg, ids[0]).expect("plugged");
        let (key, args) = refusal(plug(&mut reg, ids[1]));
        assert_eq!(key, "OnlyOneImplantActive");
        assert_eq!(
            args,
            vec![(
                PyValue::Unicode("Fixture Implant 990480012".to_owned()),
                bstr("typeName")
            )]
        );
        assert_eq!(
            reg.store().get(ids[1]).map(|i| i.flag_id),
            Some(crate::model::FLAG_HANGAR),
            "still in the hangar"
        );
        assert_eq!(plugged(&reg, ALICE).len(), 1);
    }

    #[test]
    fn a_missing_skill_is_refused_with_the_skills_named() {
        let (mut reg, ids) = world();
        let (key, args) = refusal(plug(&mut reg, ids[2]));
        assert_eq!(key, "ImplantHasSkillPrerequisites");
        let get = |k: &str| {
            args.iter()
                .find(|(_, name)| *name == bstr(k))
                .map(|(v, _)| v.clone())
                .unwrap_or_else(|| panic!("no {k}"))
        };
        assert_eq!(get("item"), PyValue::Int(NEEDS_GUNNERY_3));
        assert_eq!(get("skillCount"), PyValue::Int(1));
        assert_eq!(
            get("requiredSkills"),
            PyValue::Unicode("Gunnery level 3".to_owned())
        );
        assert!(plugged(&reg, ALICE).is_empty());
        // At the level asked, it goes in.
        reg.skills
            .get_mut(&ALICE)
            .expect("skills")
            .grant_in(&content(), GUNNERY, 3);
        plug(&mut reg, ids[2]).expect("now plugged");
        assert_eq!(plugged(&reg, ALICE).len(), 1);
    }

    #[test]
    fn what_is_not_an_implant_or_not_mine_is_not_plugged() {
        let (mut reg, ids) = world();
        // A booster has no implantness.
        let _ = refusal(plug(&mut reg, ids[5]));
        assert!(plugged(&reg, ALICE).is_empty());
        // No such item, and an implant in somebody else's hangar.
        assert!(matches!(
            plug(&mut reg, 1),
            Err(ServiceError::BadArguments { .. })
        ));
        let other = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: MEMORY_1,
                quantity: 1,
                to: crate::item::Slot::new(990_480_777, STATION, crate::model::FLAG_HANGAR),
            }])
            .expect("created");
        let id = other[0].item_id();
        assert!(matches!(
            plug(&mut reg, id),
            Err(ServiceError::BadArguments { .. })
        ));
    }

    #[test]
    fn the_client_is_told_to_re_read_and_the_head_is_what_it_reads() {
        let (mut reg, ids) = world();
        let _ = events(&mut reg);
        plug(&mut reg, ids[0]).expect("plugged");
        let seen = events(&mut reg);
        assert!(seen.contains(&"OnServerImplantsChanged"), "{seen:?}");
        assert!(seen.contains(&"OnItemChange"), "{seen:?}");
    }

    /// The attribute implants raise what a skill trains with, through the
    /// engine and the effects' own modifiers; the sheet and the rate read it.
    #[test]
    fn an_attribute_implant_raises_the_training_attribute_and_the_rate() {
        let (mut reg, ids) = world();
        let base = crate::skill_queue::skill_attribute(&reg, ALICE, 166);
        let rate = |reg: &Registry| {
            crate::skill::rate_for_in(reg.content(), 3300, &|a| {
                crate::skill_queue::skill_attribute(reg, ALICE, a)
            })
            .expect("Gunnery has a rate")
        };
        let before = rate(&reg);
        plug(&mut reg, ids[0]).expect("plugged");
        assert_eq!(
            crate::skill_queue::skill_attribute(&reg, ALICE, 166),
            base + 3.0
        );
        // Gunnery trains on perception and willpower, not memory: the rate
        // moves only when one of its two does.
        assert_eq!(rate(&reg), before);
        plug(&mut reg, ids[4]).expect("plugged");
        assert!(rate(&reg) > before, "willpower is Gunnery's secondary");
        // Taking it out takes the bonus out.
        let item = plugged(&reg, ALICE)
            .iter()
            .find(|p| p.type_id == NEEDS_GUNNERY_2)
            .map(|p| p.item_id)
            .expect("plugged");
        call(&mut reg, "DestroyImplant", &[PyValue::Int(item)]).expect("destroyed");
        assert_eq!(rate(&reg), before);
    }

    /// **A skill already training when the implant goes in**: what it
    /// trained at the old rate is credited, and the rest runs at the new one
    /// -- not the whole elapsed time re-priced at the new rate.
    #[test]
    fn plugging_while_a_skill_trains_credits_the_old_rate_and_restarts_at_the_new() {
        const TEN_MINUTES: i64 = 10 * 60 * 10_000_000;
        let (mut reg, ids) = world();
        let rate = |reg: &Registry| {
            crate::skill::rate_for_in(reg.content(), GUNNERY, &|a| {
                crate::skill_queue::skill_attribute(reg, ALICE, a)
            })
            .expect("a rate")
        };
        let old_rate = rate(&reg);
        let now = crate::pyutil::filetime_now();
        {
            let q = reg.skill_queues.entry(ALICE);
            q.entries = vec![(GUNNERY, 3)];
            q.start = Some(now - TEN_MINUTES);
        }
        let points = |reg: &Registry| {
            reg.skills
                .get(&ALICE)
                .and_then(|s| s.get(GUNNERY))
                .map(|e| e.points)
        };
        let before = points(&reg).expect("Gunnery is trained");
        let _ = events(&mut reg);
        plug(&mut reg, ids[4]).expect("plugged");
        let credited = points(&reg).expect("still trained") - before;
        let owed = crate::skill::sp_trained(TEN_MINUTES, old_rate);
        assert!(
            (credited - owed).abs() <= 2,
            "ten minutes at the old rate: credited {credited}, owed {owed}"
        );
        let q = reg.skill_queues.get(ALICE).expect("a queue");
        assert!(q.is_training(), "still training");
        assert!(
            q.start.expect("running") >= now,
            "restarted from now, not from ten minutes ago"
        );
        assert!(rate(&reg) > old_rate);
        let seen = events(&mut reg);
        assert!(seen.contains(&"OnServerSkillsChanged"), "{seen:?}");
        assert!(seen.contains(&"OnServerImplantsChanged"), "{seen:?}");
    }

    #[test]
    fn destroying_what_is_not_plugged_in_is_cant_remove_implant() {
        let (mut reg, ids) = world();
        // In the hangar, not in the head.
        let (key, _) = refusal(call(&mut reg, "DestroyImplant", &[PyValue::Int(ids[0])]));
        assert_eq!(key, "CantRemoveImplant");
        assert!(reg.store().get(ids[0]).is_some());
    }

    #[test]
    fn a_podding_takes_the_destructible_implants_and_keeps_the_others() {
        let (mut reg, ids) = world();
        plug(&mut reg, ids[0]).expect("plugged");
        plug(&mut reg, ids[3]).expect("plugged");
        assert_eq!(destroyed_in_a_pod(&reg, ALICE), vec![ids[0]]);
        assert!(dies_with_the_pod(reg.content(), MEMORY_1));
        assert!(!dies_with_the_pod(reg.content(), KEEPER));
        crate::clones::revive_in_clone(&mut reg, ALICE).expect("revived");
        assert!(
            reg.store().get(ids[0]).is_none(),
            "the memory implant is gone"
        );
        assert!(reg.store().get(ids[3]).is_some(), "the keeper stayed");
        assert_eq!(head_types(&reg, ALICE), vec![KEEPER]);
    }

    #[test]
    fn swapping_the_head_destroys_one_set_and_makes_the_other() {
        let (mut reg, ids) = world();
        plug(&mut reg, ids[0]).expect("plugged");
        let _ = events(&mut reg);
        swap_head(&mut reg, ALICE, &[NEEDS_GUNNERY_2, KEEPER]).expect("swapped");
        assert!(reg.store().get(ids[0]).is_none());
        assert_eq!(head_types(&reg, ALICE), vec![KEEPER, NEEDS_GUNNERY_2]);
        assert!(events(&mut reg).contains(&"OnServerImplantsChanged"));
        // Empty for empty says nothing.
        swap_head(&mut reg, ALICE, &[]).expect("emptied");
        let _ = events(&mut reg);
        swap_head(&mut reg, ALICE, &[]).expect("a no-op");
        assert!(events(&mut reg).is_empty());
    }
}
