//! **Jump clones installed in a ship** (GAPS-2, X-GAPS item 5; lane note
//! `docs/lanes/gaps-2.md`): a pilot with room in their hull offers another
//! pilot a place to keep a clone, and the other accepts.
//!
//! The client (`clonejumpsvc.py`) [V]:
//!
//! - `OfferShipCloneInstallation(charID)` (`:129`, on the pilot of the ship;
//!   a `UserError` closes the progress window as "install aborted",
//!   `:130-137`), answer dropped;
//! - the target is told by `OnShipJumpCloneInstallationOffered((offeringCharID,
//!   targetCharID, shipID, _))` -- **one argument, a 4-tuple** (`:355-356`:
//!   `def OnShipJumpCloneInstallationOffered(self, args)` unpacks
//!   `args[0..3]`) -- and answers a yes/no dialog (`JumpCloneInstallationOffered`,
//!   showing `GetPriceForClone()` as the cost, `:365-366`) with
//!   `AcceptShipCloneInstallation()` (`:369`) or `CancelShipCloneInstallation()`
//!   (`:372`);
//! - the offerer, waiting in a progress window, can abort with
//!   `CancelShipCloneInstallation()` (`:216`); the other side is told by
//!   `OnShipJumpCloneInstallationCanceled(args)` (`:391-404`), whose handler
//!   itself calls `CancelShipCloneInstallation()` back -- so a cancel with
//!   nothing pending answers quietly and tells nobody, or two clients
//!   would answer each other for ever;
//! - on success both are told `OnShipJumpCloneInstallationDone((offerer,
//!   target, shipID, _))` (`:379-387`: it closes the wait and scatters
//!   `OnShipJumpCloneUpdate`), and `OnShipJumpCloneCacheInvalidated(shipID,
//!   charID)` (`:343-346`) makes a client holding that ship drop its cached
//!   list;
//! - the list is `GetShipCloneState()` (`:105`): a rowset the Ship
//!   Configuration window reads by column (`shipconfig.ShowCloneFacility:
//!   143-147`: `ownerID`, `jumpCloneID`) and `DestroyInstalledClone` indexes
//!   by `jumpCloneID` and reads `ownerID` and `locationID` (`:169-176`);
//!   `DestroyInstalledClone(cloneID)` (`:188`) removes one.
//!
//! **What this server does** [A], the client names none of it:
//!
//! - **Room**: the hull's `maxJumpClones` (attribute 979 in the bundle's
//!   typedata, `dogma` sheet when there is one) is the number of clones the
//!   ship holds; 0 means no bay and the offer is refused. A pilot has at
//!   most one clone in a ship.
//! - **Who**: the offerer flies the ship, the target is another character in
//!   the same solar system (or docked in the same station) as the offerer.
//!   One offer at a time per pilot, one waiting per target.
//! - **Cost**: nothing. `GetPriceForClone` answers 0.0 (it did before) and
//!   the dialog shows it.
//! - **Ids**: ship clones count up from [`FIRST_ID`], a range apart from the
//!   station clones' (`crate::clones`, from 1), because both end in the same
//!   `jumpCloneID` column and the client keys its caches by it.
//! - **A hull that is gone** takes its clones: rows whose ship item no
//!   longer exists are dropped when read.
//! - **Not done**: jumping *to* a ship clone, and the clone going with a ship
//!   that is sold; the lane note says so.
//!
//! Clones are stored as delta kind [`KIND`]: `clone_id -> owner ship_id
//! installed_at`. A pending offer is in memory ([`SLOT`]): it is a question
//! two clients are looking at, and a restart ends both.

use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue, PyValue};
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "shipclone";

/// `maxJumpClones` (`dogmaAttributes`, bundle typedata) [V].
pub const ATTR_MAX_JUMP_CLONES: i64 = 979;

/// The first ship clone id. **[A]** (see the module docs).
pub const FIRST_ID: i64 = 1_000_000_000;

/// The world slot. **Coordinator-only** registration (`world_ext::SLOTS`).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "ship_clone",
    make: |_| Box::new(Offers::default()),
};

/// An offer waiting for its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    pub offerer: i64,
    pub target: i64,
    pub ship_id: i64,
}

/// Every pending offer, by offerer.
#[derive(Debug, Default)]
pub struct Offers {
    by_offerer: BTreeMap<i64, Offer>,
}

/// One installed ship clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipClone {
    pub clone_id: i64,
    pub owner_id: i64,
    pub ship_id: i64,
}

