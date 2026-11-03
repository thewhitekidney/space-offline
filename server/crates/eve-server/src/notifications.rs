//! **Notifications** (NOTIF-1, wave C-2): a per-character store of the
//! client's notifications, the `notify` API other packets call, and the
//! first producers.
//!
//! Lane note: `docs/lanes/notif-1.md`. The wire is
//! `service/notifications.rs` (`notificationMgr`). EVE mail is a separate,
//! later packet (MAIL-1): nothing here reads or writes mail, and a mail
//! store must not reach into this one.
//!
//! # What the client does with a notification [V]
//!
//! `notificationSvc` (`eve/client/script/ui/services/mail/
//! notificationSvc.py`) holds `sm.RemoteSvc('notificationMgr')` and reads
//! every notification as a DTO with seven attributes: `notificationID,
//! typeID, senderID, receiverID, processed, created, data`
//! (`Notification.FromDTO`). Two windows draw them:
//!
//! - **the notification centre** (`notificationUI._NotificationProvider`):
//!   `GetAllFormattedNotifications(fromID=lastClearedNotificationId)`, which
//!   is `GetAllNotifications(fromID)` minus the deprecated types. "Clear
//!   history" remembers the highest id seen, so the server answers the ids
//!   **above** `fromID` [I].
//! - **the mail window's Notifications tab** (`neocom/notifications.py`):
//!   one entry per group, `GetByGroupID(groupID)` for a group and
//!   `GetUnprocessed()` for "unread"; both drop the types
//!   `IsTypeInCommunications` refuses (`notificationconst.
//!   nonCommunicationTypes`: kill reports, contracts, skills ...), which
//!   therefore show in the centre only.
//!
//! A new one arrives as `OnNotificationReceived(notificationID, typeID,
//! senderID, created, data)`; `notificationSvc` inserts it in its caches and
//! scatters `OnNewNotificationReceived`, which pops it up. Formatting is
//! entirely the client's: `GetFormatterForType(typeID)` for the newer types
//! (`notifications/common/formatters/*`), else `notificationUtil.Format`,
//! which calls `localization.GetByLabel(label, **data)` -- so **the data
//! dict's keys are the label's parameter names**, and each producer below
//! cites the formatter or label that names them. `data` may be a dict or a
//! YAML string (`TryUnYamlNotificationData` only parses a `basestring`); a
//! dict is sent, because `Format` requires `type(data) is DictType` before
//! it adds `notification_senderID` and friends to it.
//!
//! `senderID` goes through `cfg.eveowners` (`PrimeOwners`, and the mail
//! window draws `cfg.eveowners.Get(senderID).ownerName`), so it must be an
//! owner the client can name: a character or player corp we serve, or one
//! the client prefills from its own data -- factions, NPC corporations, NPC
//! characters and owner `1`, "EVE System" (`eveCfg.LoadEveOwners`) [V].
//!
//! # The store
//!
//! One `notification` delta row per notification, keyed `(receiver, id)`,
//! plus a `next` row for the id counter (ids are global, so they never
//! repeat across characters). [`KEEP_PER_CHARACTER`] are kept per
//! character, oldest evicted first. The functions on [`Deltas`] are pure;
//! [`notify`] is the world's entry point.
//!
//! # Sending one (for other packets)
//!
//! ```text
//! crate::notifications::notify(reg, character, TYPE, data([("key", Value::Int(n)), ...]));
//! ```
//!
//! stores it and pushes `OnNotificationReceived` to the character on the
//! clock (safe from the world tick as well as from a call); [`notify_from`]
//! names a sender, [`notify_at`] also a time. `TYPE` is the client's type id
//! and the keys are the ones its formatter reads: find the formatter in
//! `notificationFormatMapping.formatterByNotificationType`, or else the
//! `(subject, body, params)` row in `notificationUtil.formatters`, and read
//! the label texts for their `{parameters}`. The group the mail window files
//! it under follows from the type ([`GROUPS`]).
//!
//! # The producers
//!
//! Here: a kill ([`on_kill`], a `kill_hooks::PLAYER` row), insurance sold
//! and paid ([`insurance_issued`], [`insurance_payout`]) and a militia rank
//! change ([`militia_rank`]). In `service/notifications.rs`, because they
//! read the corporations: militia requests (`militia_request`), corp
//! applications and leaving (`corp_events`). **Deliberately none** for
//! contracts (the client builds those notifications itself from the
//! `OnContract*` pushes, `contracts.py` `MakeAndScatterNotification`), NPC
//! bounty payouts (client-made from `OnBountyAddedToPayout`,
//! `bountyNotificationAdapter`) or alliance applications (this build has no
//! notification type for them) **[V]**.

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`): `receiver id ->
/// type sender created processed data`, and `next -> n`.
pub const KIND: &str = "notification";

/// The `next` row's key.
const NEXT: &str = "next";

/// Notifications kept per character, oldest evicted first **[A]**: a
/// little over 100 bytes each, so 50 bot pilots fighting all day stay
/// near a megabyte of world file. Falsified by a player missing a
/// notification they had not deleted within a week of play, or a world file
/// that grows past it.
pub const KEEP_PER_CHARACTER: usize = 250;

/// The most ids one mark or delete call acts on: the client refuses more
/// itself (`const.notificationsMaxUpdated`, `appConst`) **[V]**, so a longer
/// list is not the client's and is cut here.
pub const MAX_UPDATED: usize = 100;

