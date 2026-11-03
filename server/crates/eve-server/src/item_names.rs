//! **Names a pilot gives to a ship or a container** (GAPS-2, X-GAPS item 1;
//! lane note `docs/lanes/gaps-2.md`).
//!
//! The client renames through one call, `invbroker.SetLabel(itemID, name)`:
//!
//! - "Set Name" on a ship or a container: `menuFunctions.SetName:402` [V] --
//!   the name is the popup's text with newlines turned to spaces, at most
//!   `maxLength` 100 (`:384`; the pop-up for a ship is
//!   `prompt_rename_ship`, whose limit is not in that function); the call's
//!   answer is dropped (`POP_TOP`), and the client then scatters
//!   `OnItemNameChange` to itself (`:403`) [V];
//! - the fitting window, to mark a fit that failed to load
//!   (`fittingSvc._RenameShip:586`, `LoadFitting:703`) [V].
//!
//! The name is read back through `cfg.evelocations` -- `SetName` primes
//! `cfg.evelocations.Prime([itemID])` before it opens the popup (`:373`) and
//! reads `.Get(itemID).name` for the box's initial text (`:379`) [V] -- so
//! `config.GetMultiLocationsEx` answers a named item with its name
//! ([`overlay`]). A client that already holds the row learns of the change
//! by `OnCfgDataChanged('evelocations', row)`, which `cfg.DataConfig` writes
//! into the recordset it names (`cfg.py:210-232`, `UpdateCfgData:190-196`)
//! [V]: [`CfgRow`] is that push. The same event is what EvEJS sends after a
//! `SetLabel` [I] (facts only: its `_emitSetLabelCfgDataChanged`).
//!
//! Stored as delta kind [`KIND`]: `item_id -> name`, one row per named item.
//! A row of an item that no longer exists is dropped at the next rename.

use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use eve_rowset::cfg::Location;

/// The delta kind this module owns (`delta_kinds::KINDS`): `item_id -> name`.
pub const KIND: &str = "itemname";

/// The longest name: the popup's `maxLength` for anything but a starbase
/// (`menuFunctions.SetName:384`, `:386-387`) [V].
pub const MAX_NAME_CHARS: usize = 100;

/// `categoryShip` [V] (`inventorycommon/const.py`).
const CATEGORY_SHIP: i64 = 6;

/// The container groups the client lets a pilot rename: `invCommon.
/// CONTAINERGROUPS` (cargo, secure, audit-log secure, freight) [V], as
/// `crate::ship_config::CONTAINER_GROUPS`.
fn renameable(group_id: i64, category_id: i64) -> bool {
    category_id == CATEGORY_SHIP || crate::ship_config::CONTAINER_GROUPS.contains(&group_id)
}

/// An item's chosen name, if it has one.
pub fn name_of(reg: &Registry, item_id: i64) -> Option<String> {
    reg.deltas()
        .of(KIND)
        .get(&vec![item_id.to_string()])
        .and_then(|v| v.first().cloned())
}

/// The row an **unnamed** ship or container still needs: `cfg.evelocations`
/// is a `Recordset` whose `Get` raises `RecordNotFound` on a miss, and
/// `_Prime` remembers a missed id (`knownLuzers`) and never asks again. A
/// ship primed while it sat in the hangar (the inventory tree lists it) and
/// boarded later then breaks `invCtrl.ShipCargo.GetName`, which has no
/// guard: the inventory window never loads (owner, 2026-10-29, a Bowhead).
/// So every ship and container the store holds answers, under its type's
/// name; a row the caller already built (the capsule) is left alone.
/// **[A]** an unnamed item's row carries the type name; falsified by a
/// Tranquility capture answering another name (e.g. empty, which the
/// client's `ShipCargo.GetName` would show as a blank title).
fn unnamed(reg: &Registry, id: i64, rows: &[Location]) -> Option<String> {
    if rows.iter().any(|l| l.location_id == id) {
        return None;
    }
    let item = reg.store().get(id)?;
    if !renameable(item.group_id, item.category_id) {
        return None;
    }
    // A type the table lacks still answers (empty, which `GetName` shows as
    // a blank title): a row with no name beats a raise.
    Some(
        crate::typedata::installed_in(reg.content())
            .table()
            .get(item.type_id)
            .map(|t| t.name.clone())
            .unwrap_or_default(),
    )
}

