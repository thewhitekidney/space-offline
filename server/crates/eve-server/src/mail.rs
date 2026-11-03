//! **EVE mail** (MAIL-1, wave C-2): the store behind `mailMgr` and
//! `mailingListsMgr`, and the one call anything else makes to send a mail.
//!
//! Lane note: `docs/lanes/mail-1.md`. The wire is `service/mail.rs`. The
//! notification store (`notifications.rs`, NOTIF-1) is a separate thing the
//! mail window also shows in a tab; nothing here reads or writes it.
//!
//! # What the client does with mail [V]
//!
//! `mailSvc` (`eve/client/script/ui/services/mail/mailSvc.py`) keeps a header
//! cache per character on disk and syncs it with
//! `mailMgr.SyncMail(firstID, lastID)`: the oldest and the newest id it holds
//! (`None, 0` for an empty cache). The answer has `newMail` (a **list** of
//! header rows, `.extend` is called on it), `oldMail` (`None` or a list) and
//! `mailStatus`, **every** mail the character holds with `statusMask` and
//! `labelMask`; a cached header missing from `mailStatus` is dropped, so
//! `mailStatus` is also how a delete is learnt. Headers the cache lacks are
//! fetched with `GetMailHeaders(ids)`, bodies with `GetBody(id, markAsRead)`
//! (zlib-compressed UTF-8). `SendMail` is called with seven arguments and
//! answers the new message id; the sender's client files the new mail
//! locally (labels from the recipients) and then calls `AssignLabels`.
//!
//! A mail reaches an online recipient as `OnMailSent(messageID, senderID,
//! sentDate, toCharacterIDs, toListID, toCorpOrAllianceID, title,
//! statusMask)`, `mailSvc.OnMailSent`. The label mask the client computes for
//! it is the one [`labels_for`] stores, so a relog changes nothing.
//!
//! # The store
//!
//! | kind | key | value |
//! | --- | --- | --- |
//! | `mail` | `id` | `sender sent to_chars to_list to_corp title body`; `next -> n` |
//! | `mailbox` | `character id` | `status labels`: one row per mail a character holds |
//! | `maillabel` | `character label` | `name color` |
//! | `mlist` | `id` | `key name owner access member_access cost welcome_title welcome_body has_welcome`; `next -> n` |
//! | `mlmember` | `list character` | `role` |
//! | `mlaccess` | `list entity` | `access` |
//!
//! A message is one row; each holder has a mailbox row (the sender, and each
//! recipient however it reached them), so deleting is per holder and the
//! message goes with its last row. A character keeps [`KEEP_PER_CHARACTER`],
//! oldest first out. Everything is pure on [`Deltas`] but [`send`] and
//! [`deliver_automated`], which need the world for the corporations and the
//! pushes.

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeSet;

pub const KIND_MAIL: &str = "mail";
pub const KIND_BOX: &str = "mailbox";
pub const KIND_LABEL: &str = "maillabel";
pub const KIND_LIST: &str = "mlist";
pub const KIND_MEMBER: &str = "mlmember";
pub const KIND_ACCESS: &str = "mlaccess";

const NEXT: &str = "next";

/// `appConst.mailStatusMask*` **[V]**.
pub mod status {
    pub const READ: i64 = 1;
    pub const REPLIED: i64 = 2;
    pub const FORWARDED: i64 = 4;
    pub const TRASHED: i64 = 8;
    /// A mail the world sent as a mailing list (a welcome mail): the client
    /// names the list as the sender.
    pub const AUTOMATED: i64 = 32;
}

/// `appConst.mailLabel*` **[V]**; custom labels are the powers of two above.
pub mod label {
    pub const INBOX: i64 = 1;
    pub const SENT: i64 = 2;
    pub const CORPORATION: i64 = 4;
    pub const ALLIANCE: i64 = 8;
}

/// `appConst.mailingList*` **[V]**: who may join, and a member's rank.
pub mod access {
    pub const BLOCKED: i64 = 0;
    pub const ALLOWED: i64 = 1;
}

pub mod role {
    pub const MUTED: i64 = 0;
    pub const DEFAULT: i64 = 1;
    pub const OPERATOR: i64 = 2;
    pub const OWNER: i64 = 3;
}

/// `appConst.mailMaxRecipients` **[V]**.
pub const MAX_RECIPIENTS: usize = 50;
/// `appConst.mailMaxSubjectSize` **[V]**.
pub const MAX_SUBJECT: usize = 150;
/// `appConst.mailMaxTaggedBodySize` **[V]**: the body with its markup.
pub const MAX_BODY: usize = 10_000;
/// `appConst.mailMaxLabelSize` **[V]**.
pub const MAX_LABEL_NAME: usize = 40;
/// `appConst.mailMaxNumLabels` **[V]**.
pub const MAX_LABELS: usize = 25;
/// `appConst.mailingListMaxNameSize` **[V]**.
pub const MAX_LIST_NAME: usize = 60;
/// `appConst.mailinglistMaxMembers` **[V]**.
pub const MAX_LIST_MEMBERS: usize = 3_000;

/// Mails kept per character, oldest evicted first **[A]**: a mail is a few
/// hundred bytes, and a recruiting corp mails all its members. Falsified by
/// a player missing a mail they had not deleted within a week of play, or a
/// world file that grows past a few megabytes of mail.
pub const KEEP_PER_CHARACTER: usize = 500;

/// Mailing list ids start here **[A]**: above every character, corp and
/// alliance id this server hands out, so a list is never mistaken for an
/// owner. Falsified by the client refusing a list id this large (it reads
/// ids only as dict keys and `toListID`).
pub const LIST_ID_BASE: i64 = 1_000_000_000_000;

// ---- ids -------------------------------------------------------------------------