/// `senderID` for a notification the world sends rather than a pilot or a
/// corp: owner `1`, which the client names itself ("EVE System",
/// `eveCfg.LoadEveOwners`) **[V]** for the id; **[A]** that retail used it
/// for kill reports and insurance (falsified by a retail notification of
/// those types naming CONCORD or the SCC).
pub const SENDER_SYSTEM: i64 = 1;

/// The notification type ids this crate sends
/// (`eve/common/script/util/notificationconst.py`) **[V]**. The corp ones
/// are `service::notifications::corp_events::kind`, beside their producers.
pub mod kind {
    /// A medal awarded (`notificationTypeCharMedalMsg`); data `medalID`, `corpID`, `reason`.
    pub const CHAR_MEDAL: i64 = 3;
    /// A dividend paid (`notificationTypeCorpDividendMsg`); data `corpID`, `isMembers`,
    /// `payout` (`ParamFmtCorpDividendNotification` passes the data to the label).
    pub const CORP_DIVIDEND: i64 = 23;
    pub const INSURANCE_PAYOUT: i64 = 35;
    pub const INSURANCE_ISSUED: i64 = 55;
    pub const INSURANCE_EXPIRED: i64 = 54;
    pub const FW_CHAR_RANK_LOSS: i64 = 64;
    pub const FW_CHAR_RANK_GAIN: i64 = 65;
    pub const FW_CORP_JOIN_REQUEST: i64 = 81;
    pub const FW_CORP_LEAVE_REQUEST: i64 = 82;
    pub const FW_CORP_JOIN_WITHDRAW: i64 = 83;
    pub const FW_CORP_LEAVE_WITHDRAW: i64 = 84;
    pub const BOUNTY_CLAIM_MSG: i64 = 14;
    pub const BOUNTY_YOUR_BOUNTY_CLAIMED: i64 = 111;
    pub const CHAR: i64 = 112;
    pub const CORP: i64 = 113;
    pub const ALLIANCE: i64 = 114;
    pub const KILL_REPORT_VICTIM: i64 = 140;
    pub const KILL_REPORT_FINAL_BLOW: i64 = 141;
}

/// The groups of the mail window's Notifications tab
/// (`notificationconst.group*`) **[V]**.
pub mod group {
    /// Not a group of types: the "unread" entry, `GetUnprocessed`.
    pub const UNREAD: i64 = 0;
    pub const AGENTS: i64 = 1;
    pub const BILLS: i64 = 2;
    pub const CORP: i64 = 3;
    pub const MISC: i64 = 4;
    pub const OLD: i64 = 5;
    pub const SOV: i64 = 6;
    pub const STRUCTURES: i64 = 7;
    pub const WAR: i64 = 8;
    pub const CONTACTS: i64 = 9;
    pub const BOUNTIES: i64 = 10;
    pub const INSURANCE: i64 = 11;
    pub const SKILL_TRAINING: i64 = 12;
    pub const CCP: i64 = 14;
    pub const RAFFLE: i64 = 15;
    pub const INSURGENCIES: i64 = 16;
    pub const FACTION_WARFARE: i64 = 17;
}

/// `notificationconst.groupTypes`, transcribed: every type the client puts
/// in a group **[V]**. `GetTypeGroup` answers the group a type is listed in
/// and `None` for the rest (`StructureLostDockingAccess`, 195, is one: a
/// formatter, but no group). Protocol constants, not static data: a
/// producer of any type lands in the tab the client draws it in without
/// touching this module.
pub const GROUPS: &[(i64, &[i64])] = &[
    (group::SKILL_TRAINING, &[1002, 1000, 1005, 1001, 1033, 6020]),
    (group::AGENTS, &[66, 69, 70, 71, 72, 73, 74, 247, 248]),
    (group::BILLS, &[4, 9, 10, 11, 12, 13, 50, 250]),
    (group::CONTACTS, &[89, 90, 2001, 2002, 166, 251]),
    (
        group::CORP,
        &[
            2, 3, 16, 17, 142, 18, 139, 19, 20, 21, 22, 23, 25, 26, 85, 92, 128, 129, 130, 131,
            143, 144, 145, 146, 260, 262, 261, 263, 264, 265, 290, 292, 296, 297, 293, 294, 295,
            270,
        ],
    ),
    (group::INSURANCE, &[34, 35, 36, 54, 55]),
    (
        group::MISC,
        &[
            138, 32, 33, 51, 52, 53, 56, 57, 67, 68, 91, 115, 116, 117, 118, 119, 120, 133, 1003,
            1004, 1006, 1008, 1007, 1021, 1022, 1030, 1031, 1032, 200, 199, 201, 1050, 3001, 1060,
            1100, 1101, 226, 227, 228, 1015, 1016, 1017, 1018, 253, 252, 6001, 6002, 6010, 6011,
            6012, 276, 275, 280, 6013, 6032,
        ],
    ),
    (group::OLD, &[1]),
    (
        group::SOV,
        &[
            37, 38, 39, 40, 41, 42, 43, 44, 48, 46, 47, 86, 87, 88, 147, 148, 149, 150, 152, 160,
            161, 162, 163, 164, 165, 167, 168, 169,
        ],
    ),
    (
        group::STRUCTURES,
        &[
            45, 49, 75, 76, 77, 78, 79, 80, 93, 94, 95, 132, 181, 182, 183, 184, 185, 186, 187,
            188, 190, 191, 192, 198, 207, 208, 209, 210, 211, 249, 202, 203, 204, 205, 206, 254,
            6030, 6031, 6040, 6041, 281, 282, 283, 284, 285, 286, 287, 288,
        ],
    ),
    (
        group::WAR,
        &[
            5, 225, 6, 7, 8, 27, 224, 28, 29, 30, 31, 58, 59, 60, 61, 62, 63, 64, 65, 81, 82, 83,
            84, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 121, 122,
            123, 124, 125, 126, 127, 140, 141, 221, 222, 223, 229, 230, 231, 232, 233, 234, 235,
            236, 237, 238, 239, 240, 241, 242, 246, 289,
        ],
    ),
    (group::BOUNTIES, &[14, 111, 112, 113, 114]),
    (group::CCP, &[170, 171, 172, 173, 4001, 4002]),
    (group::RAFFLE, &[243, 244, 245]),
    (
        group::INSURGENCIES,
        &[1110, 1111, 1112, 1113, 1114, 1115, 1116],
    ),
    (group::FACTION_WARFARE, &[298, 299, 300, 301, 302, 303]),
];

