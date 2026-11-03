//! **Cargo containers** (issue #57, lane `containers-mtu`): assembling one,
//! putting items in and taking them out, the password of a secure one, and
//! breaking a courier package's seal.
//!
//! MTU and the depot are not here. A container floating in space (launching
//! one with `Jettison`, scooping one, opening one beside the ship) is not here
//! either: this is the station half, which is where a container is assembled
//! and filled.
//!
//! # What the client does
//!
//! - **Assemble.** The hangar menu offers "Assemble Container" for a
//!   packaged container of `invConst.playerDeployedContainers` (cargo 12,
//!   secure 340, audit-log secure 448, freight 649) that sits in the pilot's
//!   own location while docked (`itemCheckers.OfferAssembleContainer`),
//!   and sends `invbroker.AssembleCargoContainer(itemID, None, 0.0)` on
//!   `invCache.GetInventoryMgr()` (`menusvc.AssembleContainer`) [V]. The
//!   answer is dropped; what the client learns is the `OnItemChange` of the
//!   new singleton, exactly as for a ship.
//! - **Open.** An assembled container is `invCtrl.StationContainer`
//!   (`invControllers.py:1501`) [V]: `locationFlag` is `None`, so its window
//!   lists `GetInventoryFromId(containerID).List()` with no flag and holds
//!   whatever has `locationID == containerID`
//!   (`BaseCelestialContainer.IsItemHere`, `:1230`) [V].
//! - **Move in.** A drop on the window is `Add(itemID, sourceID, qty=n,
//!   flag=None)` on the container's inventory (`BaseInvContainer.__AddItem`,
//!   `:203`/`:216`) [V]; several are `MultiAdd([ids], sourceID, flag=None)`.
//!   The refusal the client understands is `NotEnoughCargoSpace`
//!   (`_AddItem` catches it and offers the quantity that fits).
//! - **Move out.** The same call on the destination inventory with the
//!   container as `sourceID`: `Add(itemID, containerID, qty=n,
//!   flag=flagHangar)` on the hangar, `flag=flagCargo` on a ship.
//! - **Passwords.** "Set New Password" on a secure container opens
//!   `menuFunctions.AskNewContainerPassword` (`:261`) [V]: it asks
//!   `container.HasExistingPasswordSet(which)`, shows an "old password"
//!   field only if that is true, refuses fewer than three characters and a
//!   mismatch itself, and sends `container.SetPassword(which, old, new)`
//!   with `which` 1 (`SCCPasswordTypeGeneral`) or 2 (`...Config`, audit-log
//!   containers only). Every call on an inventory goes through
//!   `PasswordHandlerCallWrapper` (`invCache.py:1405`) [V]: a `UserError`
//!   `SCGeneralPasswordRequired` or `SCConfigPasswordRequired` (not from
//!   `SetPassword`) opens a password box, calls
//!   `EnterPassword(which, passw)` and **repeats the call**; the key
//!   `PasswordProtectedCacheInvalid` drops the client's cache of the
//!   location in `args[1]` and then raises `SCGeneralPasswordRequired`.
//! - **Break the seal.** The courier package (`typePlasticWrap`) offers
//!   "Break Contract" (`OfferBreakContract`), confirms with
//!   `ConfirmBreakCourierPackage` ("Fail contract?" / "Breaking the seal on
//!   this package will make it impossible to complete any contract
//!   associated with the package") and calls
//!   `GetInventoryFromId(packageID).BreakPlasticWrap()` (`invItemFunctions.
//!   Break:609`) [V]. So it is the courier failing the contract.
//!
//! # What the server does
//!
//! - **Contents** are items located *in* the container, owner the pilot,
//!   flag [`FLAG_IN_CONTAINER`] (0), the convention of a jettisoned can and a
//!   hacked can (`hacking_loot::FLAG_IN_CAN`) **[A]**: a retail container's
//!   contents may carry another flag; falsified by a capture of
//!   `OnItemChange` for an item dropped into one. The client does not care:
//!   the container window ignores the flag.
//! - **Reach.** A container is usable only while the pilot is docked and the
//!   container is theirs, in their station hangar or in the cargo hold of the
//!   active ship or an assembled ship of theirs in that hangar. Items come
//!   from the hangar, from such a hold, or from another such container.
//!   Corporation containers are not served **[A]**.
//! - **Capacity** is the container type's own `capacity` against the volume
//!   of everything in it. A container cannot hold an assembled container or
//!   an assembled ship **[A]**: the client has no check, and retail's answer
//!   for a container in a container is not in the client.
//! - **Passwords** are stored as delta kind [`KIND`]: `container which ->
//!   password`. A password is clear text, as the retail one is visible to
//!   its owner (`RetrievePassword`). Who has entered one is not saved: it is
//!   a session's knowledge, so a restart asks again. The pilot who sets a
//!   password has entered it. Changing the password invalidates every other
//!   pilot's entry, because an entry is compared with the password as it is
//!   now. A secure container asks for its general password to **list,
//!   add to, stack and take from**; the config password (audit-log
//!   containers) is stored and checked for `SetPassword` only, and the
//!   audit log itself is not served **[A]**.
//! - **Repackaging** a container moves its contents to the hangar (the
//!   `repackagingSvc` arm already does) and does not clear its password,
//!   because it keeps its id; **[A]** retail clears it. A fresh assemble
//!   forgets any password row of that id.

