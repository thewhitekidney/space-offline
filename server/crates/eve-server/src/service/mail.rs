//! **The mail wire** (MAIL-1): `mailMgr` and `mailingListsMgr`, the two
//! services behind the mail window. The store, the sending and the pushes are
//! `crate::mail`'s; lane note `docs/lanes/mail-1.md`.
//!
//! Every call is `mailSvc`'s or `mailingListsSvc`'s own **[V]**; the answers
//! are the shapes those read (the module doc of `crate::mail` has the
//! `SyncMail` contract).
//!
//! | `mailMgr` call | answer |
//! | --- | --- |
//! | `SyncMail(firstID, lastID)` | `KeyVal(newMail: list, oldMail: None or list, mailStatus: list)` |
//! | `GetMailHeaders(ids)` | list of header rows |
//! | `GetBody(id, markAsRead)` | zlib stream of the UTF-8 body, `None` for a mail not held |
//! | `SendMail(toChars, toListID, toCorpOrAllianceID, title, body, replyTo, fwdFrom)` | the new id |
//! | `MarkAsRead/Unread`, `MoveToTrash`, `MoveFromTrash`, `DeleteMail` (ids) | discarded |
//! | `...ByLabel(labelID)`, `...ByList(listID)`, `MarkAll...`, `MoveAll...`, `EmptyTrash` | discarded |
//! | `GetLabels()` | dict `labelID -> KeyVal(labelID, name, color)` |
//! | `CreateLabel(name, color)` | the new label id |
//! | `EditLabel`, `DeleteLabel`, `AssignLabels`, `RemoveLabels`, `ReplaceLabels` | discarded |
//!
//! `SendMail` may also carry the keyword `approvedCost` (the client's CSPA
//! retry, `eveMisc.CSPAChargedAction`): nothing here costs ISK, so it is
//! never asked for and ignored if sent.
//!
//! Each call acts on the caller's own mailbox only: the store's key carries the
//! holder, so an id that is somebody else's matches nothing. An id list is
//! cut at [`MAX_IDS`].

use super::*;
use crate::mail::{self as store, Draft, ListRefusal, Refusal, role, status};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["mailMgr", "mailingListsMgr"];

/// The most ids one call acts on: the client pages its mail by 100 and a
/// longer list is not the client's.
pub const MAX_IDS: usize = 1_000;