/// The next id of `kind`'s counter: the `next` row, or one past the highest
/// kept.
fn next_id(d: &mut Deltas, kind: &str) -> i64 {
    let stored = d
        .of(kind)
        .get(&vec![NEXT.to_owned()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok());
    let highest = d
        .of(kind)
        .keys()
        .filter(|k| k.len() == 1)
        .filter_map(|k| k[0].parse::<i64>().ok())
        .map(|id| {
            if kind == KIND_LIST {
                id - LIST_ID_BASE
            } else {
                id
            }
        })
        .max()
        .unwrap_or(0);
    let id = stored.unwrap_or(0).max(highest + 1).max(1);
    d.put(kind, vec![NEXT.to_owned()], vec![(id + 1).to_string()]);
    id
}

fn num(v: &[String], i: usize) -> Option<i64> {
    v.get(i)?.parse().ok()
}

fn opt_num(v: &[String], i: usize) -> Option<Option<i64>> {
    let s = v.get(i)?;
    if s.is_empty() {
        Some(None)
    } else {
        s.parse().ok().map(Some)
    }
}

fn opt_s(v: Option<i64>) -> String {
    v.map_or_else(String::new, |n| n.to_string())
}

fn py_opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

// ---- messages and mailboxes ----------------------------------------------------------

/// One mail.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: i64,
    pub sender: i64,
    pub sent: BlueTime,
    pub to_chars: Vec<i64>,
    pub to_list: Option<i64>,
    pub to_corp: Option<i64>,
    pub title: String,
    pub body: String,
}

impl Message {
    fn to_row(&self) -> Vec<String> {
        vec![
            self.sender.to_string(),
            self.sent.to_string(),
            self.to_chars
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(","),
            opt_s(self.to_list),
            opt_s(self.to_corp),
            self.title.clone(),
            self.body.clone(),
        ]
    }

    fn from_row(id: i64, v: &[String]) -> Option<Message> {
        Some(Message {
            id,
            sender: num(v, 0)?,
            sent: num(v, 1)?,
            to_chars: v
                .get(2)?
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect(),
            to_list: opt_num(v, 3)?,
            to_corp: opt_num(v, 4)?,
            title: v.get(5)?.clone(),
            body: v.get(6)?.clone(),
        })
    }

    /// The header row `SyncMail` and `GetMailHeaders` answer **[V]**
    /// (`mailRow.messageID, senderID, toCharacterIDs, toListID,
    /// toCorpOrAllianceID, title, sentDate`): `toCharacterIDs` is the ids
    /// as one comma-separated string, `None` for none (the client splits it
    /// when it is truthy).
    pub fn header(&self) -> PyValue {
        let chars = if self.to_chars.is_empty() {
            PyValue::None
        } else {
            PyValue::Bytes(
                self.to_chars
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
                    .into_bytes(),
            )
        };
        crate::pyutil::keyval(vec![
            ("messageID", PyValue::Int(self.id)),
            ("senderID", PyValue::Int(self.sender)),
            ("toCharacterIDs", chars),
            ("toListID", py_opt(self.to_list)),
            ("toCorpOrAllianceID", py_opt(self.to_corp)),
            ("title", uni(&self.title)),
            ("sentDate", PyValue::Int(self.sent)),
        ])
    }
}

/// One character's hold on one mail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub id: i64,
    pub status: i64,
    pub labels: i64,
}

impl Slot {
    /// The `mailStatus` row **[V]**: `messageID, statusMask, labelMask`.
    pub fn row(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("messageID", PyValue::Int(self.id)),
            ("statusMask", PyValue::Int(self.status)),
            ("labelMask", PyValue::Int(self.labels)),
        ])
    }

    pub fn read(&self) -> bool {
        self.status & status::READ != 0
    }

    pub fn trashed(&self) -> bool {
        self.status & status::TRASHED != 0
    }
}

fn pair_key(a: i64, b: i64) -> Vec<String> {
    vec![a.to_string(), b.to_string()]
}

/// The message `id`, if it is stored.
pub fn message(d: &Deltas, id: i64) -> Option<Message> {
    Message::from_row(id, d.of(KIND_MAIL).get(&vec![id.to_string()])?)
}

