//! **Ship configuration, the fleet hangar and the ship maintenance bay**
//! (CAP-H; lane note `docs/lanes/cap-h.md`; spec `R-CAP.md` §3.6).
//!
//! Three things live here, all of them the client's own design and none of
//! them served before this lane.
//!
//! 1. **The ship config.** `shipConfigSvc` asks `ship.GetShipConfiguration(
//!    shipID)` and indexes the answer by four keys, so all four must be
//!    present or the client raises `KeyError` (`shipConfigSvc.py:62-72`) [V]:
//!    `FleetHangar_AllowFleetAccess`, `FleetHangar_AllowCorpAccess`,
//!    `SMB_AllowFleetAccess`, `SMB_AllowCorpAccess`. The owner toggles one
//!    from the inventory tree (`treeViewEntries.py:TreeViewEntryAccessConfig`)
//!    and the service sends `ship.ConfigureShip({key: value})` for **the
//!    active ship**: the call carries no ship id, and the service's own
//!    `self.shipid` is `eveCfg.GetActiveShip()` [V]. Stored as delta kind
//!    [`KIND`], one row per ship that differs from [`ShipConfig::DEFAULT`].
//! 2. **The access rule** ([`may_use`]), the client's own
//!    `CanUseShipServices` (`celestialCheckers.py:2177`) [V]: the pilot must
//!    be in the ship owner's corporation, or in the owner's fleet, and the
//!    owner's config must allow that route **for that bay**. The client's
//!    expression is `A and B and C or D and E`, so its menu check lets the
//!    fleet-hangar fleet key open the maintenance-bay *entry*; the server
//!    holds each bay to its own keys, so that entry is offered and refused.
//!    The client has no alliance key, so there is no alliance route.
//! 3. **Moves in and out of the two bays** ([`plan`], [`move_item`]): the
//!    drop on a bay is `Add(itemID, sourceID, qty=n, flag=155|90)` on the
//!    ship's inventory, and the way out is the same call on the destination
//!    (`invControllers.py:__AddItem`, `:203`/`:216`) [V]. A pilot in space
//!    may use a fleet member's bay inside [`MAX_TRANSFER_DISTANCE`]
//!    (`invControllers.ShipFleetHangar.GetOperationalDistance` is
//!    `maxCargoContainerTransferDistance`, `appConst.py:1460`) [V].
//!
//! What goes where, by bay:
//!
//! | bay | takes | owner of what arrives |
//! |---|---|---|
//! | fleet hangar 155 | any item | the ship's owner (a guest's deposit is one way) |
//! | maintenance bay 90 | assembled ships, not a capsule | stays the depositor's |
//!
//! The ownership rule for the fleet hangar is the client's own warning: a
//! move into a container in another pilot's fleet hangar asks the depositor
//! to confirm `ConfirmOneWayItemMove` (`invControllers.py:1549`) [V]. The
//! maintenance bay never asks, so what a ferried ship is owned by does not
//! change [I] (EvEJS's reading of the same two lines, facts only).

use crate::destiny;
use crate::fitting::{FLAG_CARGO, Refusal};
use crate::item::{Edit, Item, Slot};
use crate::model::{CATEGORY_SHIP, Character, FLAG_HANGAR};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`): `ship_id ->
/// fleetHangarFleet fleetHangarCorp smbFleet smbCorp`, each `1` or `0`; only
/// a ship that differs from [`ShipConfig::DEFAULT`] has a row.
pub const KIND: &str = "shipcfg";

/// `flagShipHangar`, the ship maintenance bay (`invControllers.
/// ShipMaintenanceBay.locationFlag`, `:1473`) [V].
pub const FLAG_SMB: i64 = 90;
/// `flagFleetHangar` (`invControllers.ShipFleetHangar.locationFlag`, `:1488`)
/// [V].
pub const FLAG_FLEET_HANGAR: i64 = 155;