/// The name as stored: control characters (newlines the client already
/// turned to spaces, anything else a hand-made call might carry) become
/// spaces, the ends are trimmed and the length is clamped.
pub fn clean(raw: &str) -> String {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.trim().chars().take(MAX_NAME_CHARS).collect()
}

fn raise(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, Vec::new()))
}

/// `invbroker.SetLabel(itemID, name)`.
///
/// Refusals, with keys the client ships and no parameters
/// (`findings/user-errors.tsv`): `SetNameNotYourStuff` for an item that is
/// not the caller's, `SetNameInvalid` for a type that is neither a ship nor
/// a container. **[A]** that these are the keys retail raised: the client
/// names them (`SetNameNotYourStuffBody`, `SetNameInvalidBody`) but only the
/// server raises them; falsified by a retail capture of a refused rename.
/// An empty name changes nothing: the client never sends one (`SetName:401`
/// tests the popup's text) [V].
///
/// Pushes `OnCfgDataChanged` to the caller so a name the client has cached
/// is replaced at once.
pub fn set_label(
    reg: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
    raw: &str,
) -> Result<(), ServiceError> {
    let Some(actor) = actor else {
        return Err(raise("SetNameNotYourStuff"));
    };
    let Some(item) = reg.item(item_id) else {
        // An item the server has never heard of: nothing to name.
        return Err(raise("SetNameInvalid"));
    };
    if item.owner_id != actor {
        return Err(raise("SetNameNotYourStuff"));
    }
    if !renameable(item.group_id, item.category_id) {
        return Err(raise("SetNameInvalid"));
    }
    let name = clean(raw);
    if name.is_empty() {
        return Ok(());
    }
    prune(reg);
    reg.deltas_mut()
        .put(KIND, vec![item_id.to_string()], vec![name.clone()]);
    reg.push(CfgRow {
        to: actor,
        location: Location::new(item_id, name),
    });
    Ok(())
}

/// Drop the rows of items that are gone.
fn prune(reg: &mut Registry) {
    let gone: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| {
            k.first()
                .and_then(|s| s.parse::<i64>().ok())
                .is_none_or(|id| reg.item(id).is_none())
        })
        .cloned()
        .collect();
    for k in gone {
        reg.deltas_mut().del(KIND, &k);
    }
}

/// `config.GetMultiLocationsEx`'s rows with the pilots' names in them: a
/// named item gets its row (or has its name replaced, when the caller
/// already made one, as for the capsule).
pub fn overlay(reg: &Registry, wanted: &[i64], rows: &mut Vec<Location>) {
    for &id in wanted {
        let Some(name) = name_of(reg, id).or_else(|| unnamed(reg, id, rows)) else {
            continue;
        };
        match rows.iter_mut().find(|l| l.location_id == id) {
            Some(l) => l.location_name = name,
            None => rows.push(Location::new(id, name)),
        }
    }
}

/// `OnCfgDataChanged('evelocations', row)`: one `cfg.evelocations` row,
/// `[locationID, locationName, solarSystemID, x, y, z, locationNameID]`
/// (`eve_rowset::cfg::LOCATION_COLUMNS`), written into the client's
/// recordset under its key column (`cfg.py:UpdateCfgData:194-196`) [V].
/// `[A]` the coordinates are real zeroes and the system `None`, as an item
/// has no place of its own in the map data; falsified by a client that
/// draws a renamed ship's location wrongly.
#[derive(Debug, Clone, PartialEq)]
pub struct CfgRow {
    pub to: i64,
    pub location: Location,
}