/// `notificationconst.GetTypeGroup`: the group `type_id` is listed in.
pub fn group_of(type_id: i64) -> Option<i64> {
    GROUPS
        .iter()
        .find(|(_, types)| types.contains(&type_id))
        .map(|(g, _)| *g)
}

// ---- the data dict ----------------------------------------------------------------

/// One value in a notification's `data`. The client reads `data` as a
/// Python dict whose values the formatters pass to `localization` or read
/// by key, so these are the shapes it needs: ids and amounts, text, flags,
/// and lists and dicts of them.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// Sent as `unicode`: names and free text may be any language.
    Text(String),
    List(Vec<Value>),
    Dict(Data),
}

/// A notification's `data`: the formatter's keys and their values, in the
/// order given.
pub type Data = Vec<(String, Value)>;

/// `data` from `(key, value)` pairs, for a producer's one line.
pub fn data<const N: usize>(pairs: [(&str, Value); N]) -> Data {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

impl Value {
    /// The marshal value the client receives.
    pub fn to_py(&self) -> PyValue {
        match self {
            Value::None => PyValue::None,
            Value::Bool(b) => PyValue::Bool(*b),
            Value::Int(n) => PyValue::Int(*n),
            Value::Real(x) => PyValue::Real(*x),
            Value::Text(s) => PyValue::Unicode(s.clone()),
            Value::List(v) => PyValue::List(v.iter().map(Value::to_py).collect()),
            Value::Dict(d) => dict_py(d),
        }
    }

    fn encode(&self, out: &mut String) {
        match self {
            Value::None => out.push('N'),
            Value::Bool(true) => out.push('T'),
            Value::Bool(false) => out.push('F'),
            Value::Int(n) => {
                out.push('i');
                out.push_str(&n.to_string());
                out.push(';');
            }
            Value::Real(x) => {
                out.push('r');
                out.push_str(&x.to_string());
                out.push(';');
            }
            Value::Text(s) => encode_text(s, out),
            Value::List(v) => {
                out.push('l');
                out.push_str(&v.len().to_string());
                out.push(':');
                for x in v {
                    x.encode(out);
                }
            }
            Value::Dict(d) => encode_dict(d, out),
        }
    }

    fn decode(s: &str, at: &mut usize) -> Option<Value> {
        let tag = *s.as_bytes().get(*at)?;
        *at += 1;
        Some(match tag {
            b'N' => Value::None,
            b'T' => Value::Bool(true),
            b'F' => Value::Bool(false),
            b'i' => Value::Int(until(s, at, b';')?.parse().ok()?),
            b'r' => Value::Real(until(s, at, b';')?.parse().ok()?),
            b's' => Value::Text(decode_text_body(s, at)?),
            b'l' => {
                let n: usize = until(s, at, b':')?.parse().ok()?;
                let mut v = Vec::with_capacity(n.min(64));
                for _ in 0..n {
                    v.push(Value::decode(s, at)?);
                }
                Value::List(v)
            }
            b'd' => Value::Dict(decode_dict_body(s, at)?),
            _ => return None,
        })
    }
}

/// A marshal dict from `data` (wire order is value, then key).
fn dict_py(d: &Data) -> PyValue {
    PyValue::Dict(
        d.iter()
            .map(|(k, v)| (v.to_py(), crate::pyutil::bstr(k)))
            .collect(),
    )
}

/// `s<len>:<bytes>`: length-prefixed, so the text needs no escaping (the
/// world file escapes tabs and newlines itself).
fn encode_text(s: &str, out: &mut String) {
    out.push('s');
    out.push_str(&s.len().to_string());
    out.push(':');
    out.push_str(s);
}

fn encode_dict(d: &Data, out: &mut String) {
    out.push('d');
    out.push_str(&d.len().to_string());
    out.push(':');
    for (k, v) in d {
        encode_text(k, out);
        v.encode(out);
    }
}

/// The text up to `end`, consumed with it.
fn until<'a>(s: &'a str, at: &mut usize, end: u8) -> Option<&'a str> {
    let rest = s.get(*at..)?;
    let n = rest.bytes().position(|b| b == end)?;
    *at += n + 1;
    Some(&rest[..n])
}

fn decode_text_body(s: &str, at: &mut usize) -> Option<String> {
    let n: usize = until(s, at, b':')?.parse().ok()?;
    let text = s.get(*at..at.checked_add(n)?)?;
    *at += n;
    Some(text.to_owned())
}

fn decode_dict_body(s: &str, at: &mut usize) -> Option<Data> {
    let n: usize = until(s, at, b':')?.parse().ok()?;
    let mut d = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        if s.as_bytes().get(*at) != Some(&b's') {
            return None;
        }
        *at += 1;
        let k = decode_text_body(s, at)?;
        d.push((k, Value::decode(s, at)?));
    }
    Some(d)
}