fn refuse(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// `CustomInfo {info}`: the client's own plain-text dialog (`TrySyncMail`
/// raises it) **[V]**.
fn info(text: &str) -> ServiceError {
    refuse(
        "CustomInfo",
        vec![("info", PyValue::Unicode(text.to_owned()))],
    )
}

fn refusal(r: &Refusal) -> ServiceError {
    match r {
        // The client's own keys **[V]** (`evemail`, `mailSvc`).
        Refusal::NoSubject => refuse("NoSubject", vec![]),
        Refusal::NoRecipient => refuse("EvemailSendingFailed", vec![]),
        Refusal::NotInList => refuse("EveMailCanOnlySendToOwnMailinglists", vec![]),
        Refusal::UnknownList => refuse("MailingListNoSuchList", vec![]),
        Refusal::TooMany => info(&format!(
            "You can send a mail to at most {} recipients.",
            store::MAX_RECIPIENTS
        )),
        Refusal::SubjectTooLong => info(&format!(
            "The subject can be at most {} characters.",
            store::MAX_SUBJECT
        )),
        Refusal::BodyTooLong => info(&format!(
            "The mail can be at most {} characters.",
            store::MAX_BODY
        )),
        Refusal::UnknownRecipient(c) => info(&format!("There is no character {c} to mail.")),
        Refusal::Muted => info("You are muted on that mailing list."),
        Refusal::NotYourCorp => info("You can only mail your own corporation."),
        Refusal::NotYourAlliance => {
            info("Only the CEO or a director of a corporation in the alliance can mail it.")
        }
    }
}

fn list_refusal(r: ListRefusal) -> ServiceError {
    match r {
        ListRefusal::NoSuchList => refuse("MailingListNoSuchList", vec![]),
        ListRefusal::BadName => info("That is not a name for a mailing list."),
        ListRefusal::NameTaken => info("There is already a mailing list with that name."),
        ListRefusal::NotAMember => info("You are not on that mailing list."),
        ListRefusal::Blocked => info("You are not allowed to join that mailing list."),
        ListRefusal::Full => info("That mailing list is full."),
        ListRefusal::OwnerCannotLeave => {
            info("The owner cannot leave a mailing list; delete it instead.")
        }
    }
}

/// Positional `i`, or the keyword `name`.
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn truthy(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

fn text(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

/// The ints in a list or tuple at `i`, cut at [`MAX_IDS`]; a lone int is a
/// list of one.
fn ids(req: &Request<'_>, i: usize, name: &str) -> Vec<i64> {
    match arg(req, i, name) {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|x| int(Some(x)))
            .take(MAX_IDS)
            .collect(),
        Some(PyValue::Int(n)) => vec![*n],
        _ => Vec::new(),
    }
}

fn dict_of(rows: Vec<(i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        rows.into_iter()
            .map(|(k, v)| (v, PyValue::Int(k)))
            .collect(),
    )
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        // Nobody: the empty answers a fresh character gets.
        return match (req.service, req.method) {
            ("mailMgr", "GetLabels") | ("mailingListsMgr", "GetJoinedLists") => {
                Ok(PyValue::Dict(Vec::new()))
            }
            ("mailMgr", "SyncMail") => Ok(sync_answer(Vec::new(), None, Vec::new())),
            ("mailMgr", "GetMailHeaders") => Ok(PyValue::List(Vec::new())),
            ("mailMgr", "GetBody") => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        };
    };
    match req.service {
        "mailMgr" => mail_mgr(reg, req, me),
        "mailingListsMgr" => lists_mgr(reg, req, me),
        _ => Err(reg.no_method(req)),
    }
}

fn sync_answer(new: Vec<PyValue>, old: Option<Vec<PyValue>>, status: Vec<PyValue>) -> PyValue {
    crate::pyutil::keyval(vec![
        ("newMail", PyValue::List(new)),
        ("oldMail", old.map_or(PyValue::None, PyValue::List)),
        ("mailStatus", PyValue::List(status)),
    ])
}

/// The label bits `who` may file mail under: the system labels and their own.
fn fileable(reg: &Registry, who: i64) -> i64 {
    store::labels(reg.deltas(), who).iter().fold(
        store::label::INBOX
            | store::label::SENT
            | store::label::CORPORATION
            | store::label::ALLIANCE,
        |m, l| m | l.id,
    )
}

fn mail_mgr(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let read = |s: &store::Slot| (s.status | status::READ, s.labels);
    let unread = |s: &store::Slot| (s.status & !status::READ, s.labels);
    let trash = |s: &store::Slot| (s.status | status::TRASHED, s.labels);
    // The client's own local view of a restored mail: not trashed, unread
    // (`mailSvc._RefreshTrashedStatusWhenMovingFromTrash`) **[V]**.
    let restore = |s: &store::Slot| (s.status & !(status::TRASHED | status::READ), s.labels);
    let live = |s: &store::Slot| !s.trashed();
    match req.method {
        "GetLabels" => Ok(dict_of(
            store::labels(reg.deltas(), me)
                .iter()
                .map(|l| (l.id, l.row()))
                .collect(),
        )),
        "SyncMail" => {
            let first = int(arg(req, 0, "firstID"));
            let last = int(arg(req, 1, "lastID")).unwrap_or(0);
            let d = reg.deltas();
            let (mut new, mut old, mut all) = (Vec::new(), Vec::new(), Vec::new());
            for s in store::slots(d, me) {
                let Some(m) = store::message(d, s.id) else {
                    continue;
                };
                all.push(s.row());
                if s.id > last {
                    new.push(m.header());
                } else if first.is_some_and(|f| s.id < f) {
                    old.push(m.header());
                }
            }
            Ok(sync_answer(new, first.map(|_| old), all))
        }
        "GetMailHeaders" => {
            let d = reg.deltas();
            Ok(PyValue::List(
                ids(req, 0, "messageIDs")
                    .into_iter()
                    .filter(|id| store::slot(d, me, *id).is_some())
                    .filter_map(|id| store::message(d, id))
                    .map(|m| m.header())
                    .collect(),
            ))
        }
        "GetBody" => {
            let Some(id) = int(arg(req, 0, "messageID")) else {
                return Ok(PyValue::None);
            };
            let d = reg.deltas_mut();
            let (Some(_), Some(m)) = (store::slot(d, me, id), store::message(d, id)) else {
                return Ok(PyValue::None);
            };
            if truthy(arg(req, 1, "markAsRead")) {
                store::update_slots(d, me, &[id], read);
            }
            Ok(PyValue::Bytes(store::zlib_stored(m.body.as_bytes())))
        }
        "SendMail" => {
            let draft = Draft {
                sender: me,
                to_chars: ids(req, 0, "toCharacterIDs"),
                to_list: crate::pyutil::opt_id(arg(req, 1, "toListID")),
                to_corp: match arg(req, 2, "toCorpOrAllianceID") {
                    // The compose window sends one id; a reply built from a
                    // header carries it in a list **[V]**.
                    Some(PyValue::List(v) | PyValue::Tuple(v)) => crate::pyutil::opt_id(v.first()),
                    other => crate::pyutil::opt_id(other),
                },
                title: text(arg(req, 3, "title")).unwrap_or_default(),
                body: text(arg(req, 4, "body")).unwrap_or_default(),
                reply_to: int(arg(req, 5, "isReplyTo")).unwrap_or(0),
                forwarded_from: int(arg(req, 6, "isForwardedFrom")).unwrap_or(0),
            };
            store::send(reg, draft)
                .map(PyValue::Int)
                .map_err(|r| refusal(&r))
        }
        "MarkAsRead" => touch(reg, me, &ids(req, 0, "messageIDs"), read),
        "MarkAsUnread" => touch(reg, me, &ids(req, 0, "messageIDs"), unread),
        "MoveToTrash" => touch(reg, me, &ids(req, 0, "messageIDs"), trash),
        "MoveFromTrash" => touch(reg, me, &ids(req, 0, "messageIDs"), restore),
        "DeleteMail" => {
            store::delete(reg.deltas_mut(), me, &ids(req, 0, "messageIDs"));
            Ok(PyValue::None)
        }
        "MarkAllAsRead" => by(reg, me, live, read),
        "MarkAllAsUnread" => by(reg, me, live, unread),
        "MoveAllToTrash" => by(reg, me, |_| true, trash),
        "MoveAllFromTrash" => by(reg, me, store::Slot::trashed, restore),
        "EmptyTrash" => {
            let d = reg.deltas_mut();
            let gone: Vec<i64> = store::slots(d, me)
                .iter()
                .filter(|s| s.trashed())
                .map(|s| s.id)
                .collect();
            store::delete(d, me, &gone);
            Ok(PyValue::None)
        }
        "MarkAsReadByLabel" | "MarkAsUnreadByLabel" | "MoveToTrashByLabel" => {
            let label = int(arg(req, 0, "labelID")).unwrap_or(0);
            let hit = move |s: &store::Slot| label != 0 && s.labels & label == label;
            match req.method {
                "MarkAsReadByLabel" => by(reg, me, move |s| live(s) && hit(s), read),
                "MarkAsUnreadByLabel" => by(reg, me, move |s| live(s) && hit(s), unread),
                _ => by(reg, me, hit, trash),
            }
        }
        "MarkAsReadByList" | "MarkAsUnreadByList" | "MoveToTrashByList" => {
            let list = int(arg(req, 0, "listID")).unwrap_or(0);
            let d = reg.deltas();
            let in_list: Vec<i64> = store::slots(d, me)
                .iter()
                .filter(|s| store::message(d, s.id).is_some_and(|m| m.to_list == Some(list)))
                .map(|s| s.id)
                .collect();
            match req.method {
                "MarkAsReadByList" => touch_live(reg, me, &in_list, read),
                "MarkAsUnreadByList" => touch_live(reg, me, &in_list, unread),
                _ => touch(reg, me, &in_list, trash),
            }
        }
        "CreateLabel" => {
            let name = text(arg(req, 0, "name")).unwrap_or_default();
            let color = int(arg(req, 1, "color"));
            store::create_label(reg.deltas_mut(), me, &name, color)
                .map(PyValue::Int)
                .ok_or_else(|| info("That label could not be created."))
        }
        "EditLabel" => {
            if let Some(id) = int(arg(req, 0, "labelID")) {
                let name = text(arg(req, 1, "name"));
                let color = int(arg(req, 2, "color"));
                store::edit_label(reg.deltas_mut(), me, id, name.as_deref(), color);
            }
            Ok(PyValue::None)
        }
        "DeleteLabel" => {
            if let Some(id) = int(arg(req, 0, "labelID")) {
                store::delete_label(reg.deltas_mut(), me, id);
            }
            Ok(PyValue::None)
        }
        "AssignLabels" | "RemoveLabels" | "ReplaceLabels" => {
            let held = ids(req, 0, "messageIDs");
            let mask = int(arg(req, 1, "labelID")).unwrap_or(0) & fileable(reg, me);
            let d = reg.deltas_mut();
            match req.method {
                "AssignLabels" => {
                    store::update_slots(d, me, &held, |s| (s.status, s.labels | mask));
                }
                "RemoveLabels" => {
                    store::update_slots(d, me, &held, |s| (s.status, s.labels & !mask));
                }
                _ => {
                    store::update_slots(d, me, &held, |s| (s.status, mask));
                }
            }
            Ok(PyValue::None)
        }
        // `xmppchatsvc.PokePlayerAboutChatMsgGm`: a GM nudge, not mail.
        "PokePlayerAboutChatMsgGm" => Ok(PyValue::None),
        _ => Err(reg.no_method(req)),
    }
}

fn touch(
    reg: &mut Registry,
    me: i64,
    ids: &[i64],
    f: impl Fn(&store::Slot) -> (i64, i64),
) -> Result<PyValue, ServiceError> {
    store::update_slots(reg.deltas_mut(), me, ids, f);
    Ok(PyValue::None)
}

fn touch_live(
    reg: &mut Registry,
    me: i64,
    ids: &[i64],
    f: impl Fn(&store::Slot) -> (i64, i64),
) -> Result<PyValue, ServiceError> {
    let d = reg.deltas_mut();
    let live: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| store::slot(d, me, *id).is_some_and(|s| !s.trashed()))
        .collect();
    store::update_slots(d, me, &live, f);
    Ok(PyValue::None)
}

