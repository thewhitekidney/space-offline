//! Saved ship fittings: `charFittingMgr` and `corpFittingMgr`.
//! Lane note `docs/lanes/fitting-market.md`.
//!
//! What the client does, read from its own code **[V]**
//! (`eve/client/script/environment/fittingSvc.py`, `fittingMgmtCont.py`):
//!
//! - `GetFittings(ownerID)` -> `{fittingID: KeyVal}` with `fittingID`,
//!   `ownerID`, `name`, `description`, `shipTypeID`, `fitData`, `savedDate`.
//!   The client keeps that very dict and mutates it, so it is a dict and not
//!   a rowset.
//! - `SaveFitting(ownerID, fitting)` -> the new fitting id (the client acts
//!   only on `fittingID > 0`); `UpdateFitting(ownerID, fitting,
//!   fittingID=id)` -> the id; `SaveManyFittings(ownerID, {tempID: fitting})`
//!   -> rows with `tempFittingID` and `realFittingID`; `DeleteFitting(owner,
//!   id)`; `DeleteManyFittings(owner, [ids])` -> the ids it deleted;
//!   `UpdateNameAndDescription(fittingID, ownerID, name, description)`.
//! - `fitData` is a list of `(typeID, flag, quantity)` tuples.
//! - **Corp fittings**: the client saves to the corporation only when
//!   `session.corprole & corpRoleFittingManager` (`fittingMgmtCont.Save`).
//!   The server checks the same bit, or director/CEO **[A]** (falsifier: a
//!   director refused by the real server). Any member may read them.
//! - **Alliance fittings** (GAP-FITSHARE): `allianceFittingMgr` answers the
//!   same calls for `ownerID == session.allianceid`
//!   (`fittingSvc.GetFittingMgr`). The save menu is offered to members of
//!   the executor corporation, and the tooltip also wants
//!   `corpRoleFittingManager` (`fittingMgmtCont.py:120-160`) **[V]**; the
//!   server asks for both, and the director/CEO allowance above applies
//!   (**[A]**, same falsifier). `maxAllianceFittings` is 600.
//! - **Pushes to the other members** (**[V]** `fittingSvc.py:1029-1068`):
//!   `OnFittingAdded(ownerID, id or [ids])`, `OnFittingDeleted(ownerID, id)`
//!   and `OnManyFittingsDeleted(ownerID, ids)` go to every online character
//!   whose own, corporation or alliance id is the owner. The saver's own
//!   cache already follows `SaveMany`, `Delete` and `DeleteMany`
//!   (`:338-343,420-425`), so those go to the others only; a single
//!   `SaveFitting` relies on the push and reaches the saver too.
//! - Limits `maxCharFittings` 500, `maxCorpFittings` 600, name 50,
//!   description 500 (`appConst`).
//!
//! Stored as delta kind [`KIND`]: `owner fittingID -> shipTypeID savedDate
//! name description fitData`, with `fitData` as `type:flag:qty` joined by
//! `;`. Ids are world-wide, one past the highest stored.

use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "fit";

/// `corpRoleFittingManager` **[V]** (`appConst`).
pub const CORP_ROLE_FITTING_MANAGER: i64 = 576_460_752_303_423_488;
/// `corpRoleDirector` **[V]**.
const CORP_ROLE_DIRECTOR: i64 = 1;
/// `maxCharFittings` **[V]**.
pub const MAX_CHAR_FITTINGS: usize = 500;
/// `maxCorpFittings` **[V]**.
pub const MAX_CORP_FITTINGS: usize = 600;
/// `maxAllianceFittings` **[V]**.
pub const MAX_ALLIANCE_FITTINGS: usize = 600;
/// `maxLengthFittingName` **[V]**.
pub const MAX_NAME: usize = 50;
/// `maxLengthFittingDescription` **[V]**.
pub const MAX_DESCRIPTION: usize = 500;
/// **[A]** the most lines one fitting may carry; the client's own limit is
/// the ship's slots plus its bays, far below this.
pub const MAX_LINES: usize = 400;

/// What a service this file answers.
pub const SERVICES: &[&str] = &["charFittingMgr", "corpFittingMgr", "allianceFittingMgr"];

/// One stored fitting.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitting {
    pub id: i64,
    pub owner: i64,
    pub ship_type: i64,
    pub saved: i64,
    pub name: String,
    pub description: String,
    pub lines: Vec<(i64, i64, i64)>,
}

