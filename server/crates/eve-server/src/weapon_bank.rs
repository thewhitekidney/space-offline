//! **Weapon banks** (K-OVERHEAT): weapons grouped so they fire as one. Lane
//! note `docs/lanes/k-overheat.md`; marks as `docs/content/README.md` §2.
//!
//! # What the client says [V]
//!
//! From `eve/client/script/dogma/clientDogmaLocation.pyc`
//! (`LinkWeapons`, `UngroupModule`, `LinkAllWeapons`, `UnlinkAllWeapons`,
//! `DestroyWeaponBank`, `OnWeaponBanksChanged`, `UnloadModuleToContainer`),
//! `eve/common/script/dogma/baseDogmaLocation.pyc` (`SetWeaponBanks`,
//! `GetMasterModuleID`) and the HUD (`shipHud/slotsContainer`,
//! `shipModuleButton/shipmodulebutton`):
//!
//! - **The shape**: a ship's banks are a dict `{masterID: [slaveID, ...]}`
//!   (`SetWeaponBanks` iterates `data.iteritems()`), `None` for no banks. It
//!   reaches the client three ways: `ship.Board`'s fourth-but-one slot `wbData`
//!   (`shipState`), the **return value** of each call that builds a bank, and
//!   the notification `OnWeaponBanksChanged(shipID, info)` (the same dict).
//! - **The calls** (`remoteDogmaLM`, the bound `dogmaIM`): `LinkWeapons(shipID,
//!   masterID, fromID)` (a module not in a bank joins the bank of
//!   `toID`), `MergeModuleGroups(shipID, masterID, slaveID)` (a whole bank joins
//!   another), `PeelAndLink(shipID, masterID, slaveID)` (one module of a bank
//!   leaves it for another; chosen when the drag is not a merge),
//!   `LinkAllWeapons(shipID)`, each answering the whole dict;
//!   `UnlinkModule(shipID, masterID)` answering the one slave that was peeled
//!   off (the client removes **that id** from its own copy), `UnlinkAllModules(
//!   shipID)` (the client sets `{}` itself) and `DestroyWeaponBank(shipID,
//!   masterID)`.
//! - **What may be grouped**: the weapon groups of `dgmGroupableGroupIDs`
//!   (`dogma/const.pyc`, ids from `inventorycommon/const.pyc`), fitted in a
//!   high slot, online (`CantLinkModuleNotOnline`, raised by the client too).
//!   The keys the client has for what only a server can see:
//!   `CantLinkModuleActive`, `CantLinkModuleDamaged`, `CantLinkModuleLoading`
//!   and `CantLinkAmmoInWeapon` (their texts: "cannot group active /
//!   damaged modules", "can't group weapons while they are being loaded",
//!   "cannot group modules loaded with charges").
//! - **A bank is one button**: the master's. `ModuleButton` shows the count
//!   (`CheckMasterSlave`), and `ProcessShipEffect` routes any bank member's
//!   effect event to the master's button
//!   (`dogmaLocation.IsInWeaponBank(effectState.itemID)`), so every member may
//!   be told its own start and stop. The client activates **only the master**;
//!   loading a charge or unloading names only the master too
//!   (`LoadChargesToModule`: `if IsModuleSlave: moduleID = master`).
//!
//! # What is ours, so [A]
//!
//! - **[A] firing as one** is every member running its own cycle from the
//!   master's click: `modules::activate` for the master and each member, one
//!   cycle each, one `OnGodmaShipEffect` and `OnSpecialFX` each. The damage,
//!   capacitor and ammunition are per module, so a bank deals what the same
//!   modules deal ungrouped; what grouping adds is one click, one cycle clock
//!   (they start together) and one button. Falsified by a live bank dealing
//!   the master's volley times the count from one capacitor draw.
//! - **[A] the same charge rule**: members must hold the **same charge type**
//!   (or all none), else `CantLinkAmmoInWeapon`. The key's text reads as
//!   "no charges at all"; that would forbid grouping loaded guns, which the
//!   HUD's own "group all" over loaded turrets contradicts. Falsified by the
//!   live refusal of two guns with the same ammunition.
//! - **Banks are persisted** per ship (delta kind [`KIND`]) and restored by
//!   [`restore`] on a load; they dissolve when a member leaves the ship's
//!   fitting. The client reads them in `shipState.wbData` at boarding.
//! - **[A] the master is the lowest slot** of the group (`LinkAllWeapons`);
//!   `UnlinkModule` peels the lowest-slot member.