fn by(
    reg: &mut Registry,
    me: i64,
    pick: impl Fn(&store::Slot) -> bool,
    f: impl Fn(&store::Slot) -> (i64, i64),
) -> Result<PyValue, ServiceError> {
    store::update_where(reg.deltas_mut(), me, pick, f);
    Ok(PyValue::None)
}

// ---- mailing lists -------------------------------------------------------------------------

/// The character, their corporation and their alliance: the owners a list's
/// access overrides name.
fn owners(reg: &Registry, me: i64) -> Vec<i64> {
    let mut v = vec![me];
    if let Some(m) = reg.corporations.membership(me) {
        v.push(m.corporation_id);
        v.extend(crate::alliance::of_corp(reg.deltas(), m.corporation_id));
    }
    v
}

/// The list `listID` and the caller's role in it, for a call that needs
/// them to be a member.
fn member_of(
    reg: &Registry,
    req: &Request<'_>,
    me: i64,
) -> Result<(store::List, i64), ServiceError> {
    let id = int(arg(req, 0, "listID")).ok_or_else(|| list_refusal(ListRefusal::NoSuchList))?;
    let d = reg.deltas();
    let l = store::get_list(d, id).ok_or_else(|| list_refusal(ListRefusal::NoSuchList))?;
    let r = store::role_of(d, id, me).ok_or_else(|| list_refusal(ListRefusal::NotAMember))?;
    Ok((l, r))
}

/// An owner-only call's list.
fn owned(reg: &Registry, req: &Request<'_>, me: i64) -> Result<store::List, ServiceError> {
    let (l, r) = member_of(reg, req, me)?;
    if r == role::OWNER {
        Ok(l)
    } else {
        Err(info("Only the owner of a mailing list can do that."))
    }
}

/// A call the owner and the operators may make.
fn managed(reg: &Registry, req: &Request<'_>, me: i64) -> Result<store::List, ServiceError> {
    let (l, r) = member_of(reg, req, me)?;
    if r == role::OWNER || r == role::OPERATOR {
        Ok(l)
    } else {
        Err(info(
            "Only the owner or an operator of a mailing list can do that.",
        ))
    }
}