/// `ShipConfig` keys [V] (`shipConfigSvc.py`).
pub const KEY_FLEET_HANGAR_FLEET: &str = "FleetHangar_AllowFleetAccess";
pub const KEY_FLEET_HANGAR_CORP: &str = "FleetHangar_AllowCorpAccess";
pub const KEY_SMB_FLEET: &str = "SMB_AllowFleetAccess";
pub const KEY_SMB_CORP: &str = "SMB_AllowCorpAccess";

/// `const.maxCargoContainerTransferDistance` [V] (`appConst.py:1460`): the
/// surface-to-surface metres inside which a bay of another ship may be used
/// (`BaseCelestialContainer.IsInRange`, `ball.surfaceDist >
/// GetOperationalDistance`, `invControllers.py:1239-1254`) [V].
pub const MAX_TRANSFER_DISTANCE: f64 = 2_500.0;

/// `inventorycommon/const.groupCapsule` [V]: the maintenance-bay row blocks
/// it (`inventoryFlagData[flagShipHangar]`, `item::HOLDS`).
const GROUP_CAPSULE: i64 = 29;

/// `invCommon.CONTAINERGROUPS` [V] (`inventory/invCommon.py:6`): cargo,
/// secure cargo, audit-log secure and freight containers.
pub const CONTAINER_GROUPS: [i64; 4] = [12, 340, 448, 649];

/// Is this flag the fleet hangar or the maintenance bay?
pub fn is_bay(flag: i64) -> bool {
    flag == FLAG_SMB || flag == FLAG_FLEET_HANGAR
}

// ---------------------------------------------------------------------------
// The config
// ---------------------------------------------------------------------------

/// A ship's four access switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipConfig {
    pub fleet_hangar_fleet: bool,
    pub fleet_hangar_corp: bool,
    pub smb_fleet: bool,
    pub smb_corp: bool,
}

impl ShipConfig {
    /// **[A]** every route closed until the owner opens it: the client has no
    /// default (it shows whatever the server answers) and the only other
    /// server we have facts from starts closed too. Falsifier: retail's
    /// first toggle state on a fresh carrier.
    pub const DEFAULT: ShipConfig = ShipConfig {
        fleet_hangar_fleet: false,
        fleet_hangar_corp: false,
        smb_fleet: false,
        smb_corp: false,
    };

    /// `ship.GetShipConfiguration`'s answer: all four keys, bools.
    pub fn to_value(self) -> PyValue {
        PyValue::Dict(crate::pyutil::pypairs(vec![
            (
                KEY_FLEET_HANGAR_FLEET,
                PyValue::Bool(self.fleet_hangar_fleet),
            ),
            (KEY_FLEET_HANGAR_CORP, PyValue::Bool(self.fleet_hangar_corp)),
            (KEY_SMB_FLEET, PyValue::Bool(self.smb_fleet)),
            (KEY_SMB_CORP, PyValue::Bool(self.smb_corp)),
        ]))
    }

    /// Set one key; false for a key the client never sends.
    fn set(&mut self, key: &[u8], on: bool) -> bool {
        match key {
            k if k == KEY_FLEET_HANGAR_FLEET.as_bytes() => self.fleet_hangar_fleet = on,
            k if k == KEY_FLEET_HANGAR_CORP.as_bytes() => self.fleet_hangar_corp = on,
            k if k == KEY_SMB_FLEET.as_bytes() => self.smb_fleet = on,
            k if k == KEY_SMB_CORP.as_bytes() => self.smb_corp = on,
            _ => return false,
        }
        true
    }

    /// Does the config let a pilot of this relation use `flag`'s bay? The
    /// corp route needs the same corp, the fleet route the same fleet, and
    /// each bay reads its own two keys.
    pub fn allows(self, flag: i64, same_corp: bool, same_fleet: bool) -> bool {
        let (fleet, corp) = match flag {
            FLAG_FLEET_HANGAR => (self.fleet_hangar_fleet, self.fleet_hangar_corp),
            FLAG_SMB => (self.smb_fleet, self.smb_corp),
            _ => return false,
        };
        (corp && same_corp) || (fleet && same_fleet)
    }