use crate::modules::user_error;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// `dgmGroupableGroupIDs` [V]: `inventorycommon.const` `groupEnergyWeapon`,
/// `groupProjectileWeapon`, `groupHybridWeapon` and the missile launchers,
/// the structure launchers last.
pub const GROUPABLE_GROUPS: [i64; 19] = [
    53, 55, 74, 56, 511, 524, 1674, 1673, 506, 512, 510, 771, 507, 508, 509, 1245, 1328, 1562, 1327,
];

/// `flagHiSlot0..7` [V]: only the high rack's weapons are grouped.
const HIGH_SLOTS: std::ops::RangeInclusive<i64> = 27..=34;

/// One ship's banks, `{master: [slaves]}`, each list in slot order.
pub type Banks = BTreeMap<i64, Vec<i64>>;

// ---------------------------------------------------------------------------
// The banks themselves: pure
// ---------------------------------------------------------------------------

/// The master of the bank `module` is in, `module` itself if it is one.
pub fn master_of(b: &Banks, module: i64) -> Option<i64> {
    if b.contains_key(&module) {
        return Some(module);
    }
    b.iter()
        .find(|(_, slaves)| slaves.contains(&module))
        .map(|(m, _)| *m)
}

/// Every module in a bank, master and slaves.
pub fn members(b: &Banks, module: i64) -> Vec<i64> {
    let Some(m) = master_of(b, module) else {
        return vec![module];
    };
    let mut v = vec![m];
    v.extend(b.get(&m).into_iter().flatten().copied());
    v
}

fn sorted(mut v: Vec<i64>, order: &dyn Fn(i64) -> i64) -> Vec<i64> {
    v.sort_by_key(|m| (order(*m), *m));
    v.dedup();
    v
}

/// `LinkWeapons`: `from`, in no bank, joins the bank of `to`.
pub fn link(b: &mut Banks, to: i64, from: i64, order: &dyn Fn(i64) -> i64) -> bool {
    let master = master_of(b, to).unwrap_or(to);
    if from == master || master_of(b, from).is_some() {
        return false;
    }
    let mut slaves = b.remove(&master).unwrap_or_default();
    slaves.push(from);
    b.insert(master, sorted(slaves, order));
    true
}

/// `MergeModuleGroups`: the whole bank of `source` joins the bank of `target`.
pub fn merge(b: &mut Banks, target: i64, source: i64, order: &dyn Fn(i64) -> i64) -> bool {
    let t = master_of(b, target).unwrap_or(target);
    let s = master_of(b, source).unwrap_or(source);
    if t == s {
        return false;
    }
    let mut slaves = b.remove(&t).unwrap_or_default();
    slaves.push(s);
    slaves.extend(b.remove(&s).unwrap_or_default());
    b.insert(t, sorted(slaves, order));
    true
}

/// The member `PeelAndLink` / `UnlinkModule` takes out of the bank of
/// `module`: the lowest slot of its slaves.
pub fn peelable(b: &Banks, module: i64) -> Option<i64> {
    let m = master_of(b, module)?;
    b.get(&m)?.first().copied()
}

/// Take `peeled` out of the bank of `from`; a bank left with no slave is gone.
fn take_out(b: &mut Banks, from: i64, peeled: i64) {
    let Some(m) = master_of(b, from) else { return };
    if let Some(slaves) = b.get_mut(&m) {
        slaves.retain(|s| *s != peeled);
        if slaves.is_empty() {
            b.remove(&m);
        }
    }
}

/// `PeelAndLink`: one module of the `source` bank leaves it for the `target`
/// bank. The module moved, or `None`.
pub fn peel_and_link(
    b: &mut Banks,
    target: i64,
    source: i64,
    order: &dyn Fn(i64) -> i64,
) -> Option<i64> {
    let t = master_of(b, target).unwrap_or(target);
    let s = master_of(b, source)?;
    if t == s {
        return None;
    }
    let peeled = peelable(b, s)?;
    take_out(b, s, peeled);
    let mut slaves = b.remove(&t).unwrap_or_default();
    slaves.push(peeled);
    b.insert(t, sorted(slaves, order));
    Some(peeled)
}