/// What `who` holds, newest first.
pub fn slots(d: &Deltas, who: i64) -> Vec<Slot> {
    let me = who.to_string();
    let mut v: Vec<Slot> = d
        .of(KIND_BOX)
        .range(vec![me.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| {
            Some(Slot {
                id: k.get(1)?.parse().ok()?,
                status: num(v, 0)?,
                labels: num(v, 1)?,
            })
        })
        .collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

/// `who`'s hold on mail `id`.
pub fn slot(d: &Deltas, who: i64, id: i64) -> Option<Slot> {
    let v = d.of(KIND_BOX).get(&pair_key(who, id))?;
    Some(Slot {
        id,
        status: num(v, 0)?,
        labels: num(v, 1)?,
    })
}

fn put_slot(d: &mut Deltas, who: i64, s: Slot) {
    d.put(
        KIND_BOX,
        pair_key(who, s.id),
        vec![s.status.to_string(), s.labels.to_string()],
    );
}

/// Change `who`'s slots in `ids` (those they hold), `f` returning the new
/// status and labels; how many were changed.
pub fn update_slots(
    d: &mut Deltas,
    who: i64,
    ids: &[i64],
    f: impl Fn(&Slot) -> (i64, i64),
) -> usize {
    let mut n = 0;
    for id in ids {
        if let Some(mut s) = slot(d, who, *id) {
            (s.status, s.labels) = f(&s);
            put_slot(d, who, s);
            n += 1;
        }
    }
    n
}

/// [`update_slots`] over every slot `pick` admits.
pub fn update_where(
    d: &mut Deltas,
    who: i64,
    pick: impl Fn(&Slot) -> bool,
    f: impl Fn(&Slot) -> (i64, i64),
) -> usize {
    let ids: Vec<i64> = slots(d, who)
        .iter()
        .filter(|s| pick(s))
        .map(|s| s.id)
        .collect();
    update_slots(d, who, &ids, f)
}

/// The label mask `who` files a mail under, as the client does when it is
/// pushed `OnMailSent` (`mailSvc.OnMailSent`) **[V]**: the Inbox for a mail
/// addressed to them by name, Sent for their own, the corporation or
/// alliance label for a mail to one. A mail that reached them through a
/// mailing list has no label of its own.
pub fn labels_for(who: i64, m: &Message, corp_is_corp: bool) -> i64 {
    let mut mask = 0;
    if m.to_chars.contains(&who) {
        mask |= label::INBOX;
    }
    if m.sender == who {
        mask |= label::SENT;
    }
    if m.to_corp.is_some() {
        mask |= if corp_is_corp {
            label::CORPORATION
        } else {
            label::ALLIANCE
        };
    }
    mask
}

/// Delete `who`'s hold on `ids`, then any message nobody holds. How many
/// holds went.
pub fn delete(d: &mut Deltas, who: i64, ids: &[i64]) -> usize {
    let mut n = 0;
    for id in ids {
        if d.del(KIND_BOX, &pair_key(who, *id)).is_some() {
            n += 1;
        }
    }
    collect_garbage(d, ids);
    n
}

/// Drop each of `ids` that no mailbox row refers to.
fn collect_garbage(d: &mut Deltas, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let held: BTreeSet<i64> = d
        .of(KIND_BOX)
        .keys()
        .filter_map(|k| k.get(1)?.parse().ok())
        .collect();
    for id in ids {
        if !held.contains(id) {
            d.del(KIND_MAIL, &[id.to_string()]);
        }
    }
}

/// Evict `who`'s oldest mails past [`KEEP_PER_CHARACTER`].
fn evict(d: &mut Deltas, who: i64) {
    let ids: Vec<i64> = slots(d, who).iter().map(|s| s.id).collect();
    if ids.len() > KEEP_PER_CHARACTER {
        let old: Vec<i64> = ids[KEEP_PER_CHARACTER..].to_vec();
        delete(d, who, &old);
    }
}

/// How many unread, untrashed mails `who` holds.
pub fn unread_count(d: &Deltas, who: i64) -> usize {
    slots(d, who)
        .iter()
        .filter(|s| !s.read() && !s.trashed())
        .count()
}

// ---- labels ---------------------------------------------------------------------------

/// One custom label **[V]** (`labelID, name, color`).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub color: Option<i64>,
}

impl Label {
    pub fn row(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("labelID", PyValue::Int(self.id)),
            ("name", uni(&self.name)),
            ("color", py_opt(self.color)),
        ])
    }
}

/// `who`'s custom labels.
pub fn labels(d: &Deltas, who: i64) -> Vec<Label> {
    let me = who.to_string();
    d.of(KIND_LABEL)
        .range(vec![me.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| {
            Some(Label {
                id: k.get(1)?.parse().ok()?,
                name: v.first()?.clone(),
                color: opt_num(v, 1)?,
            })
        })
        .collect()
}

/// A new label: the lowest free bit above the system labels, or `None` at
/// [`MAX_LABELS`] (or for an empty or over-long name).
pub fn create_label(d: &mut Deltas, who: i64, name: &str, color: Option<i64>) -> Option<i64> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_LABEL_NAME {
        return None;
    }
    let held = labels(d, who);
    if held.len() >= MAX_LABELS {
        return None;
    }
    let id = (4..4 + MAX_LABELS as u32)
        .map(|b| 1_i64 << b)
        .find(|id| held.iter().all(|l| l.id != *id))?;
    d.put(
        KIND_LABEL,
        pair_key(who, id),
        vec![name.to_owned(), opt_s(color)],
    );
    Some(id)
}

/// Rename or recolour; `false` for a label `who` does not have.
pub fn edit_label(
    d: &mut Deltas,
    who: i64,
    id: i64,
    name: Option<&str>,
    color: Option<i64>,
) -> bool {
    let Some(mut l) = labels(d, who).into_iter().find(|l| l.id == id) else {
        return false;
    };
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        l.name = n.chars().take(MAX_LABEL_NAME).collect();
    }
    if color.is_some() {
        l.color = color;
    }
    d.put(KIND_LABEL, pair_key(who, id), vec![l.name, opt_s(l.color)]);
    true
}

/// Delete a label and clear its bit from every mail `who` holds.
pub fn delete_label(d: &mut Deltas, who: i64, id: i64) -> bool {
    if d.del(KIND_LABEL, &pair_key(who, id)).is_none() {
        return false;
    }
    update_where(
        d,
        who,
        |s| s.labels & id != 0,
        |s| (s.status, s.labels & !id),
    );
    true
}

// ---- mailing lists ------------------------------------------------------------------------

/// One mailing list.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub id: i64,
    /// The normalised name, unique.
    pub key: String,
    pub name: String,
    pub owner: i64,
    pub default_access: i64,
    pub default_member_access: i64,
    pub cost: i64,
    pub welcome: Option<(String, String)>,
}

/// `GetKeyAndNormalize` **[I]**: the key is the trimmed name in lower case
/// and the display name the trimmed name; the client builds the same pair
/// for a list it created, so the two only need to agree about what the
/// display name is.
pub fn normalise(name: &str) -> (String, String) {
    let display = name.trim().to_owned();
    (display.to_lowercase(), display)
}

impl List {
    fn to_row(&self) -> Vec<String> {
        let (t, b) = self.welcome.clone().unwrap_or_default();
        vec![
            self.key.clone(),
            self.name.clone(),
            self.owner.to_string(),
            self.default_access.to_string(),
            self.default_member_access.to_string(),
            self.cost.to_string(),
            t,
            b,
            u8::from(self.welcome.is_some()).to_string(),
        ]
    }

