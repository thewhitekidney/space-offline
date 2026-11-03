//! **Tactical destroyer stances** (lane `t3-e`, R-T3 packet T3-E; note
//! `docs/lanes/t3-e.md`).
//!
//! A tactical destroyer (group 1305) has three *modes*, called stances in
//! the client's code: defence 1, speed 2, sniper 3 [V]
//! (`shipmode/data.shipStanceDefense`, `...Speed`, `...Sniper`). A mode is a
//! type of group 1306 sitting as an item in the ship's hidden flag 156
//! (`flagHiddenModifers`); its passive effects are plain `modifierInfo` rows
//! with the PostDiv operator, so the dogma engine applies them once the item
//! is part of the ship's fit (`crate::dogma::inputs` already reads flag 156:
//! `item::is_ship_fitting_flag`). Nothing in this module does arithmetic on
//! resonances; it puts the right item in the right place and tells the client.
//!
//! # What the client does and what it leaves to us [V]
//!
//! - **The call.** `shipstance.ShipStanceButtonController.set_stance`:
//!   `sm.RemoteSvc('shipStanceMgr').SetShipStance(shipID, stanceID)`, result
//!   discarded, and only for a hull whose type is in the client's own table.
//! - **What the server does** (`shipmode/shipstance.ShipStance.set_stance`):
//!   if the stance is the current one, nothing; else destroy every item at
//!   `(ship, 156)`, create the mode item there, push `OnStanceActive`, then
//!   hold the call for the switch time.
//! - **The push** (`shipmode/notifier`): `OnStanceActive(shipID, stanceID)`
//!   to the pilot, and, from `SystemNotifier`, the slim field
//!   `shipStance = (oldStanceID, time, newStanceID)` through
//!   `UpdateSlimItemField`. `Ship.GetStanceIDFromSlimItem` reads the third
//!   member of the slim's `shipStance`.
//! - **Where the client reads the stance**: `invCache.GetInventoryFromId(ship)
//!   .List(156)`, matched against its table by type, so the mode item must be
//!   an ordinary singleton row at flag 156 (`ship_stance_button`, `shipstance`).
//! - **The ramp**: `ShipStanceButton.OnStanceActive` animates
//!   `ship.stanceSwitchTime * MSEC` (attribute 1985, 10 s on every hull).
//!
//! # The lock
//!
//! The retail call sleeps for the switch time inside the server, so a second
//! click finds the ship busy. We have no sleeping calls, so a switch records
//! when the ship is free again ([`Stances`]) and a call before then is
//! refused with the client's key `ShipAlreadyChangingStance` (FSD 4666, no
//! arguments, no client raise site). The lock is derived state: it is not
//! saved, and a restart ends it.
//!
//! # Which type is which mode
//!
//! The client's table (hull -> three mode types) is a Python module; ours is
//! **read from the type table by name**: the mode of group 1306 called
//! `"<hull name> Defense Mode"`, `"... Propulsion Mode"`, `"... Sharpshooter
//! Mode"` ([`SUFFIXES`]). [A] The names are the data's; a bundle check
//! (`--check`) that every group-1305 hull finds all three is the falsifier.
//! No hull id is written in the source.
//!
//! # Marks
//!
//! `[V]` read in our client (build 3619879); `[I]` inferred; `[A]` assumed,
//! with its falsifier. No `[A]` value is written into a test.

use crate::content::Content;
use crate::damage::BlueTime;
use crate::item::{Edit, Slot};
use crate::notify::Push;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

#[cfg(test)]
pub(crate) mod tests;

/// `flagHiddenModifers` [V]: where a ship's mode item sits.
pub const FLAG_HIDDEN_MODIFIERS: i64 = 156;
/// `shipStanceDefense`, `shipStanceSpeed`, `shipStanceSniper` [V].
pub const STANCE_DEFENCE: i64 = 1;
pub const STANCE_SPEED: i64 = 2;
pub const STANCE_SNIPER: i64 = 3;
/// `groupTacticalDestroyer` [V-data]: a hull with stances.
pub const GROUP_TACTICAL_DESTROYER: i64 = 1305;
/// The group of a mode type [V-data].
pub const GROUP_STANCE_MODE: i64 = 1306;
/// `stanceSwitchTime` [V-data]: milliseconds, on the hull.
pub const ATTR_STANCE_SWITCH_TIME: i64 = 1985;
/// Filetime ticks in a millisecond (`const.MSEC`) [V].
pub const MSEC: i64 = 10_000;
/// The client's key for a switch while one is running (FSD 4666).
pub const CHANGING_KEY: &str = "ShipAlreadyChangingStance";