/// `UnlinkModule`: the lowest-slot slave leaves the bank; its id.
pub fn unlink(b: &mut Banks, module: i64) -> Option<i64> {
    let peeled = peelable(b, module)?;
    take_out(b, module, peeled);
    Some(peeled)
}

/// `DestroyWeaponBank`: the bank of `module` is gone.
pub fn destroy(b: &mut Banks, module: i64) -> bool {
    master_of(b, module).is_some_and(|m| b.remove(&m).is_some())
}

/// `LinkAllWeapons`: every group of two or more modules with one **key** (type
/// and charge) becomes a bank. The master is the group's existing master, or
/// else its lowest slot.
pub fn link_all(
    b: &Banks,
    groups: &BTreeMap<(i64, i64), Vec<i64>>,
    order: &dyn Fn(i64) -> i64,
) -> Banks {
    let mut out = Banks::new();
    for modules in groups.values() {
        if modules.len() < 2 {
            continue;
        }
        let modules = sorted(modules.clone(), order);
        let master = modules
            .iter()
            .copied()
            .find(|m| b.contains_key(m))
            .unwrap_or(modules[0]);
        let slaves: Vec<i64> = modules.into_iter().filter(|m| *m != master).collect();
        out.insert(master, slaves);
    }
    out
}

/// The dict the client reads: `{masterID: [slaveID, ...]}`, entries as
/// `(value, key)` (marshal dicts are inverted here).
pub fn bank_value(b: &Banks) -> PyValue {
    PyValue::Dict(
        b.iter()
            .map(|(m, slaves)| {
                (
                    PyValue::List(slaves.iter().map(|s| PyValue::Int(*s)).collect()),
                    PyValue::Int(*m),
                )
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The slot
// ---------------------------------------------------------------------------

/// Every ship's banks; mirrored to the world file by `set_banks`.
#[derive(Debug, Default)]
pub struct WeaponBanks {
    ships: BTreeMap<i64, Banks>,
}

impl WeaponBanks {
    /// A ship's banks (for the tests).
    pub fn of(&self, ship_id: i64) -> Banks {
        self.ships.get(&ship_id).cloned().unwrap_or_default()
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "weapon_bank",
    make: |_| Box::new(WeaponBanks::default()) as Boxed,
};

/// `OnWeaponBanksChanged(shipID, info)` [V] (`cdl:1067`): the server took a
/// bank apart (a member left the fitting) and says what is left.
#[derive(Debug, Clone, PartialEq)]
pub struct BanksChanged {
    pub to: i64,
    pub ship_id: i64,
    pub banks: Banks,
}

impl Push for BanksChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnWeaponBanksChanged",
            vec![PyValue::Int(self.ship_id), bank_value(&self.banks)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The world half
// ---------------------------------------------------------------------------

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// The actor's ship: banks are the flown ship's.
fn ship_of(w: &Registry, actor: Option<i64>) -> Result<i64, ServiceError> {
    actor
        .and_then(|a| w.character(a))
        .map(|c| c.ship_id)
        .ok_or_else(|| notify("No character is selected."))
}

/// Slot order of the ship's modules, owned so the world can be changed after.
fn order_of(w: &Registry, ship: i64) -> impl Fn(i64) -> i64 + use<> {
    let flags: BTreeMap<i64, i64> = w
        .store()
        .contents_of(ship)
        .map(|i| (i.item_id, i.flag_id))
        .collect();
    move |m| flags.get(&m).copied().unwrap_or(i64::MAX)
}

fn banks_of(w: &Registry, ship: i64) -> Banks {
    w.ext::<WeaponBanks>().of(ship)
}

fn set_banks(w: &mut Registry, ship: i64, b: Banks) {
    let key = vec![ship.to_string()];
    if b.is_empty() {
        w.ext_mut::<WeaponBanks>().ships.remove(&ship);
        if w.deltas().of(KIND).contains_key(&key) {
            w.deltas_mut().del(KIND, &key);
        }
    } else {
        let row = banks_row(&b);
        w.ext_mut::<WeaponBanks>().ships.insert(ship, b);
        if w.deltas().of(KIND).get(&key) != Some(&row) {
            w.deltas_mut().put(KIND, key, row);
        }
    }
}

/// The delta kind this module owns (`delta_kinds::KINDS`): `ship_id ->
/// master:slave,slave ...`, one field per bank, only for a ship with a bank.
pub const KIND: &str = "wbank";

fn banks_row(b: &Banks) -> Vec<String> {
    b.iter()
        .map(|(m, slaves)| {
            let list: Vec<String> = slaves.iter().map(i64::to_string).collect();
            format!("{m}:{}", list.join(","))
        })
        .collect()
}

fn banks_from_row(row: &[String]) -> Banks {
    let mut out = Banks::new();
    for f in row {
        let Some((m, rest)) = f.split_once(':') else {
            continue;
        };
        let Ok(m) = m.parse::<i64>() else { continue };
        let slaves: Vec<i64> = rest.split(',').filter_map(|s| s.parse().ok()).collect();
        if !slaves.is_empty() {
            out.insert(m, slaves);
        }
    }
    out
}

/// **A load's step**: the stored banks come back into the slot, each trimmed to
/// the members still fitted in the ship's high rack (an unfit, a repackaged or
/// destroyed or traded ship leaves nothing); a row that came back changed or
/// empty is rewritten or deleted. The client reads them in `shipState.wbData`
/// at boarding ([`ship_state`]), so nothing is pushed here.
pub fn restore(w: &mut Registry) {
    let rows: Vec<(i64, Banks)> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse::<i64>().ok()?, banks_from_row(v))))
        .collect();
    for (ship, b) in rows {
        let ship_ok = w.item(ship).is_some();
        let next = if ship_ok {
            prune(w, ship, &b)
        } else {
            Banks::new()
        };
        set_banks(w, ship, next);
    }
}

/// `b` without the members that are not in `ship`'s high rack; a bank left
/// with no slave, or no master, is gone.
fn prune(w: &Registry, ship: i64, b: &Banks) -> Banks {
    let valid = |m: i64| {
        w.item(m)
            .is_some_and(|i| i.location_id == ship && HIGH_SLOTS.contains(&i.flag_id))
    };
    let mut next = Banks::new();
    for (master, slaves) in b {
        if !valid(*master) {
            continue;
        }
        let kept: Vec<i64> = slaves.iter().copied().filter(|s| valid(*s)).collect();
        if !kept.is_empty() {
            next.insert(*master, kept);
        }
    }
    next
}

/// What the server can see that the client cannot, of one pair about to be
/// grouped (each module on its own is checked against `ship`).
fn check_module(w: &Registry, ship: i64, module: i64, now: i64) -> Result<(), ServiceError> {
    let Some(i) = w.item(module) else {
        return Err(notify("This server does not know that module."));
    };
    if i.location_id != ship || !HIGH_SLOTS.contains(&i.flag_id) || !i.singleton {
        return Err(notify("That module is not a weapon fitted to your ship."));
    }
    if !GROUPABLE_GROUPS.contains(&i.group_id) {
        return Err(notify("Only weapons can be grouped."));
    }
    if !w.modules().is_online(module) {
        return Err(user_error("CantLinkModuleNotOnline", vec![]));
    }
    if w.modules().running(module).is_some() {
        return Err(user_error("CantLinkModuleActive", vec![]));
    }
    if crate::overheat::module_damage(w, module) > 0.0 {
        return Err(user_error("CantLinkModuleDamaged", vec![]));
    }
    if w.modules().reloading_until(module, now).is_some() {
        return Err(user_error("CantLinkModuleLoading", vec![]));
    }
    Ok(())
}

fn charge_of(w: &Registry, ship: i64, module: i64) -> i64 {
    w.item(module)
        .and_then(|i| crate::modules::loaded(w.store(), ship, i.flag_id))
        .map_or(0, |c| c.type_id)
}

fn check_pair(w: &Registry, ship: i64, a: i64, b: i64, now: i64) -> Result<(), ServiceError> {
    check_module(w, ship, a, now)?;
    check_module(w, ship, b, now)?;
    if w.item(a).map(|i| i.type_id) != w.item(b).map(|i| i.type_id) {
        return Err(notify("Only modules of the same type can be grouped."));
    }
    if charge_of(w, ship, a) != charge_of(w, ship, b) {
        return Err(user_error("CantLinkAmmoInWeapon", vec![]));
    }
    Ok(())
}

/// `dogmaIM.LinkWeapons(shipID, masterID, fromID)`: answers every bank.
pub fn link_weapons(
    w: &mut Registry,
    actor: Option<i64>,
    to: i64,
    from: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let mut b = banks_of(w, ship);
    let master = master_of(&b, to).unwrap_or(to);
    check_pair(w, ship, master, from, now)?;
    let order = order_of(w, ship);
    if link(&mut b, to, from, &order) {
        set_banks(w, ship, b.clone());
    }
    Ok(bank_value(&b))
}

/// `dogmaIM.MergeModuleGroups(shipID, masterID, slaveID)`.
pub fn merge_groups(
    w: &mut Registry,
    actor: Option<i64>,
    target: i64,
    source: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let mut b = banks_of(w, ship);
    let t = master_of(&b, target).unwrap_or(target);
    let s = master_of(&b, source).unwrap_or(source);
    for m in members(&b, t).into_iter().chain(members(&b, s)) {
        check_module(w, ship, m, now)?;
    }
    check_pair(w, ship, t, s, now)?;
    let order = order_of(w, ship);
    if merge(&mut b, target, source, &order) {
        set_banks(w, ship, b.clone());
    }
    Ok(bank_value(&b))
}

/// `dogmaIM.PeelAndLink(shipID, masterID, slaveID)`.
pub fn peel_link(
    w: &mut Registry,
    actor: Option<i64>,
    target: i64,
    source: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let mut b = banks_of(w, ship);
    let t = master_of(&b, target).unwrap_or(target);
    if let Some(p) = master_of(&b, source).and_then(|s| peelable(&b, s)) {
        check_pair(w, ship, t, p, now)?;
    }
    let order = order_of(w, ship);
    if peel_and_link(&mut b, target, source, &order).is_some() {
        set_banks(w, ship, b.clone());
    }
    Ok(bank_value(&b))
}

/// `dogmaIM.UnlinkModule(shipID, masterID)`: the slave peeled off.
pub fn unlink_module(
    w: &mut Registry,
    actor: Option<i64>,
    module: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let mut b = banks_of(w, ship);
    let Some(peeled) = unlink(&mut b, module) else {
        return Err(notify("That weapon bank no longer exists."));
    };
    set_banks(w, ship, b);
    Ok(PyValue::Int(peeled))
}

/// `dogmaIM.LinkAllWeapons(shipID)`: every online, idle, undamaged weapon of
/// the high rack, grouped by type and charge.
pub fn link_all_weapons(
    w: &mut Registry,
    actor: Option<i64>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let b = banks_of(w, ship);
    let mut groups: BTreeMap<(i64, i64), Vec<i64>> = BTreeMap::new();
    for i in w.store().contents_of(ship) {
        if check_module(w, ship, i.item_id, now).is_ok() {
            groups
                .entry((i.type_id, charge_of(w, ship, i.item_id)))
                .or_default()
                .push(i.item_id);
        }
    }
    let order = order_of(w, ship);
    let next = link_all(&b, &groups, &order);
    set_banks(w, ship, next.clone());
    Ok(bank_value(&next))
}

/// `dogmaIM.UnlinkAllModules(shipID)`.
pub fn unlink_all(w: &mut Registry, actor: Option<i64>) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    set_banks(w, ship, Banks::new());
    Ok(bank_value(&Banks::new()))
}

/// `dogmaIM.DestroyWeaponBank(shipID, masterID)`. Returns `None`.
pub fn destroy_bank(
    w: &mut Registry,
    actor: Option<i64>,
    module: i64,
) -> Result<PyValue, ServiceError> {
    let ship = ship_of(w, actor)?;
    let mut b = banks_of(w, ship);
    if destroy(&mut b, module) {
        set_banks(w, ship, b);
    }
    Ok(PyValue::None)
}

/// The modules a call acting on `module` acts on: its bank (master first), or
/// the module alone.
pub fn bank_of(w: &Registry, actor: Option<i64>, module: i64) -> Vec<i64> {
    let Ok(ship) = ship_of(w, actor) else {
        return vec![module];
    };
    members(&banks_of(w, ship), module)
}

/// The master of the bank `module` is in on `ship`; `None` for a module in
/// no bank. What the combat stage groups a batch of cycles by.
pub fn master_in(w: &Registry, ship: i64, module: i64) -> Option<i64> {
    master_of(&banks_of(w, ship), module)
}

/// Group `master` with `slaves` on `ship`, with none of `link_weapons`'s
/// checks (tests of the bank's consumers).
#[cfg(test)]
pub(crate) fn set_bank_for_test(w: &mut Registry, ship: i64, master: i64, slaves: &[i64]) {
    let mut b = banks_of(w, ship);
    b.insert(master, slaves.to_vec());
    set_banks(w, ship, b);
}

/// `Activate`, for a bank: the master and each member, one click. The
/// master's own refusal is the answer; a member that cannot start (no
/// rounds, say) does not stop the rest. A name of `online` is one module's.
pub fn activate(
    w: &mut Registry,
    actor: Option<i64>,
    module: i64,
    effect_name: &str,
    target: Option<i64>,
    repeat: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let reply = crate::modules::activate(w, actor, module, effect_name, target, repeat, now)?;
    // The master that did not start (no capacitor) starts nothing else.
    if effect_name != "online" && w.modules().running(module).is_some() {
        for m in bank_of(w, actor, module)
            .into_iter()
            .filter(|m| *m != module)
        {
            // **One button, one icon**: a slave runs quietly, its
            // `OnGodmaShipEffect` withheld (`Modules::set_quiet`); the
            // master's is the bank's.
            w.modules_mut().set_quiet(m, true);
            let _ = crate::modules::activate(w, actor, m, effect_name, target, repeat, now);
            if w.modules().running(m).is_none() {
                w.modules_mut().set_quiet(m, false);
            }
        }
    }
    Ok(reply)
}

/// `Deactivate`, for a bank: every member stops at its cycle's end.
pub fn deactivate(
    w: &mut Registry,
    actor: Option<i64>,
    module: i64,
    effect_name: &str,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let reply = crate::modules::deactivate(w, actor, module, effect_name, now)?;
    if effect_name != "online" {
        for m in bank_of(w, actor, module)
            .into_iter()
            .filter(|m| *m != module)
        {
            let _ = crate::modules::deactivate(w, actor, m, effect_name, now);
        }
    }
    Ok(reply)
}

/// A module list, each bank master's bank added: what `LoadAmmo` and
/// `UnloadAmmo` act on when the client names only the master.
pub fn expand(w: &Registry, actor: Option<i64>, modules: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for m in modules {
        for x in bank_of(w, actor, *m) {
            if seen.insert(x) {
                out.push(x);
            }
        }
    }
    out
}

/// What `ship.Board` and `GetAllInfo` carry as `shipState`: the loaded
/// charges (`modules::ship_state`) with the ship's banks in `wbData`, `None`
/// for no bank.
pub fn ship_state(w: &Registry, ship_id: i64) -> PyValue {
    let state = crate::modules::ship_state(w.store(), ship_id);
    let b = banks_of(w, ship_id);
    match state {
        PyValue::Tuple(mut v) if v.len() == 4 && !b.is_empty() => {
            v[2] = bank_value(&b);
            PyValue::Tuple(v)
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// A quiet slave runs only while its master does: one whose master stopped
/// (out of rounds or capacitor, a lost lock) or that left the bank (an unlink,
/// a dissolved bank) stops too, silently, as its button is the master's.
fn quiet_slaves(w: &mut Registry, now: i64) {
    for m in w.modules().quiet() {
        let Some(ship) = w.modules().running(m).map(|r| r.ship_id) else {
            continue;
        };
        let b = banks_of(w, ship);
        let master_runs = master_of(&b, m)
            .filter(|master| *master != m)
            .is_some_and(|master| {
                w.modules()
                    .running(master)
                    .is_some_and(|r| r.stop_at.is_none())
            });
        if !master_runs {
            crate::modules::stop(w, m, now, None);
        }
    }
    w.modules_mut().forget_idle_quiet();
}

/// **The world clock's stage**: a bank whose member left the ship's high rack
/// (an unfit, a loss, a move to another slot rack) is dissolved, and the pilot
/// is told what is left.
pub fn tick(w: &mut Registry, _: Instant, now: i64) {
    quiet_slaves(w, now);
    let ships: Vec<i64> = w.ext::<WeaponBanks>().ships.keys().copied().collect();
    for ship in ships {
        let b = banks_of(w, ship);
        let next = prune(w, ship, &b);
        if next == b {
            continue;
        }
        set_banks(w, ship, next.clone());
        if let Some(pilot) = w.roster().by_ship(ship).map(|p| p.character_id) {
            w.schedule(
                Duration::ZERO,
                BanksChanged {
                    to: pilot,
                    ship_id: ship,
                    banks: next,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests;