/// `data` as one delta field.
pub fn encode_data(d: &Data) -> String {
    let mut out = String::new();
    encode_dict(d, &mut out);
    out
}

/// The inverse of [`encode_data`]; `None` for a field it cannot read whole.
pub fn decode_data(s: &str) -> Option<Data> {
    let mut at = 0;
    if s.as_bytes().first() != Some(&b'd') {
        return None;
    }
    at += 1;
    let d = decode_dict_body(s, &mut at)?;
    (at == s.len()).then_some(d)
}

// ---- the store ---------------------------------------------------------------------

/// One stored notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: i64,
    pub receiver: i64,
    pub type_id: i64,
    pub sender: i64,
    /// Blue FILETIME: `notificationUI` sorts by it and the mail window
    /// formats it as a date.
    pub created: BlueTime,
    /// Read. The client's word is `processed`.
    pub processed: bool,
    pub data: Data,
}

impl Notification {
    fn key(&self) -> Vec<String> {
        vec![self.receiver.to_string(), self.id.to_string()]
    }

    /// The delta row's value: type, sender, created, processed, data.
    pub fn to_row(&self) -> Vec<String> {
        vec![
            self.type_id.to_string(),
            self.sender.to_string(),
            self.created.to_string(),
            u8::from(self.processed).to_string(),
            encode_data(&self.data),
        ]
    }

    /// The inverse of [`Self::to_row`] with its key; `None` for a row it
    /// cannot read (the `next` row among them).
    pub fn from_row(key: &[String], v: &[String]) -> Option<Notification> {
        let [receiver, id] = key else { return None };
        let [type_id, sender, created, processed, data] = v else {
            return None;
        };
        Some(Notification {
            id: id.parse().ok()?,
            receiver: receiver.parse().ok()?,
            type_id: type_id.parse().ok()?,
            sender: sender.parse().ok()?,
            created: created.parse().ok()?,
            processed: processed == "1",
            data: decode_data(data)?,
        })
    }

    /// The group the mail window files it under.
    pub fn group(&self) -> Option<i64> {
        group_of(self.type_id)
    }

    /// The DTO `Notification.FromDTO` reads, by attribute **[V]**. A
    /// `KeyVal`: the client's own `Notification` class is not whitelisted.
    pub fn row(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("notificationID", PyValue::Int(self.id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("senderID", PyValue::Int(self.sender)),
            ("receiverID", PyValue::Int(self.receiver)),
            ("processed", PyValue::Bool(self.processed)),
            ("created", PyValue::Int(self.created)),
            ("data", dict_py(&self.data)),
        ])
    }
}

/// Which of a character's notifications a mark or delete acts on: the
/// three shapes of the client's calls (one list of ids, a group, all).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selection<'a> {
    Ids(&'a [i64]),
    Group(i64),
    All,
}

impl Selection<'_> {
    fn admits(&self, n: &Notification) -> bool {
        match self {
            Selection::Ids(ids) => ids.iter().take(MAX_UPDATED).any(|i| *i == n.id),
            Selection::Group(g) => n.group() == Some(*g),
            Selection::All => true,
        }
    }
}

/// The next notification id, counted on: the `next` row, or one past the
/// highest kept.
fn next_id(d: &mut Deltas) -> i64 {
    let stored = d
        .of(KIND)
        .get(&vec![NEXT.to_owned()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok());
    let highest = d
        .of(KIND)
        .keys()
        .filter(|k| k.len() == 2)
        .filter_map(|k| k[1].parse::<i64>().ok())
        .max()
        .unwrap_or(0);
    let id = stored.unwrap_or(0).max(highest + 1).max(1);
    d.put(KIND, vec![NEXT.to_owned()], vec![(id + 1).to_string()]);
    id
}

/// Every notification `receiver` holds, newest first.
pub fn of(d: &Deltas, receiver: i64) -> Vec<Notification> {
    let who = receiver.to_string();
    let mut v: Vec<Notification> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&who))
        .filter_map(|(k, v)| Notification::from_row(k, v))
        .collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

/// One of `receiver`'s notifications.
pub fn get(d: &Deltas, receiver: i64, id: i64) -> Option<Notification> {
    let key = vec![receiver.to_string(), id.to_string()];
    Notification::from_row(&key, d.of(KIND).get(&key)?)
}

/// `GetAllNotifications(fromID)`: those above `from_id`, newest first.
pub fn after(d: &Deltas, receiver: i64, from_id: i64) -> Vec<Notification> {
    of(d, receiver)
        .into_iter()
        .filter(|n| n.id > from_id)
        .collect()
}

/// `GetUnprocessed()`: the unread ones, newest first.
pub fn unread(d: &Deltas, receiver: i64) -> Vec<Notification> {
    of(d, receiver)
        .into_iter()
        .filter(|n| !n.processed)
        .collect()
}

/// `GetByGroupID(groupID)`: one group's, read or not, newest first.
pub fn in_group(d: &Deltas, receiver: i64, group: i64) -> Vec<Notification> {
    of(d, receiver)
        .into_iter()
        .filter(|n| n.group() == Some(group))
        .collect()
}

