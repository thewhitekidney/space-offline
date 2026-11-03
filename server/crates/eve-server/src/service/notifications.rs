//! **The notifications wire** (NOTIF-1): `notificationMgr`, the service
//! behind the notification centre and the mail window's Notifications tab,
//! and the producers that need the corporations (a child of `service`, so
//! they can read them). The store is `crate::notifications`'s.
//!
//! Every call is `notificationSvc`'s, through `self.notificationMgr =
//! sm.RemoteSvc('notificationMgr')` (`notificationSvc.Run`) **[V]**:
//!
//! | call | caller | answer |
//! | --- | --- | --- |
//! | `GetAllNotifications(fromID=n)` | `_GetAllNotifications`, a **kwarg** | the ids above `fromID`, newest first |
//! | `GetUnprocessed()` | `GetUnreadNotifications` | the unread |
//! | `GetByGroupID(groupID)` | `GetNotificationsByGroupID` | one group's, read or not |
//! | `MarkAsProcessed(ids)` | `MarkAsRead` | discarded |
//! | `MarkGroupAsProcessed(groupID)` | `MarkAllReadInGroup` | discarded |
//! | `MarkAllAsProcessed()` | `MarkAllRead` | discarded |
//! | `DeleteNotifications(ids)` | `DeleteNotifications` | discarded |
//! | `DeleteGroupNotifications(groupID)` | `DeleteAllFromGroup` | discarded |
//! | `DeleteAllNotifications()` | `DeleteAll` | discarded |
//! | `LogNotificationInteraction(ref)` | `notificationCenter.OnEntryClick`, CCP news | discarded |
//!
//! **Lists, not rowsets**: the client only iterates them and reads
//! attributes, and it assigns `allNotifications = []` itself, so `[]` is a
//! shape it already makes; `[]` also caches (the guard is `is None`). Each
//! row is a `KeyVal` with the seven attributes `Notification.FromDTO` reads
//! (`crate::notifications::Notification::row`).
//!
//! `fromID` is not a page cursor: it is the highest id the player had seen
//! when they pressed "Clear history" in the notification centre, kept in
//! client settings (`notificationUI.ClearHistory`), so the faithful reading
//! is `notificationID > fromID` **[I]**. There is no page size and no
//! second call.
//!
//! Every mutator's answer is discarded by its caller (`POP_TOP`), and each
//! acts on the caller's own notifications only: the store's key carries the
//! receiver, so an id that is somebody else's matches nothing. An id list
//! is cut at `notifications::MAX_UPDATED`, the client's own cap.

use super::*;
use crate::notifications::{self as store, Selection, Value, data, kind, notify_from};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["notificationMgr"];