    fn from_row(id: i64, v: &[String]) -> Option<List> {
        Some(List {
            id,
            key: v.first()?.clone(),
            name: v.get(1)?.clone(),
            owner: num(v, 2)?,
            default_access: num(v, 3)?,
            default_member_access: num(v, 4)?,
            cost: num(v, 5)?,
            welcome: (v.get(8)? == "1").then(|| (v[6].clone(), v[7].clone())),
        })
    }

    /// The row `GetInfo` answers **[V]** (`displayName` is what
    /// `GetDisplayName` reads).
    pub fn info(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("id", PyValue::Int(self.id)),
            ("name", uni(&self.key)),
            ("displayName", uni(&self.name)),
        ])
    }

    /// The row `GetSettings` answers: `defaultAccess`, `defaultMemberAccess`
    /// and `access` (the per-owner overrides, a dict owner -> access) are
    /// the attributes `evemailingListConfig` reads **[V]**; `mailCost` is
    /// the setting's own name in `SetDefaultAccess` **[I]**.
    pub fn settings(&self, overrides: &[(i64, i64)]) -> PyValue {
        crate::pyutil::keyval(vec![
            ("defaultAccess", PyValue::Int(self.default_access)),
            (
                "defaultMemberAccess",
                PyValue::Int(self.default_member_access),
            ),
            ("mailCost", PyValue::Int(self.cost)),
            (
                "access",
                PyValue::Dict(
                    overrides
                        .iter()
                        .map(|(e, a)| (PyValue::Int(*a), PyValue::Int(*e)))
                        .collect(),
                ),
            ),
        ])
    }
}

/// The list `id`.
pub fn get_list(d: &Deltas, id: i64) -> Option<List> {
    List::from_row(id, d.of(KIND_LIST).get(&vec![id.to_string()])?)
}

/// The list called `name` (by its key).
pub fn list_named(d: &Deltas, name: &str) -> Option<List> {
    let (key, _) = normalise(name);
    d.of(KIND_LIST)
        .iter()
        .filter(|(k, _)| k.len() == 1)
        .filter_map(|(k, v)| List::from_row(k[0].parse().ok()?, v))
        .find(|l| l.key == key)
}

/// Store a list as it now is.
pub fn save_list(d: &mut Deltas, l: &List) {
    d.put(KIND_LIST, vec![l.id.to_string()], l.to_row());
}

/// Why a list call was refused. The wire turns each into the client's own
/// text or a plain one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListRefusal {
    BadName,
    NameTaken,
    NoSuchList,
    NotAMember,
    Blocked,
    Full,
    OwnerCannotLeave,
}

/// Create a list owned by `owner`, who is its first member.
pub fn create_list(
    d: &mut Deltas,
    owner: i64,
    name: &str,
    default_access: i64,
    default_member_access: i64,
    cost: i64,
) -> Result<i64, ListRefusal> {
    let (key, display) = normalise(name);
    if key.is_empty() || display.chars().count() > MAX_LIST_NAME {
        return Err(ListRefusal::BadName);
    }
    if list_named(d, name).is_some() {
        return Err(ListRefusal::NameTaken);
    }
    let id = LIST_ID_BASE + next_id(d, KIND_LIST);
    save_list(
        d,
        &List {
            id,
            key,
            name: display,
            owner,
            default_access: default_access.clamp(access::BLOCKED, access::ALLOWED),
            default_member_access: default_member_access.clamp(role::MUTED, role::OPERATOR),
            cost: cost.max(0),
            welcome: None,
        },
    );
    set_role(d, id, owner, role::OWNER);
    Ok(id)
}

/// `who`'s role in the list `list_id`.
pub fn role_of(d: &Deltas, list_id: i64, who: i64) -> Option<i64> {
    num(d.of(KIND_MEMBER).get(&pair_key(list_id, who))?, 0)
}

fn set_role(d: &mut Deltas, list_id: i64, who: i64, r: i64) {
    d.put(KIND_MEMBER, pair_key(list_id, who), vec![r.to_string()]);
}