/// **Store one** for `receiver`, evicting their oldest past
/// [`KEEP_PER_CHARACTER`]. Pure: the push is [`notify`]'s.
pub fn insert(
    d: &mut Deltas,
    receiver: i64,
    type_id: i64,
    sender: i64,
    created: BlueTime,
    data: Data,
) -> Notification {
    let n = Notification {
        id: next_id(d),
        receiver,
        type_id,
        sender,
        created,
        processed: false,
        data,
    };
    d.put(KIND, n.key(), n.to_row());
    let mut ids: Vec<i64> = of(d, receiver).iter().map(|n| n.id).collect();
    if ids.len() > KEEP_PER_CHARACTER {
        ids.sort_unstable();
        for id in &ids[..ids.len() - KEEP_PER_CHARACTER] {
            d.del(KIND, &[receiver.to_string(), id.to_string()]);
        }
    }
    n
}

/// Mark `receiver`'s selected notifications read; how many changed. Another
/// character's ids are never touched: the key carries the receiver.
pub fn mark_read(d: &mut Deltas, receiver: i64, sel: Selection<'_>) -> usize {
    let mut changed = 0;
    for mut n in of(d, receiver) {
        if !n.processed && sel.admits(&n) {
            n.processed = true;
            d.put(KIND, n.key(), n.to_row());
            changed += 1;
        }
    }
    changed
}

/// Delete `receiver`'s selected notifications; how many went.
pub fn delete(d: &mut Deltas, receiver: i64, sel: Selection<'_>) -> usize {
    let mut gone = 0;
    for n in of(d, receiver) {
        if sel.admits(&n) {
            d.del(KIND, &n.key());
            gone += 1;
        }
    }
    gone
}

// ---- the world's entry point ---------------------------------------------------------

/// `OnNotificationReceived(notificationID, typeID, senderID, created, data)`
/// (`notificationSvc.__notifyevents__`, the handler's own signature)
/// **[V]**, to the receiver.
#[derive(Debug, Clone, PartialEq)]
pub struct Received(pub Notification);

impl crate::notify::Push for Received {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let n = &self.0;
        (
            "OnNotificationReceived",
            vec![
                PyValue::Int(n.id),
                PyValue::Int(n.type_id),
                PyValue::Int(n.sender),
                PyValue::Int(n.created),
                dict_py(&n.data),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.0.receiver])
    }
}

/// **Notify `receiver`** with a notification of `type_id` carrying `data`,
/// from [`SENDER_SYSTEM`]. The one call a producer makes. `None` when the
/// receiver is not a character of this world; otherwise the id.
pub fn notify(reg: &mut Registry, receiver: i64, type_id: i64, data: Data) -> Option<i64> {
    notify_from(reg, receiver, type_id, SENDER_SYSTEM, data)
}

/// [`notify`] from a named sender (a pilot, a corporation, a faction).
pub fn notify_from(
    reg: &mut Registry,
    receiver: i64,
    type_id: i64,
    sender: i64,
    data: Data,
) -> Option<i64> {
    notify_at(
        reg,
        receiver,
        type_id,
        sender,
        crate::pyutil::filetime_now(),
        data,
    )
}

/// [`notify_from`] dated `created`, for an event that carries its own time.
///
/// Stored, then pushed **on the clock**, addressed to the receiver: a
/// producer may run inside the world tick (a kill), where nobody's call is
/// being answered and `pending` would reach whoever spoke next. A receiver
/// who is offline finds it in `GetAllNotifications` at the next login.
pub fn notify_at(
    reg: &mut Registry,
    receiver: i64,
    type_id: i64,
    sender: i64,
    created: BlueTime,
    data: Data,
) -> Option<i64> {
    reg.character(receiver)?;
    let n = insert(reg.deltas_mut(), receiver, type_id, sender, created, data);
    let id = n.id;
    reg.schedule(std::time::Duration::ZERO, Received(n));
    Some(id)
}

// ---- producers -------------------------------------------------------------------------

/// `crimewatch.util.GetKillReportHashValue` **[V]**: the SHA-1 hex digest of
/// `'%s%s%s%s' % (victimCharacterID, finalCharacterID, victimShipTypeID,
/// killTime)`, `None` printed as `None`. The notification carries it beside
/// the kill id, and the centre opens the report with
/// `warStatisticMgr.GetKillMail(killMailID, killMailHash)`.
pub fn kill_hash(r: &crate::killmail::Report) -> String {
    kill_hash_of(r.victim, r.final_blow.character, r.ship_type, r.time)
}

/// [`kill_hash`] from the four fields the client hashes (a client, or a test,
/// that holds a report's `KeyVal` and not our `Report`).
pub fn kill_hash_of(victim: i64, final_blow: Option<i64>, ship_type: i64, time: i64) -> String {
    let fin = final_blow.map_or_else(|| "None".to_owned(), |c| c.to_string());
    let text = format!("{victim}{fin}{ship_type}{time}");
    hex(&sha1(text.as_bytes()))
}