use crate::fitting::{FLAG_CARGO, Refusal};
use crate::item::{Change, Edit, Item, Slot};
use crate::model::{CATEGORY_SHIP, Character, FLAG_HANGAR};
use crate::service::{Registry, Request, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`): `container which
/// -> password`.
pub const KIND: &str = "cpass";

/// Where the contents of a container sit: `flagNone`, as a can's
/// (`hacking_loot::FLAG_IN_CAN`) **[A]**, see the module note.
pub const FLAG_IN_CONTAINER: i64 = crate::hacking_loot::FLAG_IN_CAN;

/// `const.SCCPasswordTypeGeneral` and `...Config` [V] (`appConst.py`).
pub const PASSWORD_GENERAL: i64 = 1;
pub const PASSWORD_CONFIG: i64 = 2;

/// `groupSecureCargoContainer` and `groupAuditLogSecureContainer` [V]: the
/// two groups that take a password (`baseCheckers.IsSecureContainer`).
const GROUP_SECURE: i64 = 340;
const GROUP_AUDIT_LOG: i64 = 448;

/// The password length limits: the popup's `maxlength` 16
/// (`AskNewContainerPassword`) and its three-letter minimum [V].
pub const MIN_PASSWORD: usize = 3;
pub const MAX_PASSWORD: usize = 16;

/// `containerHangar` [V]: the id the station hangar's inventory is bound on.
const HANGAR_INVENTORY: i64 = 10_004;

/// Who has entered which password this run: `(character, container, which)
/// -> the text entered`. Compared with the stored password on every use.
#[derive(Debug, Default)]
struct Entered(BTreeMap<(i64, i64, i64), String>);

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "containers",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Entered::default())
}

// ---------------------------------------------------------------------------
// What is a container
// ---------------------------------------------------------------------------

/// An assembled container a pilot can open: one of the four client groups
/// (`crate::ship_config::CONTAINER_GROUPS`), a singleton, and not the courier
/// package, which shares group 649.
pub fn is_container(i: &Item) -> bool {
    i.singleton
        && i.type_id != crate::contracts::TYPE_PLASTIC_WRAP
        && crate::ship_config::CONTAINER_GROUPS.contains(&i.group_id)
}

/// A packaged one: the same groups, not yet assembled.
fn is_packaged_container(i: &Item) -> bool {
    !i.singleton
        && i.type_id != crate::contracts::TYPE_PLASTIC_WRAP
        && crate::ship_config::CONTAINER_GROUPS.contains(&i.group_id)
}