fn refuse(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

fn key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

/// Every installed ship clone, ascending by id; rows of a ship that no
/// longer exists are dropped here (see the module docs).
pub fn all(reg: &mut Registry) -> Vec<ShipClone> {
    let rows: Vec<ShipClone> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            Some(ShipClone {
                clone_id: k.first()?.parse().ok()?,
                owner_id: v.first()?.parse().ok()?,
                ship_id: v.get(1)?.parse().ok()?,
            })
        })
        .collect();
    let (live, dead): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|c| reg.item(c.ship_id).is_some());
    for c in dead {
        reg.deltas_mut().del(KIND, &key(c.clone_id));
    }
    live
}

/// The clones in one ship.
pub fn in_ship(reg: &mut Registry, ship_id: i64) -> Vec<ShipClone> {
    all(reg)
        .into_iter()
        .filter(|c| c.ship_id == ship_id)
        .collect()
}

/// How many clones a hull holds: its `maxJumpClones`.
pub fn capacity(reg: &Registry, ship_id: i64) -> i64 {
    if let Some(v) =
        crate::dogma::sheet(reg, ship_id).and_then(|s| s.ship_opt(ATTR_MAX_JUMP_CLONES))
    {
        return v.round() as i64;
    }
    reg.item(ship_id)
        .and_then(|i| crate::typedata::attribute_in(reg.content(), i.type_id, ATTR_MAX_JUMP_CLONES))
        .map_or(0, |v| v.round() as i64)
}

/// Where a character is for "near each other": the solar system when in
/// space, else the station.
fn place(reg: &Registry, ch: i64) -> Option<(bool, i64)> {
    if let Some(p) = reg.roster().get(ch) {
        return Some((true, p.solar_system_id));
    }
    reg.character(ch)
        .and_then(|c| c.station_id)
        .map(|s| (false, s))
}