/// **The `kill_hooks::PLAYER` row** after `killmail_on_kill`: the victim
/// gets a kill report notification (type 140) and the final blow's pilot,
/// when a character other than the victim, another (141), each linking the
/// mail `killmail::on_kill` just wrote.
///
/// Data: `{killMailID, killMailHash, victimShipTypeID}` for the victim
/// (`KillMailVictimFormatter.MakeData`) and `{killMailID, killMailHash,
/// victimID, victimShipTypeID}` for the final blow
/// (`KillMailFinalBlowFormatter.MakeData`) **[V]**. Only these two are told
/// (the only kill report types there are); the other attackers find it in
/// their combat log.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    let Some(report) = crate::killmail::involving(w, ev.victim_character, None, 1)
        .into_iter()
        .find(|r| r.victim == ev.victim_character && r.time == ev.when)
    else {
        return;
    };
    let hash = Value::Text(kill_hash(&report));
    notify_at(
        w,
        report.victim,
        kind::KILL_REPORT_VICTIM,
        SENDER_SYSTEM,
        report.time,
        data([
            ("killMailID", Value::Int(report.kill_id)),
            ("killMailHash", hash.clone()),
            ("victimShipTypeID", Value::Int(report.ship_type)),
        ]),
    );
    if let Some(killer) = report.final_blow.character.filter(|k| *k != report.victim) {
        notify_at(
            w,
            killer,
            kind::KILL_REPORT_FINAL_BLOW,
            SENDER_SYSTEM,
            report.time,
            data([
                ("killMailID", Value::Int(report.kill_id)),
                ("killMailHash", hash),
                ("victimID", Value::Int(report.victim)),
                ("victimShipTypeID", Value::Int(report.ship_type)),
            ]),
        );
    }
}

/// **Insurance paid** (CP44, `service::insurance::on_kill`): type 35 to the
/// owner, `{amount, itemID, payout}` (`bodyInsurancePayout` names `amount`
/// and `itemID`; `ParamFmtInsurancePayout` reads `payout` and adds the
/// "default payout for an uninsured ship" line when it is true) **[V]**.
/// `itemID` is the lost ship **[I]** (the label prints it as the "RefID");
/// `payout` is false, since only a contract pays here.
pub fn insurance_payout(w: &mut Registry, owner: i64, ship_id: i64, amount: f64) {
    insurance_payout_kind(w, owner, ship_id, amount, false);
}

/// **The default payout of an uninsured ship** (#134): the same type 35 with
/// `payout` true, the flag `ParamFmtInsurancePayout` turns into the "default
/// payout for an uninsured ship" line **[V]**. Sent to the owner at death of
/// any uninsured loss, and of a stolen hull, who holds no policy on it.
pub fn insurance_default_payout(w: &mut Registry, owner: i64, ship_id: i64, amount: f64) {
    insurance_payout_kind(w, owner, ship_id, amount, true);
}

fn insurance_payout_kind(w: &mut Registry, owner: i64, ship_id: i64, amount: f64, default: bool) {
    notify(
        w,
        owner,
        kind::INSURANCE_PAYOUT,
        data([
            ("amount", Value::Real(amount)),
            ("itemID", Value::Int(ship_id)),
            ("payout", Value::Bool(default)),
        ]),
    );
}

/// **A contract sold** (CP44, `service::insurance::insure`): type 55 to the
/// owner. `bodyInsuranceIssued` names `typeID`, `itemID`, `shipName`,
/// `typeID2` (added from `typeID` by its parameter function), `level`,
/// `endDate` and `numWeeks`; `bodyInsuranceExpired`, the same contract's
/// later notification, adds `startDate` **[V]**. `level` is the package as
/// a whole percentage and `shipName` the hull's type name **[I]** (items
/// carry no names here).
pub fn insurance_issued(w: &mut Registry, c: &crate::service::insurance::Contract) {
    let ship_name = crate::typedata::installed_in(w.content())
        .table()
        .get(c.type_id)
        .map(|t| t.name.clone())
        .unwrap_or_default();
    notify(
        w,
        c.owner_id,
        kind::INSURANCE_ISSUED,
        data([
            ("itemID", Value::Int(c.ship_id)),
            ("typeID", Value::Int(c.type_id)),
            ("shipName", Value::Text(ship_name)),
            ("level", Value::Int((c.fraction * 100.0).round() as i64)),
            ("startDate", Value::Int(c.start)),
            ("endDate", Value::Int(c.end)),
            (
                "numWeeks",
                Value::Int(crate::service::insurance::CONTRACT_WEEKS),
            ),
        ]),
    );
}

/// **A contract ran out** (X-INSURE, `service::insurance::prune`): type 54
/// to the owner, the issued notification's fields (`bodyInsuranceExpired`
/// adds `startDate` to `bodyInsuranceIssued`'s) **[V]**; `level` and
/// `shipName` are **[I]** as for [`insurance_issued`].
pub fn insurance_expired(w: &mut Registry, c: &crate::service::insurance::Contract) {
    let ship_name = crate::typedata::installed_in(w.content())
        .table()
        .get(c.type_id)
        .map(|t| t.name.clone())
        .unwrap_or_default();
    notify(
        w,
        c.owner_id,
        kind::INSURANCE_EXPIRED,
        data([
            ("itemID", Value::Int(c.ship_id)),
            ("typeID", Value::Int(c.type_id)),
            ("shipName", Value::Text(ship_name)),
            ("level", Value::Int((c.fraction * 100.0).round() as i64)),
            ("startDate", Value::Int(c.start)),
            ("endDate", Value::Int(c.end)),
            (
                "numWeeks",
                Value::Int(crate::service::insurance::CONTRACT_WEEKS),
            ),
        ]),
    );
}

/// **A militia rank changed** (`facWarMgr.CheckForRankChange`): type 65 up,
/// 64 down, `{factionID, newRank}` (`FormatFWCharRankGainNotification` and
/// `...LossNotification` read both and ask the client's `facwar` service
/// for the rank's name) **[V]**. A first observation (`old` is `None`) is
/// not a promotion and sends nothing **[A]**: falsified by a retail rank-1
/// notification on enlisting.
pub fn militia_rank(w: &mut Registry, ch: i64, faction: i64, old: Option<i64>, new: i64) {
    let Some(old) = old.filter(|o| *o != new) else {
        return;
    };
    let type_id = if new > old {
        kind::FW_CHAR_RANK_GAIN
    } else {
        kind::FW_CHAR_RANK_LOSS
    };
    notify_from(
        w,
        ch,
        type_id,
        faction,
        data([
            ("factionID", Value::Int(faction)),
            ("newRank", Value::Int(new)),
        ]),
    );
}