fn lists_mgr(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetJoinedLists" => Ok(dict_of(
            store::joined(reg.deltas(), me)
                .iter()
                .map(|(l, r)| (l.id, store::joined_row(l, *r)))
                .collect(),
        )),
        "Create" => {
            let name = text(arg(req, 0, "name")).unwrap_or_default();
            let default_access =
                int(arg(req, 1, "defaultAccess")).unwrap_or(store::access::ALLOWED);
            let default_member = int(arg(req, 2, "defaultMemberAccess")).unwrap_or(role::DEFAULT);
            let cost = int(arg(req, 3, "cost")).unwrap_or(0);
            store::create_list(
                reg.deltas_mut(),
                me,
                &name,
                default_access,
                default_member,
                cost,
            )
            .map(PyValue::Int)
            .map_err(list_refusal)
        }
        "Join" => {
            let name = text(arg(req, 0, "name")).unwrap_or_default();
            let who = owners(reg, me);
            let already = store::list_named(reg.deltas(), &name)
                .is_some_and(|l| store::role_of(reg.deltas(), l.id, me).is_some());
            let (l, r) = store::join(reg.deltas_mut(), me, &who, &name).map_err(list_refusal)?;
            // A new member is sent the list's welcome mail.
            if !already
                && let Some((title, body)) = &l.welcome
                && !title.is_empty()
            {
                store::deliver_automated(reg, l.id, &[me], title, body);
            }
            Ok(store::joined_row(&l, r))
        }
        "Leave" => {
            let id = int(arg(req, 0, "listID")).unwrap_or(0);
            store::leave(reg.deltas_mut(), me, id).map_err(list_refusal)?;
            Ok(PyValue::None)
        }
        "Delete" => {
            let l = owned(reg, req, me)?;
            let members = store::delete_list(reg.deltas_mut(), l.id);
            for c in members.into_iter().filter(|c| *c != me) {
                store::tell(reg, c, "OnMailingListDeleted", vec![l.id]);
            }
            Ok(PyValue::None)
        }
        "GetInfo" => Ok(int(arg(req, 0, "listID"))
            .and_then(|id| store::get_list(reg.deltas(), id))
            .map_or(PyValue::None, |l| l.info())),
        "GetMembers" => {
            let (l, _) = member_of(reg, req, me)?;
            Ok(dict_of(
                store::members(reg.deltas(), l.id)
                    .into_iter()
                    .map(|(c, r)| (c, PyValue::Int(r)))
                    .collect(),
            ))
        }
        "GetSettings" => {
            let (l, _) = member_of(reg, req, me)?;
            Ok(l.settings(&store::overrides(reg.deltas(), l.id)))
        }
        "SetDefaultAccess" => {
            let mut l = owned(reg, req, me)?;
            l.default_access = int(arg(req, 1, "defaultAccess"))
                .unwrap_or(l.default_access)
                .clamp(store::access::BLOCKED, store::access::ALLOWED);
            l.default_member_access = int(arg(req, 2, "defaultMemberAccess"))
                .unwrap_or(l.default_member_access)
                .clamp(role::MUTED, role::OPERATOR);
            l.cost = int(arg(req, 3, "mailCost")).unwrap_or(l.cost).max(0);
            store::save_list(reg.deltas_mut(), &l);
            Ok(PyValue::None)
        }
        "SetEntitiesAccess" => {
            let l = managed(reg, req, me)?;
            if let Some(PyValue::Dict(rows)) = arg(req, 1, "accessByEntityID") {
                for (a, e) in rows {
                    if let (Some(a), Some(e)) = (int(Some(a)), int(Some(e))) {
                        store::set_access(reg.deltas_mut(), l.id, e, a);
                    }
                }
            }
            Ok(PyValue::None)
        }
        "ClearEntityAccess" => {
            let l = managed(reg, req, me)?;
            if let Some(e) = int(arg(req, 1, "entityID")) {
                store::clear_access(reg.deltas_mut(), l.id, e);
            }
            Ok(PyValue::None)
        }
        "SetMembersMuted" | "SetMembersClear" => {
            let l = managed(reg, req, me)?;
            let (new, event) = if req.method == "SetMembersMuted" {
                (role::MUTED, "OnMailingListSetMuted")
            } else {
                (role::DEFAULT, "OnMailingListSetClear")
            };
            let done = store::set_roles(reg.deltas_mut(), l.id, &ids(req, 1, "memberIDs"), new);
            for c in done {
                store::tell(reg, c, event, vec![l.id]);
            }
            Ok(PyValue::None)
        }
        "SetMembersOperator" => {
            let l = owned(reg, req, me)?;
            let done = store::set_roles(
                reg.deltas_mut(),
                l.id,
                &ids(req, 1, "memberIDs"),
                role::OPERATOR,
            );
            for c in done {
                store::tell(reg, c, "OnMailingListSetOperator", vec![l.id]);
            }
            Ok(PyValue::None)
        }
        "KickMembers" => {
            let l = managed(reg, req, me)?;
            let done = store::kick(reg.deltas_mut(), l.id, &ids(req, 1, "memberIDs"));
            for c in done {
                store::tell(reg, c, "OnMailingListLeave", vec![l.id, c]);
            }
            Ok(PyValue::None)
        }
        "GetWelcomeMail" => {
            let (l, _) = member_of(reg, req, me)?;
            Ok(PyValue::List(
                l.welcome
                    .iter()
                    .map(|(t, b)| {
                        crate::pyutil::keyval(vec![
                            ("title", PyValue::Unicode(t.clone())),
                            ("body", PyValue::Unicode(b.clone())),
                        ])
                    })
                    .collect(),
            ))
        }
        "SaveWelcomeMail" | "SendWelcomeMail" => {
            let mut l = owned(reg, req, me)?;
            let title: String = text(arg(req, 1, "title"))
                .unwrap_or_default()
                .chars()
                .take(store::MAX_SUBJECT)
                .collect();
            let body: String = text(arg(req, 2, "body"))
                .unwrap_or_default()
                .chars()
                .take(store::MAX_BODY)
                .collect();
            if title.trim().is_empty() {
                return Err(refuse("NoSubject", vec![]));
            }
            l.welcome = Some((title.clone(), body.clone()));
            store::save_list(reg.deltas_mut(), &l);
            if req.method == "SendWelcomeMail" {
                let to: Vec<i64> = store::members(reg.deltas(), l.id)
                    .into_iter()
                    .map(|(c, _)| c)
                    .filter(|c| *c != me)
                    .collect();
                store::deliver_automated(reg, l.id, &to, &title, &body);
            }
            Ok(PyValue::None)
        }
        "ClearWelcomeMail" => {
            let mut l = owned(reg, req, me)?;
            l.welcome = None;
            store::save_list(reg.deltas_mut(), &l);
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{label, status};

    /// Invented characters: nothing here is a client row.
    const ALICE: i64 = 90_000_601;
    const BOB: i64 = 90_000_602;
    const CAROL: i64 = 90_000_603;
    const HULL: i64 = 990_640_001;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ship_type_id: HULL,
            ..crate::model::Character::default()
        }
    }

    fn world() -> Registry {
        crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ];
        })
    }

    fn call_as(
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

    fn mail(reg: &mut Registry, who: i64, method: &str, args: &[PyValue]) -> PyValue {
        call_as(reg, who, "mailMgr", method, args).expect("answered")
    }

    fn lists(reg: &mut Registry, who: i64, method: &str, args: &[PyValue]) -> PyValue {
        call_as(reg, who, "mailingListsMgr", method, args).expect("answered")
    }

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn ids_v(v: &[i64]) -> PyValue {
        PyValue::List(v.iter().map(|n| PyValue::Int(*n)).collect())
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

    fn list_of(v: &PyValue) -> &Vec<PyValue> {
        let PyValue::List(rows) = v else {
            panic!("a list: {v:?}")
        };
        rows
    }

    /// `(messageID, statusMask, labelMask)` of `who`'s `mailStatus`.
    fn statuses(reg: &mut Registry, who: i64) -> Vec<(i64, i64, i64)> {
        let sync = mail(reg, who, "SyncMail", &[PyValue::None, i(0)]);
        list_of(attr(&sync, "mailStatus"))
            .iter()
            .map(|r| {
                let n = |k: &str| match attr(r, k) {
                    PyValue::Int(n) => *n,
                    other => panic!("{k}: {other:?}"),
                };
                (n("messageID"), n("statusMask"), n("labelMask"))
            })
            .collect()
    }

    fn send(reg: &mut Registry, who: i64, to: &[i64], title: &str, body: &str) -> i64 {
        let PyValue::Int(id) = mail(
            reg,
            who,
            "SendMail",
            &[
                ids_v(to),
                PyValue::None,
                PyValue::None,
                u(title),
                u(body),
                i(0),
                i(0),
            ],
        ) else {
            panic!("an id")
        };
        id
    }

    /// `(event, audience)` of every `OnMailSent` due.
    fn mail_pushes(reg: &mut Registry) -> Vec<(i64, Vec<i64>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .filter_map(|p| {
                let (event, args) = p.to_push();
                (event == "OnMailSent").then(|| {
                    let PyValue::Int(id) = args[0] else {
                        panic!("an id")
                    };
                    (id, p.audience().unwrap_or_default())
                })
            })
            .collect()
    }

    fn found_corp(reg: &mut Registry) -> i64 {
        reg.character_mut(ALICE).unwrap().balance = 1.0e10;
        call_as(
            reg,
            ALICE,
            "corpRegistry",
            "AddCorporation",
            &[u("Mailed Ltd"), u("MAIL"), u(""), u("")],
        )
        .expect("the corp is founded");
        let corp = reg.corporations.membership(ALICE).unwrap().corporation_id;
        enrol(reg, BOB, corp, 0);
        corp
    }

    fn enrol(reg: &mut Registry, who: i64, corp: i64, roles: i64) {
        let (who, corp, roles) = (who.to_string(), corp.to_string(), roles.to_string());
        reg.corporations
            .parse_record(&["corpmember", &who, &corp, &roles, "1"])
            .expect("a membership record");
    }

    /// **A mail between two players**, over the wire: Alice sends, Bob (who
    /// is pushed) syncs, fetches the body and reads it; Alice's own copy is
    /// filed Sent and already read. A relog's `SyncMail(first, last)` with
    /// what Bob holds answers nothing new.
    #[test]
    fn a_player_mails_another_who_reads_it() {
        let mut reg = world();
        let id = send(&mut reg, ALICE, &[BOB], "Hello", "Fly safe.");
        assert_eq!(mail_pushes(&mut reg), vec![(id, vec![BOB])]);

        let sync = mail(&mut reg, BOB, "SyncMail", &[PyValue::None, i(0)]);
        assert_eq!(list_of(attr(&sync, "newMail")).len(), 1);
        assert_eq!(attr(&sync, "oldMail"), &PyValue::None);
        let header = &list_of(attr(&sync, "newMail"))[0];
        assert_eq!(attr(header, "senderID"), &i(ALICE));
        assert_eq!(attr(header, "title"), &u("Hello"));
        assert_eq!(
            attr(header, "toCharacterIDs"),
            &PyValue::Bytes(BOB.to_string().into_bytes())
        );
        assert_eq!(attr(header, "toListID"), &PyValue::None);
        assert_eq!(statuses(&mut reg, BOB), vec![(id, 0, label::INBOX)]);
        assert_eq!(
            statuses(&mut reg, ALICE),
            vec![(id, status::READ, label::SENT)],
            "the sender's own copy"
        );

        // The body is a zlib stream; read marks it.
        let PyValue::Bytes(z) = mail(&mut reg, BOB, "GetBody", &[i(id), PyValue::Bool(true)])
        else {
            panic!("bytes")
        };
        assert_eq!(&z[..2], &[0x78, 0x01]);
        assert!(z.windows(9).any(|w| w == b"Fly safe."));
        assert_eq!(
            statuses(&mut reg, BOB),
            vec![(id, status::READ, label::INBOX)]
        );

        // Nothing new for a client that holds it.
        let again = mail(&mut reg, BOB, "SyncMail", &[i(id), i(id)]);
        assert!(list_of(attr(&again, "newMail")).is_empty());
        assert_eq!(list_of(attr(&again, "oldMail")).len(), 0);
        // Not somebody else's.
        assert_eq!(
            mail(&mut reg, CAROL, "GetBody", &[i(id), PyValue::Bool(true)]),
            PyValue::None
        );
        assert!(list_of(&mail(&mut reg, CAROL, "GetMailHeaders", &[ids_v(&[id])])).is_empty());
    }

    /// **Offline recipients** find the mail at their next sync: nothing about
    /// delivery needs them connected, and the push goes to whoever is.
    #[test]
    fn a_mail_waits_for_an_offline_recipient() {
        let mut reg = world();
        let id = send(&mut reg, ALICE, &[CAROL], "While you were out", "x");
        let _ = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1));
        assert_eq!(statuses(&mut reg, CAROL), vec![(id, 0, label::INBOX)]);
    }

    /// **A mail to a corp** reaches each member, filed under the corp label,
    /// and not an outsider; an outsider cannot send to it.
    #[test]
    fn a_mail_to_a_corp_reaches_each_member() {
        let mut reg = world();
        let corp = found_corp(&mut reg);
        let sent = mail(
            &mut reg,
            ALICE,
            "SendMail",
            &[
                PyValue::List(vec![]),
                PyValue::None,
                i(corp),
                u("Fleet tonight"),
                u("Bring ammo."),
                i(0),
                i(0),
            ],
        );
        let PyValue::Int(id) = sent else {
            panic!("an id")
        };
        assert_eq!(mail_pushes(&mut reg), vec![(id, vec![BOB])]);
        assert_eq!(statuses(&mut reg, BOB), vec![(id, 0, label::CORPORATION)]);
        assert_eq!(
            statuses(&mut reg, ALICE),
            vec![(id, 0, label::SENT | label::CORPORATION)],
            "the sender's client files it unread too"
        );
        assert!(statuses(&mut reg, CAROL).is_empty(), "not a member");
        let outsider = call_as(
            &mut reg,
            CAROL,
            "mailMgr",
            "SendMail",
            &[
                PyValue::List(vec![]),
                PyValue::None,
                i(corp),
                u("x"),
                u("y"),
                i(0),
                i(0),
            ],
        );
        assert!(outsider.is_err(), "Carol is not in it");
        assert_eq!(statuses(&mut reg, BOB).len(), 1);
    }

    /// **A mail to an alliance** reaches every member of every corp in it,
    /// from the CEO of a member corp only.
    #[test]
    fn a_mail_to_an_alliance_reaches_its_corps_members() {
        let mut reg = world();
        let corp = found_corp(&mut reg);
        let to_master = [(PyValue::Int(1000), PyValue::Bytes(b"toAccountKey".to_vec()))];
        reg.dispatch(&Request {
            service: "account",
            method: "GiveCash",
            args: &[i(corp), PyValue::Real(1_500_000_000.0), u("test")],
            kwargs: &to_master,
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("the deposit");
        call_as(
            &mut reg,
            ALICE,
            "corpRegistry",
            "CreateAlliance",
            &[u("Mailed Pact"), u("MPAC"), u(""), u("")],
        )
        .expect("the alliance is created");
        let alliance = crate::alliance::of_corp(reg.deltas(), corp).expect("in an alliance");
        let to = |reg: &mut Registry, who: i64| {
            call_as(
                reg,
                who,
                "mailMgr",
                "SendMail",
                &[
                    PyValue::List(vec![]),
                    PyValue::None,
                    i(alliance),
                    u("Pact news"),
                    u("y"),
                    i(0),
                    i(0),
                ],
            )
        };
        assert!(to(&mut reg, BOB).is_err(), "a plain member may not");
        assert!(to(&mut reg, CAROL).is_err(), "nor an outsider");
        let Ok(PyValue::Int(id)) = to(&mut reg, ALICE) else {
            panic!("the CEO may")
        };
        assert_eq!(statuses(&mut reg, BOB), vec![(id, 0, label::ALLIANCE)]);
        assert!(statuses(&mut reg, CAROL).is_empty());
    }

    /// **Status, trash, delete and labels** over the service, each on the
    /// caller's own mailbox only.
    #[test]
    fn marking_trashing_deleting_and_labelling() {
        let mut reg = world();
        let a = send(&mut reg, ALICE, &[BOB], "one", "1");
        let b = send(&mut reg, ALICE, &[BOB], "two", "2");
        mail(&mut reg, BOB, "MarkAsRead", &[ids_v(&[a])]);
        assert_eq!(statuses(&mut reg, BOB)[1].1, status::READ, "a is read");
        mail(&mut reg, BOB, "MarkAsUnread", &[ids_v(&[a])]);
        mail(&mut reg, BOB, "MarkAllAsRead", &[]);
        assert!(statuses(&mut reg, BOB).iter().all(|s| s.1 == status::READ));
        // Carol cannot touch Bob's.
        mail(&mut reg, CAROL, "MoveToTrash", &[ids_v(&[a, b])]);
        assert!(
            statuses(&mut reg, BOB)
                .iter()
                .all(|s| s.1 & status::TRASHED == 0)
        );
        mail(&mut reg, BOB, "MoveToTrash", &[ids_v(&[a])]);
        assert_eq!(
            statuses(&mut reg, BOB)[1].1 & status::TRASHED,
            status::TRASHED
        );
        mail(&mut reg, BOB, "MoveFromTrash", &[ids_v(&[a])]);
        assert_eq!(statuses(&mut reg, BOB)[1].1, 0, "restored and unread");

        // A label: create, assign, trash by it, delete it.
        let PyValue::Int(l) = mail(&mut reg, BOB, "CreateLabel", &[u("Work"), i(3)]) else {
            panic!("an id")
        };
        let labels = mail(&mut reg, BOB, "GetLabels", &[]);
        let PyValue::Dict(rows) = &labels else {
            panic!("a dict")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0].0, "name"), &u("Work"));
        mail(&mut reg, BOB, "AssignLabels", &[ids_v(&[b]), i(l)]);
        assert_eq!(statuses(&mut reg, BOB)[0].2, label::INBOX | l);
        mail(&mut reg, BOB, "AssignLabels", &[ids_v(&[b]), i(1 << 40)]);
        assert_eq!(
            statuses(&mut reg, BOB)[0].2,
            label::INBOX | l,
            "not a label of his"
        );
        mail(&mut reg, BOB, "MoveToTrashByLabel", &[i(l)]);
        assert_eq!(
            statuses(&mut reg, BOB)[0].1 & status::TRASHED,
            status::TRASHED
        );
        mail(&mut reg, BOB, "RemoveLabels", &[ids_v(&[b]), i(l)]);
        assert_eq!(statuses(&mut reg, BOB)[0].2, label::INBOX);
        mail(&mut reg, BOB, "DeleteLabel", &[i(l)]);
        assert!(matches!(mail(&mut reg, BOB, "GetLabels", &[]), PyValue::Dict(r) if r.is_empty()));

        // The trash, emptied; Alice's copies stay.
        mail(&mut reg, BOB, "EmptyTrash", &[]);
        assert_eq!(statuses(&mut reg, BOB).len(), 1, "b went");
        mail(&mut reg, BOB, "DeleteMail", &[ids_v(&[a])]);
        assert!(statuses(&mut reg, BOB).is_empty());
        assert_eq!(statuses(&mut reg, ALICE).len(), 2);
    }

    /// **A reply and a forward** mark the original on the sender's side.
    #[test]
    fn a_reply_marks_the_original_replied() {
        let mut reg = world();
        let a = send(&mut reg, ALICE, &[BOB], "q", "?");
        let PyValue::Int(r) = mail(
            &mut reg,
            BOB,
            "SendMail",
            &[
                ids_v(&[ALICE]),
                PyValue::None,
                PyValue::None,
                u("Re: q"),
                u("!"),
                i(a),
                i(0),
            ],
        ) else {
            panic!("an id")
        };
        let bob = statuses(&mut reg, BOB);
        assert_eq!(bob[1], (a, status::REPLIED, label::INBOX));
        assert_eq!(bob[0], (r, status::READ, label::SENT));
    }

    /// **Refusals** change nothing: no recipient, no subject, a stranger, too
    /// many, too long; each a `UserError`.
    #[test]
    fn bad_sends_are_refused_and_store_nothing() {
        let mut reg = world();
        let attempt = |reg: &mut Registry, to: Vec<i64>, title: &str, body: &str| {
            call_as(
                reg,
                ALICE,
                "mailMgr",
                "SendMail",
                &[
                    ids_v(&to),
                    PyValue::None,
                    PyValue::None,
                    u(title),
                    u(body),
                    i(0),
                    i(0),
                ],
            )
        };
        assert!(attempt(&mut reg, vec![], "t", "b").is_err());
        assert!(attempt(&mut reg, vec![BOB], "  ", "b").is_err());
        assert!(
            attempt(&mut reg, vec![42], "t", "b").is_err(),
            "no such pilot"
        );
        assert!(
            attempt(
                &mut reg,
                vec![BOB],
                &"x".repeat(store::MAX_SUBJECT + 1),
                "b"
            )
            .is_err()
        );
        assert!(attempt(&mut reg, vec![BOB], "t", &"x".repeat(store::MAX_BODY + 1)).is_err());
        let many: Vec<i64> = (0..=store::MAX_RECIPIENTS as i64)
            .map(|n| BOB + n * 1_000)
            .collect();
        assert!(attempt(&mut reg, many, "t", "b").is_err());
        assert!(attempt(&mut reg, vec![BOB], "t", "b").is_ok());
        assert_eq!(statuses(&mut reg, BOB).len(), 1);
        // A list the sender is not in: the client's own key.
        let to_list = call_as(
            &mut reg,
            ALICE,
            "mailMgr",
            "SendMail",
            &[
                PyValue::List(vec![]),
                i(store::LIST_ID_BASE + 9),
                PyValue::None,
                u("t"),
                u("b"),
                i(0),
                i(0),
            ],
        );
        assert!(to_list.is_err());
    }

    /// **A mailing list's whole life**: create, join, mail it, mute, kick,
    /// leave and delete, with the pushes the client's handlers take.
    #[test]
    fn a_mailing_list_from_create_to_delete() {
        let mut reg = world();
        let PyValue::Int(list) = lists(
            &mut reg,
            ALICE,
            "Create",
            &[u("Free Beer"), i(1), i(1), i(0)],
        ) else {
            panic!("an id")
        };
        let joined = lists(&mut reg, ALICE, "GetJoinedLists", &[]);
        let PyValue::Dict(rows) = &joined else {
            panic!("a dict")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0].0, "isOwner"), &PyValue::Bool(true));
        assert_eq!(rows[0].1, i(list));
        assert!(
            call_as(
                &mut reg,
                ALICE,
                "mailingListsMgr",
                "Create",
                &[u("free beer"), i(1), i(1), i(0)]
            )
            .is_err(),
            "the name is taken"
        );

        let bob = lists(&mut reg, BOB, "Join", &[u("FREE BEER")]);
        assert_eq!(attr(&bob, "id"), &i(list));
        assert_eq!(attr(&bob, "displayName"), &u("Free Beer"));
        lists(&mut reg, CAROL, "Join", &[u("free beer")]);
        let members = lists(&mut reg, ALICE, "GetMembers", &[i(list)]);
        let PyValue::Dict(m) = &members else {
            panic!("a dict")
        };
        assert_eq!(m.len(), 3);
        assert_eq!(
            attr(
                &lists(&mut reg, CAROL, "GetInfo", &[i(list)]),
                "displayName"
            ),
            &u("Free Beer")
        );
        assert_eq!(lists(&mut reg, CAROL, "GetInfo", &[i(1)]), PyValue::None);

        // Bob mails the list: Alice and Carol hold it, with no label.
        let PyValue::Int(id) = mail(
            &mut reg,
            BOB,
            "SendMail",
            &[
                PyValue::List(vec![]),
                i(list),
                PyValue::None,
                u("Beer"),
                u("Free"),
                i(0),
                i(0),
            ],
        ) else {
            panic!("an id")
        };
        let mut told = mail_pushes(&mut reg);
        told.sort();
        assert_eq!(told, vec![(id, vec![ALICE]), (id, vec![CAROL])]);
        assert_eq!(statuses(&mut reg, CAROL), vec![(id, 0, 0)]);
        mail(&mut reg, CAROL, "MarkAsReadByList", &[i(list)]);
        assert_eq!(statuses(&mut reg, CAROL), vec![(id, status::READ, 0)]);

        // A plain member cannot rank anybody; the owner mutes Bob.
        assert!(
            call_as(
                &mut reg,
                CAROL,
                "mailingListsMgr",
                "SetMembersMuted",
                &[i(list), ids_v(&[BOB])]
            )
            .is_err()
        );
        lists(
            &mut reg,
            ALICE,
            "SetMembersMuted",
            &[i(list), ids_v(&[BOB])],
        );
        let events = |reg: &mut Registry| -> Vec<(String, Vec<i64>)> {
            reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
                .iter()
                .filter(|p| p.to_push().0.starts_with("OnMailingList"))
                .map(|p| (p.to_push().0.to_owned(), p.audience().unwrap_or_default()))
                .collect()
        };
        assert_eq!(
            events(&mut reg),
            vec![("OnMailingListSetMuted".to_owned(), vec![BOB])]
        );
        assert!(
            call_as(
                &mut reg,
                BOB,
                "mailMgr",
                "SendMail",
                &[
                    PyValue::List(vec![]),
                    i(list),
                    PyValue::None,
                    u("t"),
                    u("b"),
                    i(0),
                    i(0)
                ],
            )
            .is_err(),
            "muted"
        );
        lists(
            &mut reg,
            ALICE,
            "SetMembersOperator",
            &[i(list), ids_v(&[BOB])],
        );
        lists(
            &mut reg,
            ALICE,
            "SetMembersClear",
            &[i(list), ids_v(&[BOB])],
        );
        assert_eq!(events(&mut reg).len(), 2);

        // Kick Carol (the owner cannot be kicked); Bob leaves.
        lists(
            &mut reg,
            ALICE,
            "KickMembers",
            &[i(list), ids_v(&[CAROL, ALICE])],
        );
        assert_eq!(
            events(&mut reg),
            vec![("OnMailingListLeave".to_owned(), vec![CAROL])]
        );
        lists(&mut reg, BOB, "Leave", &[i(list)]);
        assert!(call_as(&mut reg, BOB, "mailingListsMgr", "Leave", &[i(list)]).is_err());
        lists(&mut reg, CAROL, "Join", &[u("free beer")]);
        lists(&mut reg, ALICE, "Delete", &[i(list)]);
        assert_eq!(
            events(&mut reg),
            vec![("OnMailingListDeleted".to_owned(), vec![CAROL])]
        );
        assert!(
            matches!(lists(&mut reg, CAROL, "GetJoinedLists", &[]), PyValue::Dict(r) if r.is_empty())
        );
    }

    /// **Access and welcome mail**: a closed list refuses a stranger and
    /// admits an allowed one; the welcome mail goes to a new member as the
    /// list, automated.
    #[test]
    fn access_settings_and_the_welcome_mail() {
        let mut reg = world();
        let PyValue::Int(list) = lists(&mut reg, ALICE, "Create", &[u("Club"), i(0), i(1), i(0)])
        else {
            panic!("an id")
        };
        assert!(
            call_as(&mut reg, BOB, "mailingListsMgr", "Join", &[u("club")]).is_err(),
            "closed"
        );
        lists(
            &mut reg,
            ALICE,
            "SetEntitiesAccess",
            &[
                i(list),
                PyValue::Dict(vec![(i(store::access::ALLOWED), i(BOB))]),
            ],
        );
        let settings = lists(&mut reg, ALICE, "GetSettings", &[i(list)]);
        assert_eq!(attr(&settings, "defaultAccess"), &i(0));
        let PyValue::Dict(access) = attr(&settings, "access") else {
            panic!("a dict")
        };
        assert_eq!(access, &vec![(i(1), i(BOB))]);
        lists(
            &mut reg,
            ALICE,
            "SaveWelcomeMail",
            &[i(list), u("Welcome"), u("Be nice.")],
        );
        let welcome = lists(&mut reg, ALICE, "GetWelcomeMail", &[i(list)]);
        assert_eq!(attr(&list_of(&welcome)[0], "title"), &u("Welcome"));
        lists(&mut reg, BOB, "Join", &[u("club")]);
        let status_row = statuses(&mut reg, BOB);
        assert_eq!(status_row.len(), 1, "{status_row:?}");
        assert_eq!(status_row[0].1, status::AUTOMATED);
        let told = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1));
        let (event, args) = told
            .iter()
            .map(|p| p.to_push())
            .find(|(e, _)| *e == "OnMailSent")
            .expect("pushed");
        assert_eq!(event, "OnMailSent");
        assert_eq!(args[1], i(list), "from the list");
        assert_eq!(args[7], i(status::AUTOMATED));
        lists(&mut reg, ALICE, "ClearWelcomeMail", &[i(list)]);
        assert!(list_of(&lists(&mut reg, ALICE, "GetWelcomeMail", &[i(list)])).is_empty());
    }

    /// No character: the empty answers, and nothing that changes anything.
    #[test]
    fn no_actor_gets_empty_answers() {
        let mut reg = world();
        let ask = |reg: &mut Registry, svc: &str, method: &str| {
            reg.dispatch(&Request {
                service: svc,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            })
        };
        assert!(
            matches!(ask(&mut reg, "mailMgr", "GetLabels"), Ok(PyValue::Dict(r)) if r.is_empty())
        );
        assert!(ask(&mut reg, "mailMgr", "SyncMail").is_ok());
        assert!(
            matches!(ask(&mut reg, "mailingListsMgr", "GetJoinedLists"), Ok(PyValue::Dict(r)) if r.is_empty())
        );
        assert!(ask(&mut reg, "mailMgr", "SendMail").is_err());
    }
}