/// `GetShipCloneState()`: the clones in the caller's active ship, as the
/// rowset the client reads.
pub fn state_value(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    let ship = actor
        .and_then(|a| reg.character(a))
        .map(|c| (c.ship_id, c.ship_type_id));
    let clones = ship.map_or_else(Vec::new, |(id, _)| in_ship(reg, id));
    let columns = [
        ("jumpCloneID", DbType::I4),
        ("ownerID", DbType::I4),
        ("locationID", DbType::I8),
        ("typeID", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the ship-clone columns are valid");
    let mut rs = eve_rowset::CRowset::new(descriptor);
    let ship_type = ship.map_or(0, |(_, t)| t);
    for c in clones {
        rs.push_values(vec![
            DbValue::Int(c.clone_id),
            DbValue::Int(c.owner_id),
            DbValue::Int(c.ship_id),
            DbValue::Int(ship_type),
        ])
        .expect("the values match the descriptor");
    }
    rs.to_pyvalue()
}

/// `OfferShipCloneInstallation(charID)`.
pub fn offer(reg: &mut Registry, actor: Option<i64>, target: i64) -> Result<(), ServiceError> {
    let Some(me) = actor else {
        return Err(refuse("No character is selected."));
    };
    let Some(ship_id) = reg.character(me).map(|c| c.ship_id) else {
        return Err(refuse("No character is selected."));
    };
    if target == me || reg.character(target).is_none() {
        return Err(refuse("That pilot cannot be offered a clone."));
    }
    if reg.item(ship_id).is_none() {
        return Err(refuse("You have no ship."));
    }
    let here = place(reg, me);
    if here.is_none() || here != place(reg, target) {
        return Err(refuse("That pilot is not here."));
    }
    let held = in_ship(reg, ship_id);
    if (held.len() as i64) >= capacity(reg, ship_id) {
        return Err(refuse("This ship has no free clone bay."));
    }
    if held.iter().any(|c| c.owner_id == target) {
        return Err(refuse("That pilot already has a clone in this ship."));
    }
    let o = reg.ext::<Offers>();
    if o.by_offerer.contains_key(&me) || o.by_offerer.values().any(|x| x.target == target) {
        return Err(refuse("An offer is already waiting."));
    }
    reg.ext_mut::<Offers>().by_offerer.insert(
        me,
        Offer {
            offerer: me,
            target,
            ship_id,
        },
    );
    reg.push(OfferPush::Offered {
        offer: Offer {
            offerer: me,
            target,
            ship_id,
        },
    });
    Ok(())
}

/// `AcceptShipCloneInstallation()`: the target's yes.
pub fn accept(reg: &mut Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    let Some(me) = actor else {
        return Err(refuse("No character is selected."));
    };
    let Some(o) = reg
        .ext::<Offers>()
        .by_offerer
        .values()
        .find(|o| o.target == me)
        .copied()
    else {
        return Err(refuse("There is no offer to accept."));
    };
    reg.ext_mut::<Offers>().by_offerer.remove(&o.offerer);
    // The world may have moved since the offer.
    let still_flying = reg
        .character(o.offerer)
        .is_some_and(|c| c.ship_id == o.ship_id);
    let near = place(reg, o.offerer).is_some() && place(reg, o.offerer) == place(reg, me);
    let held = in_ship(reg, o.ship_id);
    if !still_flying
        || !near
        || (held.len() as i64) >= capacity(reg, o.ship_id)
        || held.iter().any(|c| c.owner_id == me)
    {
        reg.push(OfferPush::Canceled {
            to: vec![o.offerer, o.target],
        });
        return Err(refuse("The installation cannot be done now."));
    }
    let id = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .max()
        .map_or(FIRST_ID, |m| m.max(FIRST_ID - 1) + 1);
    reg.deltas_mut().put(
        KIND,
        key(id),
        vec![
            me.to_string(),
            o.ship_id.to_string(),
            crate::pyutil::filetime_now().to_string(),
        ],
    );
    reg.push(OfferPush::Done { offer: o });
    reg.push(OfferPush::CacheInvalidated {
        to: vec![o.offerer, o.target],
        ship_id: o.ship_id,
        char_id: me,
    });
    Ok(())
}

/// `CancelShipCloneInstallation()`: either side's no. Nothing pending is a
/// quiet success (see the module docs on the echo).
pub fn cancel(reg: &mut Registry, actor: Option<i64>) {
    let Some(me) = actor else { return };
    let found = reg
        .ext::<Offers>()
        .by_offerer
        .values()
        .find(|o| o.offerer == me || o.target == me)
        .copied();
    let Some(o) = found else { return };
    reg.ext_mut::<Offers>().by_offerer.remove(&o.offerer);
    let other = if o.offerer == me { o.target } else { o.offerer };
    reg.push(OfferPush::Canceled { to: vec![other] });
}

/// `DestroyInstalledClone(cloneID)` for a ship clone: its owner, or the
/// owner of the ship, may. `false` when `clone_id` is not a ship clone.
pub fn destroy(reg: &mut Registry, actor: Option<i64>, clone_id: i64) -> bool {
    let Some(c) = all(reg).into_iter().find(|c| c.clone_id == clone_id) else {
        return false;
    };
    let ship_owner = reg.item(c.ship_id).map(|i| i.owner_id);
    if actor == Some(c.owner_id) || (actor.is_some() && actor == ship_owner) {
        reg.deltas_mut().del(KIND, &key(clone_id));
        let mut to = vec![c.owner_id];
        if let Some(o) = ship_owner.filter(|o| *o != c.owner_id) {
            to.push(o);
        }
        reg.push(OfferPush::CacheInvalidated {
            to,
            ship_id: c.ship_id,
            char_id: c.owner_id,
        });
    }
    true
}

/// The four pushes of the installation, each addressed.
#[derive(Debug, Clone, PartialEq)]
pub enum OfferPush {
    /// `OnShipJumpCloneInstallationOffered((offerer, target, shipID, None))`
    /// to the target.
    Offered { offer: Offer },
    /// `OnShipJumpCloneInstallationDone((offerer, target, shipID, None))` to
    /// both.
    Done { offer: Offer },
    /// `OnShipJumpCloneInstallationCanceled(args)` to the other side.
    Canceled { to: Vec<i64> },
    /// `OnShipJumpCloneCacheInvalidated(shipID, charID)`.
    CacheInvalidated {
        to: Vec<i64>,
        ship_id: i64,
        char_id: i64,
    },
}

fn four(o: &Offer) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(o.offerer),
        PyValue::Int(o.target),
        PyValue::Int(o.ship_id),
        PyValue::None,
    ])
}