    fn row(self) -> Vec<String> {
        [
            self.fleet_hangar_fleet,
            self.fleet_hangar_corp,
            self.smb_fleet,
            self.smb_corp,
        ]
        .iter()
        .map(|b| u8::from(*b).to_string())
        .collect()
    }

    fn from_row(row: &[String]) -> Self {
        let on = |i: usize| row.get(i).is_some_and(|s| s == "1");
        ShipConfig {
            fleet_hangar_fleet: on(0),
            fleet_hangar_corp: on(1),
            smb_fleet: on(2),
            smb_corp: on(3),
        }
    }
}

/// A ship's stored config, [`ShipConfig::DEFAULT`] when it has no row.
pub fn stored(reg: &Registry, ship_id: i64) -> ShipConfig {
    reg.deltas()
        .of(KIND)
        .get(&vec![ship_id.to_string()])
        .map_or(ShipConfig::DEFAULT, |r| ShipConfig::from_row(r))
}

fn store(reg: &mut Registry, ship_id: i64, cfg: ShipConfig) {
    let key = vec![ship_id.to_string()];
    if cfg == ShipConfig::DEFAULT {
        if reg.deltas().of(KIND).contains_key(&key) {
            reg.deltas_mut().del(KIND, &key);
        }
        return;
    }
    reg.deltas_mut().put(KIND, key, cfg.row());
}

/// `ship.GetShipConfiguration(shipID)`: any pilot may read it (the client
/// asks about other pilots' ships to decide what its menu offers).
pub fn configuration_value(reg: &Registry, ship_id: i64) -> PyValue {
    stored(reg, ship_id).to_value()
}

/// `ship.ConfigureShip({key: bool})`: the actor's **active ship**'s
/// switches. Unknown keys are ignored [A]: the client sends only the four.
/// Falsifier: a client path that sends another key and expects an error.
pub fn configure(
    reg: &mut Registry,
    actor: Option<i64>,
    changes: Option<&PyValue>,
) -> Result<PyValue, ServiceError> {
    let ship_id = actor
        .and_then(|a| reg.character(a))
        .map(|c| c.ship_id)
        .ok_or_else(|| notify("No character is selected."))?;
    let owned = reg
        .item(ship_id)
        .is_some_and(|s| Some(s.owner_id) == actor && s.category_id == CATEGORY_SHIP);
    if !owned {
        return Err(notify("That is not your ship."));
    }
    let Some(PyValue::Dict(entries)) = changes else {
        return Err(ServiceError::BadArguments {
            service: "ship".to_owned(),
            method: "ConfigureShip".to_owned(),
            why: "expected ({key: bool},)".to_owned(),
        });
    };
    let mut cfg = stored(reg, ship_id);
    for (value, key) in entries {
        let on = match value {
            PyValue::Bool(b) => *b,
            PyValue::Int(n) => *n != 0,
            _ => continue,
        };
        if let Some(k) = crate::pyutil::as_bytes(key) {
            cfg.set(k, on);
        }
    }
    store(reg, ship_id, cfg);
    Ok(PyValue::None)
}

fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

fn same_corp(reg: &Registry, a: i64, b: i64) -> bool {
    match (reg.character(a), reg.character(b)) {
        (Some(x), Some(y)) => x.corporation_id != 0 && x.corporation_id == y.corporation_id,
        _ => false,
    }
}

fn same_fleet(reg: &Registry, a: i64, b: i64) -> bool {
    match (reg.fleets().fleet_of(a), reg.fleets().fleet_of(b)) {
        (Some(x), Some(y)) => x.fleet_id == y.fleet_id,
        _ => false,
    }
}