/// The name a mode type ends with, by stance [A: see the module header].
pub const SUFFIXES: [(i64, &str); 3] = [
    (STANCE_DEFENCE, "Defense Mode"),
    (STANCE_SPEED, "Propulsion Mode"),
    (STANCE_SNIPER, "Sharpshooter Mode"),
];

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Whether a type is a tactical destroyer.
pub fn is_tactical_destroyer(content: &Content, type_id: i64) -> bool {
    crate::typedata::installed_in(content)
        .table()
        .get(type_id)
        .is_some_and(|t| t.group_id == GROUP_TACTICAL_DESTROYER)
}

/// The mode type of `stance_id` for hull `hull_type`: the group-1306 type
/// named after the hull. `None` for a hull that has none, and for a stance
/// that is not one of the three.
pub fn mode_type(content: &Content, hull_type: i64, stance_id: i64) -> Option<i64> {
    let suffix = SUFFIXES.iter().find(|(s, _)| *s == stance_id)?.1;
    let table = crate::typedata::installed_in(content).table();
    let hull = table.get(hull_type)?;
    if hull.group_id != GROUP_TACTICAL_DESTROYER {
        return None;
    }
    let want = format!("{} {suffix}", hull.name);
    table
        .types()
        .find(|t| t.group_id == GROUP_STANCE_MODE && t.name == want)
        .map(|t| t.type_id)
}

/// The stances a hull has: all three, or none.
pub fn stances_of(content: &Content, hull_type: i64) -> Vec<i64> {
    let all: Vec<i64> = SUFFIXES
        .iter()
        .filter(|(s, _)| mode_type(content, hull_type, *s).is_some())
        .map(|(s, _)| *s)
        .collect();
    if all.len() == SUFFIXES.len() {
        all
    } else {
        Vec::new()
    }
}

/// The stance a mode type stands for, by its name.
pub fn stance_of(content: &Content, mode_type_id: i64) -> Option<i64> {
    let t = crate::typedata::installed_in(content)
        .table()
        .get(mode_type_id)?;
    if t.group_id != GROUP_STANCE_MODE {
        return None;
    }
    SUFFIXES
        .iter()
        .find(|(_, suffix)| t.name.ends_with(suffix))
        .map(|(s, _)| *s)
}

/// The ship's current stance: the mode item at flag 156, if any.
pub fn current(w: &Registry, ship_id: i64) -> Option<i64> {
    w.store()
        .contents_of(ship_id)
        .filter(|i| i.flag_id == FLAG_HIDDEN_MODIFIERS)
        .find_map(|i| stance_of(w.content(), i.type_id))
}

// ---------------------------------------------------------------------------
// The lock
// ---------------------------------------------------------------------------

/// One ship's last switch: what the slim says, and when the ship is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub old: Option<i64>,
    pub new: i64,
    /// Filetime of the switch (the slim's `time`).
    pub at: BlueTime,
    /// Filetime from which another switch is allowed.
    pub free_at: BlueTime,
}

/// The world slot: the last switch of each ship that has made one since the
/// server started. Not persisted: the item is the state.
#[derive(Debug, Default)]
pub struct Stances {
    ships: BTreeMap<i64, Switch>,
}

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "stance",
    make: |_| Box::new(Stances::default()) as Boxed,
};

/// The ship's last switch, if it made one this run.
pub fn last_switch(w: &Registry, ship_id: i64) -> Option<Switch> {
    w.ext::<Stances>().ships.get(&ship_id).copied()
}

/// How long a hull takes to switch, in filetime ticks: its
/// `stanceSwitchTime` (ms) [V-data]. A hull without the attribute has no lock.
pub fn switch_ticks(content: &Content, hull_type: i64) -> BlueTime {
    let ms = crate::typedata::attribute_in(content, hull_type, ATTR_STANCE_SWITCH_TIME)
        .unwrap_or(0.0)
        .max(0.0);
    (ms as i64) * MSEC
}