impl Push for OfferPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        match self {
            OfferPush::Offered { offer } => {
                ("OnShipJumpCloneInstallationOffered", vec![four(offer)])
            }
            OfferPush::Done { offer } => ("OnShipJumpCloneInstallationDone", vec![four(offer)]),
            OfferPush::Canceled { .. } => (
                "OnShipJumpCloneInstallationCanceled",
                vec![PyValue::Tuple(Vec::new())],
            ),
            OfferPush::CacheInvalidated {
                ship_id, char_id, ..
            } => (
                "OnShipJumpCloneCacheInvalidated",
                vec![PyValue::Int(*ship_id), PyValue::Int(*char_id)],
            ),
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(match self {
            OfferPush::Offered { offer } => vec![offer.target],
            OfferPush::Done { offer } => vec![offer.offerer, offer.target],
            OfferPush::Canceled { to } | OfferPush::CacheInvalidated { to, .. } => to.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::model::{Character, SandboxConfig};
    use crate::service::Request;
    use crate::typedata;
    use std::sync::Arc;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;
    const ALICE_SHIP: i64 = 100_000_001;
    /// Invented hulls: one with two clone bays, one with none.
    const BAY_HULL: i64 = 990_930_001;
    const PLAIN_HULL: i64 = 990_930_002;
    const ROWS: &str = "\
attribute\t979\t0.0\tmaxJumpClones
type\t990930001\t99930\t6\t1\t1\t50000.0\t400.0\t1000000.0\t100.0\t0\t\t979=2.0\tFixture Bay Hull
type\t990930002\t99931\t6\t1\t1\t50000.0\t400.0\t1000000.0\t100.0\t0\t\t\tFixture Plain Hull
";

    fn world(alice_hull: i64) -> Registry {
        let mut content = Content::fixture();
        let text = format!("{}{ROWS}", typedata::FIXTURE);
        content.insert(
            "typedata",
            typedata::Installed::build(typedata::Table::parse(&text).expect("rows parse")),
        );
        let pilot = |id: i64, name: &str, ship: i64, ty: i64| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: ty,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP, alice_hull),
                pilot(BOB, "Bob", ALICE_SHIP + 1, PLAIN_HULL),
                pilot(CAROL, "Carol", ALICE_SHIP + 2, PLAIN_HULL),
            ],
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(content));
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = alice_hull;
            i.group_id = 99_930;
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 200_000,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        reg
    }

    /// `(event, audience, arguments)` of every queued push.
    type Pushed = (&'static str, Option<Vec<i64>>, Vec<PyValue>);

    fn pushes(reg: &mut Registry) -> Vec<Pushed> {
        reg.take_pending()
            .iter()
            .map(|(_, p)| {
                let (n, a) = p.to_push();
                (n, p.audience(), a)
            })
            .collect()
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "jumpCloneSvc",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    /// The `(jumpCloneID, ownerID, locationID)` of every row `GetShipCloneState`
    /// answers, found by walking the value for its `DbRow`s.
    fn rows(reg: &mut Registry, who: i64) -> Vec<(i64, i64, i64)> {
        fn walk(v: &PyValue, out: &mut Vec<(i64, i64, i64)>) {
            match v {
                PyValue::DbRow(r) => {
                    if let [DbValue::Int(a), DbValue::Int(b), DbValue::Int(c), ..] = r.values() {
                        out.push((*a, *b, *c));
                    }
                }
                PyValue::List(xs) | PyValue::Tuple(xs) => xs.iter().for_each(|x| walk(x, out)),
                PyValue::ObjectEx {
                    header, list, dict, ..
                } => {
                    walk(header, out);
                    list.iter().for_each(|x| walk(x, out));
                    dict.iter().for_each(|(a, b)| {
                        walk(a, out);
                        walk(b, out);
                    });
                }
                PyValue::Instance { state, .. } => walk(state, out),
                _ => {}
            }
        }
        let v = call(reg, who, "GetShipCloneState", &[]).expect("answered");
        let mut out = Vec::new();
        walk(&v, &mut out);
        out
    }

    /// The whole dance: offer, the target is asked, accept, both are told,
    /// the ship's list shows the clone.
    #[test]
    fn a_pilot_installs_a_clone_in_a_ship() {
        let mut reg = world(BAY_HULL);
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .expect("offered");
        let p = pushes(&mut reg);
        let asked: Vec<_> = p
            .iter()
            .filter(|x| x.0 == "OnShipJumpCloneInstallationOffered")
            .collect();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].1, Some(vec![BOB]));
        assert_eq!(
            asked[0].2,
            vec![PyValue::Tuple(vec![
                PyValue::Int(ALICE),
                PyValue::Int(BOB),
                PyValue::Int(ALICE_SHIP),
                PyValue::None
            ])],
            "one argument: the 4-tuple the handler unpacks"
        );
        assert_eq!(all(&mut reg).len(), 0, "nothing is installed by an offer");
        call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).expect("accepted");
        let p = pushes(&mut reg);
        let names: Vec<&str> = p.iter().map(|x| x.0).collect();
        assert!(
            names.contains(&"OnShipJumpCloneInstallationDone"),
            "{names:?}"
        );
        let inv = p
            .iter()
            .find(|x| x.0 == "OnShipJumpCloneCacheInvalidated")
            .expect("cache push");
        assert_eq!(inv.2, vec![PyValue::Int(ALICE_SHIP), PyValue::Int(BOB)]);
        let clones = all(&mut reg);
        assert_eq!(clones.len(), 1);
        assert_eq!((clones[0].owner_id, clones[0].ship_id), (BOB, ALICE_SHIP));
        assert!(clones[0].clone_id >= FIRST_ID);
        assert_eq!(
            rows(&mut reg, ALICE),
            vec![(clones[0].clone_id, BOB, ALICE_SHIP)],
            "the Ship Configuration window's row"
        );
        assert!(rows(&mut reg, BOB).is_empty(), "Bob's own ship holds none");
        // Bob's own ship (a plain hull) has none.
        assert_eq!(in_ship(&mut reg, ALICE_SHIP + 1).len(), 0);
        // A second pilot fills the second bay; a third offer is full.
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(CAROL)],
        )
        .expect("offered");
        call(&mut reg, CAROL, "AcceptShipCloneInstallation", &[]).expect("accepted");
        assert_eq!(all(&mut reg).len(), 2);
        let e = call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        );
        assert!(e.is_err(), "two bays, two clones");
    }

    /// The refusals, each with nothing changed: no bay, self, nobody,
    /// someone who already has one here, a second offer waiting.
    #[test]
    fn the_offer_is_refused_when_it_cannot_be_kept() {
        let mut reg = world(PLAIN_HULL);
        let e = call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        );
        assert!(e.is_err(), "a hull with no bay");
        let mut reg = world(BAY_HULL);
        for target in [ALICE, 12_345] {
            let e = call(
                &mut reg,
                ALICE,
                "OfferShipCloneInstallation",
                &[PyValue::Int(target)],
            );
            assert!(e.is_err(), "{target}");
        }
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .expect("first");
        let e = call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(CAROL)],
        );
        assert!(e.is_err(), "one offer at a time");
        call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).expect("accepted");
        let e = call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        );
        assert!(e.is_err(), "one clone per pilot per ship");
        // Nothing to accept.
        assert!(call(&mut reg, CAROL, "AcceptShipCloneInstallation", &[]).is_err());
    }

    /// Cancelling tells the other side once, and a cancel with nothing
    /// pending tells nobody (the client's handler answers a cancel with one).
    #[test]
    fn a_cancel_is_heard_once() {
        let mut reg = world(BAY_HULL);
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .expect("offered");
        let _ = pushes(&mut reg);
        call(&mut reg, BOB, "CancelShipCloneInstallation", &[]).expect("cancelled");
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        assert_eq!(
            (p[0].0, p[0].1.clone()),
            ("OnShipJumpCloneInstallationCanceled", Some(vec![ALICE]))
        );
        // The echo from Alice's client changes nothing and says nothing.
        call(&mut reg, ALICE, "CancelShipCloneInstallation", &[]).expect("quiet");
        assert!(pushes(&mut reg).is_empty());
        assert!(
            call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).is_err(),
            "no offer left"
        );
    }

    /// The owner of a clone and the owner of the ship may destroy it; a
    /// stranger may not; a station clone id is not this module's.
    #[test]
    fn a_ship_clone_is_destroyed_by_its_owner_or_the_ships() {
        let mut reg = world(BAY_HULL);
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .unwrap();
        call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).unwrap();
        let id = all(&mut reg)[0].clone_id;
        let arg = [PyValue::Int(id)];
        call(&mut reg, CAROL, "DestroyInstalledClone", &arg).expect("answered");
        assert_eq!(all(&mut reg).len(), 1, "a stranger cannot");
        call(&mut reg, ALICE, "DestroyInstalledClone", &arg).expect("the ship's owner");
        assert_eq!(all(&mut reg).len(), 0);
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .unwrap();
        call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).unwrap();
        let id = all(&mut reg)[0].clone_id;
        call(&mut reg, BOB, "DestroyInstalledClone", &[PyValue::Int(id)]).expect("its owner");
        assert_eq!(all(&mut reg).len(), 0);
        assert!(!destroy(&mut reg, Some(BOB), 3), "not a ship clone");
    }

    /// A hull that no longer exists takes its clones when the list is read.
    #[test]
    fn a_lost_ship_takes_its_clones() {
        let mut reg = world(BAY_HULL);
        call(
            &mut reg,
            ALICE,
            "OfferShipCloneInstallation",
            &[PyValue::Int(BOB)],
        )
        .unwrap();
        call(&mut reg, BOB, "AcceptShipCloneInstallation", &[]).unwrap();
        reg.apply_items(&[crate::item::Edit::Destroy {
            item_id: ALICE_SHIP,
        }])
        .expect("destroyed");
        assert!(all(&mut reg).is_empty());
        assert!(reg.deltas().of(KIND).is_empty(), "and the rows go");
    }
}