fn takes_password(i: &Item) -> bool {
    matches!(i.group_id, GROUP_SECURE | GROUP_AUDIT_LOG)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn raise(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, Vec::new()))
}

fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

fn refused(r: Refusal) -> ServiceError {
    r.to_error()
}

fn plan_refused(e: crate::item::EditError) -> ServiceError {
    crate::outln!("[containers] plan refused: {e}");
    notify("The server could not move that item.")
}

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

fn key(container: i64, which: i64) -> Vec<String> {
    vec![container.to_string(), which.to_string()]
}

/// The stored password of a container, if it has one.
pub fn password_of(reg: &Registry, container: i64, which: i64) -> Option<String> {
    reg.deltas()
        .of(KIND)
        .get(&key(container, which))
        .and_then(|v| v.first().cloned())
}

fn entered(reg: &Registry, who: i64, container: i64, which: i64) -> Option<&str> {
    reg.ext::<Entered>()
        .0
        .get(&(who, container, which))
        .map(String::as_str)
}

/// Whether `who` may open `container` now: it takes no password, has none
/// set, or `who` has entered it.
fn open_to(reg: &Registry, who: i64, container: &Item) -> bool {
    if !takes_password(container) {
        return true;
    }
    match password_of(reg, container.item_id, PASSWORD_GENERAL) {
        None => true,
        Some(pw) => entered(reg, who, container.item_id, PASSWORD_GENERAL) == Some(pw.as_str()),
    }
}

/// `SCGeneralPasswordRequired` unless `who` may open the container: the
/// client asks for the password, calls `EnterPassword` and repeats the call.
pub fn gate(reg: &Registry, who: i64, container: &Item) -> Result<(), ServiceError> {
    if open_to(reg, who, container) {
        Ok(())
    } else {
        Err(raise("SCGeneralPasswordRequired"))
    }
}

/// For the container an item is **taken from** (the call is on another
/// inventory, so the box cannot be answered there): `PasswordProtectedCacheInvalid`,
/// which makes the client drop its cache of `args[1]` and raise
/// `SCGeneralPasswordRequired` itself (`invCache.py:1413`) [V].
fn gate_source(reg: &Registry, who: i64, container: &Item) -> Result<(), ServiceError> {
    if open_to(reg, who, container) {
        Ok(())
    } else {
        Err(raise("PasswordProtectedCacheInvalid"))
    }
}

/// Whether `who` may scoop a floating container: it takes no password or has
/// none, `who` entered it this run, or `password` is it (#82).
pub fn scoop_open(reg: &Registry, who: i64, container: &Item, password: Option<&str>) -> bool {
    open_to(reg, who, container)
        || password.is_some_and(|p| {
            password_of(reg, container.item_id, PASSWORD_GENERAL).as_deref() == Some(p)
        })
}

fn clean_password(raw: &str) -> bool {
    !raw.chars().any(char::is_control)
}

/// `HasExistingPasswordSet(which)`: a bool, as the client tests its truth.
fn has_password(reg: &Registry, c: &Item, args: &[PyValue]) -> PyValue {
    let which = int(args.first());
    PyValue::Bool(which.is_some_and(|w| password_of(reg, c.item_id, w).is_some()))
}