fn key(owner: i64, id: i64) -> Vec<String> {
    vec![owner.to_string(), id.to_string()]
}

fn encode_lines(lines: &[(i64, i64, i64)]) -> String {
    lines
        .iter()
        .map(|(t, f, q)| format!("{t}:{f}:{q}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn decode_lines(s: &str) -> Vec<(i64, i64, i64)> {
    s.split(';')
        .filter_map(|p| {
            let mut it = p.split(':').map(|n| n.parse::<i64>().ok());
            Some((it.next()??, it.next()??, it.next()??))
        })
        .collect()
}

fn row(f: &Fitting) -> Vec<String> {
    vec![
        f.ship_type.to_string(),
        f.saved.to_string(),
        f.name.clone(),
        f.description.clone(),
        encode_lines(&f.lines),
    ]
}

/// Every fitting of `owner`, by id.
pub fn fittings(reg: &Registry, owner: i64) -> Vec<Fitting> {
    let want = owner.to_string();
    let mut out: Vec<Fitting> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let [o, id] = k.as_slice() else {
                return None;
            };
            if *o != want {
                return None;
            }
            let [ship, saved, name, desc, lines] = v.as_slice() else {
                return None;
            };
            Some(Fitting {
                id: id.parse().ok()?,
                owner,
                ship_type: ship.parse().ok()?,
                saved: saved.parse().ok()?,
                name: name.clone(),
                description: desc.clone(),
                lines: decode_lines(lines),
            })
        })
        .collect();
    out.sort_by_key(|f| f.id);
    out
}

fn next_id(reg: &Registry) -> i64 {
    reg.deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.get(1)?.parse::<i64>().ok())
        .max()
        .map_or(1, |m| m + 1)
}

fn value_of(f: &Fitting) -> PyValue {
    crate::pyutil::keyval(vec![
        ("fittingID", PyValue::Int(f.id)),
        ("ownerID", PyValue::Int(f.owner)),
        ("name", PyValue::Unicode(f.name.clone())),
        ("description", PyValue::Unicode(f.description.clone())),
        ("shipTypeID", PyValue::Int(f.ship_type)),
        (
            "fitData",
            PyValue::List(
                f.lines
                    .iter()
                    .map(|&(t, fl, q)| {
                        PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(fl), PyValue::Int(q)])
                    })
                    .collect(),
            ),
        ),
        ("savedDate", PyValue::Int(f.saved)),
    ])
}

/// `OnFittingAdded(ownerID, fittingID)`: tells the client to drop its cached
/// list for `owner` and read it again.
///
/// **This is why a saved fitting never showed.** `fittingSvc.PersistFitting`
/// puts a *new* fitting into its cache only on the update-by-name branch
/// (`if existingFitting:`); for a new one it relies on this notification
/// (`fittingSvc.OnFittingAdded`: `fitID not in cache` -> `del
/// self.fittings[ownerID]`, redraw, and the next `GetFittings` reads the
/// store) **[V]** from the decompiled client. Corporation fittings go to the
/// saver only **[A]**; falsifier: a second member who does not see a corp
/// fitting until they reopen the window.
#[derive(Debug, Clone, PartialEq)]
pub struct FittingAdded {
    pub owner: i64,
    /// One id goes over as an int, several as a list (`:1029-1043`).
    pub ids: Vec<i64>,
    /// Character ids, or `None` for the acting session alone.
    pub to: Option<Vec<i64>>,
}

impl crate::notify::Push for FittingAdded {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let ids = match self.ids.as_slice() {
            [one] => PyValue::Int(*one),
            many => PyValue::List(many.iter().map(|i| PyValue::Int(*i)).collect()),
        };
        ("OnFittingAdded", vec![PyValue::Int(self.owner), ids])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        self.to.clone()
    }
}

/// `OnFittingDeleted(ownerID, fittingID)` and `OnManyFittingsDeleted(ownerID,
/// ids)` to the other members **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct FittingsDeleted {
    pub owner: i64,
    pub ids: Vec<i64>,
    /// `true` for the `OnManyFittingsDeleted` form.
    pub many: bool,
    pub to: Vec<i64>,
}