impl Push for CfgRow {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let l = &self.location;
        (
            "OnCfgDataChanged",
            vec![
                PyValue::Unicode("evelocations".to_owned()),
                PyValue::List(vec![
                    PyValue::Int(l.location_id),
                    PyValue::Unicode(l.location_name.clone()),
                    l.solar_system_id.map_or(PyValue::None, PyValue::Int),
                    PyValue::Real(l.x),
                    PyValue::Real(l.y),
                    PyValue::Real(l.z),
                    PyValue::None,
                ]),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;

    fn world() -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ],
            ..SandboxConfig::default()
        })
    }

    /// A renamed ship answers to its name in `GetMultiLocationsEx`, and the
    /// name survives a snapshot and a load of the deltas.
    #[test]
    fn a_renamed_ship_is_named_and_the_name_persists() {
        let mut reg = world();
        set_label(&mut reg, Some(ALICE), ALICE_SHIP, "  The\nBeast  ").expect("renamed");
        assert_eq!(name_of(&reg, ALICE_SHIP).as_deref(), Some("The Beast"));
        let mut rows = vec![Location::new(ALICE_SHIP, "Capsule")];
        overlay(&reg, &[ALICE_SHIP, BOB_SHIP], &mut rows);
        assert_eq!(
            rows.iter().filter(|l| l.location_id == ALICE_SHIP).count(),
            1,
            "the capsule row is renamed, not repeated"
        );
        assert_eq!(rows[0].location_name, "The Beast");
        let rows_again = reg.deltas().of(KIND).clone();
        let mut fresh = world();
        for (k, v) in rows_again {
            fresh.deltas_mut().put(KIND, k, v);
        }
        assert_eq!(name_of(&fresh, ALICE_SHIP).as_deref(), Some("The Beast"));
    }

    /// An unnamed ship still answers (under its type's name), so a ship the
    /// client primed in the hangar and boards later has a row; an id that
    /// is no item gets none, and a row already built is left alone.
    #[test]
    fn an_unnamed_ship_answers_under_its_type_name() {
        let reg = world();
        let type_id = reg
            .store()
            .get(BOB_SHIP)
            .expect("Bob's ship is an item")
            .type_id;
        let type_name = crate::typedata::installed_in(reg.content())
            .table()
            .get(type_id)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let mut rows = vec![Location::new(ALICE_SHIP, "Capsule")];
        overlay(&reg, &[ALICE_SHIP, BOB_SHIP, 1], &mut rows);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].location_name, "Capsule");
        assert_eq!(rows[1].location_id, BOB_SHIP);
        assert_eq!(rows[1].location_name, type_name);
    }

    /// Somebody else's ship, an unknown id and an empty name.
    #[test]
    fn a_rename_refuses_what_is_not_the_callers() {
        let mut reg = world();
        let e = set_label(&mut reg, Some(ALICE), BOB_SHIP, "Mine").unwrap_err();
        assert!(matches!(e, ServiceError::Raise(_)));
        assert_eq!(name_of(&reg, BOB_SHIP), None);
        assert!(set_label(&mut reg, Some(ALICE), 1, "x").is_err());
        set_label(&mut reg, Some(ALICE), ALICE_SHIP, "   ").expect("a no-op");
        assert_eq!(name_of(&reg, ALICE_SHIP), None);
    }

    /// The name is cut at the popup's limit.
    #[test]
    fn a_long_name_is_cut_at_the_limit() {
        let long = "x".repeat(MAX_NAME_CHARS + 40);
        assert_eq!(clean(&long).chars().count(), MAX_NAME_CHARS);
    }

    /// The push: `OnCfgDataChanged('evelocations', [id, name, None, 0.0, 0.0,
    /// 0.0, None])` addressed to the renamer.
    #[test]
    fn the_rename_pushes_the_new_row() {
        let mut reg = world();
        set_label(&mut reg, Some(ALICE), ALICE_SHIP, "Reaper").expect("renamed");
        let pushed = reg.take_pending();
        let (event, args) = pushed
            .iter()
            .map(|(_, p)| p.to_push())
            .find(|(e, _)| *e == "OnCfgDataChanged")
            .expect("a cfg push");
        assert_eq!(event, "OnCfgDataChanged");
        assert_eq!(args[0], PyValue::Unicode("evelocations".to_owned()));
        let PyValue::List(row) = &args[1] else {
            panic!("a row list")
        };
        assert_eq!(row[0], PyValue::Int(ALICE_SHIP));
        assert_eq!(row[1], PyValue::Unicode("Reaper".to_owned()));
        assert_eq!(row.len(), 7);
        let who = pushed[0].1.audience();
        assert_eq!(who, Some(vec![ALICE]));
    }
}