/// `SetPassword(which, old, new)`.
///
/// Refusals, with keys the client ships: `PermissionDenied` for a container
/// that is not the caller's; `CantSetPasswordTooLong` (arg `count`);
/// `CantChangePasswordToSame`; and for a wrong old password the key that asks
/// for it, `SCGeneralPasswordRequired` or `SCConfigPasswordRequired`
/// **[A]** (the client re-raises it for `SetPassword`, so the dialog is what
/// the pilot reads; retail's key for a wrong old password is not in the
/// client).
fn set_password(
    reg: &mut Registry,
    actor: i64,
    c: &Item,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    if c.owner_id != actor {
        return Err(raise("PermissionDenied"));
    }
    let which = match int(args.first()) {
        Some(w @ (PASSWORD_GENERAL | PASSWORD_CONFIG)) => w,
        _ => return Err(notify("That is not a password this container has.")),
    };
    if !takes_password(c) || (which == PASSWORD_CONFIG && c.group_id != GROUP_AUDIT_LOG) {
        return Err(notify("That is not a password this container has."));
    }
    let old = text(args.get(1));
    let new = text(args.get(2)).unwrap_or_default();
    if new.chars().count() < MIN_PASSWORD {
        return Err(notify(
            "A container password needs at least three characters.",
        ));
    }
    if new.chars().count() > MAX_PASSWORD {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "CantSetPasswordTooLong",
            crate::pyutil::pypairs(vec![("count", PyValue::Int(MAX_PASSWORD as i64))]),
        )));
    }
    if !clean_password(&new) {
        return Err(notify(
            "A container password cannot hold control characters.",
        ));
    }
    if let Some(current) = password_of(reg, c.item_id, which) {
        if old.as_deref() != Some(current.as_str()) {
            return Err(raise(if which == PASSWORD_GENERAL {
                "SCGeneralPasswordRequired"
            } else {
                "SCConfigPasswordRequired"
            }));
        }
        if new == current {
            return Err(raise("CantChangePasswordToSame"));
        }
    }
    prune(reg);
    reg.deltas_mut()
        .put(KIND, key(c.item_id, which), vec![new.clone()]);
    reg.ext_mut::<Entered>()
        .0
        .insert((actor, c.item_id, which), new);
    Ok(PyValue::None)
}

/// `EnterPassword(which, passw)`: remembered for this run, compared at use.
/// Answers whether it matches (the client drops the answer and repeats its
/// call).
fn enter_password(reg: &mut Registry, actor: i64, c: &Item, args: &[PyValue]) -> PyValue {
    let (Some(which), Some(pw)) = (int(args.first()), text(args.get(1))) else {
        return PyValue::Bool(false);
    };
    let right = password_of(reg, c.item_id, which).is_none_or(|stored| stored == pw);
    reg.ext_mut::<Entered>()
        .0
        .insert((actor, c.item_id, which), pw);
    PyValue::Bool(right)
}

/// `RetrievePassword(which)`: the owner reads it back; nobody else.
fn retrieve_password(
    reg: &Registry,
    actor: i64,
    c: &Item,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    if c.owner_id != actor {
        return Err(raise("PermissionDenied"));
    }
    Ok(int(args.first())
        .and_then(|w| password_of(reg, c.item_id, w))
        .map_or(PyValue::None, PyValue::Unicode))
}

/// Drop the password rows of containers that are gone, or no longer
/// assembled.
fn prune(reg: &mut Registry) {
    let stale: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| {
            k.first()
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(|id| reg.item(id))
                .is_none_or(|i| !is_container(i))
        })
        .cloned()
        .collect();
    for k in stale {
        reg.deltas_mut().del(KIND, &k);
    }
}

/// Forget every password row of a container id: it was just assembled.
fn forget(reg: &mut Registry, id: i64) {
    for which in [PASSWORD_GENERAL, PASSWORD_CONFIG] {
        if password_of(reg, id, which).is_some() {
            reg.deltas_mut().del(KIND, &key(id, which));
        }
    }
}

// ---------------------------------------------------------------------------
// Small argument readers
// ---------------------------------------------------------------------------

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