/// `CanUseShipServices` [V]: may `actor` use `flag`'s bay of `ship`? The
/// owner always may; anyone else needs the corp or fleet relation **and**
/// the owner's switch for that route and bay.
pub fn may_use(reg: &Registry, actor: i64, ship: &Item, flag: i64) -> bool {
    if ship.owner_id == actor {
        return true;
    }
    stored(reg, ship.item_id).allows(
        flag,
        same_corp(reg, actor, ship.owner_id),
        same_fleet(reg, actor, ship.owner_id),
    )
}

/// Whether `actor` may open one of `container`'s two bays as a guest: what
/// lets `GetSelfInvItem` answer with the ship rather than the station.
pub fn can_open_any(reg: &Registry, actor: i64, container: i64) -> bool {
    reg.item(container).is_some_and(|s| {
        s.category_id == CATEGORY_SHIP
            && s.owner_id != actor
            && (may_use(reg, actor, s, FLAG_FLEET_HANGAR) || may_use(reg, actor, s, FLAG_SMB))
    })
}

/// Why a bay move was refused: the client's access keys, or an ordinary
/// inventory refusal.
#[derive(Debug)]
pub enum BayRefusal {
    /// `CannotAccessShipHangar` [V key, text in the client's localisation]:
    /// "you are not a fellow corporation member of the pilot, or in their
    /// fleet (if they have allowed fleet members access)". No client raise
    /// site (`raise_sites 0`), so the raise is ours [A].
    NoAccess,
    /// `FleetHangarAccessDenied` [V key and text]: "the ship is not
    /// configured to allow fleet members to access the hangar". Raised for
    /// a pilot who is in the owner's corp or fleet and is refused by the
    /// switches [A: which of the two keys retail raises for that case].
    NotConfigured,
    /// `CannotTakeContainerFromFleetHangar` [V key and text]: "containers in
    /// fleet hangars can only be moved by the captain of the ship".
    ContainerNotYours,
    /// `CannotAddShipTypeToMaintainanceBay` [V key]: a non-ship, or a
    /// capsule, into the maintenance bay.
    NotAShip,
    /// Out of [`MAX_TRANSFER_DISTANCE`]. The client checks range before it
    /// sends (`BaseCelestialContainer.IsInRange`) and has no key of its own
    /// for it, so the words are ours [A: a client path that raises a named
    /// error for a bay out of range]; none of the `TooFar` keys in
    /// `findings/user-errors.tsv` is about a ship's bay.
    TooFar,
    Refused(Refusal),
}

impl From<Refusal> for BayRefusal {
    fn from(r: Refusal) -> Self {
        BayRefusal::Refused(r)
    }
}

impl BayRefusal {
    pub fn to_error(&self) -> ServiceError {
        let key = match self {
            BayRefusal::NoAccess => "CannotAccessShipHangar",
            BayRefusal::NotConfigured => "FleetHangarAccessDenied",
            BayRefusal::ContainerNotYours => "CannotTakeContainerFromFleetHangar",
            BayRefusal::NotAShip => "CannotAddShipTypeToMaintainanceBay",
            BayRefusal::TooFar => {
                return notify(&format!(
                    "That ship is too far away: its bays work within {MAX_TRANSFER_DISTANCE} m."
                ));
            }
            BayRefusal::Refused(r) => return r.to_error(),
        };
        ServiceError::Raise(eve_proto::user_error(key, vec![]))
    }
}

/// Surface-to-surface metres between two pilots' ships, both in space in
/// one system; `None` when either is not.
fn surface_distance(reg: &Registry, a: &Character, b_owner: i64, b_ship: &Item) -> Option<f64> {
    let me = reg.roster().get(a.character_id)?;
    let them = reg.roster().get(b_owner)?;
    if me.ship_id != a.ship_id
        || them.ship_id != b_ship.item_id
        || me.solar_system_id != them.solar_system_id
    {
        return None;
    }
    let r = |type_id| f64::from(destiny::hull::of_in(reg.content(), type_id).radius);
    Some(
        crate::space_state::distance(me.motion.position, them.motion.position)
            - r(a.ship_type_id)
            - r(b_ship.type_id),
    )
}