/// Everyone in a list with their role, in key order.
pub fn members(d: &Deltas, list_id: i64) -> Vec<(i64, i64)> {
    let me = list_id.to_string();
    d.of(KIND_MEMBER)
        .range(vec![me.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| Some((k.get(1)?.parse().ok()?, num(v, 0)?)))
        .collect()
}

/// The lists `who` belongs to, with their role.
pub fn joined(d: &Deltas, who: i64) -> Vec<(List, i64)> {
    let me = who.to_string();
    d.of(KIND_MEMBER)
        .iter()
        .filter(|(k, _)| k.get(1) == Some(&me))
        .filter_map(|(k, v)| Some((get_list(d, k.first()?.parse().ok()?)?, num(v, 0)?)))
        .collect()
}

/// A member's entry in the dict `GetJoinedLists` answers and `Join`
/// returns one of **[V]** (`mailingListsSvc`: `name`, `displayName`,
/// `isMuted`, `isOperator`, `isOwner`; `Join`'s answer also has `id`).
pub fn joined_row(l: &List, r: i64) -> PyValue {
    crate::pyutil::keyval(vec![
        ("id", PyValue::Int(l.id)),
        ("name", uni(&l.key)),
        ("displayName", uni(&l.name)),
        ("isMuted", PyValue::Bool(r == role::MUTED)),
        ("isOperator", PyValue::Bool(r == role::OPERATOR)),
        ("isOwner", PyValue::Bool(r == role::OWNER)),
    ])
}

/// The per-owner access overrides of a list.
pub fn overrides(d: &Deltas, list_id: i64) -> Vec<(i64, i64)> {
    let me = list_id.to_string();
    d.of(KIND_ACCESS)
        .range(vec![me.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| Some((k.get(1)?.parse().ok()?, num(v, 0)?)))
        .collect()
}

pub fn set_access(d: &mut Deltas, list_id: i64, entity: i64, a: i64) {
    d.put(
        KIND_ACCESS,
        pair_key(list_id, entity),
        vec![a.clamp(access::BLOCKED, access::ALLOWED).to_string()],
    );
}

pub fn clear_access(d: &mut Deltas, list_id: i64, entity: i64) {
    d.del(KIND_ACCESS, &pair_key(list_id, entity));
}

/// **Join a list by name.** `owners` are the character, their corporation
/// and their alliance: an override on any of them decides (a block beats an
/// allow), else the list's default does. The member gets the list's default
/// role. Already a member: no change.
pub fn join(
    d: &mut Deltas,
    who: i64,
    owners: &[i64],
    name: &str,
) -> Result<(List, i64), ListRefusal> {
    let l = list_named(d, name).ok_or(ListRefusal::NoSuchList)?;
    if let Some(r) = role_of(d, l.id, who) {
        return Ok((l, r));
    }
    let set = overrides(d, l.id);
    let found: Vec<i64> = owners
        .iter()
        .filter_map(|o| set.iter().find(|(e, _)| e == o).map(|(_, a)| *a))
        .collect();
    let allowed = if found.contains(&access::BLOCKED) {
        false
    } else if found.contains(&access::ALLOWED) {
        true
    } else {
        l.default_access == access::ALLOWED
    };
    if !allowed {
        return Err(ListRefusal::Blocked);
    }
    if members(d, l.id).len() >= MAX_LIST_MEMBERS {
        return Err(ListRefusal::Full);
    }
    let r = l.default_member_access;
    set_role(d, l.id, who, r);
    Ok((l, r))
}

/// Leave a list; the owner cannot (they delete it).
pub fn leave(d: &mut Deltas, who: i64, list_id: i64) -> Result<(), ListRefusal> {
    match role_of(d, list_id, who) {
        None => Err(ListRefusal::NotAMember),
        Some(role::OWNER) => Err(ListRefusal::OwnerCannotLeave),
        Some(_) => {
            d.del(KIND_MEMBER, &pair_key(list_id, who));
            Ok(())
        }
    }
}

/// Delete a list with its members and overrides; the members it had.
pub fn delete_list(d: &mut Deltas, list_id: i64) -> Vec<i64> {
    let who: Vec<i64> = members(d, list_id).iter().map(|(c, _)| *c).collect();
    for c in &who {
        d.del(KIND_MEMBER, &pair_key(list_id, *c));
    }
    for (e, _) in overrides(d, list_id) {
        d.del(KIND_ACCESS, &pair_key(list_id, e));
    }
    d.del(KIND_LIST, &[list_id.to_string()]);
    who
}

/// Give each of `targets` that is in the list, and is not its owner, the
/// role `new`; those changed.
pub fn set_roles(d: &mut Deltas, list_id: i64, targets: &[i64], new: i64) -> Vec<i64> {
    let mut done = Vec::new();
    for c in targets {
        match role_of(d, list_id, *c) {
            Some(role::OWNER) | None => {}
            Some(_) => {
                set_role(d, list_id, *c, new);
                done.push(*c);
            }
        }
    }
    done
}

/// Remove members (never the owner); those removed.
pub fn kick(d: &mut Deltas, list_id: i64, targets: &[i64]) -> Vec<i64> {
    let mut done = Vec::new();
    for c in targets {
        match role_of(d, list_id, *c) {
            Some(role::OWNER) | None => {}
            Some(_) => {
                d.del(KIND_MEMBER, &pair_key(list_id, *c));
                done.push(*c);
            }
        }
    }
    done
}

// ---- sending ------------------------------------------------------------------------------

/// A mail to send.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub sender: i64,
    pub to_chars: Vec<i64>,
    pub to_list: Option<i64>,
    pub to_corp: Option<i64>,
    pub title: String,
    pub body: String,
    pub reply_to: i64,
    pub forwarded_from: i64,
}

/// Why a send was refused; the wire says it in the client's words.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    NoRecipient,
    TooMany,
    NoSubject,
    SubjectTooLong,
    BodyTooLong,
    UnknownRecipient(i64),
    UnknownList,
    /// The sender is not in the list (the client refuses first).
    NotInList,
    Muted,
    /// A corp the sender is not in.
    NotYourCorp,
    /// An alliance the sender may not mail.
    NotYourAlliance,
}

/// Who a mail to `id` reaches, and whether `id` is a corporation. An
/// alliance's mail needs the sender to be the CEO or a director of a member
/// corporation **[A]**; a corporation's, only to be a member
/// (`corpRegistry.DoesCorpRestrictCorpMails` answers `False` here, the
/// client's "anyone may" default).
fn corp_audience(reg: &Registry, sender: i64, id: i64) -> Result<(Vec<i64>, bool), Refusal> {
    if reg.corporations().get(id).is_some() {
        let mine = reg
            .corporations()
            .membership(sender)
            .map(|m| m.corporation_id);
        if mine != Some(id) {
            return Err(Refusal::NotYourCorp);
        }
        return Ok((reg.corporations().members_of(id), true));
    }
    if crate::alliance::name_of(reg.deltas(), id).is_some() {
        let all = crate::alliance::Alliances::load(reg.deltas());
        let corps: Vec<i64> = all.members_of(id).iter().map(|(c, _)| *c).collect();
        let m = reg
            .corporations()
            .membership(sender)
            .filter(|m| corps.contains(&m.corporation_id))
            .ok_or(Refusal::NotYourAlliance)?;
        let ceo = reg
            .corporations()
            .get(m.corporation_id)
            .is_some_and(|c| c.ceo_id == sender);
        if !ceo && m.roles & crate::corp_roles::CORP_ROLE_DIRECTOR == 0 {
            return Err(Refusal::NotYourAlliance);
        }
        let people = corps
            .iter()
            .flat_map(|c| reg.corporations().members_of(*c))
            .collect();
        return Ok((people, false));
    }
    Err(Refusal::NotYourCorp)
}