fn text(v: Option<&PyValue>) -> Option<String> {
    match v {
        Some(PyValue::Unicode(s)) => Some(s.clone()),
        Some(PyValue::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn int_kwarg(req: &Request<'_>, name: &str) -> Option<i64> {
    int(crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

fn ids_of(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|i| match i {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The item id an inventory is bound on.
fn bound_id(bind: Option<&PyValue>) -> Option<i64> {
    match bind? {
        PyValue::Tuple(parts) | PyValue::List(parts) => int(parts.first()),
        PyValue::Int(id) => Some(*id),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Reach
// ---------------------------------------------------------------------------

/// A ship whose hold the pilot can use now: the active ship, or an
/// assembled ship of theirs in the hangar of the station they are docked at.
fn hold_reachable(reg: &Registry, c: &Character, station: i64, ship_id: i64) -> bool {
    reg.store().get(ship_id).is_some_and(|s| {
        s.owner_id == c.character_id
            && s.category_id == CATEGORY_SHIP
            && s.singleton
            && (ship_id == c.ship_id || (s.location_id == station && s.flag_id == FLAG_HANGAR))
    })
}

/// A container the pilot can reach: theirs, in their hangar or a hold they
/// can reach.
fn container_reachable(reg: &Registry, c: &Character, station: i64, container: &Item) -> bool {
    is_container(container)
        && container.owner_id == c.character_id
        && ((container.location_id == station && container.flag_id == FLAG_HANGAR)
            || (container.flag_id == FLAG_CARGO
                && hold_reachable(reg, c, station, container.location_id)))
}

fn docked(reg: &Registry, actor: i64) -> Option<(Character, i64)> {
    let station = reg.docked_station(actor)?;
    Some((reg.character(actor)?.clone(), station))
}

// ---------------------------------------------------------------------------
// Assembling
// ---------------------------------------------------------------------------

/// `invbroker.AssembleCargoContainer(itemID, name, 0.0)`: one packaged
/// container becomes an assembled one, in place; from a stack it is one off
/// the stack with a new id (`Edit::Assemble`). The name, if the pilot gave
/// one, is the container's name (`item_names`). Answers `None`.
///
/// Refused with `CanOnlyDoInStations` in space and a `CustomNotify` for what
/// the client's menu would not have offered. An already assembled container
/// is left alone.
pub fn assemble(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(item_id) = int(req.args.first()) else {
        return Err(ServiceError::BadArguments {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
            why: "(itemID, name, ?)".to_owned(),
        });
    };
    let Some(actor) = req.actor else {
        return Err(notify("No character is selected."));
    };
    let Some((c, station)) = docked(reg, actor) else {
        return Err(raise("CanOnlyDoInStations"));
    };
    let item = reg
        .item(item_id)
        .cloned()
        .ok_or_else(|| notify("This server does not know that container."))?;
    if item.owner_id != actor {
        return Err(notify("That container is not yours."));
    }
    if is_container(&item) {
        return Ok(PyValue::None);
    }
    if !is_packaged_container(&item) {
        return Err(notify("That is not a container."));
    }
    let in_hangar = item.location_id == station && item.flag_id == FLAG_HANGAR;
    let in_hold = item.flag_id == FLAG_CARGO && hold_reachable(reg, &c, station, item.location_id);
    if !in_hangar && !in_hold {
        return Err(notify(
            "A container is assembled from your hangar or the cargo hold of your ship.",
        ));
    }
    let changes = reg
        .apply_items(&[Edit::Assemble {
            item_id,
            to: item.slot(),
        }])
        .map_err(plan_refused)?;
    let made = changes.iter().find_map(|ch| match ch {
        Change::Assembled { item_id, .. } | Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    });
    if let Some(id) = made {
        forget(reg, id);
        if let Some(name) = text(req.args.get(1)).filter(|n| !n.trim().is_empty()) {
            let _ = crate::item_names::set_label(reg, req.actor, id, &name);
        }
    }
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// Moving items
// ---------------------------------------------------------------------------

/// Where a move is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// The docked station's hangar.
    Hangar,
    /// A plain hold of a ship the pilot can reach.
    Hold { ship_id: i64, flag: i64 },
    /// A container the pilot can reach.
    Container(i64),
}

/// The m3 in a container: everything located in it, at any flag.
pub fn used_volume(reg: &Registry, container: i64) -> f64 {
    reg.store()
        .contents_of(container)
        .map(|i| {
            crate::item::find_in(reg.content(), i.type_id).map_or(0.0, |t| t.volume)
                * i.quantity as f64
        })
        .sum()
}

/// A container's capacity in m3: its type's own **[A]** (no module or skill
/// changes it).
pub fn capacity(reg: &Registry, container: &Item) -> f64 {
    crate::item::find_in(reg.content(), container.type_id).map_or(0.0, |t| t.capacity)
}

/// The edits that move `quantity` of `item_id` (all of it when `None`) to
/// `dest`, or why not. Nothing is applied.
///
/// The item must be the pilot's and either in a container they can reach (the
/// way out) or, for a move into a container, in the hangar or a hold they
/// can reach. A move out leaves a container that asks for a password
/// untouched until it is entered.
pub fn plan(
    reg: &Registry,
    c: &Character,
    station: i64,
    dest: Dest,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, ServiceError> {
    let me = c.character_id;
    let item = reg
        .store()
        .get(item_id)
        .ok_or_else(|| refused(Refusal::NotAvailable))?;
    if item.owner_id != me || item_id == c.ship_id || crate::item::is_loaded_charge(item) {
        return Err(refused(Refusal::NotAvailable));
    }
    // ---- the source ----
    let from_container = reg
        .item(item.location_id)
        .filter(|p| is_container(p))
        .cloned();
    if let Some(p) = &from_container {
        if !container_reachable(reg, c, station, p) {
            return Err(refused(Refusal::NotAvailable));
        }
        gate_source(reg, me, p)?;
    } else {
        let from_hangar = item.location_id == station && item.flag_id == FLAG_HANGAR;
        let from_hold = crate::fitting::is_plain_hold(item.flag_id)
            && hold_reachable(reg, c, station, item.location_id);
        if !(from_hangar || from_hold) {
            return Err(refused(Refusal::NotAvailable));
        }
    }
    // ---- the destination ----
    let to = match dest {
        Dest::Hangar => Slot::new(me, station, FLAG_HANGAR),
        Dest::Hold { ship_id, flag } => {
            if !hold_reachable(reg, c, station, ship_id)
                || !(flag == FLAG_CARGO || crate::fitting::is_plain_hold(flag))
            {
                return Err(refused(Refusal::Unsupported(
                    "Only a hold of your own ship takes items from a container.",
                )));
            }
            if item_id == ship_id {
                return Err(refused(Refusal::InsideItself));
            }
            if flag != FLAG_CARGO
                && crate::item::hold_accepts(flag, item.category_id, item.group_id) != Some(true)
            {
                return Err(refused(Refusal::WrongHold {
                    type_id: item.type_id,
                }));
            }
            if item.category_id == CATEGORY_SHIP && item.singleton {
                return Err(refused(Refusal::AssembledShip));
            }
            Slot::new(me, ship_id, flag)
        }
        Dest::Container(cid) => {
            let target = reg
                .item(cid)
                .filter(|t| container_reachable(reg, c, station, t))
                .ok_or_else(|| refused(Refusal::NotAvailable))?;
            if cid == item_id {
                return Err(refused(Refusal::InsideItself));
            }
            if item.category_id == CATEGORY_SHIP && item.singleton {
                return Err(refused(Refusal::AssembledShip));
            }
            if is_container(item) {
                return Err(refused(Refusal::Unsupported(
                    "A container cannot be placed inside another container.",
                )));
            }
            Slot::new(me, target.item_id, FLAG_IN_CONTAINER)
        }
    };
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    if item.slot() == to {
        return Ok(part.map_or_else(Vec::new, |q| {
            vec![Edit::Split {
                item_id,
                quantity: q,
                to,
            }]
        }));
    }
    let n = part.unwrap_or(item.quantity);
    // ---- room ----
    match dest {
        Dest::Container(cid) => {
            let target = reg
                .item(cid)
                .ok_or_else(|| refused(Refusal::NotAvailable))?;
            let unit = crate::item::find_in(reg.content(), item.type_id).map_or(0.0, |t| t.volume);
            let free = (capacity(reg, target) - used_volume(reg, cid)).max(0.0);
            if crate::fitting::units_that_fit(free, unit) < n {
                return Err(refused(Refusal::NoRoom {
                    available: free,
                    volume: unit * n as f64,
                }));
            }
        }
        Dest::Hold { ship_id, flag } => {
            crate::fitting::check_room_in_hold(
                reg.content(),
                reg.store(),
                ship_id,
                item.type_id,
                n,
                flag,
            )
            .map_err(refused)?;
        }
        Dest::Hangar => {}
    }
    Ok(match part {
        Some(q) => vec![Edit::Split {
            item_id,
            quantity: q,
            to,
        }],
        None => vec![Edit::Move { item_id, to }],
    })
}

/// Move one item and apply the plan.
fn move_one(
    reg: &mut Registry,
    c: &Character,
    station: i64,
    dest: Dest,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    let edits = plan(reg, c, station, dest, item_id, quantity)?;
    if edits.is_empty() {
        return Ok(());
    }
    reg.apply_items(&edits).map(|_| ()).map_err(plan_refused)
}

/// `Add` or `MultiAdd` carrying `ids` to `dest`. The first refusal is the
/// answer when nothing moved; for a `MultiAdd` that moved some, the first
/// refusal is raised after the rest moved, so a part-full container says so.
fn move_many(
    reg: &mut Registry,
    c: &Character,
    station: i64,
    dest: Dest,
    ids: &[i64],
    quantity: Option<i64>,
    single: bool,
) -> Result<PyValue, ServiceError> {
    let mut first = None;
    for &id in ids {
        if let Err(e) = move_one(reg, c, station, dest, id, quantity) {
            if single {
                return Err(e);
            }
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok(PyValue::None),
    }
}

// ---------------------------------------------------------------------------
// The bound inventory
// ---------------------------------------------------------------------------

/// A call on a bound inventory that is a container's, a courier package's, or
/// a move of items **out of** a container onto the hangar or a ship. `None`
/// for everything else, which carries on in `service::inventory`.
pub(crate) fn inventory_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if req.service != "invbroker::inventory" {
        return None;
    }
    let actor = req.actor?;
    let bound = bound_id(req.bind_params)?;
    if let Some(item) = reg.item(bound).cloned() {
        if item.type_id == crate::contracts::TYPE_PLASTIC_WRAP && req.method == "BreakPlasticWrap" {
            return Some(crate::service::break_package(reg, actor, &item));
        }
        if is_container(&item) {
            return container_call(reg, req, actor, &item);
        }
        if crate::mtu::is_unit(reg.content(), &item)
            && let Some(answer) = crate::mtu::unit_call(reg, req, actor, &item)
        {
            return Some(answer);
        }
    }
    out_of_container(reg, req, actor, bound)
}

/// The methods of an inventory bound on an assembled container.
fn container_call(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    container: &Item,
) -> Option<Result<PyValue, ServiceError>> {
    match req.method {
        "HasExistingPasswordSet" => Some(Ok(has_password(reg, container, req.args))),
        "SetPassword" => Some(set_password(reg, actor, container, req.args)),
        "EnterPassword" => Some(Ok(enter_password(reg, actor, container, req.args))),
        "RetrievePassword" => Some(retrieve_password(reg, actor, container, req.args)),
        // The audit-log container's configuration word: no lock-on-add, no
        // password needed for any action **[A]**.
        "ALSCConfigGet" => Some(Ok(PyValue::Int(0))),
        // Asked while the window is drawn; it is information, not contents.
        "GetCapacity" => Some(Ok(crate::pyutil::keyval(vec![
            ("capacity", PyValue::Real(capacity(reg, container))),
            ("used", PyValue::Real(used_volume(reg, container.item_id))),
        ]))),
        "Add" | "MultiAdd" | "StackAll" => {
            if let Err(e) = gate(reg, actor, container) {
                return Some(Err(e));
            }
            let Some((c, station)) = docked(reg, actor) else {
                // Launched whole and floating (#82): the pilot beside it.
                if crate::mtu::in_space(reg, container.item_id) {
                    return Some(crate::mtu::space_add(
                        reg,
                        req,
                        actor,
                        container,
                        crate::combat::LOOT_RANGE,
                    ));
                }
                return Some(Err(refused(Refusal::NotDocked)));
            };
            Some(match req.method {
                "StackAll" => stack_all(reg, actor, container),
                "Add" => {
                    let ids: Vec<i64> = int(req.args.first()).into_iter().collect();
                    move_many(
                        reg,
                        &c,
                        station,
                        Dest::Container(container.item_id),
                        &ids,
                        int_kwarg(req, "qty"),
                        true,
                    )
                }
                _ => move_many(
                    reg,
                    &c,
                    station,
                    Dest::Container(container.item_id),
                    &ids_of(req.args.first()),
                    None,
                    false,
                ),
            })
        }
        // Listing and merging stay where they were; the password stands in
        // front of them.
        "List" | "ListByFlags" | "ListIncludingContainers" | "MultiMerge" => {
            gate(reg, actor, container).err().map(Err)
        }
        _ => None,
    }
}

/// "Stack All" inside a container.
pub(crate) fn stack_all(
    reg: &mut Registry,
    actor: i64,
    container: &Item,
) -> Result<PyValue, ServiceError> {
    let slot = Slot::new(actor, container.item_id, FLAG_IN_CONTAINER);
    let plan = reg.store().stack_plan(slot);
    if plan.is_empty() {
        return Ok(PyValue::None);
    }
    reg.apply_items(&plan).map_err(plan_refused)?;
    Ok(PyValue::None)
}

/// `Add(itemID, containerID, flag=…)` / `MultiAdd([…], containerID, flag=…)`
/// on the hangar's or a ship's inventory, when the items come out of a
/// container. Anything else carries on unchanged.
fn out_of_container(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    bound: i64,
) -> Option<Result<PyValue, ServiceError>> {
    let single = match req.method {
        "Add" => true,
        "MultiAdd" => false,
        _ => return None,
    };
    let ids: Vec<i64> = if single {
        int(req.args.first()).into_iter().collect()
    } else {
        ids_of(req.args.first())
    };
    let parents: Vec<Item> = ids
        .iter()
        .filter_map(|id| reg.item(*id).and_then(|i| reg.item(i.location_id)).cloned())
        .collect();
    if ids.is_empty() || parents.len() != ids.len() {
        return None;
    }
    // Out of a unit or a container floating in space (#82): the owner's
    // gate or the password, and then the loot path takes the items.
    let floating = |p: &Item| {
        (crate::mtu::is_unit(reg.content(), p) || is_container(p))
            && crate::mtu::in_space(reg, p.item_id)
    };
    if parents.iter().all(floating) {
        for p in &parents {
            let gated = if crate::mtu::is_unit(reg.content(), p) {
                crate::mtu::gate_owner(reg, actor, p)
            } else {
                gate_source(reg, actor, p)
            };
            if let Err(e) = gated {
                return Some(Err(e));
            }
        }
        return None;
    }
    if !parents.iter().all(is_container) {
        return None;
    }
    let flag = int_kwarg(req, "flag");
    let dest = if bound == HANGAR_INVENTORY {
        Dest::Hangar
    } else {
        Dest::Hold {
            ship_id: bound,
            flag: flag.unwrap_or(FLAG_CARGO),
        }
    };
    let Some((c, station)) = docked(reg, actor) else {
        return Some(Err(refused(Refusal::NotDocked)));
    };
    let quantity = if single { int_kwarg(req, "qty") } else { None };
    Some(move_many(reg, &c, station, dest, &ids, quantity, single))
}

#[cfg(test)]
mod tests;