// ---------------------------------------------------------------------------
// The switch
// ---------------------------------------------------------------------------

/// Why a switch was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not the caller's active ship.
    NotYours,
    /// The hull has no stances, or the stance is not one of its three.
    NoSuchStance,
    /// The last switch is still running.
    Changing,
    /// The store refused the plan.
    Store,
}

/// A switch that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switched {
    pub old: Option<i64>,
    pub new: i64,
    pub item_id: i64,
}

/// **`shipStanceMgr.SetShipStance`**: put the mode of `stance_id` in the
/// caller's ship, tell the pilot and everyone who sees the ship.
///
/// `Ok(None)` when the ship is already in that stance (the client's early
/// return). A ship with no mode item yet takes any of its three. `now` is a
/// filetime.
pub fn set_stance(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    stance_id: i64,
    now: BlueTime,
) -> Result<Option<Switched>, Refusal> {
    let c = w.character(character_id).ok_or(Refusal::NotYours)?;
    if c.ship_id != ship_id {
        return Err(Refusal::NotYours);
    }
    let ship = w.item(ship_id).ok_or(Refusal::NotYours)?;
    let (hull, owner) = (ship.type_id, ship.owner_id);
    let mode = mode_type(w.content(), hull, stance_id).ok_or(Refusal::NoSuchStance)?;
    if stances_of(w.content(), hull).is_empty() {
        return Err(Refusal::NoSuchStance);
    }
    let old = current(w, ship_id);
    if old == Some(stance_id) {
        return Ok(None);
    }
    if last_switch(w, ship_id).is_some_and(|s| now < s.free_at) {
        return Err(Refusal::Changing);
    }
    let at = Slot::new(owner, ship_id, FLAG_HIDDEN_MODIFIERS);
    let old_modes: Vec<crate::item::Item> = w
        .store()
        .contents_of(ship_id)
        .filter(|i| i.flag_id == FLAG_HIDDEN_MODIFIERS)
        .cloned()
        .collect();
    // A type stacks, so the item is minted and assembled: only a singleton
    // is a module to the client and to the dogma inputs. The mint is quiet;
    // the one `apply_items` below destroys the old mode, then assembles the
    // new one, so the pilot gets one invalidation per switch, not two (a
    // second one landed on a hold tracker that had just refetched, and its
    // read hit a container with no moniker).
    let made = w
        .apply_items_quiet(&[Edit::Create {
            type_id: mode,
            quantity: 1,
            to: at,
        }])
        .map_err(|_| Refusal::Store)?;
    let item_id = made
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .ok_or(Refusal::Store)?;
    // Flag 156 is a fitting flag to the client (`IsFittingFlag`), so its
    // dogma holds the old mode's modifiers until the junkyard `OnItemChange`
    // that `apply_items` sends for the destroyed mode takes the item off the
    // ship.
    let mut edits: Vec<Edit> = old_modes
        .iter()
        .map(|i| Edit::Destroy { item_id: i.item_id })
        .collect();
    edits.push(Edit::Assemble { item_id, to: at });
    w.apply_items(&edits).map_err(|_| Refusal::Store)?;

    let free_at = now + switch_ticks(w.content(), hull);
    let alive: Vec<i64> = w.ext::<Stances>().ships.keys().copied().collect();
    let dead: Vec<i64> = alive
        .into_iter()
        .filter(|id| w.item(*id).is_none())
        .collect();
    let slot = w.ext_mut::<Stances>();
    for id in dead {
        slot.ships.remove(&id);
    }
    slot.ships.insert(
        ship_id,
        Switch {
            old,
            new: stance_id,
            at: now,
            free_at,
        },
    );
    w.push(StanceActive {
        to: character_id,
        ship_id,
        stance_id,
    });
    refresh_slim(w, character_id);
    Ok(Some(Switched {
        old,
        new: stance_id,
        item_id,
    }))
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// `OnStanceActive(shipID, stanceID)` to the pilot [V]
/// (`shipmode/notifier.Notifier.on_stance_changed`, `SinglecastByCharID`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StanceActive {
    pub to: i64,
    pub ship_id: i64,
    pub stance_id: i64,
}