/// May `c` reach `flag` of `ship` at all: his own ship (the active one, or
/// one parked in the hangar he is docked at), or a fleet member's in space,
/// by the access rule and within [`MAX_TRANSFER_DISTANCE`].
fn reach(w: &Registry, c: &Character, ship: &Item, flag: i64) -> Result<(), BayRefusal> {
    if ship.category_id != CATEGORY_SHIP || !ship.singleton {
        return Err(Refusal::NotAvailable.into());
    }
    if ship.owner_id == c.character_id {
        if ship.item_id == c.ship_id {
            return Ok(());
        }
        let parked = w
            .docked_station(c.character_id)
            .is_some_and(|st| ship.location_id == st && ship.flag_id == FLAG_HANGAR);
        return if parked {
            Ok(())
        } else {
            Err(Refusal::NotAvailable.into())
        };
    }
    if !is_bay(flag) {
        return Err(
            Refusal::Unsupported("Only your own ship's cargo hold can take items here.").into(),
        );
    }
    if w.docked_station(c.character_id).is_some() {
        return Err(Refusal::Unsupported("Another pilot's bay can only be used in space.").into());
    }
    if !may_use(w, c.character_id, ship, flag) {
        let related = same_corp(w, c.character_id, ship.owner_id)
            || same_fleet(w, c.character_id, ship.owner_id);
        return Err(if related && flag == FLAG_FLEET_HANGAR {
            BayRefusal::NotConfigured
        } else {
            BayRefusal::NoAccess
        });
    }
    // The slack is `combat::LOOT_RANGE_SLACK`'s: positions are integrated on
    // read and the client measured a moment earlier [A].
    match surface_distance(w, c, ship.owner_id, ship) {
        Some(d) if d <= MAX_TRANSFER_DISTANCE + crate::combat::LOOT_RANGE_SLACK => Ok(()),
        Some(_) | None => Err(BayRefusal::TooFar),
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// What `List(flag=155|90)` on a ship shows `actor`, or `None` when the call
/// is not a bay listing (the caller's ordinary rule applies).
///
/// The owner sees **everything** in his bay -- a guest's ship in his
/// maintenance bay is not his item, and the ordinary "mine only" filter
/// would hide it. A pilot the rule admits (same system, no range test, like a
/// wreck: the client has already decided the window may open) sees the bay
/// too; anyone else sees an empty one.
pub fn bay_listing(
    reg: &Registry,
    actor: i64,
    container: i64,
    flag: Option<i64>,
) -> Option<Vec<Item>> {
    let flag = flag.filter(|f| is_bay(*f))?;
    let ship = reg
        .item(container)
        .filter(|s| s.category_id == CATEGORY_SHIP)?;
    let allowed = ship.owner_id == actor
        || (may_use(reg, actor, ship, flag)
            && matches!(
                (reg.roster().get(actor), reg.roster().get(ship.owner_id)),
                (Some(a), Some(b)) if a.solar_system_id == b.solar_system_id
            ));
    if !allowed {
        return Some(Vec::new());
    }
    Some(
        reg.store()
            .contents_of(container)
            .filter(|i| i.flag_id == flag && !crate::item::is_loaded_charge(i))
            .cloned()
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Moves
// ---------------------------------------------------------------------------

/// Where an `Add` is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// A flag of one ship's inventory: cargo, fuel bay, or one of the bays.
    Hold { ship_id: i64, flag: i64 },
    /// The station hangar (the bound inventory is the hangar's).
    Hangar,
}

/// The ship an item sits in a bay of, if it does.
fn bay_ship<'a>(w: &'a Registry, item: &Item) -> Option<&'a Item> {
    if !is_bay(item.flag_id) {
        return None;
    }
    w.store()
        .get(item.location_id)
        .filter(|s| s.category_id == CATEGORY_SHIP)
}

/// Is `target` `at` itself or somewhere inside it?
fn inside(w: &Registry, mut at: i64, target: i64) -> bool {
    for _ in 0..16 {
        if at == target {
            return true;
        }
        match w.store().get(at) {
            Some(i) => at = i.location_id,
            None => return false,
        }
    }
    false
}

fn is_plain_hold(flag: i64) -> bool {
    matches!(
        flag,
        FLAG_CARGO | crate::item::FLAG_FUEL_BAY | crate::mining_hold::FLAG_GENERAL_MINING_HOLD
    )
}

/// Does moving `item_id` to `dest` touch a bay: the destination is one, or
/// the item sits in one?
fn touches_bay(w: &Registry, dest: Dest, item_id: i64) -> bool {
    let Some(item) = w.store().get(item_id) else {
        return false;
    };
    bay_ship(w, item).is_some() || matches!(dest, Dest::Hold { flag, .. } if is_bay(flag))
}

/// The plan for moving `item_id` to `dest`, **when the move touches a bay**
/// (the destination is one, or the item sits in one); `None` otherwise, and
/// the caller's ordinary rules apply.
pub fn plan(
    w: &Registry,
    c: &Character,
    dest: Dest,
    item_id: i64,
    quantity: Option<i64>,
) -> Option<Result<Vec<Edit>, BayRefusal>> {
    if !touches_bay(w, dest, item_id) {
        return None;
    }
    let item = w.store().get(item_id)?;
    Some(plan_inner(w, c, dest, item, bay_ship(w, item), quantity))
}

fn plan_inner(
    w: &Registry,
    c: &Character,
    dest: Dest,
    item: &Item,
    src: Option<&Item>,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, BayRefusal> {
    let me = c.character_id;
    if item.item_id == c.ship_id {
        return Err(Refusal::ActiveShip.into());
    }
    if crate::item::is_loaded_charge(item) {
        return Err(Refusal::NotAvailable.into());
    }

    // ---- the source ----
    if let Some(ship) = src {
        reach(w, c, ship, item.flag_id)?;
        // A ferried ship is its owner's to take back, nobody else's [A]: the
        // client's bay has no per-item rule, and a guest's ship in my bay
        // being mine to move would be theft. Falsifier: retail lets the
        // bay's owner eject a guest's ship.
        if item.flag_id == FLAG_SMB && item.owner_id != me {
            return Err(BayRefusal::NoAccess);
        }
        // The client's own words: a container in a fleet hangar is moved by
        // the captain alone [V].
        if item.flag_id == FLAG_FLEET_HANGAR
            && ship.owner_id != me
            && CONTAINER_GROUPS.contains(&item.group_id)
        {
            return Err(BayRefusal::ContainerNotYours);
        }
    } else {
        // From the hangar or one of my own holds, my items only.
        let station = w.docked_station(me);
        let from_hangar = station == Some(item.location_id) && item.flag_id == FLAG_HANGAR;
        let from_my_hold = item.location_id == c.ship_id && is_plain_hold(item.flag_id);
        let from_parked = station.is_some_and(|st| {
            item.flag_id == FLAG_CARGO
                && w.store().get(item.location_id).is_some_and(|s| {
                    s.owner_id == me
                        && s.category_id == CATEGORY_SHIP
                        && s.singleton
                        && s.location_id == st
                        && s.flag_id == FLAG_HANGAR
                })
        });
        if item.owner_id != me || !(from_hangar || from_my_hold || from_parked) {
            return Err(Refusal::NotAvailable.into());
        }
    }

    // ---- the destination ----
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    let n = part.unwrap_or(item.quantity);
    let to = match dest {
        Dest::Hangar => {
            let station = w.docked_station(me).ok_or(Refusal::Unsupported(
                "Items leave a bay for the hangar only while docked.",
            ))?;
            Slot::new(me, station, FLAG_HANGAR)
        }
        Dest::Hold { ship_id, flag } => {
            let ship = w.store().get(ship_id).ok_or(Refusal::NotAvailable)?;
            if inside(w, ship_id, item.item_id) {
                return Err(Refusal::InsideItself.into());
            }
            reach(w, c, ship, flag)?;
            let assembled_ship = item.category_id == CATEGORY_SHIP && item.singleton;
            if flag == FLAG_SMB {
                if item.category_id != CATEGORY_SHIP || item.group_id == GROUP_CAPSULE {
                    return Err(BayRefusal::NotAShip);
                }
                if !item.singleton {
                    // The client weighs a packaged ship by a per-group table
                    // this server does not carry, so the bay takes the ships
                    // whose volume it knows: assembled ones.
                    return Err(Refusal::Unsupported(
                        "Assemble the ship before putting it in a maintenance bay.",
                    )
                    .into());
                }
            } else {
                if assembled_ship {
                    return Err(Refusal::AssembledShip.into());
                }
                if flag != FLAG_CARGO
                    && crate::item::hold_accepts(flag, item.category_id, item.group_id)
                        != Some(true)
                {
                    return Err(Refusal::WrongHold {
                        type_id: item.type_id,
                    }
                    .into());
                }
            }
            // The owner of what arrives (see the module table).
            let owner = if flag == FLAG_FLEET_HANGAR {
                ship.owner_id
            } else if flag == FLAG_SMB {
                item.owner_id
            } else {
                me
            };
            let to = Slot::new(owner, ship_id, flag);
            if item.slot() == to {
                // Already there: a split, or nothing.
                return Ok(part.map_or_else(Vec::new, |q| {
                    vec![Edit::Split {
                        item_id: item.item_id,
                        quantity: q,
                        to,
                    }]
                }));
            }
            crate::fitting::check_room_in_hold(
                w.content(),
                w.store(),
                ship_id,
                item.type_id,
                n,
                flag,
            )?;
            to
        }
    };
    let mut edits = vec![match part {
        Some(quantity) => Edit::Split {
            item_id: item.item_id,
            quantity,
            to,
        },
        None => Edit::Move {
            item_id: item.item_id,
            to,
        },
    }];
    // A container's contents change hands with it (one way).
    if part.is_none() && to.owner_id != item.owner_id {
        let mut open = vec![item.item_id];
        while let Some(container) = open.pop() {
            for i in w.store().contents_of(container) {
                edits.push(Edit::Move {
                    item_id: i.item_id,
                    to: Slot::new(to.owner_id, i.location_id, i.flag_id),
                });
                open.push(i.item_id);
            }
        }
    }
    Ok(edits)
}

/// Plan and apply a bay move for `actor`; `None` when the move touches no
/// bay (the caller's own rules follow). The rosters of both pilots are
/// brought up to date first, so the range is where the ships are now.
pub fn move_item(
    reg: &mut Registry,
    actor: Option<i64>,
    dest: Dest,
    item_id: i64,
    quantity: Option<i64>,
) -> Option<Result<(), ServiceError>> {
    let actor = actor?;
    if !touches_bay(reg, dest, item_id) {
        return None;
    }
    let c = reg.character(actor)?.clone();
    reg.advance_character(actor);
    if let Dest::Hold { ship_id, .. } = dest
        && let Some(owner) = reg.item(ship_id).map(|s| s.owner_id)
    {
        reg.advance_character(owner);
    }
    let planned = plan(reg, &c, dest, item_id, quantity)?;
    Some(match planned {
        Err(e) => Err(e.to_error()),
        Ok(edits) if edits.is_empty() => Ok(()),
        Ok(edits) => reg.apply_items(&edits).map(|_| ()).map_err(|e| {
            crate::outln!("[inv] bay plan refused: {e}");
            notify("The server could not move that item.")
        }),
    })
}

#[cfg(test)]
mod tests;