/// **Send a mail.** Validates, files it for the sender and every recipient
/// (named characters, the members of the list, the members of the corp or
/// alliance), evicts what the recipients can no longer keep, and schedules
/// `OnMailSent` to each recipient other than the sender. The new id.
pub fn send(reg: &mut Registry, draft: Draft) -> Result<i64, Refusal> {
    let title = draft.title.trim().to_owned();
    if title.is_empty() {
        return Err(Refusal::NoSubject);
    }
    if title.chars().count() > MAX_SUBJECT {
        return Err(Refusal::SubjectTooLong);
    }
    if draft.body.chars().count() > MAX_BODY {
        return Err(Refusal::BodyTooLong);
    }
    let chars: Vec<i64> = {
        let mut seen = BTreeSet::new();
        draft
            .to_chars
            .iter()
            .copied()
            .filter(|c| seen.insert(*c))
            .collect()
    };
    if chars.len() > MAX_RECIPIENTS {
        return Err(Refusal::TooMany);
    }
    if chars.is_empty() && draft.to_list.is_none() && draft.to_corp.is_none() {
        return Err(Refusal::NoRecipient);
    }
    if let Some(c) = chars.iter().find(|c| reg.character(**c).is_none()) {
        return Err(Refusal::UnknownRecipient(*c));
    }
    let mut people: BTreeSet<i64> = chars.iter().copied().collect();
    if let Some(l) = draft.to_list {
        let d = reg.deltas();
        get_list(d, l).ok_or(Refusal::UnknownList)?;
        match role_of(d, l, draft.sender) {
            None => return Err(Refusal::NotInList),
            Some(role::MUTED) => return Err(Refusal::Muted),
            Some(_) => {}
        }
        people.extend(members(d, l).iter().map(|(c, _)| *c));
    }
    let mut corp_is_corp = true;
    if let Some(c) = draft.to_corp {
        let (who, is_corp) = corp_audience(reg, draft.sender, c)?;
        corp_is_corp = is_corp;
        people.extend(who);
    }
    people.retain(|c| reg.character(*c).is_some());
    people.insert(draft.sender);

    let d = reg.deltas_mut();
    let id = next_id(d, KIND_MAIL);
    let m = Message {
        id,
        sender: draft.sender,
        sent: crate::pyutil::filetime_now(),
        to_chars: chars,
        to_list: draft.to_list,
        to_corp: draft.to_corp,
        title,
        body: draft.body,
    };
    d.put(KIND_MAIL, vec![id.to_string()], m.to_row());
    for who in &people {
        let labels = labels_for(*who, &m, corp_is_corp);
        // The sender's client marks a mail read when it is only filed Sent
        // (`mailSvc.OnMailSent`): so does the store, or a relog would
        // disagree with it.
        let read = *who == m.sender && labels == label::SENT;
        put_slot(
            d,
            *who,
            Slot {
                id,
                status: if read { status::READ } else { 0 },
                labels,
            },
        );
        evict(d, *who);
    }
    for (orig, bit) in [
        (draft.reply_to, status::REPLIED),
        (draft.forwarded_from, status::FORWARDED),
    ] {
        update_slots(d, m.sender, &[orig], |s| (s.status | bit, s.labels));
    }
    announce(reg, &m, &people, 0);
    Ok(id)
}

/// A mail the world sends as a mailing list (a welcome mail): from the list
/// to `to`, marked automated, so the client names the list.
pub fn deliver_automated(reg: &mut Registry, list_id: i64, to: &[i64], title: &str, body: &str) {
    let people: BTreeSet<i64> = to
        .iter()
        .copied()
        .filter(|c| reg.character(*c).is_some())
        .collect();
    if people.is_empty() {
        return;
    }
    let d = reg.deltas_mut();
    let id = next_id(d, KIND_MAIL);
    let m = Message {
        id,
        sender: list_id,
        sent: crate::pyutil::filetime_now(),
        to_chars: Vec::new(),
        to_list: Some(list_id),
        to_corp: None,
        title: title.chars().take(MAX_SUBJECT).collect(),
        body: body.chars().take(MAX_BODY).collect(),
    };
    d.put(KIND_MAIL, vec![id.to_string()], m.to_row());
    for who in &people {
        put_slot(
            d,
            *who,
            Slot {
                id,
                status: status::AUTOMATED,
                labels: 0,
            },
        );
        evict(d, *who);
    }
    announce(reg, &m, &people, status::AUTOMATED);
}

fn announce(reg: &mut Registry, m: &Message, people: &BTreeSet<i64>, status: i64) {
    for who in people.iter().filter(|c| **c != m.sender) {
        reg.schedule(
            std::time::Duration::ZERO,
            Sent {
                to: *who,
                message: m.clone(),
                status,
            },
        );
    }
}

// ---- the pushes -----------------------------------------------------------------------------

/// `OnMailSent(messageID, senderID, sentDate, toCharacterIDs, toListID,
/// toCorpOrAllianceID, title, statusMask)`, `mailSvc.OnMailSent`'s own
/// signature **[V]**, to one recipient.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    pub to: i64,
    pub message: Message,
    pub status: i64,
}