impl Push for StanceActive {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnStanceActive",
            vec![PyValue::Int(self.ship_id), PyValue::Int(self.stance_id)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// The slim field `shipStance = (old, time, new)` [V]
/// (`shipmode/notifier.get_slim_item_field`). `old` is `None` for a first
/// stance. A ship that switched before the last restart has no record: it
/// reads `(current, 0, current)`.
pub fn slim_field(w: &Registry, ship_id: i64) -> Option<PyValue> {
    let now = current(w, ship_id)?;
    let (old, at, new) = match last_switch(w, ship_id).filter(|s| s.new == now) {
        Some(s) => (s.old, s.at, s.new),
        None => (Some(now), 0, now),
    };
    Some(PyValue::Tuple(vec![
        old.map_or(PyValue::None, PyValue::Int),
        PyValue::Int(at),
        PyValue::Int(new),
    ]))
}

/// **The `slim_extra::SOURCES` row**: `shipStance` on a pilot's ship that
/// has a mode item. The cheap test first: a ship with nothing at flag 156 is
/// never looked up in the type table.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter(|id| {
            w.store()
                .contents_of(**id)
                .any(|i| i.flag_id == FLAG_HIDDEN_MODIFIERS)
        })
        .filter_map(|id| {
            w.roster().by_ship(*id)?;
            Some((*id, vec![("shipStance", slim_field(w, *id)?)]))
        })
        .collect()
}

/// A fresh `OnSlimItemChange` for the pilot's ship to everyone in its
/// system (`UpdateSlimItemField`), as `sec_status::on_changed` sends one.
/// A pilot docked has nothing to refresh: the next bootstrap carries it.
pub(crate) fn refresh_slim(w: &mut Registry, character: i64) {
    let Some(p) = w.roster().get(character).copied() else {
        return;
    };
    let Some(c) = w.character(character) else {
        return;
    };
    let (type_id, corp) = (c.ship_type_id, c.corporation_id);
    let to = crate::dungeon::audience(w, p.solar_system_id);
    if to.is_empty() {
        return;
    }
    let mut pairs: Vec<(&'static str, PyValue)> = vec![
        ("itemID", PyValue::Int(p.ship_id)),
        ("typeID", PyValue::Int(type_id)),
        ("ownerID", PyValue::Int(character)),
        ("charID", PyValue::Int(character)),
        ("corpID", PyValue::Int(corp)),
    ];
    let fitted = crate::modules::slim_modules(w.store(), p.ship_id);
    if !fitted.is_empty() {
        pairs.push((
            "modules",
            PyValue::List(
                fitted
                    .iter()
                    .map(|m| {
                        PyValue::Tuple(vec![
                            PyValue::Int(m.item_id),
                            PyValue::Int(m.type_id),
                            PyValue::Int(m.flag_id),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    for (id, extra) in crate::slim_extra::for_slims(w, &[p.ship_id]) {
        if id == p.ship_id {
            pairs.extend(extra);
        }
    }
    let stamp = w.next_destiny_stamp();
    w.schedule(
        std::time::Duration::ZERO,
        crate::sec_status::ShipSlimChange {
            to,
            stamp,
            item_id: p.ship_id,
            pairs,
        },
    );
}

// ---------------------------------------------------------------------------
// A bot's choice
// ---------------------------------------------------------------------------

/// **Which stance a bot flies in** (kept simple on purpose). Hurt: defence.
/// A target past the weapon's reach, or none to shoot: speed, to close or to
/// roam. A target in reach: sniper. `hurt` is the share of shield and armour
/// lost; `range_m` the distance to the target, `reach_m` the weapon's
/// optimal plus falloff.
///
/// [A] the 0.5 threshold; falsified by a bot that dies sitting in sniper
/// mode at half health. No bot flies a tactical destroyer yet
/// (`hull_choice` lists none); this is the rule for the one that does.
pub const HURT_AT: f64 = 0.5;

pub fn pick(hurt: f64, range_m: Option<f64>, reach_m: f64) -> i64 {
    if hurt >= HURT_AT {
        return STANCE_DEFENCE;
    }
    match range_m {
        Some(r) if r <= reach_m => STANCE_SNIPER,
        _ => STANCE_SPEED,
    }
}