// ---- SHA-1 (FIPS 180-4), for the kill report hash ----------------------------------

/// SHA-1 of `data`. std-only, as everything here is; `eve-probe` has its
/// own copy, and the server does not depend on that crate.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// Lowercase hex, as `hexdigest()` prints it.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::notifications::corp_events::kind as corp_kind;

    /// Invented characters and types: nothing here is a client row.
    const ALICE: i64 = 90_000_201;
    const BOB: i64 = 90_000_202;
    const SOME_TYPE: i64 = 9_001;
    const T: BlueTime = 134_200_000_000_000_000;

    fn stored(d: &mut Deltas, who: i64, type_id: i64) -> Notification {
        insert(
            d,
            who,
            type_id,
            SENDER_SYSTEM,
            T,
            data([("charID", Value::Int(who))]),
        )
    }

    /// Every value shape round-trips through the delta field, text with the
    /// characters the world file and the encoding care about included, and
    /// a field cut short or padded is refused rather than half-read.
    #[test]
    fn data_round_trips_through_its_field() {
        let d = data([
            ("n", Value::None),
            ("t", Value::Bool(true)),
            ("f", Value::Bool(false)),
            ("i", Value::Int(-42)),
            ("r", Value::Real(1234.5678)),
            ("s", Value::Text("tab\there;s5:d1:\nnæ".to_owned())),
            ("", Value::Text(String::new())),
            (
                "l",
                Value::List(vec![
                    Value::Int(1),
                    Value::List(vec![Value::Text("x".to_owned())]),
                ]),
            ),
            ("d", Value::Dict(data([("k", Value::Real(0.25))]))),
        ]);
        let field = encode_data(&d);
        assert_eq!(decode_data(&field), Some(d.clone()));
        let escaped = crate::store::unesc(&crate::store::esc(&field));
        assert_eq!(decode_data(&escaped), Some(d));
        assert_eq!(decode_data(&field[..field.len() - 1]), None);
        assert_eq!(decode_data(&format!("{field}N")), None);
        assert_eq!(decode_data("garbage"), None);
        assert_eq!(decode_data(""), None);
    }

    /// A notification round-trips through its row; the `next` row is not
    /// one.
    #[test]
    fn a_notification_round_trips_through_its_row() {
        let mut d = Deltas::default();
        let n = stored(&mut d, ALICE, SOME_TYPE);
        assert_eq!(get(&d, ALICE, n.id), Some(n.clone()));
        assert_eq!(Notification::from_row(&n.key(), &n.to_row()), Some(n));
        assert_eq!(
            Notification::from_row(&[NEXT.to_owned()], &["5".to_owned()]),
            None
        );
    }

    /// Ids count on across characters and survive deleting the newest, so
    /// an id the client has seen is never handed out again.
    #[test]
    fn ids_are_global_and_never_reused() {
        let mut d = Deltas::default();
        let a = stored(&mut d, ALICE, SOME_TYPE).id;
        let b = stored(&mut d, BOB, SOME_TYPE).id;
        assert!(b > a);
        assert_eq!(delete(&mut d, BOB, Selection::All), 1);
        let c = stored(&mut d, ALICE, SOME_TYPE).id;
        assert!(c > b, "{c} after {b}");
    }

    /// Each character sees only their own, newest first; `after` is the
    /// ids above `fromID`.
    #[test]
    fn a_character_lists_their_own_newest_first() {
        let mut d = Deltas::default();
        let a1 = stored(&mut d, ALICE, SOME_TYPE).id;
        let b1 = stored(&mut d, BOB, SOME_TYPE).id;
        let a2 = stored(&mut d, ALICE, SOME_TYPE).id;
        let ids = |v: Vec<Notification>| v.iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(ids(of(&d, ALICE)), vec![a2, a1]);
        assert_eq!(ids(of(&d, BOB)), vec![b1]);
        assert_eq!(ids(after(&d, ALICE, a1)), vec![a2]);
        assert_eq!(ids(after(&d, ALICE, 0)), vec![a2, a1]);
        assert!(get(&d, BOB, a1).is_none(), "Alice's id under Bob's key");
    }

    /// Read and unread: marking by id, by group and all; a stranger's ids
    /// change nothing; an id list longer than the client's own cap is cut.
    #[test]
    fn marking_read_by_ids_group_and_all() {
        let mut d = Deltas::default();
        let war = stored(&mut d, ALICE, kind::KILL_REPORT_VICTIM).id;
        stored(&mut d, ALICE, corp_kind::CORP_APP_ACCEPT);
        stored(&mut d, ALICE, corp_kind::CORP_KICKED);
        let bobs = stored(&mut d, BOB, corp_kind::CORP_KICKED).id;
        assert_eq!(unread(&d, ALICE).len(), 3);

        assert_eq!(mark_read(&mut d, ALICE, Selection::Ids(&[bobs])), 0);
        assert_eq!(mark_read(&mut d, ALICE, Selection::Ids(&[war, war])), 1);
        assert_eq!(mark_read(&mut d, ALICE, Selection::Ids(&[war])), 0);
        assert!(get(&d, ALICE, war).unwrap().processed);
        assert_eq!(mark_read(&mut d, ALICE, Selection::Group(group::CORP)), 2);
        assert!(unread(&d, ALICE).is_empty());
        assert_eq!(unread(&d, BOB).len(), 1, "Bob's stays unread");
        assert_eq!(mark_read(&mut d, BOB, Selection::All), 1);

        let mut many = vec![0; MAX_UPDATED];
        let late = stored(&mut d, ALICE, SOME_TYPE).id;
        many.push(late);
        assert_eq!(mark_read(&mut d, ALICE, Selection::Ids(&many)), 0);
    }

    /// Deleting by id, by group and all, only ever the caller's own.
    #[test]
    fn deleting_by_ids_group_and_all() {
        let mut d = Deltas::default();
        let war = stored(&mut d, ALICE, kind::KILL_REPORT_FINAL_BLOW).id;
        stored(&mut d, ALICE, corp_kind::CORP_LEFT);
        stored(&mut d, ALICE, corp_kind::CHAR_LEFT_CORP);
        let other = stored(&mut d, ALICE, SOME_TYPE).id;
        let bobs = stored(&mut d, BOB, corp_kind::CORP_LEFT).id;

        assert_eq!(delete(&mut d, ALICE, Selection::Ids(&[bobs])), 0);
        assert!(get(&d, BOB, bobs).is_some());
        assert_eq!(in_group(&d, ALICE, group::WAR).len(), 1);
        assert_eq!(delete(&mut d, ALICE, Selection::Ids(&[war])), 1);
        assert!(in_group(&d, ALICE, group::WAR).is_empty());
        assert_eq!(delete(&mut d, ALICE, Selection::Group(group::CORP)), 2);
        assert_eq!(of(&d, ALICE).len(), 1);
        assert_eq!(of(&d, ALICE)[0].id, other);
        assert_eq!(delete(&mut d, ALICE, Selection::All), 1);
        assert!(of(&d, ALICE).is_empty());
        assert_eq!(of(&d, BOB).len(), 1);
    }

    /// The oldest go once a character holds more than the cap; another
    /// character's are not counted against it.
    #[test]
    fn the_store_keeps_the_newest_per_character() {
        let mut d = Deltas::default();
        let bobs = stored(&mut d, BOB, SOME_TYPE).id;
        let first = stored(&mut d, ALICE, SOME_TYPE).id;
        for _ in 0..KEEP_PER_CHARACTER {
            stored(&mut d, ALICE, SOME_TYPE);
        }
        assert_eq!(of(&d, ALICE).len(), KEEP_PER_CHARACTER);
        assert!(get(&d, ALICE, first).is_none(), "the oldest went");
        assert!(get(&d, BOB, bobs).is_some());
    }

    /// The group table is the client's shape: no type in two groups (the
    /// client's lookup would pick one silently), "unread" is not a group of
    /// types, and every type this crate sends has the group its formatter
    /// table puts it in.
    #[test]
    fn every_type_is_in_at_most_one_group() {
        let mut seen = std::collections::BTreeSet::new();
        for (g, types) in GROUPS {
            assert_ne!(*g, group::UNREAD);
            for t in *types {
                assert!(seen.insert(*t), "type {t} is in two groups");
            }
        }
        for (t, g) in [
            (kind::KILL_REPORT_VICTIM, group::WAR),
            (kind::KILL_REPORT_FINAL_BLOW, group::WAR),
            (kind::FW_CORP_JOIN_REQUEST, group::WAR),
            (kind::FW_CHAR_RANK_GAIN, group::WAR),
            (corp_kind::CORP_APP_NEW, group::CORP),
            (corp_kind::CORP_LEFT, group::CORP),
            (corp_kind::CORP_KICKED, group::CORP),
            (kind::INSURANCE_PAYOUT, group::INSURANCE),
            (kind::INSURANCE_ISSUED, group::INSURANCE),
        ] {
            assert_eq!(group_of(t), Some(g), "type {t}");
        }
        assert_eq!(group_of(SOME_TYPE), None);
    }

    /// The SHA-1 is FIPS 180's (the published "abc" and two-block vectors),
    /// and the kill hash prints an absent final blow as Python's `None`.
    #[test]
    fn sha1_and_the_kill_hash() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let r = crate::killmail::build(
            1,
            T,
            1,
            crate::killmail::Victim {
                character: BOB,
                corporation: 1,
                alliance: None,
                ship_type: SOME_TYPE,
            },
            &[],
            None,
            Vec::new(),
            |_| None,
            |_| None,
        );
        assert_eq!(
            kill_hash(&r),
            hex(&sha1(format!("{BOB}None{SOME_TYPE}{T}").as_bytes()))
        );
    }

    /// The push is the handler's five positional arguments, to the
    /// receiver only.
    #[test]
    fn the_push_is_the_handlers_signature() {
        use crate::notify::Push;
        let mut d = Deltas::default();
        let n = stored(&mut d, ALICE, SOME_TYPE);
        let p = Received(n.clone());
        let (event, args) = p.to_push();
        assert_eq!(event, "OnNotificationReceived");
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], PyValue::Int(n.id));
        assert_eq!(args[1], PyValue::Int(SOME_TYPE));
        assert_eq!(args[2], PyValue::Int(SENDER_SYSTEM));
        assert_eq!(args[3], PyValue::Int(T));
        assert_eq!(
            crate::pyutil::dict_get(&args[4], b"charID"),
            Some(&PyValue::Int(ALICE))
        );
        assert_eq!(p.audience(), Some(vec![ALICE]));
    }
}