impl crate::notify::Push for Sent {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let m = &self.message;
        (
            "OnMailSent",
            vec![
                PyValue::Int(m.id),
                PyValue::Int(m.sender),
                PyValue::Int(m.sent),
                PyValue::List(m.to_chars.iter().map(|c| PyValue::Int(*c)).collect()),
                py_opt(m.to_list),
                py_opt(m.to_corp),
                uni(&m.title),
                PyValue::Int(self.status),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// A mailing list event for one member: `OnMailingListSetOperator(listID)`,
/// `...SetMuted(listID)`, `...SetClear(listID)`, `OnMailingListLeave(listID,
/// characterID)` and `OnMailingListDeleted(listID)`, the handlers'
/// signatures in `mailingListsSvc` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct ListPush {
    pub to: i64,
    pub event: &'static str,
    pub args: Vec<i64>,
}

impl crate::notify::Push for ListPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            self.event,
            self.args.iter().map(|a| PyValue::Int(*a)).collect(),
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// Tell `to` about a list event, on the clock.
pub fn tell(reg: &mut Registry, to: i64, event: &'static str, args: Vec<i64>) {
    reg.schedule(std::time::Duration::ZERO, ListPush { to, event, args });
}

// ---- zlib --------------------------------------------------------------------------------------

/// `data` as a zlib stream of stored (uncompressed) deflate blocks: what
/// `GetBody` answers, since the client calls `zlib.decompress` on it **[V]**
/// and any valid stream will do; std-only means no compressor.
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(65_535).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        let n = u16::try_from(c.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(c);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i64 = 90_000_501;
    const B: i64 = 90_000_502;
    const C: i64 = 90_000_503;

    fn msg(id: i64) -> Message {
        Message {
            id,
            sender: A,
            sent: 1234,
            to_chars: vec![B, C],
            to_list: None,
            to_corp: None,
            title: "Hello\tthere".to_owned(),
            body: "line one\nline two".to_owned(),
        }
    }

    fn held(id: i64) -> Slot {
        Slot {
            id,
            status: 0,
            labels: 1,
        }
    }

    #[test]
    fn a_message_round_trips_through_its_row() {
        let m = msg(7);
        assert_eq!(Message::from_row(7, &m.to_row()), Some(m));
        let m = Message {
            to_chars: vec![],
            to_list: Some(LIST_ID_BASE + 3),
            to_corp: Some(98_000_001),
            ..msg(8)
        };
        assert_eq!(Message::from_row(8, &m.to_row()), Some(m));
    }

    #[test]
    fn ids_count_on_and_are_never_reused() {
        let mut d = Deltas::default();
        let a = next_id(&mut d, KIND_MAIL);
        let b = next_id(&mut d, KIND_MAIL);
        assert_eq!((a, b), (1, 2));
        d.del(KIND_MAIL, &[NEXT.to_owned()]);
        d.put(KIND_MAIL, vec!["5".to_owned()], msg(5).to_row());
        assert_eq!(next_id(&mut d, KIND_MAIL), 6, "past the highest kept");
    }

    #[test]
    fn labels_follow_the_clients_own_filing() {
        let m = msg(1);
        assert_eq!(labels_for(A, &m, true), label::SENT);
        assert_eq!(labels_for(B, &m, true), label::INBOX);
        assert_eq!(labels_for(90_000_599, &m, true), 0, "a list reader");
        let corp = Message {
            to_corp: Some(98_000_001),
            to_chars: vec![A],
            ..msg(2)
        };
        assert_eq!(
            labels_for(A, &corp, true),
            label::INBOX | label::SENT | label::CORPORATION
        );
        assert_eq!(labels_for(B, &corp, false), label::ALLIANCE);
    }

    #[test]
    fn a_mailbox_lists_only_its_own_and_deleting_collects_the_message() {
        let mut d = Deltas::default();
        d.put(KIND_MAIL, vec!["1".to_owned()], msg(1).to_row());
        for who in [A, B, 99] {
            put_slot(&mut d, who, held(1));
        }
        assert_eq!(slots(&d, A).len(), 1);
        assert_eq!(slots(&d, 9).len(), 0, "a prefix is not a key");
        assert_eq!(delete(&mut d, A, &[1]), 1);
        assert!(message(&d, 1).is_some(), "B still holds it");
        delete(&mut d, B, &[1]);
        delete(&mut d, 99, &[1]);
        assert!(message(&d, 1).is_none(), "the last holder took it");
    }

    #[test]
    fn status_updates_touch_only_what_the_holder_has() {
        let mut d = Deltas::default();
        put_slot(&mut d, A, held(1));
        put_slot(
            &mut d,
            A,
            Slot {
                id: 2,
                status: status::TRASHED,
                labels: 1,
            },
        );
        let read = |s: &Slot| (s.status | status::READ, s.labels);
        assert_eq!(update_slots(&mut d, A, &[1, 9], read), 1);
        assert!(slot(&d, A, 1).unwrap().read());
        assert!(!slot(&d, A, 2).unwrap().read());
        assert_eq!(unread_count(&d, A), 0, "the other is trashed");
        update_where(&mut d, A, Slot::trashed, |s| {
            (s.status & !status::TRASHED, s.labels)
        });
        assert_eq!(unread_count(&d, A), 1);
    }

    #[test]
    fn labels_take_the_lowest_free_bit_and_delete_clears_it() {
        let mut d = Deltas::default();
        let one = create_label(&mut d, A, "Work", Some(3)).unwrap();
        let two = create_label(&mut d, A, "Play", None).unwrap();
        assert_eq!((one, two), (16, 32));
        assert_eq!(create_label(&mut d, B, "Work", None), Some(16));
        assert!(create_label(&mut d, A, "  ", None).is_none());
        put_slot(
            &mut d,
            A,
            Slot {
                id: 1,
                status: 0,
                labels: 1 | 16,
            },
        );
        assert!(edit_label(&mut d, A, 16, Some("Job"), None));
        assert_eq!(labels(&d, A)[0].name, "Job");
        assert_eq!(labels(&d, A)[0].color, Some(3), "the colour is kept");
        assert!(delete_label(&mut d, A, 16));
        assert_eq!(slot(&d, A, 1).unwrap().labels, 1);
        assert_eq!(create_label(&mut d, A, "Again", None), Some(16));
        for i in 0..MAX_LABELS {
            let _ = create_label(&mut d, C, &format!("l{i}"), None);
        }
        assert!(create_label(&mut d, C, "one more", None).is_none());
        assert_eq!(labels(&d, C).len(), MAX_LABELS);
    }

    #[test]
    fn lists_are_created_joined_ranked_kicked_and_deleted() {
        let mut d = Deltas::default();
        let id = create_list(&mut d, A, "  Free Beer ", access::ALLOWED, role::DEFAULT, 0).unwrap();
        assert!(id > LIST_ID_BASE);
        assert_eq!(get_list(&d, id).unwrap().name, "Free Beer");
        assert_eq!(
            create_list(&mut d, B, "free beer", access::ALLOWED, role::DEFAULT, 0),
            Err(ListRefusal::NameTaken)
        );
        assert_eq!(
            create_list(&mut d, B, " ", 1, 1, 0),
            Err(ListRefusal::BadName)
        );
        assert_eq!(role_of(&d, id, A), Some(role::OWNER));
        let (l, r) = join(&mut d, B, &[B, 1, 2], "FREE BEER").unwrap();
        assert_eq!((l.id, r), (id, role::DEFAULT));
        assert_eq!(join(&mut d, B, &[B], "free beer").unwrap().1, role::DEFAULT);
        assert_eq!(
            join(&mut d, C, &[C], "nothing").err(),
            Some(ListRefusal::NoSuchList)
        );
        assert_eq!(joined(&d, B).len(), 1);
        assert_eq!(set_roles(&mut d, id, &[B, A, 77], role::MUTED), vec![B]);
        assert_eq!(role_of(&d, id, A), Some(role::OWNER), "the owner's seat");
        assert_eq!(kick(&mut d, id, &[B, A]), vec![B]);
        assert_eq!(members(&d, id), vec![(A, role::OWNER)]);
        assert_eq!(leave(&mut d, A, id), Err(ListRefusal::OwnerCannotLeave));
        assert_eq!(leave(&mut d, B, id), Err(ListRefusal::NotAMember));
        join(&mut d, C, &[C], "free beer").unwrap();
        let mut gone = delete_list(&mut d, id);
        gone.sort_unstable();
        assert_eq!(gone, vec![A, C]);
        assert!(get_list(&d, id).is_none());
        assert!(members(&d, id).is_empty());
    }

    #[test]
    fn a_closed_list_admits_by_override_and_a_block_beats_an_allow() {
        let mut d = Deltas::default();
        let id = create_list(&mut d, A, "Closed", access::BLOCKED, role::DEFAULT, 0).unwrap();
        assert_eq!(
            join(&mut d, B, &[B, 10], "closed").err(),
            Some(ListRefusal::Blocked)
        );
        set_access(&mut d, id, 10, access::ALLOWED);
        assert!(join(&mut d, B, &[B, 10], "closed").is_ok(), "their corp");
        set_access(&mut d, id, C, access::BLOCKED);
        assert_eq!(
            join(&mut d, C, &[C, 10], "closed").err(),
            Some(ListRefusal::Blocked)
        );
        clear_access(&mut d, id, C);
        assert!(join(&mut d, C, &[C, 10], "closed").is_ok());
    }

    #[test]
    fn a_list_round_trips_and_its_rows_carry_what_the_client_reads() {
        let l = List {
            id: LIST_ID_BASE + 1,
            key: "k".into(),
            name: "K".into(),
            owner: A,
            default_access: 1,
            default_member_access: 1,
            cost: 0,
            welcome: Some(("t".into(), "b".into())),
        };
        assert_eq!(List::from_row(l.id, &l.to_row()), Some(l.clone()));
        let PyValue::Instance { state, .. } = joined_row(&l, role::OPERATOR) else {
            panic!("a KeyVal")
        };
        let names: Vec<String> = state
            .dict_as_kv()
            .unwrap()
            .iter()
            .filter_map(|(k, _)| {
                crate::pyutil::as_bytes(k).map(|b| String::from_utf8_lossy(b).into_owned())
            })
            .collect();
        for want in [
            "id",
            "name",
            "displayName",
            "isMuted",
            "isOperator",
            "isOwner",
        ] {
            assert!(names.iter().any(|n| n == want), "{want}");
        }
    }

    /// A stored-block zlib stream: the header, the block, Adler-32 (the
    /// well-known `abc` stream's checksum is `024d0127`).
    #[test]
    fn zlib_stored_is_a_valid_stream() {
        let z = zlib_stored(b"abc");
        assert_eq!(
            z,
            vec![
                0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 0x02, 0x4d, 0x01, 0x27
            ]
        );
        assert_eq!(zlib_stored(b"").len(), 2 + 5 + 4);
        let z = zlib_stored(&vec![7_u8; 70_000]);
        assert_eq!(z.len(), 2 + 5 + 65_535 + 5 + (70_000 - 65_535) + 4);
        assert_eq!(z[2], 0, "the first block is not the last");
    }

    #[test]
    fn the_pushes_are_the_handlers_signatures() {
        use crate::notify::Push;
        let p = Sent {
            to: B,
            message: msg(3),
            status: 0,
        };
        let (event, args) = p.to_push();
        assert_eq!(event, "OnMailSent");
        assert_eq!(args.len(), 8);
        assert_eq!(p.audience(), Some(vec![B]));
        let l = ListPush {
            to: B,
            event: "OnMailingListLeave",
            args: vec![5, B],
        };
        assert_eq!(l.to_push().1.len(), 2);
    }
}