impl crate::notify::Push for FittingsDeleted {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        if self.many {
            (
                "OnManyFittingsDeleted",
                vec![
                    PyValue::Int(self.owner),
                    PyValue::List(self.ids.iter().map(|i| PyValue::Int(*i)).collect()),
                ],
            )
        } else {
            (
                "OnFittingDeleted",
                vec![PyValue::Int(self.owner), PyValue::Int(self.ids[0])],
            )
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `{fittingID: KeyVal}` for `GetFittings`.
pub fn fittings_value(reg: &Registry, owner: i64) -> PyValue {
    PyValue::Dict(
        fittings(reg, owner)
            .iter()
            .map(|f| (value_of(f), PyValue::Int(f.id)))
            .collect(),
    )
}

// ---- who may do what ------------------------------------------------------

/// The player corporation `who` is in, and whether they may manage its
/// fittings.
fn corp_standing(reg: &Registry, who: i64) -> Option<(i64, bool)> {
    let m = reg.corporations().membership(who)?;
    let corp = reg
        .corporations()
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    let roles = m.roles | m.roles_at_other;
    Some((
        corp.corporation_id,
        corp.ceo_id == who || roles & (CORP_ROLE_FITTING_MANAGER | CORP_ROLE_DIRECTOR) != 0,
    ))
}

/// The alliance `who`'s corporation is in.
fn alliance_of(reg: &Registry, who: i64) -> Option<i64> {
    let m = reg.corporations().membership(who)?;
    crate::alliance::of_corp(reg.deltas(), m.corporation_id)
}

/// Whether `owner` is an alliance.
fn is_alliance(reg: &Registry, owner: i64) -> bool {
    crate::alliance::name_of(reg.deltas(), owner).is_some()
}

/// May `who` read `owner`'s fittings: themselves, their own corporation or
/// their own alliance.
fn may_read(reg: &Registry, who: i64, owner: i64) -> bool {
    owner == who
        || corp_standing(reg, who).is_some_and(|(c, _)| c == owner)
        || alliance_of(reg, who) == Some(owner)
}

/// May `who` change `owner`'s fittings. An alliance's: only the executor
/// corporation's fitting managers (and, **[A]**, its CEO and directors, as
/// for a corporation).
fn may_write(reg: &Registry, who: i64, owner: i64) -> bool {
    if owner == who {
        return true;
    }
    let Some((corp, manager)) = corp_standing(reg, who) else {
        return false;
    };
    if corp == owner {
        return manager;
    }
    manager
        && alliance_of(reg, who) == Some(owner)
        && crate::alliance::Alliances::load(reg.deltas())
            .get(owner)
            .and_then(|a| a.executor_corp_id)
            == Some(corp)
}

/// The online characters whose own, corporation or alliance id is `owner`.
fn holders(reg: &Registry, owner: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|&id| {
            id == owner
                || reg
                    .corporations()
                    .membership(id)
                    .is_some_and(|m| m.corporation_id == owner)
                || alliance_of(reg, id) == Some(owner)
        })
        .collect()
}

/// Tell the members that fittings of `owner` were added. `caller_cache` is
/// whether the saver's own client already follows the call (`SaveMany`).
fn announce_added(reg: &mut Registry, who: i64, owner: i64, ids: Vec<i64>, caller_cache: bool) {
    if owner == who {
        // A character's own list: the saver is the only one who holds it.
        if !caller_cache {
            reg.push(FittingAdded {
                owner,
                ids,
                to: None,
            });
        }
        return;
    }
    let to: Vec<i64> = holders(reg, owner)
        .into_iter()
        .filter(|id| !(caller_cache && *id == who))
        .collect();
    if !to.is_empty() {
        reg.push(FittingAdded {
            owner,
            ids,
            to: Some(to),
        });
    }
}

/// Tell the other members that fittings of `owner` are gone.
fn announce_deleted(reg: &mut Registry, who: i64, owner: i64, ids: Vec<i64>, many: bool) {
    if ids.is_empty() || owner == who {
        return;
    }
    let to: Vec<i64> = holders(reg, owner)
        .into_iter()
        .filter(|id| *id != who)
        .collect();
    if !to.is_empty() {
        reg.push(FittingsDeleted {
            owner,
            ids,
            many,
            to,
        });
    }
}

// ---- mutations ------------------------------------------------------------

fn text(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The fitting a client sent: `(shipTypeID, name, description, lines)`;
/// `None` when it is malformed.
/// `(typeID, flag, quantity)` lines.
type Lines = Vec<(i64, i64, i64)>;

fn parse(v: &PyValue) -> Option<(i64, String, String, Lines)> {
    let at = |n: &str| crate::contracts::attr(v, n);
    let ship = int(at("shipTypeID"))?;
    let name = clip(text(at("name"))?.trim(), MAX_NAME);
    let desc = clip(
        &text(at("description")).unwrap_or_default(),
        MAX_DESCRIPTION,
    );
    let (PyValue::List(items) | PyValue::Tuple(items)) = at("fitData")? else {
        return None;
    };
    if name.is_empty() || items.len() > MAX_LINES {
        return None;
    }
    let mut lines = Vec::new();
    for it in items {
        let (PyValue::Tuple(p) | PyValue::List(p)) = it else {
            return None;
        };
        let [PyValue::Int(t), PyValue::Int(f), PyValue::Int(q)] = p.as_slice() else {
            return None;
        };
        if *q <= 0 {
            return None;
        }
        lines.push((*t, *f, *q));
    }
    Some((ship, name, desc, lines))
}

fn user_error(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

fn limit(owner: i64, reg: &Registry) -> usize {
    if is_alliance(reg, owner) {
        MAX_ALLIANCE_FITTINGS
    } else if reg.corporations().get(owner).is_some() {
        MAX_CORP_FITTINGS
    } else {
        MAX_CHAR_FITTINGS
    }
}

/// Store a new fitting for `owner`; the id.
pub fn save(
    reg: &mut Registry,
    who: i64,
    owner: i64,
    fitting: &PyValue,
    now: i64,
) -> Result<i64, ServiceError> {
    if !may_write(reg, who, owner) {
        return Err(user_error("You may not save fittings for that owner."));
    }
    let Some((ship, name, description, lines)) = parse(fitting) else {
        return Err(user_error("That fitting is not valid."));
    };
    if fittings(reg, owner).len() >= limit(owner, reg) {
        return Err(user_error("That owner has too many saved fittings."));
    }
    let id = next_id(reg);
    let f = Fitting {
        id,
        owner,
        ship_type: ship,
        saved: now,
        name,
        description,
        lines,
    };
    reg.deltas_mut().put(KIND, key(owner, id), row(&f));
    Ok(id)
}

/// `UpdateFitting`: replace fitting `id` of `owner`. `None` when there is
/// none.
pub fn update(
    reg: &mut Registry,
    who: i64,
    owner: i64,
    id: i64,
    fitting: &PyValue,
    now: i64,
) -> Result<Option<i64>, ServiceError> {
    if !may_write(reg, who, owner) {
        return Err(user_error("You may not change fittings of that owner."));
    }
    let Some(old) = fittings(reg, owner).into_iter().find(|f| f.id == id) else {
        return Ok(None);
    };
    let Some((ship, name, description, lines)) = parse(fitting) else {
        return Err(user_error("That fitting is not valid."));
    };
    let f = Fitting {
        ship_type: ship,
        saved: now,
        name,
        description: if description.is_empty() {
            old.description
        } else {
            description
        },
        lines,
        ..old
    };
    reg.deltas_mut().put(KIND, key(owner, id), row(&f));
    Ok(Some(id))
}

/// Delete one; whether it existed.
pub fn delete(reg: &mut Registry, who: i64, owner: i64, id: i64) -> bool {
    may_write(reg, who, owner) && reg.deltas_mut().del(KIND, &key(owner, id)).is_some()
}

/// `UpdateNameAndDescription(fittingID, ownerID, name, description)`.
pub fn rename(
    reg: &mut Registry,
    who: i64,
    owner: i64,
    id: i64,
    name: &str,
    description: &str,
) -> bool {
    let name = clip(name.trim(), MAX_NAME);
    if name.is_empty() || !may_write(reg, who, owner) {
        return false;
    }
    let Some(mut f) = fittings(reg, owner).into_iter().find(|f| f.id == id) else {
        return false;
    };
    f.name = name;
    f.description = clip(description, MAX_DESCRIPTION);
    reg.deltas_mut().put(KIND, key(owner, id), row(&f));
    true
}

// ---- the service ----------------------------------------------------------

/// `charFittingMgr` and `corpFittingMgr`. `None` for a method not owned, so
/// the caller keeps its fallback.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Option<Result<PyValue, ServiceError>> {
    let now = crate::pyutil::filetime_now();
    let who = req.actor;
    let owner = int(req.args.first());
    Some(match req.method {
        "GetFittings" => Ok(match (who, owner) {
            (Some(w), Some(o)) if may_read(reg, w, o) => fittings_value(reg, o),
            _ => PyValue::Dict(vec![]),
        }),
        "SaveFitting" => match (who, owner, req.args.get(1)) {
            (Some(w), Some(o), Some(f)) => save(reg, w, o, f, now).map(|id| {
                announce_added(reg, w, o, vec![id], false);
                PyValue::Int(id)
            }),
            _ => Err(user_error("That fitting is not valid.")),
        },
        "UpdateFitting" => {
            let id = int(req.args.get(2))
                .or_else(|| int(crate::pyutil::pairs_get(req.kwargs, b"fittingID")));
            match (who, owner, req.args.get(1), id) {
                (Some(w), Some(o), Some(f), Some(id)) => {
                    update(reg, w, o, id, f, now).map(|r| r.map_or(PyValue::None, PyValue::Int))
                }
                _ => Err(user_error("That fitting is not valid.")),
            }
        }
        // `SaveManyFittings(ownerID, {tempID: fitting})` -> rows of
        // `(tempFittingID, realFittingID)`.
        "SaveManyFittings" => match (who, owner, req.args.get(1)) {
            (Some(w), Some(o), Some(PyValue::Dict(entries))) => {
                let mut rows = Vec::new();
                let mut saved = Vec::new();
                let mut result = Ok(());
                for (fitting, temp) in entries {
                    match save(reg, w, o, fitting, now) {
                        Ok(real) => {
                            saved.push(real);
                            rows.push(crate::pyutil::keyval(vec![
                                ("tempFittingID", temp.clone()),
                                ("realFittingID", PyValue::Int(real)),
                            ]));
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                // Those saved before a failure are real: the others hear.
                if !saved.is_empty() {
                    announce_added(reg, w, o, saved, true);
                }
                result.map(|()| PyValue::List(rows))
            }
            _ => Err(user_error("Those fittings are not valid.")),
        },
        "DeleteFitting" => {
            if let (Some(w), Some(o), Some(id)) = (who, owner, int(req.args.get(1)))
                && delete(reg, w, o, id)
            {
                announce_deleted(reg, w, o, vec![id], false);
            }
            Ok(PyValue::None)
        }
        "DeleteManyFittings" => {
            let mut gone = Vec::new();
            if let (Some(w), Some(o), Some(PyValue::List(ids) | PyValue::Tuple(ids))) =
                (who, owner, req.args.get(1))
            {
                for id in ids.iter().filter_map(|v| int(Some(v))) {
                    if delete(reg, w, o, id) {
                        gone.push(PyValue::Int(id));
                    }
                }
                let gone_ids = gone.iter().filter_map(|v| int(Some(v))).collect();
                announce_deleted(reg, w, o, gone_ids, true);
            }
            Ok(PyValue::List(gone))
        }
        "UpdateNameAndDescription" => {
            if let (Some(w), Some(id), Some(o), Some(n), Some(d)) = (
                who,
                int(req.args.first()),
                int(req.args.get(1)),
                text(req.args.get(2)),
                text(req.args.get(3)),
            ) {
                rename(reg, w, o, id, &n, &d);
            }
            Ok(PyValue::None)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::pyutil::keyval;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const SHIP: i64 = 7_001;
    const MODULE: i64 = 7_002;
    const FLAG_LOW: i64 = 11;

    fn registry() -> Registry {
        let config = SandboxConfig {
            characters: [ALICE, BOB]
                .into_iter()
                .map(|id| Character {
                    character_id: id,
                    name: format!("P{id}"),
                    ship_id: id + 5_000_000,
                    ..Character::default()
                })
                .collect(),
            ..SandboxConfig::default()
        };
        Registry::new(config)
    }

    fn fit(name: &str, qty: i64) -> PyValue {
        keyval(vec![
            ("shipTypeID", PyValue::Int(SHIP)),
            ("name", PyValue::Unicode(name.to_owned())),
            ("description", PyValue::Unicode("d".to_owned())),
            (
                "fitData",
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(MODULE),
                    PyValue::Int(FLAG_LOW),
                    PyValue::Int(qty),
                ])]),
            ),
        ])
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    fn ids(v: &PyValue) -> Vec<i64> {
        let PyValue::Dict(e) = v else {
            panic!("GetFittings is a dict");
        };
        e.iter()
            .map(|(_, k)| match k {
                PyValue::Int(n) => *n,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn save_get_update_rename_delete_round_trip() {
        let mut reg = registry();
        let svc = "charFittingMgr";
        let id = match call(
            &mut reg,
            ALICE,
            svc,
            "SaveFitting",
            &[PyValue::Int(ALICE), fit("one", 2)],
        )
        .expect("saves")
        {
            PyValue::Int(n) => n,
            other => panic!("an id, got {other:?}"),
        };
        assert!(id > 0);
        let got = call(&mut reg, ALICE, svc, "GetFittings", &[PyValue::Int(ALICE)]).unwrap();
        assert_eq!(ids(&got), vec![id]);
        // The client only learns of a new fitting from this push.
        let pushes: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .collect();
        assert!(
            pushes.contains(&(
                "OnFittingAdded",
                vec![PyValue::Int(ALICE), PyValue::Int(id)]
            )),
            "{pushes:?}"
        );
        let f = &fittings(&reg, ALICE)[0];
        assert_eq!((f.name.as_str(), f.ship_type), ("one", SHIP));
        assert_eq!(f.lines, vec![(MODULE, FLAG_LOW, 2)]);
        // Update keeps the id and swaps the data.
        call(
            &mut reg,
            ALICE,
            svc,
            "UpdateFitting",
            &[PyValue::Int(ALICE), fit("one", 5), PyValue::Int(id)],
        )
        .expect("updates");
        assert_eq!(fittings(&reg, ALICE)[0].lines, vec![(MODULE, FLAG_LOW, 5)]);
        call(
            &mut reg,
            ALICE,
            svc,
            "UpdateNameAndDescription",
            &[
                PyValue::Int(id),
                PyValue::Int(ALICE),
                PyValue::Unicode("renamed".into()),
                PyValue::Unicode("new text".into()),
            ],
        )
        .unwrap();
        assert_eq!(fittings(&reg, ALICE)[0].name, "renamed");
        // A second fitting gets a different id; Bob sees neither.
        let id2 = save(&mut reg, ALICE, ALICE, &fit("two", 1), 0).unwrap();
        assert_ne!(id2, id);
        let bob = call(&mut reg, BOB, svc, "GetFittings", &[PyValue::Int(BOB)]).unwrap();
        assert!(ids(&bob).is_empty());
        let none = call(&mut reg, BOB, svc, "GetFittings", &[PyValue::Int(ALICE)]).unwrap();
        assert!(
            ids(&none).is_empty(),
            "another pilot's list is not readable"
        );
        call(
            &mut reg,
            ALICE,
            svc,
            "DeleteFitting",
            &[PyValue::Int(ALICE), PyValue::Int(id)],
        )
        .unwrap();
        assert_eq!(
            ids(&call(&mut reg, ALICE, svc, "GetFittings", &[PyValue::Int(ALICE)]).unwrap()),
            vec![id2]
        );
        // Bob cannot delete Alice's.
        call(
            &mut reg,
            BOB,
            svc,
            "DeleteFitting",
            &[PyValue::Int(ALICE), PyValue::Int(id2)],
        )
        .unwrap();
        assert_eq!(fittings(&reg, ALICE).len(), 1);
        // Many: the rows map temp ids to real ones, and many deletes list what went.
        let many = PyValue::Dict(vec![
            (fit("a", 1), PyValue::Int(-1)),
            (fit("b", 1), PyValue::Int(-2)),
        ]);
        let PyValue::List(rows) = call(
            &mut reg,
            ALICE,
            svc,
            "SaveManyFittings",
            &[PyValue::Int(ALICE), many],
        )
        .unwrap() else {
            panic!("rows");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(fittings(&reg, ALICE).len(), 3);
        let PyValue::List(gone) = call(
            &mut reg,
            ALICE,
            svc,
            "DeleteManyFittings",
            &[
                PyValue::Int(ALICE),
                PyValue::List(vec![PyValue::Int(id2), PyValue::Int(999_999)]),
            ],
        )
        .unwrap() else {
            panic!("list");
        };
        assert_eq!(gone, vec![PyValue::Int(id2)]);
    }

    #[test]
    fn a_fitting_survives_a_save_and_load_of_the_deltas() {
        let mut reg = registry();
        let id = save(&mut reg, ALICE, ALICE, &fit("kept\tname", 3), 77).unwrap();
        let rows = reg.deltas().of(KIND).clone();
        let mut fresh = registry();
        for (k, v) in rows {
            fresh.deltas_mut().put(KIND, k, v);
        }
        let f = fittings(&fresh, ALICE);
        assert_eq!(
            (f[0].id, f[0].saved, f[0].name.as_str()),
            (id, 77, "kept\tname")
        );
    }

    #[test]
    fn bad_fittings_are_refused_not_stored() {
        let mut reg = registry();
        let no_name = fit("  ", 1);
        assert!(save(&mut reg, ALICE, ALICE, &no_name, 0).is_err());
        assert!(save(&mut reg, ALICE, ALICE, &fit("x", 0), 0).is_err());
        assert!(save(&mut reg, BOB, ALICE, &fit("x", 1), 0).is_err());
        assert!(fittings(&reg, ALICE).is_empty());
        let long = "n".repeat(MAX_NAME + 20);
        save(&mut reg, ALICE, ALICE, &fit(&long, 1), 0).unwrap();
        assert_eq!(fittings(&reg, ALICE)[0].name.chars().count(), MAX_NAME);
    }

    #[test]
    fn the_wire_form_has_the_fields_the_client_reads() {
        let mut reg = registry();
        save(&mut reg, ALICE, ALICE, &fit("one", 2), 5).unwrap();
        let PyValue::Dict(e) = fittings_value(&reg, ALICE) else {
            panic!("dict");
        };
        let (kv, _) = &e[0];
        for name in [
            "fittingID",
            "ownerID",
            "name",
            "description",
            "shipTypeID",
            "fitData",
            "savedDate",
        ] {
            assert!(crate::contracts::attr(kv, name).is_some(), "{name}");
        }
    }

    // ---- GAP-FITSHARE: corporation and alliance fittings ----------------

    const CAROL: i64 = 90_000_003;
    const DAVE: i64 = 90_000_004;
    const ERIN: i64 = 90_000_005;

    /// Alice (CEO) and Bob (plain member) of corporation one, Carol (CEO)
    /// and Dave (plain member) of corporation two, Erin in neither. One and
    /// two form an alliance with one as the executor. All but Erin online.
    /// `(registry, corp one, corp two, alliance)`.
    fn alliance_world() -> (Registry, i64, i64, i64) {
        let config = SandboxConfig {
            characters: [ALICE, BOB, CAROL, DAVE, ERIN]
                .into_iter()
                .map(|id| Character {
                    character_id: id,
                    name: format!("P{id}"),
                    ship_id: id + 5_000_000,
                    ..Character::default()
                })
                .collect(),
            ..SandboxConfig::default()
        };
        let mut reg = Registry::new(config);
        crate::service::corp::dev_join(&mut reg, ALICE, "AAA");
        crate::service::corp::dev_join(&mut reg, BOB, "AAA");
        crate::service::corp::dev_join(&mut reg, CAROL, "CCC");
        crate::service::corp::dev_join(&mut reg, DAVE, "CCC");
        let corp_of = |reg: &Registry, who| {
            reg.corporations()
                .membership(who)
                .expect("in a corp")
                .corporation_id
        };
        let (one, two) = (corp_of(&reg, ALICE), corp_of(&reg, CAROL));
        let mut al = crate::alliance::Alliances::load(reg.deltas());
        let id = al
            .create(one, ALICE, "Invented Pact", "INVP", "", "", 1)
            .expect("created");
        al.apply(two, id, "", 2).expect("applied");
        al.answer(id, two, crate::alliance::APP_ACCEPTED, None, 3)
            .expect("accepted");
        al.save(reg.deltas_mut());
        crate::alliance::sync_characters(&mut reg);
        for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n as i64 + 1,
                session_id: n as i64 + 1,
                user_id: n as i64 + 1,
                character_id: Some(who),
            });
        }
        (reg, one, two, id)
    }

    /// `(event, args, audience)` of every queued push.
    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);
    fn pushed(reg: &mut Registry) -> Vec<Sent> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (e, a) = p.to_push();
                (e, a, p.audience())
            })
            .collect()
    }

    #[test]
    fn alliance_members_read_alliance_fittings_and_outsiders_get_an_empty_dict() {
        let (mut reg, _, _, pact) = alliance_world();
        let svc = "allianceFittingMgr";
        let id = save(&mut reg, ALICE, pact, &fit("shared", 1), 0).expect("the executor saves");
        // A member of the other corporation reads it; an outsider gets {}.
        let got = call(&mut reg, DAVE, svc, "GetFittings", &[PyValue::Int(pact)]).unwrap();
        assert_eq!(ids(&got), vec![id]);
        let none = call(&mut reg, ERIN, svc, "GetFittings", &[PyValue::Int(pact)]).unwrap();
        assert!(ids(&none).is_empty());
    }

    #[test]
    fn only_the_executor_corps_fitting_manager_writes_alliance_fittings() {
        let (mut reg, _, _, pact) = alliance_world();
        // The executor's CEO may; its plain member, the other corporation's
        // CEO and an outsider may not.
        assert!(save(&mut reg, ALICE, pact, &fit("a", 1), 0).is_ok());
        for who in [BOB, CAROL, DAVE, ERIN] {
            assert!(save(&mut reg, who, pact, &fit("x", 1), 0).is_err(), "{who}");
        }
        assert_eq!(fittings(&reg, pact).len(), 1);
        let id = fittings(&reg, pact)[0].id;
        assert!(!delete(&mut reg, CAROL, pact, id));
        assert!(delete(&mut reg, ALICE, pact, id));
    }

    #[test]
    fn a_corp_fitting_save_is_pushed_to_every_online_corp_member() {
        let (mut reg, one, _, _) = alliance_world();
        let _ = pushed(&mut reg);
        let id = match call(
            &mut reg,
            ALICE,
            "corpFittingMgr",
            "SaveFitting",
            &[PyValue::Int(one), fit("corp", 1)],
        )
        .unwrap()
        {
            PyValue::Int(n) => n,
            other => panic!("an id, got {other:?}"),
        };
        let added: Vec<_> = pushed(&mut reg)
            .into_iter()
            .filter(|(e, ..)| *e == "OnFittingAdded")
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].1, vec![PyValue::Int(one), PyValue::Int(id)]);
        let mut to = added[0].2.clone().expect("addressed");
        to.sort_unstable();
        assert_eq!(
            to,
            vec![ALICE, BOB],
            "the corp's members, not the alliance's"
        );
    }