/// An int argument at `i`, or the keyword `name`.
fn int_arg(req: &Request<'_>, i: usize, name: &str) -> Option<i64> {
    match req
        .args
        .get(i)
        .or_else(|| crate::space_state::kwarg(req.kwargs, name))?
    {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// The id list at `i` (a list or a tuple of ints, as `MarkAsRead` and
/// `DeleteNotifications` send it), cut at the client's cap. Anything that is
/// not an int is skipped.
fn ids_arg(req: &Request<'_>, i: usize, name: &str) -> Vec<i64> {
    let items = match req
        .args
        .get(i)
        .or_else(|| crate::space_state::kwarg(req.kwargs, name))
    {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v.as_slice(),
        _ => &[],
    };
    items
        .iter()
        .filter_map(|v| match v {
            PyValue::Int(n) => Some(*n),
            _ => None,
        })
        .take(store::MAX_UPDATED)
        .collect()
}

fn rows(v: Vec<store::Notification>) -> PyValue {
    PyValue::List(v.iter().map(store::Notification::row).collect())
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "notificationMgr" => {
            // No character, nothing stored and nothing to change: the same
            // empty answers a fresh character gets.
            let Some(me) = req.actor else {
                return match req.method {
                    "GetAllNotifications" | "GetUnprocessed" | "GetByGroupID" => {
                        Ok(PyValue::List(Vec::new()))
                    }
                    "MarkAsProcessed"
                    | "MarkAllAsProcessed"
                    | "MarkGroupAsProcessed"
                    | "DeleteNotifications"
                    | "DeleteAllNotifications"
                    | "DeleteGroupNotifications"
                    | "LogNotificationInteraction" => Ok(PyValue::None),
                    _ => Err(reg.no_method(req)),
                };
            };
            match req.method {
                "GetAllNotifications" => {
                    let from = int_arg(req, 0, "fromID").unwrap_or(0);
                    Ok(rows(store::after(reg.deltas(), me, from)))
                }
                "GetUnprocessed" => Ok(rows(store::unread(reg.deltas(), me))),
                // The "unread" entry is not a group of types; the client
                // asks `GetUnprocessed` for it, and the same answer here
                // keeps a stray `GetByGroupID(0)` honest.
                "GetByGroupID" => Ok(match int_arg(req, 0, "groupID") {
                    Some(store::group::UNREAD) => rows(store::unread(reg.deltas(), me)),
                    Some(g) => rows(store::in_group(reg.deltas(), me, g)),
                    None => PyValue::List(Vec::new()),
                }),
                "MarkAsProcessed" => {
                    let ids = ids_arg(req, 0, "notificationIDs");
                    if !ids.is_empty() {
                        store::mark_read(reg.deltas_mut(), me, Selection::Ids(&ids));
                    }
                    Ok(PyValue::None)
                }
                "MarkGroupAsProcessed" => {
                    if let Some(g) = int_arg(req, 0, "groupID") {
                        store::mark_read(reg.deltas_mut(), me, Selection::Group(g));
                    }
                    Ok(PyValue::None)
                }
                "MarkAllAsProcessed" => {
                    store::mark_read(reg.deltas_mut(), me, Selection::All);
                    Ok(PyValue::None)
                }
                "DeleteNotifications" => {
                    let ids = ids_arg(req, 0, "notificationIDs");
                    if !ids.is_empty() {
                        store::delete(reg.deltas_mut(), me, Selection::Ids(&ids));
                    }
                    Ok(PyValue::None)
                }
                "DeleteGroupNotifications" => {
                    if let Some(g) = int_arg(req, 0, "groupID") {
                        store::delete(reg.deltas_mut(), me, Selection::Group(g));
                    }
                    Ok(PyValue::None)
                }
                "DeleteAllNotifications" => {
                    store::delete(reg.deltas_mut(), me, Selection::All);
                    Ok(PyValue::None)
                }
                // Telemetry on a CCP news item: nothing to record.
                "LogNotificationInteraction" => Ok(PyValue::None),
                _ => Err(reg.no_method(req)),
            }
        }
        // Unreachable: `SERVICES` gates the call. An error, not a panic.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

// ---- producers that read the corporations ---------------------------------------------

/// The members of a player corporation, or of every corporation in an
/// alliance, in id order.
fn members_of_owner(w: &Registry, owner: i64) -> Vec<i64> {
    let corps: Vec<i64> = if crate::alliance::name_of(w.deltas(), owner).is_some() {
        crate::alliance::Alliances::load(w.deltas())
            .members_of(owner)
            .into_iter()
            .map(|(corp, _)| corp)
            .collect()
    } else {
        vec![owner]
    };
    let mut out: Vec<i64> = corps
        .into_iter()
        .flat_map(|c| w.corporations.members_of(c))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// **A militia request** (CP53a, `service::facwar_mgr::corp_op`, once the
/// row is written): a corporation or alliance asked to join or leave
/// `faction`'s militia, or withdrew the request. Every member is told: types
/// 81-84, `{corpID, factionID}`, which `ParamFmtFactionWarfareCorps` turns
/// into the labels' `corporationName` and `factionName` **[V]**. For an
/// alliance `corpID` is the alliance **[I]**: the parameter function only
/// links the id. The sender is the faction, which the client names itself
/// **[V]**; the audience (every member) is **[A]**, falsified by a retail
/// militia request reaching only the corp's directors.
///
/// The join settling 24 hours later has no event (`corp_state` settles
/// lazily), so `FWCorpJoinMsg` (58) is not sent: lane note, Not done.
pub fn militia_request(w: &mut Registry, owner: i64, faction: i64, op: facwar_mgr::CorpOp) {
    use facwar_mgr::CorpOp;
    let type_id = match op {
        CorpOp::Join => kind::FW_CORP_JOIN_REQUEST,
        CorpOp::Leave => kind::FW_CORP_LEAVE_REQUEST,
        CorpOp::WithdrawJoin => kind::FW_CORP_JOIN_WITHDRAW,
        CorpOp::WithdrawLeave => kind::FW_CORP_LEAVE_WITHDRAW,
    };
    for member in members_of_owner(w, owner) {
        notify_from(
            w,
            member,
            type_id,
            faction,
            data([
                ("corpID", Value::Int(owner)),
                ("factionID", Value::Int(faction)),
            ]),
        );
    }
}

/// **The corp producers** (SOC-C3's hooks): applications and leaving.
///
/// Wired into `service/corp.rs` by the coordinator when NOTIF-1 landed:
/// `update_application`, `InsertApplication`, `InsertInvitation`,
/// `kick_out` and `hand_over`.
pub mod corp_events {
    use super::*;

    /// The corp notification types (`notificationconst.py`) **[V]**.
    pub mod kind {
        pub const CORP_APP_NEW: i64 = 16;
        pub const CORP_APP_REJECT: i64 = 17;
        pub const CORP_APP_ACCEPT: i64 = 18;
        pub const CHAR_LEFT_CORP: i64 = 21;
        pub const CORP_NEW_CEO: i64 = 22;
        pub const CORP_KICKED: i64 = 92;
        pub const CHAR_APP_ACCEPT: i64 = 128;
        pub const CHAR_APP_REJECT: i64 = 129;
        pub const CHAR_APP_WITHDRAW: i64 = 130;
        pub const CORP_APP_INVITED: i64 = 139;
        pub const CORP_APP_REJECT_CUSTOM: i64 = 142;
        pub const CORP_LEFT: i64 = 270;
    }

    /// A corporation's application handlers: its CEO and every director or
    /// personnel manager **[A]** (falsified by a retail application
    /// notification reaching a member with neither role, or missing a
    /// director).
    pub fn corp_managers(w: &Registry, corp: i64) -> Vec<i64> {
        let ceo = w.corporations.get(corp).map(|c| c.ceo_id);
        w.corporations
            .members_of(corp)
            .into_iter()
            .filter(|m| {
                Some(*m) == ceo
                    || w.corporations.membership(*m).is_some_and(|x| {
                        x.roles & (corp::CORP_ROLE_DIRECTOR | corp::CORP_ROLE_PERSONNEL_MANAGER)
                            != 0
                    })
            })
            .collect()
    }

    /// The application statuses `service/corp.rs` stores, which are the
    /// client's (`corp_ui_applications`, `CorpApplicationEntry`) **[V]**.
    pub mod app {
        pub const APPLIED: i64 = 0;
        pub const RENEGOTIATED: i64 = 1;
        pub const ACCEPTED_BY_CHARACTER: i64 = 2;
        pub const REJECTED_BY_CHARACTER: i64 = 3;
        pub const REJECTED_BY_CORPORATION: i64 = 4;
        pub const ACCEPTED_BY_CORPORATION: i64 = 6;
        pub const WITHDRAWN_BY_CHARACTER: i64 = 7;
        pub const INVITED_BY_CORPORATION: i64 = 8;
    }

    /// **A corp application moved** (SOC-C3's hooks in `service/corp.rs`:
    /// `insert_application` and `update_application`, once the row is written).
    /// `actor` made the change; `character` and `corp` are the application's;
    /// `from` is the status before (`None` for a new row); `text` the
    /// application text and `custom` the rejection's custom message (empty for
    /// none).
    ///
    /// | to | told | type | data keys **[V]** (labels and parameter functions) |
    /// | --- | --- | --- | --- |
    /// | applied (new) | managers | 16 | `charID, corpID, applicationText` |
    /// | invited (new) | the pilot | 139 | `charID, corpID, invokingCharID` |
    /// | accepted by the corp | the pilot | 18 | `charID, corpID` |
    /// | rejected by the corp, from applied | the pilot | 17, or 142 with `customMessage` | `charID, corpID` |
    /// | accepted by the pilot (joins) | managers | 128 | `charID, corpID, applicationText` |
    /// | rejected by the pilot | managers | 129 | the same |
    /// | withdrawn by the pilot | managers | 130 | the same |
    ///
    /// A withdrawn invitation (invited, then rejected by the corp) tells nobody
    /// **[A]**: no type fits it. The pilot's come from the corp, the managers'
    /// from the pilot; the pilot is never one of the managers told.
    #[allow(clippy::too_many_arguments)]
    pub fn corp_application(
        w: &mut Registry,
        actor: i64,
        character: i64,
        corp: i64,
        from: Option<i64>,
        to: i64,
        text: &str,
        custom: &str,
    ) {
        let pilot_data = || {
            data([
                ("charID", Value::Int(character)),
                ("corpID", Value::Int(corp)),
            ])
        };
        let to_managers = |w: &mut Registry, type_id: i64| {
            for m in corp_managers(w, corp)
                .into_iter()
                .filter(|m| *m != character)
            {
                notify_from(
                    w,
                    m,
                    type_id,
                    character,
                    data([
                        ("charID", Value::Int(character)),
                        ("corpID", Value::Int(corp)),
                        ("applicationText", Value::Text(text.to_owned())),
                    ]),
                );
            }
        };
        match (from, to) {
            (None, app::APPLIED) => to_managers(w, kind::CORP_APP_NEW),
            (None, app::INVITED_BY_CORPORATION) => {
                let mut d = pilot_data();
                d.push(("invokingCharID".to_owned(), Value::Int(actor)));
                notify_from(w, character, kind::CORP_APP_INVITED, corp, d);
            }
            (Some(_), app::ACCEPTED_BY_CORPORATION) => {
                notify_from(w, character, kind::CORP_APP_ACCEPT, corp, pilot_data());
            }
            (Some(app::APPLIED | app::RENEGOTIATED), app::REJECTED_BY_CORPORATION) => {
                if custom.is_empty() {
                    notify_from(w, character, kind::CORP_APP_REJECT, corp, pilot_data());
                } else {
                    let mut d = pilot_data();
                    d.push(("customMessage".to_owned(), Value::Text(custom.to_owned())));
                    notify_from(w, character, kind::CORP_APP_REJECT_CUSTOM, corp, d);
                }
            }
            (Some(_), app::ACCEPTED_BY_CHARACTER) => to_managers(w, kind::CHAR_APP_ACCEPT),
            (Some(_), app::REJECTED_BY_CHARACTER) => to_managers(w, kind::CHAR_APP_REJECT),
            (Some(_), app::WITHDRAWN_BY_CHARACTER) => to_managers(w, kind::CHAR_APP_WITHDRAW),
            _ => {}
        }
    }

    /// **A pilot left a player corp** (SOC-C3's hook in `service/corp.rs`:
    /// `kick_out`, after the move; `corp` is the corp they left). Quitting tells
    /// the pilot `CorpLeft` (270, `{corpID}`, `FormatCorpLeft`); being expelled
    /// tells them `CorpKicked` (92, `{corpID}`) **[V]**. Either way the corp's
    /// managers get `CharLeftCorpMsg` (21, `{charID, corpID}`) from the pilot
    /// **[V]** for the keys, **[A]** for the audience, as [`corp_managers`].
    pub fn corp_member_left(w: &mut Registry, character: i64, corp: i64, kicked: bool) {
        let type_id = if kicked {
            kind::CORP_KICKED
        } else {
            kind::CORP_LEFT
        };
        notify_from(
            w,
            character,
            type_id,
            corp,
            data([("corpID", Value::Int(corp))]),
        );
        for m in corp_managers(w, corp)
            .into_iter()
            .filter(|m| *m != character)
        {
            notify_from(
                w,
                m,
                kind::CHAR_LEFT_CORP,
                character,
                data([
                    ("charID", Value::Int(character)),
                    ("corpID", Value::Int(corp)),
                ]),
            );
        }
    }

    /// **A new CEO** (SOC-C3's hook in `service/corp.rs`: `hand_over`, once
    /// the seat moved): `CorpNewCEOMsg` (22) to every member, `{corpID,
    /// oldCeoID, newCeoID}` (`subjCEOQuit` and `bodyCEOQuit` name the two
    /// CEOs; the parameter function links `corpID`) **[V]**. From the corp
    /// **[A]**; every member is told **[A]** (falsified by a retail one
    /// reaching only the managers).
    pub fn corp_new_ceo(w: &mut Registry, corp: i64, old_ceo: i64, new_ceo: i64) {
        for m in w.corporations.members_of(corp) {
            notify_from(
                w,
                m,
                kind::CORP_NEW_CEO,
                corp,
                data([
                    ("corpID", Value::Int(corp)),
                    ("oldCeoID", Value::Int(old_ceo)),
                    ("newCeoID", Value::Int(new_ceo)),
                ]),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::corp_events::kind as ck;
    use super::corp_events::{
        app, corp_application, corp_managers, corp_member_left, corp_new_ceo,
    };
    use super::*;
    use crate::damage::{Damage, Shot};
    use crate::notifications::group;

    /// Invented characters, ships and types: nothing here is a client row.
    const ALICE: i64 = 90_000_301;
    const BOB: i64 = 90_000_302;
    const CAROL: i64 = 90_000_303;
    const ALICE_SHIP: i64 = 100_000_301;
    const BOB_SHIP: i64 = 100_000_302;
    const CAROL_SHIP: i64 = 100_000_303;
    const HULL: i64 = 990_620_001;
    const GUN: i64 = 990_620_002;
    const SOME_TYPE: i64 = 9_002;

    fn pilot(id: i64, name: &str, ship: i64) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: HULL,
            ..crate::model::Character::default()
        }
    }

    fn docked() -> Registry {
        crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
                pilot(CAROL, "Carol", CAROL_SHIP),
            ];
        })
    }

    /// Alice and Bob undocked into one system, each in a ship that is an
    /// item (`service/killmail.rs`'s world, without the cargo).
    fn in_space() -> Registry {
        let mut reg = docked();
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = HULL;
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_400,
            ..crate::store::Snapshot::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        reg
    }

    fn volley(reg: &mut Registry, shooter: i64, victim: i64, em: f64) {
        let t0 = crate::pyutil::filetime_now();
        let v = reg.roster().get(victim).copied().expect("in space");
        let from = reg.roster().get(shooter).map_or(0, |p| p.ship_id);
        let shot = Shot {
            at: t0,
            attacker_ship_id: from,
            weapon_item_id: 1,
            weapon_type_id: GUN,
            target_ship_id: v.ship_id,
            damage: Damage {
                em,
                explosive: 0.0,
                kinetic: 0.0,
                thermal: 0.0,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, shooter, v, t0);
    }

    fn call_kw(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> PyValue {
        reg.dispatch(&Request {
            service: "notificationMgr",
            method,
            args,
            kwargs,
            bind_params: None,
            actor: Some(actor),
        })
        .expect("answered")
    }

    fn call(reg: &mut Registry, actor: i64, method: &str, args: &[PyValue]) -> PyValue {
        call_kw(reg, actor, method, args, &[])
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("not a KeyVal: {v:?}")
        };
        state
            .dict_as_kv()
            .and_then(|kv| {
                kv.into_iter().find_map(|(k, val)| {
                    (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(val)
                })
            })
            .unwrap_or_else(|| panic!("no {name} on {v:?}"))
    }

    fn list(v: PyValue) -> Vec<PyValue> {
        let PyValue::List(rows) = v else {
            panic!("a list: {v:?}")
        };
        rows
    }

    /// `(type, sender)` of each of `who`'s notifications, newest first.
    fn held(reg: &Registry, who: i64) -> Vec<(i64, i64)> {
        store::of(reg.deltas(), who)
            .iter()
            .map(|n| (n.type_id, n.sender))
            .collect()
    }

    fn int(i: i64) -> PyValue {
        PyValue::Int(i)
    }

    fn ids(v: &[i64]) -> PyValue {
        PyValue::List(v.iter().map(|i| PyValue::Int(*i)).collect())
    }

    /// **A kill through the real path**: Alice's volleys kill Bob's ship.
    /// Bob holds a kill report (victim), Alice one (final blow), each
    /// linking the mail the combat log lists, with the client's hash; each
    /// was pushed to its pilot on the clock; and `warStatisticMgr` opens
    /// the mail from the notification's own two values.
    #[test]
    fn a_kill_notifies_the_victim_and_the_final_blow() {
        let mut reg = in_space();
        volley(&mut reg, ALICE, BOB, 10.0);
        volley(&mut reg, ALICE, BOB, 1.0e9);
        assert!(
            crate::kill_hooks::PLAYER
                .iter()
                .any(|r| r.name == "notifications_on_kill"),
            "the producer is a kill_hooks::PLAYER row"
        );
        let report = crate::killmail::involving(&reg, BOB, None, 1)
            .pop()
            .expect("the kill was reported");

        let bobs = store::of(reg.deltas(), BOB);
        assert_eq!(bobs.len(), 1, "{bobs:?}");
        assert_eq!(bobs[0].type_id, kind::KILL_REPORT_VICTIM);
        assert_eq!(bobs[0].created, report.time);
        let alices = store::of(reg.deltas(), ALICE);
        assert_eq!(alices.len(), 1, "{alices:?}");
        assert_eq!(alices[0].type_id, kind::KILL_REPORT_FINAL_BLOW);
        let hash = Value::Text(store::kill_hash(&report));
        let get = |n: &store::Notification, k: &str| {
            n.data
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no {k}"))
        };
        assert_eq!(get(&bobs[0], "killMailID"), Value::Int(report.kill_id));
        assert_eq!(get(&bobs[0], "killMailHash"), hash);
        assert_eq!(get(&bobs[0], "victimShipTypeID"), Value::Int(HULL));
        assert_eq!(get(&alices[0], "victimID"), Value::Int(BOB));
        assert_eq!(get(&alices[0], "killMailHash"), hash);

        let due = reg.take_due(std::time::Instant::now());
        let pushed: Vec<(Option<Vec<i64>>, Vec<PyValue>)> = due
            .iter()
            .filter_map(|p| {
                let (event, args) = p.to_push();
                (event == "OnNotificationReceived").then(|| (p.audience(), args))
            })
            .collect();
        assert_eq!(pushed.len(), 2, "{pushed:?}");
        assert!(pushed.contains(&(
            Some(vec![BOB]),
            vec![
                int(bobs[0].id),
                int(kind::KILL_REPORT_VICTIM),
                int(store::SENDER_SYSTEM),
                int(report.time),
                crate::pyutil::pydict(vec![
                    ("killMailID", int(report.kill_id)),
                    ("killMailHash", PyValue::Unicode(store::kill_hash(&report))),
                    ("victimShipTypeID", int(HULL)),
                ]),
            ]
        )));

        let opened = reg
            .dispatch(&Request {
                service: "warStatisticMgr",
                method: "GetKillMail",
                args: &[
                    int(report.kill_id),
                    PyValue::Unicode(store::kill_hash(&report)),
                ],
                kwargs: &[],
                bind_params: None,
                actor: Some(BOB),
            })
            .expect("answered");
        assert_eq!(attr(&opened, "killID"), &int(report.kill_id));
    }

    /// **The window's whole cycle** over `notificationMgr`: list (all, by
    /// group, unread), read, delete, with the rows the client reads by
    /// attribute; a stranger's calls change nothing of Alice's.
    #[test]
    fn list_read_and_delete_over_the_service() {
        let mut reg = docked();
        let war = store::notify(&mut reg, ALICE, kind::KILL_REPORT_VICTIM, Vec::new()).unwrap();
        let corp = store::notify_from(
            &mut reg,
            ALICE,
            ck::CORP_KICKED,
            BOB,
            data([("corpID", Value::Int(42))]),
        )
        .unwrap();
        let none = store::notify(&mut reg, ALICE, SOME_TYPE, Vec::new()).unwrap();
        assert!(store::notify(&mut reg, 1_234, SOME_TYPE, Vec::new()).is_none());

        let from = [(int(0), crate::pyutil::bstr("fromID"))];
        let all = list(call_kw(&mut reg, ALICE, "GetAllNotifications", &[], &from));
        assert_eq!(all.len(), 3);
        let row = &all[1];
        assert_eq!(attr(row, "notificationID"), &int(corp));
        assert_eq!(attr(row, "typeID"), &int(ck::CORP_KICKED));
        assert_eq!(attr(row, "senderID"), &int(BOB));
        assert_eq!(attr(row, "receiverID"), &int(ALICE));
        assert_eq!(attr(row, "processed"), &PyValue::Bool(false));
        assert!(matches!(attr(row, "created"), PyValue::Int(t) if *t > 0));
        assert_eq!(
            crate::pyutil::dict_get(attr(row, "data"), b"corpID"),
            Some(&int(42))
        );
        let after = [(int(corp), crate::pyutil::bstr("fromID"))];
        assert_eq!(
            list(call_kw(&mut reg, ALICE, "GetAllNotifications", &[], &after)).len(),
            1,
            "only the id above fromID"
        );
        assert!(list(call(&mut reg, BOB, "GetAllNotifications", &[])).is_empty());

        let by = |reg: &mut Registry, g: i64| list(call(reg, ALICE, "GetByGroupID", &[int(g)]));
        assert_eq!(by(&mut reg, group::WAR).len(), 1);
        assert_eq!(by(&mut reg, group::CORP).len(), 1);
        assert!(by(&mut reg, group::SOV).is_empty());

        assert_eq!(
            call(&mut reg, BOB, "MarkAsProcessed", &[ids(&[war, corp])]),
            PyValue::None
        );
        assert_eq!(
            list(call(&mut reg, ALICE, "GetUnprocessed", &[])).len(),
            3,
            "Bob cannot read Alice's"
        );
        call(&mut reg, ALICE, "MarkAsProcessed", &[ids(&[war])]);
        assert_eq!(list(call(&mut reg, ALICE, "GetUnprocessed", &[])).len(), 2);
        call(&mut reg, ALICE, "MarkGroupAsProcessed", &[int(group::CORP)]);
        let unread = list(call(&mut reg, ALICE, "GetUnprocessed", &[]));
        assert_eq!(unread.len(), 1);
        assert_eq!(attr(&unread[0], "notificationID"), &int(none));
        call(&mut reg, ALICE, "MarkAllAsProcessed", &[]);
        assert!(list(call(&mut reg, ALICE, "GetUnprocessed", &[])).is_empty());
        assert_eq!(
            attr(&by(&mut reg, group::WAR)[0], "processed"),
            &PyValue::Bool(true)
        );

        call(&mut reg, BOB, "DeleteAllNotifications", &[]);
        assert_eq!(store::of(reg.deltas(), ALICE).len(), 3);
        call(
            &mut reg,
            ALICE,
            "DeleteNotifications",
            &[PyValue::Tuple(vec![int(war)])],
        );
        assert!(by(&mut reg, group::WAR).is_empty());
        call(
            &mut reg,
            ALICE,
            "DeleteGroupNotifications",
            &[int(group::CORP)],
        );
        assert_eq!(store::of(reg.deltas(), ALICE).len(), 1);
        call(&mut reg, ALICE, "DeleteAllNotifications", &[]);
        assert!(list(call(&mut reg, ALICE, "GetAllNotifications", &[])).is_empty());
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "LogNotificationInteraction",
                &[PyValue::None]
            ),
            PyValue::None
        );
    }

    /// Junk arguments are no-ops, never errors: a missing group, a list of
    /// strings, a string where the list goes.
    #[test]
    fn bad_arguments_change_nothing() {
        let mut reg = docked();
        store::notify(&mut reg, ALICE, ck::CORP_LEFT, Vec::new()).unwrap();
        for (method, args) in [
            ("MarkAsProcessed", vec![crate::pyutil::bstr("x")]),
            ("MarkAsProcessed", vec![PyValue::List(vec![PyValue::None])]),
            ("DeleteNotifications", vec![]),
            ("DeleteGroupNotifications", vec![PyValue::None]),
            ("MarkGroupAsProcessed", vec![]),
        ] {
            assert_eq!(call(&mut reg, ALICE, method, &args), PyValue::None);
        }
        assert_eq!(store::unread(reg.deltas(), ALICE).len(), 1);
        assert!(list(call(&mut reg, ALICE, "GetByGroupID", &[])).is_empty());
    }

    /// Insurance pays (CP44): a contract on Bob's ship pays out when an NPC
    /// kills it, and Bob holds the payout notification with the amount the
    /// journal booked; selling the contract told him too.
    #[test]
    fn insurance_issued_and_paid() {
        let mut reg = docked();
        let c = crate::service::insurance::Contract {
            ship_id: BOB_SHIP,
            owner_id: BOB,
            type_id: HULL,
            fraction: 0.7,
            start: 10,
            end: 20,
        };
        store::insurance_issued(&mut reg, &c);
        store::insurance_payout(&mut reg, BOB, BOB_SHIP, 1234.5);
        let held = store::of(reg.deltas(), BOB);
        assert_eq!(held.len(), 2);
        assert_eq!(held[0].type_id, kind::INSURANCE_PAYOUT);
        assert_eq!(
            held[0].data,
            data([
                ("amount", Value::Real(1234.5)),
                ("itemID", Value::Int(BOB_SHIP)),
                ("payout", Value::Bool(false)),
            ])
        );
        assert_eq!(held[1].type_id, kind::INSURANCE_ISSUED);
        let issued: std::collections::BTreeMap<_, _> = held[1].data.iter().cloned().collect();
        assert_eq!(issued["level"], Value::Int(70));
        assert_eq!(issued["itemID"], Value::Int(BOB_SHIP));
        assert_eq!(issued["typeID"], Value::Int(HULL));
        assert_eq!(issued["endDate"], Value::Int(20));
        assert!(issued.contains_key("shipName"));
        assert!(issued.contains_key("numWeeks"));
    }

    /// FW: a promotion and a demotion are told, a first sighting is not.
    #[test]
    fn militia_rank_changes() {
        let mut reg = docked();
        const FACTION: i64 = 500_901;
        store::militia_rank(&mut reg, ALICE, FACTION, None, 2);
        store::militia_rank(&mut reg, ALICE, FACTION, Some(2), 2);
        assert!(store::of(reg.deltas(), ALICE).is_empty());
        store::militia_rank(&mut reg, ALICE, FACTION, Some(2), 3);
        store::militia_rank(&mut reg, ALICE, FACTION, Some(3), 1);
        assert_eq!(
            held(&reg, ALICE),
            vec![
                (kind::FW_CHAR_RANK_LOSS, FACTION),
                (kind::FW_CHAR_RANK_GAIN, FACTION)
            ]
        );
        assert_eq!(
            store::of(reg.deltas(), ALICE)[1].data,
            data([
                ("factionID", Value::Int(FACTION)),
                ("newRank", Value::Int(3)),
            ])
        );
    }

    /// A player corp with Alice as CEO and Bob as a plain member, made
    /// through the client's own call.
    fn with_corp(reg: &mut Registry) -> i64 {
        reg.character_mut(ALICE).unwrap().balance = 1.0e10;
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &[
                PyValue::Unicode("Notified Ltd".to_owned()),
                PyValue::Unicode("NTFD".to_owned()),
                PyValue::Unicode(String::new()),
                PyValue::Unicode(String::new()),
            ],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("the corp is founded");
        let corp = reg.character(ALICE).unwrap().corporation_id;
        enrol(reg, BOB, corp, 0);
        corp
    }

    /// Put `who` in `corp` with `roles`, as the world file's `corpmember`
    /// record would.
    fn enrol(reg: &mut Registry, who: i64, corp: i64, roles: i64) {
        let (who, corp, roles) = (who.to_string(), corp.to_string(), roles.to_string());
        reg.corporations
            .parse_record(&["corpmember", &who, &corp, &roles, "1"])
            .expect("a membership record");
    }

    /// **Corp applications**, every row of `corp_application`'s table: who
    /// is told, with what type and from whom; a pilot's own move never
    /// tells them as a manager.
    #[test]
    fn corp_applications_tell_the_right_side() {
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        assert_eq!(corp_managers(&reg, corp), vec![ALICE], "the CEO, not Bob");

        corp_application(&mut reg, CAROL, CAROL, corp, None, app::APPLIED, "hi", "");
        assert_eq!(held(&reg, ALICE), vec![(ck::CORP_APP_NEW, CAROL)]);
        assert!(held(&reg, BOB).is_empty(), "a plain member is not told");
        let n = &store::of(reg.deltas(), ALICE)[0];
        assert_eq!(
            n.data,
            data([
                ("charID", Value::Int(CAROL)),
                ("corpID", Value::Int(corp)),
                ("applicationText", Value::Text("hi".to_owned())),
            ])
        );

        let pilot = |reg: &mut Registry, from, to, custom: &str| {
            let _ = store::delete(reg.deltas_mut(), CAROL, Selection::All);
            corp_application(reg, ALICE, CAROL, corp, from, to, "", custom);
            held(reg, CAROL)
        };
        assert_eq!(
            pilot(&mut reg, None, app::INVITED_BY_CORPORATION, ""),
            vec![(ck::CORP_APP_INVITED, corp)]
        );
        assert_eq!(
            store::of(reg.deltas(), CAROL)[0].data[2],
            ("invokingCharID".to_owned(), Value::Int(ALICE))
        );
        assert_eq!(
            pilot(
                &mut reg,
                Some(app::APPLIED),
                app::ACCEPTED_BY_CORPORATION,
                ""
            ),
            vec![(ck::CORP_APP_ACCEPT, corp)]
        );
        assert_eq!(
            pilot(
                &mut reg,
                Some(app::APPLIED),
                app::REJECTED_BY_CORPORATION,
                ""
            ),
            vec![(ck::CORP_APP_REJECT, corp)]
        );
        assert_eq!(
            pilot(
                &mut reg,
                Some(app::RENEGOTIATED),
                app::REJECTED_BY_CORPORATION,
                "no"
            ),
            vec![(ck::CORP_APP_REJECT_CUSTOM, corp)]
        );
        assert_eq!(
            store::of(reg.deltas(), CAROL)[0].data[2],
            ("customMessage".to_owned(), Value::Text("no".to_owned()))
        );
        assert!(
            pilot(
                &mut reg,
                Some(app::INVITED_BY_CORPORATION),
                app::REJECTED_BY_CORPORATION,
                ""
            )
            .is_empty(),
            "a withdrawn invitation tells nobody"
        );

        for (to, type_id) in [
            (app::ACCEPTED_BY_CHARACTER, ck::CHAR_APP_ACCEPT),
            (app::REJECTED_BY_CHARACTER, ck::CHAR_APP_REJECT),
            (app::WITHDRAWN_BY_CHARACTER, ck::CHAR_APP_WITHDRAW),
        ] {
            let _ = store::delete(reg.deltas_mut(), ALICE, Selection::All);
            corp_application(
                &mut reg,
                CAROL,
                CAROL,
                corp,
                Some(app::APPLIED),
                to,
                "t",
                "",
            );
            assert_eq!(held(&reg, ALICE), vec![(type_id, CAROL)], "status {to}");
        }

        // The CEO applying elsewhere is not told as a manager of their own.
        let _ = store::delete(reg.deltas_mut(), ALICE, Selection::All);
        corp_application(
            &mut reg,
            ALICE,
            ALICE,
            corp,
            Some(app::APPLIED),
            app::WITHDRAWN_BY_CHARACTER,
            "",
            "",
        );
        assert!(held(&reg, ALICE).is_empty());
    }

    /// **Leaving a corp**: quitting and being kicked tell the pilot
    /// different things, the managers the same thing.
    #[test]
    fn leaving_a_corp_tells_the_pilot_and_the_managers() {
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        corp_member_left(&mut reg, BOB, corp, false);
        assert_eq!(held(&reg, BOB), vec![(ck::CORP_LEFT, corp)]);
        assert_eq!(held(&reg, ALICE), vec![(ck::CHAR_LEFT_CORP, BOB)]);
        corp_member_left(&mut reg, BOB, corp, true);
        assert_eq!(held(&reg, BOB)[0], (ck::CORP_KICKED, corp));
        assert_eq!(held(&reg, ALICE).len(), 2);
        assert_eq!(
            store::of(reg.deltas(), BOB)[0].data,
            data([("corpID", Value::Int(corp))])
        );
    }

    /// **A new CEO** is announced to every member, naming both CEOs.
    #[test]
    fn a_new_ceo_is_told_to_every_member() {
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        corp_new_ceo(&mut reg, corp, ALICE, BOB);
        for who in [ALICE, BOB] {
            assert_eq!(held(&reg, who), vec![(ck::CORP_NEW_CEO, corp)], "{who}");
        }
        assert!(held(&reg, CAROL).is_empty());
        assert_eq!(
            store::of(reg.deltas(), BOB)[0].data,
            data([
                ("corpID", Value::Int(corp)),
                ("oldCeoID", Value::Int(ALICE)),
                ("newCeoID", Value::Int(BOB)),
            ])
        );
    }

    /// **Militia requests**: every member of the corp is told, from the
    /// faction, with the request's type; the CEO of a one-man corp too.
    #[test]
    fn militia_requests_tell_every_member() {
        use facwar_mgr::CorpOp;
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        const FACTION: i64 = 500_902;
        for (op, type_id) in [
            (CorpOp::Join, kind::FW_CORP_JOIN_REQUEST),
            (CorpOp::WithdrawJoin, kind::FW_CORP_JOIN_WITHDRAW),
            (CorpOp::Leave, kind::FW_CORP_LEAVE_REQUEST),
            (CorpOp::WithdrawLeave, kind::FW_CORP_LEAVE_WITHDRAW),
        ] {
            militia_request(&mut reg, corp, FACTION, op);
            for who in [ALICE, BOB] {
                assert_eq!(held(&reg, who)[0], (type_id, FACTION), "{op:?} {who}");
            }
        }
        assert!(held(&reg, CAROL).is_empty(), "not a member");
        assert_eq!(
            store::of(reg.deltas(), BOB)[0].data,
            data([
                ("corpID", Value::Int(corp)),
                ("factionID", Value::Int(FACTION)),
            ])
        );
    }

    /// An alliance's request reaches the members of its corporations, and
    /// names the alliance as `corpID`.
    #[test]
    fn an_alliances_militia_request_reaches_its_corps_members() {
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        // CORP-W: the alliance fee comes from the corp's Master Wallet.
        let to_master = [(PyValue::Int(1000), PyValue::Bytes(b"toAccountKey".to_vec()))];
        reg.dispatch(&Request {
            service: "account",
            method: "GiveCash",
            args: &[
                PyValue::Int(corp),
                PyValue::Real(1_500_000_000.0),
                PyValue::Unicode("test".to_owned()),
            ],
            kwargs: &to_master,
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("the deposit");
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "CreateAlliance",
            args: &[
                PyValue::Unicode("Notified Pact".to_owned()),
                PyValue::Unicode("NTFP".to_owned()),
                PyValue::Unicode(String::new()),
                PyValue::Unicode(String::new()),
            ],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("the alliance is created");
        let alliance = crate::alliance::of_corp(reg.deltas(), corp).expect("in an alliance");
        const FACTION: i64 = 500_903;
        militia_request(&mut reg, alliance, FACTION, facwar_mgr::CorpOp::Join);
        for who in [ALICE, BOB] {
            assert_eq!(
                held(&reg, who),
                vec![(kind::FW_CORP_JOIN_REQUEST, FACTION)],
                "{who}"
            );
        }
        assert!(held(&reg, CAROL).is_empty());
        assert_eq!(
            store::of(reg.deltas(), BOB)[0].data[0],
            ("corpID".to_owned(), Value::Int(alliance))
        );
    }

    /// The roles `corp_managers` reads are the corp module's own.
    #[test]
    fn a_director_is_a_manager() {
        let mut reg = docked();
        let corp = with_corp(&mut reg);
        enrol(&mut reg, CAROL, corp, corp::CORP_ROLE_PERSONNEL_MANAGER);
        assert_eq!(corp_managers(&reg, corp), vec![ALICE, CAROL]);
        enrol(&mut reg, BOB, corp, corp::CORP_ROLE_DIRECTOR);
        assert_eq!(corp_managers(&reg, corp), vec![ALICE, BOB, CAROL]);
    }
}