    #[test]
    fn delete_pushes_to_the_others_not_the_caller() {
        let (mut reg, _, _, pact) = alliance_world();
        let svc = "allianceFittingMgr";
        let a = save(&mut reg, ALICE, pact, &fit("a", 1), 0).unwrap();
        let b = save(&mut reg, ALICE, pact, &fit("b", 1), 0).unwrap();
        let _ = pushed(&mut reg);
        call(
            &mut reg,
            ALICE,
            svc,
            "DeleteFitting",
            &[PyValue::Int(pact), PyValue::Int(a)],
        )
        .unwrap();
        let sent = pushed(&mut reg);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].0, "OnFittingDeleted");
        assert_eq!(sent[0].1, vec![PyValue::Int(pact), PyValue::Int(a)]);
        let mut to = sent[0].2.clone().expect("addressed");
        to.sort_unstable();
        assert_eq!(to, vec![BOB, CAROL, DAVE]);
        call(
            &mut reg,
            ALICE,
            svc,
            "DeleteManyFittings",
            &[PyValue::Int(pact), PyValue::List(vec![PyValue::Int(b)])],
        )
        .unwrap();
        let sent = pushed(&mut reg);
        assert_eq!(sent[0].0, "OnManyFittingsDeleted");
        assert_eq!(
            sent[0].1,
            vec![PyValue::Int(pact), PyValue::List(vec![PyValue::Int(b)])]
        );
        assert!(!sent[0].2.clone().expect("addressed").contains(&ALICE));
    }

    #[test]
    fn save_many_pushes_one_list() {
        let (mut reg, _, _, pact) = alliance_world();
        let _ = pushed(&mut reg);
        let many = PyValue::Dict(vec![
            (fit("a", 1), PyValue::Int(-1)),
            (fit("b", 1), PyValue::Int(-2)),
        ]);
        call(
            &mut reg,
            ALICE,
            "allianceFittingMgr",
            "SaveManyFittings",
            &[PyValue::Int(pact), many],
        )
        .unwrap();
        let sent = pushed(&mut reg);
        assert_eq!(sent.len(), 1, "one push for the batch: {sent:?}");
        let PyValue::List(listed) = &sent[0].1[1] else {
            panic!("a list of ids, got {:?}", sent[0].1);
        };
        assert_eq!(listed.len(), 2);
        assert!(!sent[0].2.clone().expect("addressed").contains(&ALICE));
    }
}
