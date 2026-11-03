//! In-game chat: the XMPP hub for Local, corp, fleet and private rooms.
//!
//! Spec: `docs/lanes/chat.md`, which every `§` below refers to. The wire
//! format (tokenizer, framer, escaper, a test client) is the `eve-xmpp`
//! crate; this module is the part that knows the game.
//!
//! ## Shape
//!
//! - **One listener thread** on `<bind ip>:5222` (the port is hard-coded in
//!   the client, §0), **a reader thread per connection** running the login
//!   state machine of §2 and then routing stanzas. **Writes go through the
//!   push writer pool** (`push::Writers`, PA-5): a connection owns a queue
//!   ([`ChatOut`]) and the shard `conn % writers` drains it onto the socket,
//!   so a bot costs two threads (machoNet and chat reader), not three.
//! - **[`ChatHub`]** holds rooms, occupants, allow-lists, auth tokens and the
//!   per-character send queues behind one `Mutex`. Every stanza any thread
//!   wants to send is *enqueued* under that lock and *written* by the
//!   connection's pool shard, so no socket write ever happens under a
//!   lock, and the order stanzas are decided in is the order they arrive in.
//! - **A reconcile thread** derives proxy-controlled membership (Local, corp,
//!   alliance, fleet) from the world once a second (§5) and turns the diff
//!   into stanzas.
//!
//! ## Lock order: world, then hub, then the directory. No chat thread takes the world.
//!
//! The service arms in `service/base.rs` run with the world write lock held
//! and then take the hub. **Nothing in this file touches the world** (PA-11;
//! a source-scan test pins it): the reconcile pass, the login's presence, the
//! auth poll and the user-data iq read the published
//! [`crate::chat_dir::ChatDirectory`], which the world side builds and swaps
//! in. Its lock is a leaf (world -> directory write, hub -> directory read;
//! nothing is acquired while holding it), and the reconcile pass reads it
//! **inside** the hub critical section, so an older directory can never undo
//! a newer `sync_character`.
//!
//! ## Membership is derived, not joined
//!
//! A Local occupant set holds every online character in the system, chat
//! connection or not -- bots with no XMPP stream are in Local, which is what
//! CCP's proxy-owned Local did. A client's join only marks its stream as
//! *listening*; whether it is *in* the room is the world's decision. So a
//! client's own leave of a derived room only stops delivery to it, and the
//! JSON `leave` the other members see comes from the reconcile pass when the
//! world says the character has gone (`[A]`, see the park note in
//! `docs/lanes/chat.md`).

use crate::push::{Drain, Drained, STALL_LIMIT, WRITE_BUDGET, Writers};
use crate::service::Registry;
use eve_xmpp::{Element, Event, Framer};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The client dials 5222 and nothing else (`XmppConnection.__init__` [V]).
pub const PORT: u16 = 5222;
/// Our XMPP domain: what `XmppChatMgr.Hostname()` answers. Rooms live on
/// `conference.<this>`. Any name works; the client only formats it.
pub const HOSTNAME: &str = "eveoffline";
/// The resource every EVE client binds (`xmppchatsvc.Login` [V]).
pub const RESOURCE: &str = "eveclient";
/// Bodies longer than this are cut (§4.4). The client's own cap is 1024
/// characters (`chatutil.MAX_TEXT_LEN` [V]), so this only bounds a peer that
/// is not the client.
pub const BODY_CAP: usize = 4096;
/// How long, and how often, the chat login re-reads the session table for a
/// character that is not online yet. `[A]` 1.5 s in all; falsified by `failed
/// auth ... (online: false)` lines in the server log with this in place.
const AUTH_ONLINE_POLL: Duration = Duration::from_millis(100);
const AUTH_ONLINE_POLLS: u32 = 15;

/// How often the reconcile pass runs. `[A]` on the cadence (§5): fast enough
/// that a jump moves Local before the player looks, cheap at N <= 10.
pub const RECONCILE_EVERY: Duration = Duration::from_secs(1);
/// Per-socket send timeout. `[A]` (§6): one stalled client must not back up
/// its writer forever; the write fails, the connection drops, the client
/// reconnects in five seconds.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// The character type every capsuleer has (`typeID` 1373, the same value
/// `service.rs` uses for `CHARACTER_TYPE_ID`). Read with `int()` from
/// `eve_user_data` (§4.1).
const CHARACTER_TYPE_ID: i64 = 1373;

const NS_SASL: &str = "urn:ietf:params:xml:ns:xmpp-sasl";
const NS_BIND: &str = "urn:ietf:params:xml:ns:xmpp-bind";
const NS_SESSION: &str = "urn:ietf:params:xml:ns:xmpp-session";
const NS_STANZAS: &str = "urn:ietf:params:xml:ns:xmpp-stanzas";
/// How long a logged-off pilot's chat stream stays open before the server
/// closes it.
const LOGOFF_LINGER: Duration = if cfg!(test) {
    Duration::from_millis(900)
} else {
    Duration::from_secs(5)
};

const NS_STREAMS: &str = "http://etherx.jabber.org/streams";
const NS_MUC: &str = "http://jabber.org/protocol/muc";
const NS_MUC_USER: &str = "http://jabber.org/protocol/muc#user";
const NS_MUC_ADMIN: &str = "http://jabber.org/protocol/muc#admin";
const NS_MUC_OWNER: &str = "http://jabber.org/protocol/muc#owner";
const NS_DATA: &str = "jabber:x:data";
const NS_ROOMCONFIG: &str = "http://jabber.org/protocol/muc#roomconfig";
const NS_DISCO_INFO: &str = "http://jabber.org/protocol/disco#info";
const NS_DISCO_ITEMS: &str = "http://jabber.org/protocol/disco#items";
const NS_EVE_USER_DATA: &str = "urn:xmpp:eve_user_data";
const NS_EXPIRING: &str = "urn:xmpp:expiring_record";
const NS_EXPIRING_SEARCH: &str = "urn:xmpp:expiring_record#search";

/// What `Config::chat` carries. `None` there keeps chat off, which is today's
/// behaviour: `XmppChatMgr` answers `None` and the client never dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatConfig {
    /// The port to listen on. Always [`PORT`] outside tests. Tests give 0
    /// and read the bound port from `Server::chat_addrs`; probing a free
    /// port and passing it in races parallel tests (tracker §83).
    pub port: u16,
    /// The address `GetDeprecatedPrefsFallback` hands the client, when set.
    /// Unset, each client is told the local address its own machoNet
    /// connection arrived on -- right in the jail (`10.137.0.1`) and right
    /// for a friend on the LAN. Set it when that is wrong (NAT).
    pub advertise: Option<IpAddr>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        ChatConfig {
            port: PORT,
            advertise: None,
        }
    }
}

// ---------------------------------------------------------------------------
// JIDs and room names
// ---------------------------------------------------------------------------

fn bare_jid(c: i64) -> String {
    format!("{c}@{HOSTNAME}")
}

fn full_jid(c: i64) -> String {
    format!("{c}@{HOSTNAME}/{RESOURCE}")
}

fn conference() -> String {
    format!("conference.{HOSTNAME}")
}

fn room_jid(room: &str) -> String {
    format!("{room}@conference.{HOSTNAME}")
}

fn occupant_jid(room: &str, c: i64) -> String {
    format!("{room}@conference.{HOSTNAME}/{c}")
}

/// The part of a room name before the first `_`: `local`, `corp`, ...
fn category(room: &str) -> &str {
    room.split('_').next().unwrap_or(room)
}

/// The client's own `VALID_CHANNELNAME_REGEX`, `^[a-z0-9]+_[a-z0-9_-]+`
/// (`chatutil/channelnames.py` [V]). It is a `match`, so a prefix suffices;
/// we require the whole name to fit, which is stricter and costs nothing.
fn valid_room_name(room: &str) -> bool {
    let Some((head, tail)) = room.split_once('_') else {
        return false;
    };
    !head.is_empty()
        && !tail.is_empty()
        && head
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && tail
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// `PROXY_CONTROLLED_CATEGORIES` (`xmppchatclient/const.py` [V]): rooms whose
/// membership the server decides. A refused join here is `407`, which the
/// client retries and then resyncs.
fn proxy_controlled(room: &str) -> bool {
    matches!(
        category(room),
        "local" | "wormhole" | "nullsec" | "alliance" | "corp" | "fleet" | "faction" | "triglavian"
    )
}

/// `CHANNELS_WITH_NO_PRESENCE_BROADCAST` among the categories we produce
/// (`xmppchatclient/const.py` [V]): J-space Local is delayed, so nobody is
/// told who else is there.
fn broadcasts_presence(room: &str) -> bool {
    !matches!(
        category(room),
        "alliance" | "wormhole" | "nullsec" | "faction" | "rw" | "incursion" | "triglavian"
    )
}

/// `CHANNELS_WITH_SPECIAL_PRESENCE_HANDLING = ('local',)` [V]: the client
/// ignores *unavailable* presence from others in Local, so a departure has
/// to be the admin JSON `leave` (§4.5).
fn leaves_by_json(room: &str) -> bool {
    category(room) == "local"
}

/// Wormhole systems' ids (`idCheckers.IsWormholeSystem`), whose Local is the
/// `wormhole_` category.
const WORMHOLE_SYSTEMS: std::ops::RangeInclusive<i64> = 31_000_000..=31_999_999;

/// The rooms the world puts this character in, in the order
/// `ResyncSystemChannelAccess` lists them: Local, corp, alliance, fleet.
///
/// `nolocal_`, `triglavian_` and `nullsec_` are not produced: `[A]`, none of
/// our systems are any of those (§5).
pub fn system_rooms(reg: &Registry, charid: i64) -> Vec<String> {
    let Some(c) = reg.character(charid) else {
        return Vec::new();
    };
    let mut rooms = Vec::new();
    // The system the pilot is in now (roster, jump, station), not the home
    // station's: a stargate jump never moves `Character::solar_system_id`.
    if let Some(ss) = reg.present_system(charid).or(c.solar_system_id) {
        if WORMHOLE_SYSTEMS.contains(&ss) {
            rooms.push(format!("wormhole_{ss}"));
        } else {
            rooms.push(format!("local_{ss}"));
        }
    }
    rooms.push(format!("corp_{}", c.corporation_id));
    if let Some(a) = c.alliance_id {
        rooms.push(format!("alliance_{a}"));
    }
    if let Some(f) = reg.fleets().fleet_of(charid) {
        rooms.push(format!("fleet_{}", f.fleet_id));
    }
    rooms
}

/// The four ints `eve_user_data` carries about a character (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UserData {
    pub corpid: i64,
    pub allianceid: i64,
    pub warfactionid: i64,
    pub typeid: i64,
}

impl UserData {
    /// `role='0'` **on purpose**: our sandbox sessions carry QA, GML and
    /// ADMIN, and `get_color_for_role` would paint every player GM-red (§4.1).
    fn element(&self) -> Element {
        Element::new("eve_user_data")
            .with_attr("corpid", self.corpid.to_string())
            .with_attr("allianceid", self.allianceid.to_string())
            .with_attr("warfactionid", self.warfactionid.to_string())
            .with_attr("typeid", self.typeid.to_string())
            .with_attr("role", "0")
    }
}

/// What the world says about one character, for `eve_user_data`.
/// `warfactionid` is the militia the pilot serves, 0 when none (FW-1).
pub fn user_data(reg: &Registry, charid: i64) -> UserData {
    match reg.character(charid) {
        Some(c) => UserData {
            corpid: c.corporation_id,
            allianceid: c.alliance_id.unwrap_or(0),
            warfactionid: crate::service::facwar_mgr::war_faction(reg, charid).unwrap_or(0),
            typeid: CHARACTER_TYPE_ID,
        },
        None => UserData::default(),
    }
}

/// One online character's derived membership.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Membership {
    pub rooms: Vec<String>,
    pub data: UserData,
}

/// Membership for one character, if they are online.
pub fn membership(reg: &Registry, charid: i64) -> Option<Membership> {
    if !reg.sessions().is_online(charid) {
        return None;
    }
    Some(Membership {
        rooms: system_rooms(reg, charid),
        data: user_data(reg, charid),
    })
}

/// Membership for everyone online: the oracle `chat_dir::build` is tested against.
#[cfg(test)]
pub(crate) fn derive_all(reg: &Registry) -> BTreeMap<i64, Membership> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter_map(|c| membership(reg, c).map(|m| (c, m)))
        .collect()
}

// ---------------------------------------------------------------------------
// Stanza builders. Every outbound stanza in the module is built here, so the
// shapes the tests assert are the shapes we send.
// ---------------------------------------------------------------------------

/// A stanza error. **Every `<error>` carries a legacy `code`** (§3 rule 4):
/// the client reads `attributes['code']` unguarded.
fn error_el(code: u16, kind: &str, condition: &str) -> Element {
    Element::new("error")
        .with_attr("code", code.to_string())
        .with_attr("type", kind)
        .with_child(Element::new(condition).with_attr("xmlns", NS_STANZAS))
}

/// §4.1: one occupant's presence in a room, addressed to `to`.
fn occupant_presence(
    room: &str,
    who: i64,
    to: i64,
    data: &UserData,
    id: Option<&str>,
    is_self: bool,
) -> Element {
    occupant_presence_as(room, who, to, data, id, is_self, ("none", "participant"))
}

/// [`occupant_presence`] with an explicit `(affiliation, role)`: a player
/// channel's owner is `owner` / `moderator`, which is what the client's
/// `IsOperator` reads.
fn occupant_presence_as(
    room: &str,
    who: i64,
    to: i64,
    data: &UserData,
    id: Option<&str>,
    is_self: bool,
    (affiliation, role): (&str, &str),
) -> Element {
    let mut p = Element::new("presence")
        .with_attr("from", occupant_jid(room, who))
        .with_attr("to", full_jid(to));
    if let Some(id) = id {
        p = p.with_attr("id", id);
    }
    let mut x = Element::new("x")
        .with_attr("xmlns", NS_MUC_USER)
        .with_child(
            Element::new("item")
                .with_attr("affiliation", affiliation)
                .with_attr("role", role)
                .with_attr("jid", full_jid(who)),
        );
    if is_self {
        x = x.with_child(Element::new("status").with_attr("code", "110"));
    }
    p.with_child(x).with_child(data.element())
}

/// §4.3: `who` is no longer in `room`. `is_self` adds status 110.
fn unavailable_presence(room: &str, who: i64, to: i64, is_self: bool) -> Element {
    let mut x = Element::new("x")
        .with_attr("xmlns", NS_MUC_USER)
        .with_child(
            Element::new("item")
                .with_attr("affiliation", "none")
                .with_attr("role", "none"),
        );
    if is_self {
        x = x.with_child(Element::new("status").with_attr("code", "110"));
    }
    Element::new("presence")
        .with_attr("from", occupant_jid(room, who))
        .with_attr("to", full_jid(to))
        .with_attr("type", "unavailable")
        .with_child(x)
}

/// §3 rule 7: a refused join, carrying the join's id. The `from` keeps the
/// resource: `_HandleXmppPresence` does `domain.split('/')[1]` [V].
fn join_error(room: &str, who: i64, id: &str, code: u16) -> Element {
    let condition = match code {
        407 => "registration-required",
        401 => "not-authorized",
        500 => "internal-server-error",
        _ => "item-not-found",
    };
    Element::new("presence")
        .with_attr("from", occupant_jid(room, who))
        .with_attr("to", full_jid(who))
        .with_attr("type", "error")
        .with_attr("id", id)
        .with_child(Element::new("x").with_attr("xmlns", NS_MUC))
        .with_child(error_el(
            code,
            if matches!(code, 401 | 407) {
                "auth"
            } else {
                "cancel"
            },
            condition,
        ))
}

/// §4.4: one groupchat line, from `R/who`.
fn groupchat(room: &str, who: i64, to: i64, id: Option<&str>, body: &str) -> Element {
    let mut m = Element::new("message")
        .with_attr("from", occupant_jid(room, who))
        .with_attr("to", full_jid(to))
        .with_attr("type", "groupchat");
    if let Some(id) = id {
        m = m.with_attr("id", id);
    }
    m.with_child(Element::new("body").with_text(body))
}

/// §4.5: a server-originated groupchat. No resource on `from`, so the
/// client's sender becomes `'admin'` and the body is read as JSON.
fn admin_groupchat(room: &str, to: i64, body: &str) -> Element {
    Element::new("message")
        .with_attr("from", room_jid(room))
        .with_attr("to", full_jid(to))
        .with_attr("type", "groupchat")
        .with_child(Element::new("body").with_text(body))
}

/// Replay policy `[A]` (EvEJS-GAP svcA B14): a private room replays up to this
/// many lines ...
const HISTORY_KEEP: usize = 20;
/// ... from the last this-many seconds, to a joiner who sent no `<history>`.
/// Falsifier: a human sees old lines replayed on login, or does not see the
/// opener's first line in a private conversation.
const PRIVATE_REPLAY_SECS: u64 = 600;

/// Local never stores: it is a presence room, and its lines are not worth
/// keeping for a joiner who arrives with the jump.
fn keeps_history(room: &str) -> bool {
    category(room) != "local"
}

/// `YYYY-MM-DDTHH:MM:SSZ` for Unix seconds (XEP-0203 `stamp`).
fn delay_stamp(unix: u64) -> String {
    crate::live_status::iso_utc((unix as i64 + 11_644_473_600) * 10_000_000)
}

/// A replayed line: the original `from`/`id`, plus the delay stamp. The
/// client reads sender, `id` and `body` and ignores other children.
fn replayed(room: &str, line: &Line, to: i64) -> Element {
    groupchat(room, line.who, to, line.id.as_deref(), &line.body).with_child(
        Element::new("delay")
            .with_attr("xmlns", "urn:xmpp:delay")
            .with_attr("from", room_jid(room))
            .with_attr("stamp", delay_stamp(line.at)),
    )
}

/// The `refresh_char` admin JSON: a character's corp, alliance or war faction
/// changed. Numbers, as `leave_json`'s `charid`.
fn refresh_json(c: i64, d: &UserData) -> String {
    format!(
        "{{\"cmd\":\"refresh_char\",\"charid\":{c},\"corpid\":{},\"allianceid\":{},\"warfactionid\":{}}}",
        d.corpid, d.allianceid, d.warfactionid
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// §4.5: the JSON a Local departure is announced with. `charid` is a JSON
/// **number**: members are keyed by `int`.
fn leave_json(c: i64) -> String {
    format!("{{\"cmd\":\"leave\",\"charid\":{c}}}")
}

/// §4.6: an invite from `admin@H`, which the client turns straight into
/// `JoinChannel(room)` with no popup. `from` is `inviter` for §4.7.
fn invite(room: &str, to: i64, inviter: &str, reason: &str) -> Element {
    Element::new("message")
        .with_attr("from", room_jid(room))
        .with_attr("to", full_jid(to))
        .with_child(
            Element::new("x")
                .with_attr("xmlns", NS_MUC_USER)
                .with_child(
                    Element::new("invite")
                        .with_attr("from", inviter)
                        .with_child(Element::new("reason").with_text(reason)),
                ),
        )
}

/// A refused invite: 405, the code the client maps to "you lack the rights"
/// for an invite [V].
fn invite_refused(room: &str, to: i64, inv: &Element) -> Element {
    let keep = Element::new("x")
        .with_attr("xmlns", NS_MUC_USER)
        .with_child(Element::new("invite").with_attr("to", inv.attr("to").unwrap_or_default()));
    message_error(
        room,
        to,
        None,
        error_el(405, "cancel", "not-allowed"),
        Some(keep),
    )
}

/// §4.8: `decline` first in `x`, `reason` first in `decline` [V].
fn decline(room: &str, to: i64, decliner: i64, reason: &str) -> Element {
    Element::new("message")
        .with_attr("from", room_jid(room))
        .with_attr("to", full_jid(to))
        .with_child(
            Element::new("x")
                .with_attr("xmlns", NS_MUC_USER)
                .with_child(
                    Element::new("decline")
                        .with_attr("from", bare_jid(decliner))
                        .with_child(Element::new("reason").with_text(reason)),
                ),
        )
}

/// A message-level error, e.g. §4.4's "you are not able to speak".
fn message_error(
    room: &str,
    to: i64,
    id: Option<&str>,
    err: Element,
    keep: Option<Element>,
) -> Element {
    let mut m = Element::new("message")
        .with_attr("type", "error")
        .with_attr("from", room_jid(room))
        .with_attr("to", full_jid(to));
    if let Some(id) = id {
        m = m.with_attr("id", id);
    }
    if let Some(k) = keep {
        m = m.with_child(k);
    }
    m.with_child(err)
}

fn iq_result(id: &str, child: Option<Element>) -> Element {
    let iq = Element::new("iq")
        .with_attr("type", "result")
        .with_attr("id", id);
    match child {
        Some(c) => iq.with_child(c),
        None => iq,
    }
}

fn iq_error(id: &str, err: Element) -> Element {
    Element::new("iq")
        .with_attr("type", "error")
        .with_attr("id", id)
        .with_child(err)
}

fn empty_query(ns: &str) -> Element {
    Element::new("query").with_attr("xmlns", ns)
}

/// A `query` in `ns` listing `who` as `<item jid>` (what
/// `GetUsersFromResponse` reads).
fn items_of(ns: &str, who: &[i64]) -> Element {
    who.iter().fold(empty_query(ns), |q, w| {
        q.with_child(Element::new("item").with_attr("jid", full_jid(*w)))
    })
}

/// A `disco#items` result of the given `<item>`s.
fn items_result(items: Vec<Element>) -> Element {
    items
        .into_iter()
        .fold(empty_query(NS_DISCO_ITEMS), |q, i| q.with_child(i))
}

/// One channel in a `forme` or `byname/` listing: `node` is the asker's
/// affiliation (`owner` files it under My Channels).
fn room_item(room: &str, name: &str, node: &str) -> Element {
    Element::new("item")
        .with_attr("jid", room_jid(room))
        .with_attr("name", name)
        .with_attr("node", node)
}

/// A room's subject (the MOTD), as `_HandleGroupchat` reads it: `<subject>`
/// and no body. `who` is the setter; `None` is the server (sender `admin`).
fn subject_message(room: &str, who: Option<i64>, to: i64, text: &str) -> Element {
    let from = match who {
        Some(w) => occupant_jid(room, w),
        None => room_jid(room),
    };
    Element::new("message")
        .with_attr("from", from)
        .with_attr("to", full_jid(to))
        .with_attr("type", "groupchat")
        .with_child(Element::new("subject").with_text(text))
}

/// A room destroyed by its owner: the occupant's own unavailable presence,
/// with the `<destroy/>` the MUC standard names.
fn destroyed_presence(room: &str, to: i64) -> Element {
    let mut p = unavailable_presence(room, to, to, true);
    if let Some(x) = p.children.first_mut() {
        x.children.push(Element::new("destroy"));
    }
    p
}

/// §4.9: `disco#info` on a room. Only the occupant count is read; `name` is
/// the identity the client shows as the channel's title (`_GetNameFromInfoQuery`).
fn disco_info(name: &str, occupants: usize) -> Element {
    Element::new("query")
        .with_attr("xmlns", NS_DISCO_INFO)
        .with_child(
            Element::new("identity")
                .with_attr("category", "conference")
                .with_attr("type", "text")
                .with_attr("name", name),
        )
        .with_child(
            Element::new("x")
                .with_attr("xmlns", "jabber:x:data")
                .with_attr("type", "result")
                .with_child(
                    Element::new("field")
                        .with_attr("var", "muc#roominfo_occupants")
                        .with_child(Element::new("value").with_text(occupants.to_string())),
                ),
        )
}

/// The server's stream header (§2 steps 1 and 3). `stream:stream` is written
/// with its literal prefix (§3 rule 2) and never closed here.
fn stream_header(id: &str) -> String {
    format!(
        "<?xml version='1.0'?><stream:stream from='{HOSTNAME}' id='{id}' version='1.0' xmlns='jabber:client' xmlns:stream='{NS_STREAMS}'>"
    )
}

fn features_sasl() -> Element {
    Element::new("stream:features").with_child(
        Element::new("mechanisms")
            .with_attr("xmlns", NS_SASL)
            .with_child(Element::new("mechanism").with_text("PLAIN")),
    )
}

fn features_bind() -> Element {
    Element::new("stream:features")
        .with_child(Element::new("bind").with_attr("xmlns", NS_BIND))
        .with_child(Element::new("session").with_attr("xmlns", NS_SESSION))
}

/// `<iq type='result' id=..><bind><jid>C@H/eveclient</jid></bind></iq>`:
/// `bind` first in the iq and `jid` first in `bind`, because `handle_bind`
/// indexes `children[0]` [V].
fn bind_result(id: &str, c: i64) -> Element {
    iq_result(
        id,
        Some(
            Element::new("bind")
                .with_attr("xmlns", NS_BIND)
                .with_child(Element::new("jid").with_text(full_jid(c))),
        ),
    )
}

/// §2 step 6: the echo of the initial presence, which is what fires the
/// client's `handle_logged_in`.
fn presence_echo(c: i64) -> Element {
    Element::new("presence")
        .with_attr("from", full_jid(c))
        .with_attr("to", full_jid(c))
}

/// Cut a body to [`BODY_CAP`] bytes on a character boundary. `[A]`: cut,
/// not refused -- the real client never sends this much (§4.4).
fn cap_body(body: &str) -> &str {
    cap_bytes(body, BODY_CAP)
}

fn cap_bytes(body: &str, cap: usize) -> &str {
    if body.len() <= cap {
        return body;
    }
    let mut end = cap;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    &body[..end]
}

/// Whether `c` may appear in XML 1.0 text: tab, LF, CR, U+0020..=U+D7FF,
/// U+E000..=U+FFFD and U+10000 up. Anything else makes the client's parser
/// drop the stream, and a stored subject or name would do it on every join.
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r')
        || matches!(c, '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// `text` without the characters XML cannot carry ([`xml_char`]).
fn xml_safe(text: &str) -> String {
    text.chars().filter(|c| xml_char(*c)).collect()
}

// ---------------------------------------------------------------------------
// The hub
// ---------------------------------------------------------------------------

/// What a connection's writer is told to do.
#[derive(Debug)]
enum Out {
    Data(String),
    /// Write `</stream:stream>` and shut the socket.
    Close,
}

/// The most bytes one chat connection may have queued. [A] a chat client
/// that reads at all keeps a few KB queued; 8 MiB is minutes of a stalled
/// reader. Falsified by a `[chat] ... outbox over cap` line for a healthy
/// client.
const OUT_CAP: usize = 8 << 20;

#[derive(Default)]
struct OutState {
    queue: VecDeque<Out>,
    /// Bytes in `queue`.
    bytes: usize,
    /// A `Close` is queued (or the cap was crossed): nothing newer is taken.
    closing: bool,
    /// The cap was crossed: drop the queue and the socket.
    abort: bool,
    /// The reader has ended; finish what is queued, then say so.
    finishing: bool,
    /// The shard has been asked to drain this connection and has not yet.
    scheduled: bool,
    /// Nothing more will be written.
    done: bool,
}

struct Sock {
    /// `None` only in tests that need a stream-less handle.
    stream: Option<TcpStream>,
    /// Framed bytes taken from the queue that the socket has not accepted.
    pending: Vec<u8>,
    off: usize,
    stalled_since: Option<Instant>,
}

/// One chat connection's write side: a queue the reader and the hub post to,
/// and a [`Drain`] the push pool calls (PA-5). Replaces the per-connection
/// `chat-write` thread and its `mpsc` channel. A connection keeps one shard
/// for life (`key % pool`), so its stanzas leave in the order they were
/// queued; the queue is posted to under the hub lock exactly as the channel
/// was, so the order stanzas are decided in is unchanged.
pub(crate) struct ChatOut {
    me: Weak<ChatOut>,
    key: u64,
    writers: Option<Arc<Writers>>,
    state: Mutex<OutState>,
    done: Condvar,
    sock: Mutex<Sock>,
}

impl ChatOut {
    fn new(stream: Option<TcpStream>, key: u64, writers: Option<Arc<Writers>>) -> Arc<ChatOut> {
        Arc::new_cyclic(|me| ChatOut {
            me: me.clone(),
            key,
            writers,
            state: Mutex::new(OutState::default()),
            done: Condvar::new(),
            sock: Mutex::new(Sock {
                stream,
                pending: Vec::new(),
                off: 0,
                stalled_since: None,
            }),
        })
    }

    fn st(&self) -> MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue `out`. Never blocks and never touches the socket. After a
    /// `Close` nothing is queued, as the old writer stopped there.
    fn send(&self, out: Out) {
        let mut st = self.st();
        if st.done || st.closing {
            return;
        }
        match out {
            Out::Data(s) => {
                st.bytes += s.len();
                if st.bytes > OUT_CAP {
                    println!("[chat] connection {} outbox over cap; hung up", self.key);
                    st.queue.clear();
                    st.abort = true;
                    st.closing = true;
                } else {
                    st.queue.push_back(Out::Data(s));
                }
            }
            Out::Close => {
                st.queue.push_back(Out::Close);
                st.closing = true;
            }
        }
        self.schedule(st);
    }

    fn schedule(&self, mut st: MutexGuard<'_, OutState>) {
        if st.scheduled {
            return;
        }
        st.scheduled = true;
        drop(st);
        let woke = match (&self.writers, self.me.upgrade()) {
            (Some(w), Some(me)) => w.wake(self.key, me),
            _ => false,
        };
        if !woke {
            self.st().scheduled = false;
        }
    }

    /// The reader has ended. Wait (bounded by [`WRITE_TIMEOUT`]) until the
    /// queue has gone out: a refused login's `<failure/>` is still queued
    /// when the reader gets here, and shutting the socket first would lose it.
    fn finish(&self) {
        let mut st = self.st();
        st.finishing = true;
        self.schedule(st);
        let mut st = self.st();
        let deadline = Instant::now() + WRITE_TIMEOUT;
        while !st.done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            st = self
                .done
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        st.done = true;
        st.queue.clear();
    }

    fn end(&self, sock: &mut Sock, shutdown: bool) {
        if shutdown && let Some(s) = &sock.stream {
            let _ = s.shutdown(Shutdown::Both);
        }
        sock.pending = Vec::new();
        sock.off = 0;
        let mut st = self.st();
        st.done = true;
        st.queue.clear();
        drop(st);
        self.done.notify_all();
    }
}

impl Drain for ChatOut {
    fn drain(&self, now: Instant) -> Drained {
        let mut sock = self.sock.lock().unwrap_or_else(|e| e.into_inner());
        let (closing, abort, finishing) = {
            let mut st = self.st();
            st.scheduled = false;
            if st.done {
                return Drained::Idle;
            }
            if sock.pending.len() - sock.off <= OUT_CAP {
                while let Some(out) = st.queue.pop_front() {
                    match out {
                        Out::Data(s) => {
                            st.bytes -= s.len();
                            sock.pending.extend_from_slice(s.as_bytes());
                        }
                        Out::Close => {
                            sock.pending.extend_from_slice(b"</stream:stream>");
                            st.queue.clear();
                            st.bytes = 0;
                            break;
                        }
                    }
                }
            }
            (st.closing, st.abort, st.finishing)
        };
        if abort {
            self.end(&mut sock, true);
            return Drained::Idle;
        }
        let start = Instant::now();
        let mut progressed = false;
        let Sock {
            stream,
            pending,
            off,
            stalled_since,
        } = &mut *sock;
        let Some(stream) = stream.as_mut() else {
            // A stream-less handle (tests) discards.
            pending.clear();
            *off = 0;
            return Drained::Idle;
        };
        let mut failed = false;
        while *off < pending.len() {
            let left = WRITE_BUDGET.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            let _ = stream.set_write_timeout(Some(left.max(Duration::from_millis(1))));
            match stream.write(&pending[*off..]) {
                Ok(0) => {
                    failed = true;
                    break;
                }
                Ok(n) => {
                    *off += n;
                    progressed = true;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            self.end(&mut sock, true);
            return Drained::Idle;
        }
        if *off >= pending.len() {
            pending.clear();
            *off = 0;
            *stalled_since = None;
            let more = !self.st().queue.is_empty();
            if closing {
                self.end(&mut sock, true);
                return Drained::Idle;
            }
            if more {
                return Drained::More;
            }
            if finishing {
                self.end(&mut sock, false);
            }
            return Drained::Idle;
        }
        pending.drain(..*off);
        *off = 0;
        if progressed {
            *stalled_since = Some(now);
        }
        let since = *stalled_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= STALL_LIMIT {
            println!(
                "[chat] connection {} took no bytes for too long; hung up",
                self.key
            );
            self.end(&mut sock, true);
            return Drained::Idle;
        }
        Drained::More
    }
}

/// One authenticated stream, per character.
struct Stream {
    conn: u64,
    /// [`Hub::token_counter`] when it registered.
    epoch: u64,
    tx: Arc<ChatOut>,
}

#[derive(Debug, Default)]
struct Room {
    /// Occupant -> "their current stream has joined this room".
    ///
    /// For a derived room this is the world's member list and the flag says
    /// whether delivery reaches them. For a private room an entry exists only
    /// once its character has joined.
    occupants: BTreeMap<i64, bool>,
    /// Who may join a private room. `None` for a derived room.
    allow: Option<BTreeSet<i64>>,
    /// The last [`HISTORY_KEEP`] lines spoken here, for a late joiner. Never
    /// filled for Local (see [`keeps_history`]).
    history: VecDeque<Line>,
}

/// One spoken line, kept for replay.
#[derive(Debug, Clone)]
struct Line {
    who: i64,
    id: Option<String>,
    body: String,
    /// Seconds since the Unix epoch.
    at: u64,
}

impl Room {
    fn listening(&self) -> impl Iterator<Item = i64> + '_ {
        self.occupants
            .iter()
            .filter(|(_, joined)| **joined)
            .map(|(c, _)| *c)
    }
}

/// The delta kind of a player channel (`delta_kinds.rs`): `room -> name
/// owner password motd`, plus `#next -> n`, the counter the names come from.
pub const KIND: &str = "chanroom";
/// The counter's row key. A room name is `player_<n>`, so it cannot collide.
const COUNTER_KEY: &str = "#next";

/// What a pilot is to a player channel (`muc#admin` affiliations). The
/// creator is the room's `owner` field and is always an owner; this holds the
/// others. `Outcast` is a standing ban.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aff {
    Owner,
    Admin,
    Member,
    Outcast,
}

impl Aff {
    fn name(self) -> &'static str {
        match self {
            Aff::Owner => "owner",
            Aff::Admin => "admin",
            Aff::Member => "member",
            Aff::Outcast => "outcast",
        }
    }

    fn parse(s: &str) -> Option<Aff> {
        match s {
            "owner" => Some(Aff::Owner),
            "admin" => Some(Aff::Admin),
            "member" => Some(Aff::Member),
            "outcast" => Some(Aff::Outcast),
            _ => None,
        }
    }
}

/// The settings window's "Default access" (`muc#roomconfig_membersonly` and
/// `members_by_default`). `[A]` on the meaning of the middle one: a pilot who
/// is not on the allowed list may join but not speak. Falsifier: the client's
/// help text for "Moderated" says otherwise (`UI/Chat/Moderated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Access {
    /// Anyone may join and speak.
    #[default]
    Allowed,
    /// Anyone may join; only the allowed list (members) and moderators speak.
    Moderated,
    /// Only the allowed list (members) and moderators may join.
    Blocked,
}

impl Access {
    fn name(self) -> &'static str {
        match self {
            Access::Allowed => "allowed",
            Access::Moderated => "moderated",
            Access::Blocked => "blocked",
        }
    }

    fn parse(s: &str) -> Access {
        match s {
            "moderated" => Access::Moderated,
            "blocked" => Access::Blocked,
            _ => Access::Allowed,
        }
    }
}

/// A timed ban or mute (`urn:xmpp:expiring_record`). A ban is also a standing
/// refusal at join; a mute makes the pilot a visitor, who cannot speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitKind {
    Ban,
    Mute,
}

impl LimitKind {
    fn name(self) -> &'static str {
        match self {
            LimitKind::Ban => "ban",
            LimitKind::Mute => "mute",
        }
    }

    fn parse(s: &str) -> Option<LimitKind> {
        match s {
            "ban" => Some(LimitKind::Ban),
            "mute" => Some(LimitKind::Mute),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limit {
    kind: LimitKind,
    who: i64,
    /// Unix seconds; it ends when the clock reaches it.
    until: u64,
}

/// The longest a mute or ban the client asks for lasts: its own "forever" is
/// 315360000 seconds (`MuteUser`), ten years.
const LIMIT_CAP_SECS: u64 = 315_360_000;

/// The most affiliations and the most running mutes and bans one channel
/// holds, and the most items one `muc#admin` set is read for. A plain pilot
/// owns a channel for free, and both lists are scanned under the hub lock and
/// stored in the world file, so they stop growing here.
const ACL_CAP: usize = 300;
const LIMITS_CAP: usize = 300;
const ADMIN_ITEMS_CAP: usize = 16;
/// Characters of a moderator's reason that reach the muted pilot.
const REASON_CAP: usize = 200;

/// Whether `id` is a corporation or an alliance, which the Allowed, Blocked,
/// Operators and Owners pickers can name but which this server never enforces
/// (`aff` compares character ids). `[A]`: corporations and alliances sit in
/// the 98-99 million and the NPC 1.0-2.0 million ranges and characters do
/// not; falsifier: a character with an id there.
fn is_corp_or_alliance(id: i64) -> bool {
    (98_000_000..100_000_000).contains(&id) || (1_000_000..2_000_000).contains(&id)
}

/// A channel a player made with the Create button. The truth lives in the hub
/// (chat threads may not take the world, so they cannot write a delta); the
/// world side drains [`Hub::dirty`] in `chat_dir::tick`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlayerRoom {
    /// What the client shows as the channel's title.
    name: String,
    owner: i64,
    /// Empty is "no password". Already passed through [`xml_safe`].
    password: String,
    motd: String,
    access: Access,
    /// Everyone but the creator who holds an affiliation.
    acl: BTreeMap<i64, Aff>,
    limits: Vec<Limit>,
}

impl PlayerRoom {
    fn new(name: String, owner: i64) -> PlayerRoom {
        PlayerRoom {
            name,
            owner,
            password: String::new(),
            motd: String::new(),
            access: Access::Allowed,
            acl: BTreeMap::new(),
            limits: Vec::new(),
        }
    }

    /// The stored row: `name owner password motd access acl limits`, the acl
    /// as `id:aff,..` and the limits as `kind:id:until,..`. Ended limits are
    /// not written.
    fn row(&self) -> Vec<String> {
        let now = now_secs();
        vec![
            self.name.clone(),
            self.owner.to_string(),
            self.password.clone(),
            self.motd.clone(),
            self.access.name().to_owned(),
            self.acl
                .iter()
                .map(|(who, a)| format!("{who}:{}", a.name()))
                .collect::<Vec<_>>()
                .join(","),
            self.limits
                .iter()
                .filter(|l| l.until > now)
                .map(|l| format!("{}:{}:{}", l.kind.name(), l.who, l.until))
                .collect::<Vec<_>>()
                .join(","),
        ]
    }

    /// A row from before the moderation columns parses with none.
    fn parse(row: &[String]) -> Option<PlayerRoom> {
        let col = |i: usize| row.get(i).map_or("", String::as_str);
        let mut p = PlayerRoom::new(row.first()?.clone(), row.get(1)?.parse().ok()?);
        p.password = col(2).to_owned();
        p.motd = col(3).to_owned();
        p.access = Access::parse(col(4));
        for part in col(5).split(',') {
            if let Some((who, a)) = part.split_once(':')
                && let (Ok(who), Some(a)) = (who.parse(), Aff::parse(a))
            {
                p.acl.insert(who, a);
            }
        }
        let now = now_secs();
        for part in col(6).split(',') {
            let mut f = part.split(':');
            if let (Some(kind), Some(who), Some(until)) = (
                f.next().and_then(LimitKind::parse),
                f.next().and_then(|w| w.parse().ok()),
                f.next().and_then(|u| u.parse::<u64>().ok()),
            ) && until > now
            {
                p.limits.push(Limit { kind, who, until });
            }
        }
        Some(p)
    }

    /// `who`'s standing affiliation, if they hold one.
    fn aff(&self, who: i64) -> Option<Aff> {
        if who == self.owner {
            Some(Aff::Owner)
        } else {
            self.acl.get(&who).copied()
        }
    }

    fn is_owner(&self, who: i64) -> bool {
        self.aff(who) == Some(Aff::Owner)
    }

    /// An owner or an admin: they may set the subject, mute and kick.
    fn is_moderator(&self, who: i64) -> bool {
        matches!(self.aff(who), Some(Aff::Owner | Aff::Admin))
    }

    /// Owners, admins and the allowed list: they pass a password and a
    /// members-only door. `[A]` that a password does not stop an invited
    /// pilot; falsifier: an owner decision.
    fn is_privileged(&self, who: i64) -> bool {
        matches!(self.aff(who), Some(Aff::Owner | Aff::Admin | Aff::Member))
    }

    fn limited(&self, kind: LimitKind, who: i64, now: u64) -> bool {
        self.limits
            .iter()
            .any(|l| l.kind == kind && l.who == who && l.until > now)
    }

    fn banned(&self, who: i64, now: u64) -> bool {
        self.aff(who) == Some(Aff::Outcast) || self.limited(LimitKind::Ban, who, now)
    }

    /// The MUC role `who` has in the room: moderators, else a visitor (who
    /// cannot speak) when muted or not on the allowed list of a moderated
    /// room, else a participant.
    fn role(&self, who: i64, now: u64) -> &'static str {
        if self.is_moderator(who) {
            "moderator"
        } else if self.limited(LimitKind::Mute, who, now)
            || (self.access == Access::Moderated && self.aff(who) != Some(Aff::Member))
        {
            "visitor"
        } else {
            "participant"
        }
    }

    /// False when the list is full; ended limits are dropped first.
    fn set_limit(&mut self, kind: LimitKind, who: i64, secs: u64, now: u64) -> bool {
        self.limits.retain(|l| l.until > now);
        self.clear_limit(kind, who);
        if self.limits.len() >= LIMITS_CAP {
            return false;
        }
        let until = now.saturating_add(secs.clamp(1, LIMIT_CAP_SECS));
        self.limits.push(Limit { kind, who, until });
        true
    }

    fn clear_limit(&mut self, kind: LimitKind, who: i64) {
        self.limits.retain(|l| !(l.kind == kind && l.who == who));
    }

    /// Everyone who holds `aff`; the creator is an owner.
    fn with_aff(&self, aff: Aff) -> Vec<i64> {
        let mut v: Vec<i64> = self
            .acl
            .iter()
            .filter(|(_, a)| **a == aff)
            .map(|(w, _)| *w)
            .collect();
        if aff == Aff::Owner {
            v.insert(0, self.owner);
        }
        v
    }
}

/// A display name the client sent, made safe to store and list: no control
/// characters, at most [`NAME_CAP`] characters.
const NAME_CAP: usize = 60;

/// The longest MOTD in characters: the client's own edit box allows 4000
/// (`xmppchannelsettings.py`), so a long multibyte one is not cut short.
const MOTD_CAP: usize = 4000;

/// Most player channels one owner may hold. `[A]`: generous, so no honest
/// pilot meets it; falsified by an owner decision on the real cap. Over it a
/// create is answered with [`FULL_ROOM`], which the join refuses with 500.
const PLAYER_ROOMS_PER_OWNER: usize = 50;

/// The longest password the settings window accepts (`maxLength=20`) and we
/// keep: the cap only bounds a peer that is not the client.
const PASSWORD_CAP: usize = 64;

/// The room a refused create hands back: never a real channel.
pub const FULL_ROOM: &str = "player_full";

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && xml_char(*c))
        .take(NAME_CAP)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[derive(Default)]
struct Hub {
    /// Single-use auth tokens, token -> (character, epoch it was minted at).
    tokens: HashMap<String, (i64, u64)>,
    /// A logical clock: bumped by every token minted and every stream
    /// registered. A reconcile pass remembers it before it reads the world, so
    /// it can tell what was made after its snapshot (see
    /// [`Hub::set_membership`]).
    token_counter: u64,
    /// The local address each machoNet connection arrived on, by client id.
    client_addrs: HashMap<i64, IpAddr>,
    streams: HashMap<i64, Stream>,
    next_conn: u64,
    rooms: BTreeMap<String, Room>,
    /// The derived membership last applied, per online character.
    members: HashMap<i64, Membership>,
    next_private: u64,
    /// Player-made channels by room name. They outlive their occupants and a
    /// restart (see [`KIND`]); a room entry in `rooms` is made on demand.
    players: BTreeMap<String, PlayerRoom>,
    /// The counter `player_<n>` comes from.
    next_player: u64,
    /// Room names changed since the world side last drained them
    /// ([`COUNTER_KEY`] for the counter).
    dirty: BTreeSet<String>,
}

/// Rooms, occupants, tokens and send queues. See the module docs for the
/// lock order, which is the one rule here that can deadlock the server.
pub struct ChatHub {
    state: Mutex<Hub>,
    advertise: Option<IpAddr>,
    /// The push writer pool chat connections write through. Set by the
    /// server ([`ChatHub::attach_writers`]); a hub without one (tests,
    /// `spawn_listener`) starts a private one-shard pool on the first
    /// connection.
    writers: OnceLock<Arc<Writers>>,
    /// What the world published for chat (`crate::chat_dir`): a leaf lock.
    dir: crate::chat_dir::Published,
}

impl std::fmt::Debug for ChatHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatHub")
            .field("advertise", &self.advertise)
            .finish_non_exhaustive()
    }
}

impl Hub {
    fn send(&self, to: i64, stanza: &Element) {
        if let Some(s) = self.streams.get(&to) {
            // A send to an ended connection is dropped; the reader will notice
            // the dead socket and clean up.
            s.tx.send(Out::Data(stanza.to_xml()));
        }
    }

    fn data(&self, c: i64) -> UserData {
        self.members.get(&c).map(|m| m.data).unwrap_or_default()
    }

    /// §5's diff for one character: (1) leave old rooms, stopping their
    /// traffic first; (2) tell those rooms; (3) join the new ones and
    /// announce; (4) invite the stream into them. `None` is "offline".
    ///
    /// **`stale_before` is the race fix** (lane undock-invuln). A reconcile
    /// pass reads the world, drops it, and only then takes the hub, so its
    /// "X is offline" can be older than a login that happened in between: a
    /// bot that reconnects mints a token and opens a stream, and the stale
    /// pass then revoked the token (`auth refused`, `online: true`) or closed
    /// the stream. An offline verdict now takes only the tokens and the stream
    /// made at or before the epoch the pass snapshotted at; the next pass,
    /// seeing the character online, settles the rooms. `u64::MAX` is "all of
    /// them" (an explicit sync from the character's own call).
    fn set_membership(&mut self, c: i64, want: Option<&Membership>, stale_before: u64) {
        let old: Vec<String> = self
            .members
            .get(&c)
            .map(|m| m.rooms.clone())
            .unwrap_or_default();
        let new: Vec<String> = want.map(|m| m.rooms.clone()).unwrap_or_default();
        let old_data = self.members.get(&c).map(|m| m.data);
        if let Some(m) = want {
            self.members.insert(c, m.clone());
        }
        for r in old.iter().filter(|r| !new.contains(r)) {
            self.remove_occupant(r, c);
        }
        for r in new.iter().filter(|r| !old.contains(r)) {
            self.add_derived(r, c);
        }
        // A changed corp, alliance or faction: rooms the character stays in
        // learn it without a leave (G2).
        if let (Some(before), Some(m)) = (old_data, want)
            && before != m.data
        {
            for r in new.iter().filter(|r| old.contains(r)) {
                self.refresh_occupant(r, c);
            }
        }
        if want.is_none() {
            // Offline: out of every room, private ones included; tokens
            // revoked; the stream closed after a linger (§5).
            let private: Vec<String> = self
                .rooms
                .iter()
                .filter(|(_, room)| room.occupants.contains_key(&c))
                .map(|(name, _)| name.clone())
                .collect();
            for r in private {
                self.remove_occupant(&r, c);
            }
            for room in self.rooms.values_mut() {
                if let Some(allow) = &mut room.allow {
                    allow.remove(&c);
                }
            }
            self.members.remove(&c);
            self.tokens
                .retain(|_, (who, epoch)| *who != c || *epoch > stale_before);
            if self
                .streams
                .get(&c)
                .is_some_and(|s| s.epoch <= stale_before)
                && let Some(s) = self.streams.remove(&c)
            {
                // Not closed at once: the client writes presence on the
                // socket right after the log-off and a dead socket (10054)
                // reads as a lost connection. The stream takes nothing more
                // from the hub; it closes after LOGOFF_LINGER, or when the
                // peer does first (a Close on a done queue is dropped).
                // [A, falsifier: a client that keeps the stream past Disconnect]
                let tx = s.tx;
                std::thread::spawn(move || {
                    std::thread::sleep(LOGOFF_LINGER);
                    tx.send(Out::Close);
                });
            }
        }
    }

    /// Tell `room`'s listeners that `c`'s `eve_user_data` changed: the
    /// available presence updates the client's cache, the admin JSON
    /// scatters `OnCharStateChanged`. Only rooms that show members.
    fn refresh_occupant(&self, room: &str, c: i64) {
        let Some(r) = self.rooms.get(room) else {
            return;
        };
        if !broadcasts_presence(room) {
            return;
        }
        let data = self.data(c);
        let json = refresh_json(c, &data);
        for o in r.listening() {
            self.send(o, &occupant_presence(room, c, o, &data, None, o == c));
            self.send(o, &admin_groupchat(room, o, &json));
        }
    }

    /// Take `c` out of `room` and tell whoever is still listening.
    fn remove_occupant(&mut self, room: &str, c: i64) {
        let Some(r) = self.rooms.get_mut(room) else {
            return;
        };
        // (1) first: from here on `c` is not an occupant, so nothing more of
        // this room reaches them (§3 rule 9).
        if r.occupants.remove(&c).is_none() {
            return;
        }
        let listeners: Vec<i64> = r.listening().collect();
        let empty = r.occupants.is_empty();
        if broadcasts_presence(room) {
            for o in listeners {
                if leaves_by_json(room) {
                    self.send(o, &admin_groupchat(room, o, &leave_json(c)));
                } else {
                    self.send(o, &unavailable_presence(room, c, o, false));
                }
            }
        }
        if empty && !self.players.contains_key(room) {
            self.rooms.remove(room);
        }
    }

    /// Put `c` in a derived room, announce them, and invite their stream.
    fn add_derived(&mut self, room: &str, c: i64) {
        let r = self.rooms.entry(room.to_owned()).or_default();
        if r.occupants.contains_key(&c) {
            return;
        }
        r.occupants.insert(c, false);
        let listeners: Vec<i64> = r.listening().collect();
        let data = self.data(c);
        if broadcasts_presence(room) {
            for o in listeners {
                self.send(o, &occupant_presence(room, c, o, &data, None, false));
            }
        }
        // (4) the client only *leaves* rooms on a session change; joining the
        // new one waits for this (§0 "who decides").
        self.send(c, &invite(room, c, &format!("admin@{HOSTNAME}"), "session"));
    }

    fn reconcile(&mut self, desired: &BTreeMap<i64, Arc<Membership>>, snapshot: u64) {
        let gone: Vec<i64> = self
            .members
            .keys()
            .chain(self.streams.keys())
            .filter(|c| !desired.contains_key(c))
            .copied()
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .collect();
        for c in gone {
            self.set_membership(c, None, snapshot);
        }
        for (c, m) in desired {
            if self.members.get(c) != Some(&**m) {
                self.set_membership(*c, Some(&**m), snapshot);
            }
        }
    }

    /// A stream stopped, or was replaced: nothing it joined is listened to.
    fn detach(&mut self, c: i64) {
        let names: Vec<String> = self.rooms.keys().cloned().collect();
        for name in names {
            let player = self.players.contains_key(&name);
            let Some(r) = self.rooms.get_mut(&name) else {
                continue;
            };
            match r.allow {
                _ if player && r.occupants.contains_key(&c) => self.remove_occupant(&name, c),
                Some(_) if r.occupants.contains_key(&c) => self.remove_occupant(&name, c),
                _ => {
                    if let Some(joined) = r.occupants.get_mut(&c) {
                        *joined = false;
                    }
                }
            }
        }
    }

    /// §4.2.
    fn join(&mut self, c: i64, room: &str, id: &str, history: Option<u64>, password: Option<&str>) {
        if room == FULL_ROOM {
            // The client answers a 500 on join with "too many channels".
            println!("[chat] {c} refused {room} (500)");
            self.send(c, &join_error(room, c, id, 500));
            return;
        }
        let player = self.players.get(room).cloned();
        if player.is_some() {
            self.rooms.entry(room.to_owned()).or_default();
        }
        if let Some(p) = &player {
            // A banned pilot, or a stranger at a members-only door: 407, which
            // the client words as "you have been banned" for a player channel.
            if p.banned(c, now_secs()) || (p.access == Access::Blocked && !p.is_privileged(c)) {
                println!("[chat] {c} refused {room} (407)");
                self.send(c, &join_error(room, c, id, 407));
                return;
            }
            // A password: the owners, admins and allowed list walk in, anyone
            // else is asked (the client prompts and retries, four times). A
            // GM-roled pilot is asked too, and the client answers it with
            // `GrantChannelOwnership` and the literal `bypass_with_roles`,
            // which is only a retry: the grant already made them an owner.
            if !p.is_privileged(c)
                && !p.password.is_empty()
                && password.map(xml_safe).as_deref() != Some(p.password.as_str())
            {
                println!("[chat] {c} refused {room} (401)");
                self.send(c, &join_error(room, c, id, 401));
                return;
            }
        }
        let allowed = valid_room_name(room)
            && (player.is_some()
                || match self.rooms.get(room) {
                    Some(r) => match &r.allow {
                        Some(allow) => allow.contains(&c),
                        None => r.occupants.contains_key(&c),
                    },
                    None => false,
                });
        if !allowed {
            // 407 for a server-controlled room: retried, then resynced.
            // 404 for anything else: `_JoinChannelInternal` gives up quietly
            // on it, where 403 would pop "channel unavailable" and 407 on a
            // player room would say "you have been banned" [V].
            let code = if proxy_controlled(room) { 407 } else { 404 };
            println!("[chat] {c} refused {room} ({code})");
            self.send(c, &join_error(room, c, id, code));
            return;
        }
        let Some(r) = self.rooms.get_mut(room) else {
            return;
        };
        let before = r.occupants.insert(c, true);
        let was_occupant = before.is_some();
        let was_listening = before == Some(true);
        let others: Vec<i64> = r.occupants.keys().copied().filter(|o| *o != c).collect();
        let listeners: Vec<i64> = r.listening().filter(|o| *o != c).collect();
        let me = self.data(c);
        if broadcasts_presence(room) {
            for o in others {
                self.send(
                    c,
                    &occupant_presence_as(
                        room,
                        o,
                        c,
                        &self.data(o),
                        None,
                        false,
                        self.roles(room, o),
                    ),
                );
            }
        }
        self.send(
            c,
            &occupant_presence_as(room, c, c, &me, Some(id), true, self.roles(room, c)),
        );
        // A derived member was announced when the world put them here; a
        // re-join is answered without telling anyone (§3 rule 7).
        if !was_occupant && broadcasts_presence(room) {
            let roles = self.roles(room, c);
            for o in listeners {
                self.send(
                    o,
                    &occupant_presence_as(room, c, o, &me, None, false, roles),
                );
            }
        }
        self.replay(c, room, history, was_listening);
        if !was_listening {
            self.send_motd(c, room);
        }
    }

    /// Send `c` the stored lines it asked for, after its join answer. An
    /// explicit `<history seconds>` is honoured in any room that stores; with
    /// none, a private room replays [`PRIVATE_REPLAY_SECS`] to a joiner not
    /// already listening.
    fn replay(&self, c: i64, room: &str, history: Option<u64>, was_listening: bool) {
        let Some(r) = self.rooms.get(room) else {
            return;
        };
        let secs = match history {
            Some(s) => s,
            None if r.allow.is_some() && !was_listening => PRIVATE_REPLAY_SECS,
            None => return,
        };
        let from = now_secs().saturating_sub(secs);
        for line in r.history.iter().filter(|l| l.at >= from) {
            self.send(c, &replayed(room, line, c));
        }
    }

    /// §4.3, for a room `c`'s client left of its own accord.
    fn leave(&mut self, c: i64, room: &str) {
        let Some(r) = self.rooms.get_mut(room) else {
            return;
        };
        let private = r.allow.is_some() || self.players.contains_key(room);
        match r.occupants.get_mut(&c) {
            Some(joined) if *joined => {
                *joined = false;
            }
            _ => return,
        }
        self.send(c, &unavailable_presence(room, c, c, true));
        if private {
            self.remove_occupant(room, c);
        }
    }

    /// §4.4. `Err` is "not an occupant", answered with 403.
    fn speak(
        &mut self,
        c: i64,
        room: &str,
        id: Option<&str>,
        body: &str,
        need_stream: bool,
    ) -> bool {
        let Some(r) = self.rooms.get(room) else {
            return false;
        };
        let ok = match r.occupants.get(&c) {
            Some(joined) => *joined || !need_stream,
            None => false,
        };
        if !ok {
            return false;
        }
        // A muted pilot, or a stranger in a moderated channel, is a visitor.
        if self
            .players
            .get(room)
            .is_some_and(|p| p.role(c, now_secs()) == "visitor")
        {
            return false;
        }
        let clean = xml_safe(body);
        let body = cap_body(&clean);
        let listeners: Vec<i64> = r.listening().collect();
        for o in listeners {
            self.send(o, &groupchat(room, c, o, id, body));
        }
        if keeps_history(room)
            && let Some(r) = self.rooms.get_mut(room)
        {
            if r.history.len() >= HISTORY_KEEP {
                r.history.pop_front();
            }
            r.history.push_back(Line {
                who: c,
                id: id.map(str::to_owned),
                body: body.to_owned(),
                at: now_secs(),
            });
        }
        true
    }

    /// `(affiliation, role)` of `who` in `room`.
    fn roles(&self, room: &str, who: i64) -> (&'static str, &'static str) {
        match self.players.get(room) {
            Some(p) => (
                p.aff(who).map_or("none", Aff::name),
                p.role(who, now_secs()),
            ),
            None => ("none", "participant"),
        }
    }

    /// Whether `c` may set `room`'s subject, mute and kick: an owner or admin.
    fn can_moderate(&self, room: &str, c: i64) -> bool {
        self.players.get(room).is_some_and(|p| p.is_moderator(c))
    }

    /// Whether `c` owns `room` (deletes it, configures it).
    fn is_owner_of(&self, room: &str, c: i64) -> bool {
        self.players.get(room).is_some_and(|p| p.is_owner(c))
    }

    /// Tell `room`'s listeners that `who`'s affiliation or role changed.
    fn refresh_roles(&self, room: &str, who: i64) {
        self.refresh_roles_why(room, who, None);
    }

    /// [`refresh_roles`](Self::refresh_roles) with the moderator's `reason`
    /// in the presence item, which the client shows a muted pilot.
    fn refresh_roles_why(&self, room: &str, who: i64, reason: Option<&str>) {
        let Some(r) = self.rooms.get(room) else {
            return;
        };
        if !r.occupants.contains_key(&who) {
            return;
        }
        let data = self.data(who);
        let roles = self.roles(room, who);
        for o in r.listening() {
            let mut p = occupant_presence_as(room, who, o, &data, None, o == who, roles);
            if let Some(why) = reason
                && let Some(item) = p.children.first_mut().and_then(|x| x.children.first_mut())
            {
                item.children
                    .push(Element::new("reason").with_text(why.to_owned()));
            }
            self.send(o, &p);
        }
    }

    /// Put `who` out of `room` with a status code: 301 banned, 307 kicked.
    fn expel(&mut self, room: &str, who: i64, status: &str) {
        if !self
            .rooms
            .get(room)
            .is_some_and(|r| r.occupants.contains_key(&who))
        {
            return;
        }
        let mut p = unavailable_presence(room, who, who, true);
        if let Some(x) = p.children.first_mut() {
            x.children
                .push(Element::new("status").with_attr("code", status));
        }
        self.send(who, &p);
        self.remove_occupant(room, who);
    }

    /// `muc#admin` set of an affiliation by `c`. `Err` is the stanza error
    /// code: 403 when `c` may not, 400 for a name we do not know.
    fn set_affiliation(&mut self, c: i64, room: &str, who: i64, aff: &str) -> Result<(), u16> {
        let new = match aff {
            "none" => None,
            a => Some(Aff::parse(a).ok_or(400u16)?),
        };
        if is_corp_or_alliance(who) {
            return Err(400);
        }
        let Some(p) = self.players.get_mut(room) else {
            return Err(404);
        };
        let by_owner = p.is_owner(c);
        // An admin hands out and takes back the allowed list and bans only,
        // and never touches an owner or an admin; the creator is untouchable.
        let allowed = (by_owner || p.is_moderator(c))
            && who != p.owner
            && (by_owner
                || (matches!(new, None | Some(Aff::Member | Aff::Outcast))
                    && !p.is_moderator(who)));
        if !allowed {
            return Err(403);
        }
        match new {
            Some(a) => {
                if !p.acl.contains_key(&who) && p.acl.len() >= ACL_CAP {
                    return Err(409);
                }
                p.acl.insert(who, a);
                // A standing ban would shut the door on the new owner or admin.
                if matches!(a, Aff::Owner | Aff::Admin) {
                    p.clear_limit(LimitKind::Ban, who);
                }
            }
            None => {
                p.acl.remove(&who);
            }
        }
        self.dirty.insert(room.to_owned());
        if new == Some(Aff::Outcast) {
            self.expel(room, who, "301");
        } else {
            self.refresh_roles(room, who);
        }
        Ok(())
    }

    /// `muc#admin` set of a role by `c`: `visitor` mutes (until the matching
    /// `expiring_record` says when), `participant` unmutes.
    fn set_role(
        &mut self,
        c: i64,
        room: &str,
        who: i64,
        role: &str,
        reason: Option<&str>,
    ) -> Result<(), u16> {
        let Some(p) = self.players.get_mut(room) else {
            return Err(404);
        };
        if !p.is_moderator(c) || p.is_moderator(who) {
            return Err(403);
        }
        match role {
            "visitor" => {
                if !p.set_limit(LimitKind::Mute, who, LIMIT_CAP_SECS, now_secs()) {
                    return Err(409);
                }
            }
            "participant" => p.clear_limit(LimitKind::Mute, who),
            _ => return Err(405),
        }
        self.dirty.insert(room.to_owned());
        self.refresh_roles_why(room, who, reason);
        Ok(())
    }

    /// `urn:xmpp:expiring_record` set or delete by `c`: a timed `ban` or
    /// `mute` of `who` in `room`.
    fn set_limit(
        &mut self,
        c: i64,
        room: &str,
        who: i64,
        kind: &str,
        secs: Option<u64>,
    ) -> Result<(), u16> {
        let kind = LimitKind::parse(kind).ok_or(400u16)?;
        if is_corp_or_alliance(who) {
            return Err(400);
        }
        let Some(p) = self.players.get_mut(room) else {
            return Err(404);
        };
        if !p.is_moderator(c) || p.is_moderator(who) {
            return Err(403);
        }
        match secs {
            Some(s) => {
                if !p.set_limit(kind, who, s, now_secs()) {
                    return Err(409);
                }
            }
            None => p.clear_limit(kind, who),
        }
        self.dirty.insert(room.to_owned());
        if secs.is_some() && kind == LimitKind::Ban {
            self.expel(room, who, "301");
        } else {
            self.refresh_roles(room, who);
        }
        Ok(())
    }

    /// The pilots with an affiliation (`item@affiliation`) or a role
    /// (`item@role`) in `room`, or with a running timed `ban` / `mute`.
    /// `None` for a query only a moderator may read, from someone who is not.
    fn listed(&self, c: i64, room: &str, what: &str, value: &str) -> Option<Vec<i64>> {
        let p = self.players.get(room)?;
        let now = now_secs();
        match what {
            "affiliation" => {
                let aff = Aff::parse(value)?;
                // Who owns and runs a channel is asked by every window; who
                // is on its allowed and blocked lists is for its moderators.
                if matches!(aff, Aff::Member | Aff::Outcast) && !p.is_moderator(c) {
                    return None;
                }
                Some(p.with_aff(aff))
            }
            "role" => Some(
                self.rooms
                    .get(room)
                    .map(|r| r.occupants.keys().copied().collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|o| p.role(*o, now) == value)
                    .collect(),
            ),
            _ => {
                let kind = LimitKind::parse(value)?;
                if !p.is_moderator(c) {
                    return None;
                }
                Some(
                    p.limits
                        .iter()
                        .filter(|l| l.kind == kind && l.until > now)
                        .map(|l| l.who)
                        .collect(),
                )
            }
        }
    }

    /// The settings window's form for `c`: `None` when `c` does not own the
    /// channel. A moderator who does not gets the empty form (read-only).
    fn config_form(&self, c: i64, room: &str) -> Option<Element> {
        let p = self.players.get(room)?;
        if !p.is_owner(c) {
            return p.is_moderator(c).then(|| empty_query(NS_MUC_OWNER));
        }
        let field = |var: &str, ty: &str, value: &str| {
            Element::new("field")
                .with_attr("var", var)
                .with_attr("type", ty)
                .with_child(Element::new("value").with_text(value))
        };
        let flag = |b: bool| if b { "1" } else { "0" };
        let x = Element::new("x")
            .with_attr("xmlns", NS_DATA)
            .with_attr("type", "form")
            .with_child(field("FORM_TYPE", "hidden", NS_ROOMCONFIG))
            .with_child(field("muc#roomconfig_roomname", "text-single", &p.name))
            // The secret itself is never sent: the window shows a dummy.
            .with_child(field(
                "muc#roomconfig_passwordprotectedroom",
                "boolean",
                flag(!p.password.is_empty()),
            ))
            .with_child(field(
                "muc#roomconfig_membersonly",
                "boolean",
                flag(p.access == Access::Blocked),
            ))
            .with_child(field(
                "members_by_default",
                "boolean",
                flag(p.access != Access::Moderated),
            ));
        Some(empty_query(NS_MUC_OWNER).with_child(x))
    }

    /// `muc#owner` set with a form, by `c`: name, password, default access.
    /// 403 for a non-owner, 409 for a name another channel has.
    fn configure(&mut self, c: i64, room: &str, fields: &[(String, String)]) -> Result<(), u16> {
        if !self.is_owner_of(room, c) {
            return Err(403);
        }
        let value = |var: &str| {
            fields
                .iter()
                .find(|(k, _)| k == var)
                .map(|(_, v)| v.as_str())
        };
        let on = |v: &str| v.trim() == "1" || v.trim() == "true";
        let name = value("muc#roomconfig_roomname").map(clean_name);
        if let Some(n) = &name {
            let taken = self
                .players
                .iter()
                .any(|(r, o)| r != room && o.name.to_lowercase() == n.to_lowercase());
            if n.is_empty() || taken {
                return Err(409);
            }
        }
        let Some(p) = self.players.get_mut(room) else {
            return Err(404);
        };
        if let Some(n) = name {
            p.name = n;
        }
        if let Some(v) = value("muc#roomconfig_passwordprotectedroom") {
            if !on(v) {
                p.password.clear();
            } else if let Some(secret) = value("muc#roomconfig_roomsecret") {
                p.password = xml_safe(secret).chars().take(PASSWORD_CAP).collect();
            }
        }
        if let Some(members_only) = value("muc#roomconfig_membersonly") {
            p.access = if on(members_only) {
                Access::Blocked
            } else if value("members_by_default").is_some_and(|v| !on(v)) {
                Access::Moderated
            } else {
                Access::Allowed
            };
        }
        self.dirty.insert(room.to_owned());
        // A changed default access changes who may speak.
        let present: Vec<i64> = self
            .rooms
            .get(room)
            .map(|r| r.occupants.keys().copied().collect())
            .unwrap_or_default();
        for o in present {
            self.refresh_roles(room, o);
        }
        Ok(())
    }

    /// A pilot with the GM or chat-admin role asked for ownership
    /// (`GrantChannelOwnership`): the caller's role is checked where the call
    /// is served. Clears a ban they were under.
    fn grant_owner(&mut self, room: &str, who: i64) -> bool {
        let Some(p) = self.players.get_mut(room) else {
            return false;
        };
        if who != p.owner {
            p.acl.insert(who, Aff::Owner);
        }
        p.clear_limit(LimitKind::Ban, who);
        self.dirty.insert(room.to_owned());
        self.refresh_roles(room, who);
        true
    }

    /// `XmppChatMgr.AddCharacterToChannel` for a player channel, and an
    /// owner's or admin's invite: put `who` on the allowed list and send the
    /// invite. `by_role` is a GM-roled caller, who needs no standing. False
    /// when `c` may not, or `who` is banned.
    fn invite_player(&mut self, c: i64, by_role: bool, room: &str, who: i64) -> bool {
        let Some(p) = self.players.get_mut(room) else {
            return false;
        };
        // Any occupant may invite into an open channel; a members-only one
        // is for its moderators.
        let in_room = self
            .rooms
            .get(room)
            .is_some_and(|r| r.occupants.get(&c).copied().unwrap_or(false));
        let may = by_role
            || p.is_moderator(c)
            || (in_room && p.access != Access::Blocked && !p.banned(c, now_secs()));
        if !may || who == c || p.banned(who, now_secs()) {
            return false;
        }
        // The allowed list is what lets the invited pilot past a password or
        // a members-only door; in an open channel it would only change who
        // may speak in a moderated one, so it is left alone there.
        // In a moderated one it would also give the invitee a voice, so only
        // a moderator (or a GM role) may write it there.
        let may_list = by_role || p.is_moderator(c) || p.access != Access::Moderated;
        if may_list
            && p.aff(who).is_none()
            && (p.access == Access::Blocked || !p.password.is_empty())
        {
            if p.acl.len() >= ACL_CAP {
                return false;
            }
            p.acl.insert(who, Aff::Member);
            self.dirty.insert(room.to_owned());
        }
        let from = if by_role {
            format!("admin@{HOSTNAME}")
        } else {
            bare_jid(c)
        };
        let reason = if by_role {
            "session".to_owned()
        } else {
            String::new()
        };
        self.send(who, &invite(room, who, &from, &reason));
        true
    }

    /// `XmppChatMgr.CreatePlayerOwnedChannel`: a new `player_<n>`.
    fn create_player(&mut self, owner: i64, name: &str) -> String {
        if self.players.values().filter(|p| p.owner == owner).count() >= PLAYER_ROOMS_PER_OWNER {
            return FULL_ROOM.to_owned();
        }
        self.next_player += 1;
        let room = format!("player_{}", self.next_player);
        self.players
            .insert(room.clone(), PlayerRoom::new(clean_name(name), owner));
        self.rooms.entry(room.clone()).or_default();
        self.dirty.insert(room.clone());
        self.dirty.insert(COUNTER_KEY.to_owned());
        room
    }

    /// The `disco#items` for `node` asked of the conference host: `forme`
    /// lists the channels `c` may see, `byname/<name>` those called `<name>`.
    fn channel_items(&self, c: i64, node: &str) -> Vec<Element> {
        if node == "forme" {
            return self
                .players
                .iter()
                .filter(|(room, p)| {
                    p.is_privileged(c)
                        || p.password.is_empty()
                        || self
                            .rooms
                            .get(*room)
                            .is_some_and(|r| r.occupants.contains_key(&c))
                })
                .map(|(room, p)| {
                    room_item(room, &p.name, if p.is_owner(c) { "owner" } else { "none" })
                })
                .collect();
        }
        let Some(wanted) = node.strip_prefix("byname/") else {
            return Vec::new();
        };
        // The client escapes the name once for the node, and XML decoding
        // undid only the attribute layer: undo the other, `&amp;` last.
        let wanted = wanted
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let wanted = clean_name(&wanted).to_lowercase();
        self.players
            .iter()
            .filter(|(_, p)| p.name.to_lowercase() == wanted)
            .map(|(room, p)| room_item(room, &p.name, "none"))
            .collect()
    }

    /// The title `disco#info` with a `node` gives a channel: a player
    /// channel's own name, else the room name.
    fn display_name(&self, room: &str) -> String {
        self.players
            .get(room)
            .map_or_else(|| room.to_owned(), |p| p.name.clone())
    }

    /// Send `c` the room's stored subject, if it has one.
    fn send_motd(&self, c: i64, room: &str) {
        if let Some(p) = self.players.get(room).filter(|p| !p.motd.is_empty()) {
            self.send(c, &subject_message(room, None, c, &p.motd));
        }
    }

    /// A subject change by `c`. False when they may not (or no such room).
    fn set_subject(&mut self, c: i64, room: &str, text: &str) -> bool {
        if !self.can_moderate(room, c) {
            return false;
        }
        let text: String = xml_safe(text).chars().take(MOTD_CAP).collect();
        if let Some(p) = self.players.get_mut(room) {
            p.motd.clone_from(&text);
        }
        self.dirty.insert(room.to_owned());
        let listeners: Vec<i64> = self
            .rooms
            .get(room)
            .map(|r| r.listening().collect())
            .unwrap_or_default();
        for o in listeners {
            self.send(o, &subject_message(room, Some(c), o, &text));
        }
        true
    }

    /// `muc#owner` `<destroy/>`: the owner deletes the channel. False when
    /// `c` is not its owner.
    fn destroy_player(&mut self, c: i64, room: &str) -> bool {
        if !self.is_owner_of(room, c) {
            return false;
        }
        self.players.remove(room);
        self.dirty.insert(room.to_owned());
        if let Some(r) = self.rooms.remove(room) {
            for o in r.listening() {
                self.send(o, &destroyed_presence(room, o));
            }
        }
        true
    }

    /// The rows the world side has not stored yet: `(key, Some(value))` to
    /// write, `(key, None)` to delete. Clears the set.
    fn take_dirty(&mut self) -> Vec<(Vec<String>, Option<Vec<String>>)> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .map(|k| {
                let row = if k == COUNTER_KEY {
                    Some(vec![self.next_player.to_string()])
                } else {
                    self.players.get(&k).map(PlayerRoom::row)
                };
                (vec![k], row)
            })
            .collect()
    }

    fn mint_token(&mut self, c: i64) -> String {
        self.token_counter += 1;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // 128 bits from two independently keyed SipHashes (std's
        // `RandomState` is seeded from the OS per instance). Not a crypto
        // library, and it does not need to be: it guards a LAN chat server
        // against a client guessing another client's token.
        let mut halves = [0u64; 2];
        for h in &mut halves {
            let mut s = std::collections::hash_map::RandomState::new().build_hasher();
            s.write_u64(self.token_counter);
            s.write_u128(now);
            s.write_i64(c);
            *h = s.finish();
        }
        let token = format!("{:016x}{:016x}", halves[0], halves[1]);
        self.tokens.insert(token.clone(), (c, self.token_counter));
        token
    }
}

impl ChatHub {
    pub fn new(config: ChatConfig) -> Arc<ChatHub> {
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Arc::new(ChatHub {
            state: Mutex::new(Hub {
                // Private room names survive in the client's settings, so a
                // restarted server must not hand `private_1` out again.
                // `[A]`: seconds since the epoch, times 1000.
                next_private: start * 1000,
                // The same for `player_<n>`: the client keeps the names it
                // joined, so a wiped world must not reuse one. A stored
                // counter ([`ChatHub::seed_players`]) only ever raises it.
                // `[A]`: seconds since the epoch.
                next_player: start,
                ..Hub::default()
            }),
            advertise: config.advertise,
            writers: OnceLock::new(),
            dir: crate::chat_dir::Published::default(),
        })
    }

    /// The directory the world publishes for chat.
    pub fn directory(&self) -> &crate::chat_dir::Published {
        &self.dir
    }

    /// Use the server's push writer pool for every chat connection.
    pub fn attach_writers(&self, writers: Arc<Writers>) {
        let _ = self.writers.set(writers);
    }

    fn pool(&self) -> Arc<Writers> {
        let w = self.writers.get_or_init(|| {
            let w = Arc::new(Writers::default());
            let _ = crate::push::spawn_writers(Arc::clone(&w), 1);
            w
        });
        Arc::clone(w)
    }

    /// The hub's lock. Recovers from poisoning on the same reasoning as
    /// `service::world_write`: a panicking stanza handler leaves a wrong room
    /// list, not an unreadable one, and refusing would end chat for everyone.
    fn lock(&self) -> MutexGuard<'_, Hub> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remember the local address a machoNet connection arrived on.
    pub fn note_client_addr(&self, client_id: i64, ip: IpAddr) {
        self.lock().client_addrs.insert(client_id, ip);
    }

    pub fn forget_client(&self, client_id: i64) {
        self.lock().client_addrs.remove(&client_id);
    }

    /// What `GetDeprecatedPrefsFallback` tells a character on these machoNet
    /// connections: the configured address, else the one they came in on.
    pub fn advertise_for(&self, client_ids: &[i64]) -> Option<IpAddr> {
        if self.advertise.is_some() {
            return self.advertise;
        }
        let hub = self.lock();
        client_ids
            .iter()
            .find_map(|id| hub.client_addrs.get(id).copied())
    }

    /// `chatAuthenticationService.GetAuthenticationToken`: a fresh single-use
    /// token bound to this character (§1).
    pub fn mint_token(&self, charid: i64) -> String {
        self.lock().mint_token(charid)
    }

    /// Spend a token. True only if it was minted for exactly this character.
    fn take_token(&self, token: &str, charid: i64) -> bool {
        let mut hub = self.lock();
        match hub.tokens.get(token) {
            Some(&(who, _)) if who == charid => {
                hub.tokens.remove(token);
                true
            }
            _ => false,
        }
    }

    /// Apply one character's derived membership now, rather than at the next
    /// reconcile. `None` takes them out of everything.
    pub fn sync_character(&self, charid: i64, want: Option<Membership>) {
        let mut hub = self.lock();
        if hub.members.get(&charid) != want.as_ref() || want.is_none() {
            hub.set_membership(charid, want.as_ref(), u64::MAX);
        }
    }

    /// One full reconcile pass (§5), from the published directory. The
    /// directory is read **inside** the hub critical section: that is the
    /// ordering rule of `crate::chat_dir`, and why `u64::MAX` (every token
    /// and stream) is safe -- a login publishes `online` before it can mint,
    /// and mint and register both need the hub.
    pub fn reconcile(&self) {
        let mut hub = self.lock();
        let dir = self.dir.current();
        hub.reconcile(&dir.members, u64::MAX);
    }

    /// `XmppChatMgr.CreatePlayerOwnedChannel(name)`: a new player channel
    /// owned by `owner`, called `name` on the client. Returns its room name.
    pub fn create_player(&self, owner: i64, name: &str) -> String {
        self.lock().create_player(owner, name)
    }

    /// `XmppChatMgr.SetMotd(room, text)`: `who` sets a player channel's
    /// subject. False when they may not.
    pub fn set_motd(&self, who: i64, room: &str, text: &str) -> bool {
        self.lock().set_subject(who, room, text)
    }

    /// `XmppChatMgr.GrantChannelOwnership(room)` for `who`, a GM or chat-admin
    /// pilot (the caller checks the role). False for a room that is not a
    /// player channel.
    pub fn grant_ownership(&self, room: &str, who: i64) -> bool {
        self.lock().grant_owner(room, who)
    }

    /// Whether `room` is a player channel.
    pub fn is_player_room(&self, room: &str) -> bool {
        self.lock().players.contains_key(room)
    }

    /// `XmppChatMgr.AddCharacterToChannel` for a player channel: see
    /// [`Hub::invite_player`]. `by_role` is a GM-roled caller.
    pub fn add_to_player(&self, c: i64, by_role: bool, room: &str, who: i64) -> bool {
        self.lock().invite_player(c, by_role, room, who)
    }

    /// Give a player channel a password (empty clears it). False for a room
    /// that is not a player channel.
    pub fn set_player_password(&self, room: &str, password: &str) -> bool {
        let mut hub = self.lock();
        let Some(p) = hub.players.get_mut(room) else {
            return false;
        };
        p.password = xml_safe(password);
        hub.dirty.insert(room.to_owned());
        true
    }

    /// Load the stored player channels ([`KIND`] rows), at start and from
    /// `Registry::set_chat`. Rows that do not parse are skipped.
    pub fn seed_players(&self, rows: &crate::delta::Rows) {
        let mut hub = self.lock();
        for (key, value) in rows {
            let Some(name) = key.first() else { continue };
            if name == COUNTER_KEY {
                if let Some(n) = value.first().and_then(|n| n.parse::<u64>().ok()) {
                    hub.next_player = hub.next_player.max(n);
                }
            } else if let Some(p) = PlayerRoom::parse(value) {
                hub.players.insert(name.clone(), p);
                hub.rooms.entry(name.clone()).or_default();
            }
        }
    }

    /// Player-channel rows the world has not stored yet, cleared as they are
    /// taken: `(key, Some(row))` to write, `(key, None)` to delete.
    /// Called from `chat_dir::tick`, which holds the world; chat threads
    /// cannot write a delta themselves.
    pub fn take_dirty(&self) -> Vec<(Vec<String>, Option<Vec<String>>)> {
        self.lock().take_dirty()
    }

    /// `XmppChatMgr.CreatePrivateChannel(other)`: a new private room whose
    /// allow-list is the two of them (§5).
    pub fn create_private(&self, owner: i64, other: i64) -> String {
        let mut hub = self.lock();
        hub.next_private += 1;
        let name = format!("private_{}", hub.next_private);
        hub.rooms.insert(
            name.clone(),
            Room {
                occupants: BTreeMap::new(),
                allow: Some([owner, other].into_iter().collect()),
                history: VecDeque::new(),
            },
        );
        name
    }

    /// `XmppChatMgr.AddCharacterToChannel(room, charid, reason)`: allow
    /// `charid` into a private room and send them the admin invite, which
    /// their client joins without asking (§5, the path GM-roled players take).
    /// False when the room is not a private room we know.
    pub fn add_to_channel(&self, room: &str, charid: i64) -> bool {
        let mut hub = self.lock();
        let Some(allow) = hub.rooms.get_mut(room).and_then(|r| r.allow.as_mut()) else {
            return false;
        };
        allow.insert(charid);
        hub.send(
            charid,
            &invite(room, charid, &format!("admin@{HOSTNAME}"), "session"),
        );
        true
    }

    /// A server-side speaker -- a bot, a GM -- says `text` in `room` as
    /// `charid`. They must be an occupant (a bot is a derived member of its
    /// Local without any stream). False if they are not.
    pub fn say(&self, room: &str, charid: i64, text: &str) -> bool {
        self.lock().speak(charid, room, None, text, false)
    }

    /// The occupants of a room, joined or not. For tests and diagnostics.
    pub fn occupants(&self, room: &str) -> Vec<i64> {
        self.lock()
            .rooms
            .get(room)
            .map(|r| r.occupants.keys().copied().collect())
            .unwrap_or_default()
    }

    fn register(&self, charid: i64, conn: u64, tx: Arc<ChatOut>) {
        let mut hub = self.lock();
        let want = self.dir.current().member(charid);
        if let Some(old) = hub.streams.remove(&charid) {
            // `[A]` A second login of one character replaces the first, which is
            // what an ejabberd resource conflict does.
            old.tx.send(Out::Close);
            hub.detach(charid);
        }
        hub.token_counter += 1;
        let epoch = hub.token_counter;
        hub.streams.insert(charid, Stream { conn, epoch, tx });
        if hub.members.get(&charid) != want.as_ref() {
            hub.set_membership(charid, want.as_ref(), u64::MAX);
        }
    }

    fn disconnected(&self, charid: i64, conn: u64) {
        let mut hub = self.lock();
        if hub.streams.get(&charid).map(|s| s.conn) == Some(conn) {
            hub.streams.remove(&charid);
            hub.detach(charid);
        }
    }

    fn next_conn(&self) -> u64 {
        let mut hub = self.lock();
        hub.next_conn += 1;
        hub.next_conn
    }
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

/// Start the listener and the reconcile thread.
pub fn spawn(listener: TcpListener, hub: Arc<ChatHub>) -> io::Result<()> {
    spawn_listener(listener, Arc::clone(&hub))?;
    std::thread::Builder::new()
        .name("chat-reconcile".to_owned())
        .spawn(move || run_reconcile(&hub, RECONCILE_EVERY))?;
    Ok(())
}

/// The reconcile loop: one pass per publish, or per `every` without one.
pub(crate) fn run_reconcile(hub: &ChatHub, every: Duration) -> ! {
    let mut seen = 0;
    loop {
        seen = hub.directory().wait_newer(seen, every);
        hub.reconcile();
    }
}

/// Start only the listener. Tests drive [`ChatHub::reconcile`] by hand.
pub fn spawn_listener(listener: TcpListener, hub: Arc<ChatHub>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("chat-accept".to_owned())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let hub = Arc::clone(&hub);
                let _ = std::thread::Builder::new()
                    .name("chat-conn".to_owned())
                    .spawn(move || serve(stream, &hub));
            }
        })?;
    Ok(())
}

/// Where a connection is in §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Header,
    Auth,
    Header2,
    Bind,
    Session,
    Presence,
    Ready,
}

struct Conn<'a> {
    conn: u64,
    peer: SocketAddr,
    tx: Arc<ChatOut>,
    hub: &'a ChatHub,
    phase: Phase,
    charid: Option<i64>,
}

/// Serve one chat connection to its end. `pub(crate)` for the capped accept
/// loops in `server.rs`, one per listener.
pub(crate) fn serve(stream: TcpStream, hub: &ChatHub) {
    let peer = stream
        .peer_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
    let _ = stream.set_nodelay(true);
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let conn = hub.next_conn();
    let tx = ChatOut::new(Some(write_half), conn, Some(hub.pool()));
    let mut c = Conn {
        conn,
        peer,
        tx,
        hub,
        phase: Phase::Header,
        charid: None,
    };
    let end = c.run(&stream);
    println!("[chat] {peer} ({}) ended: {end}", c.who());
    if let Some(ch) = c.charid
        && c.phase == Phase::Ready
    {
        hub.disconnected(ch, c.conn);
    }
    // **Let the writer finish before the socket goes.** A refused login's
    // `<failure/>` is still in the queue when the reader gets here, and
    // shutting the socket first loses it: the client sees a bare EOF and
    // retries instead of reading "not authorised". `finish` waits for the
    // pool shard to write the queue out; the write timeout bounds it.
    c.tx.finish();
    drop(c);
    let _ = stream.shutdown(Shutdown::Both);
}

impl Conn<'_> {
    fn who(&self) -> String {
        self.charid
            .map_or_else(|| "unauthenticated".to_owned(), |c| c.to_string())
    }

    fn out(&self, s: String) {
        self.tx.send(Out::Data(s));
    }

    fn send(&self, e: &Element) {
        self.out(e.to_xml());
    }

    fn close(&self) {
        self.tx.send(Out::Close);
    }

    fn run(&mut self, stream: &TcpStream) -> String {
        let mut framer = Framer::new();
        let mut buf = [0u8; 4096];
        let mut first = true;
        let mut reader = stream;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => return "closed by peer".to_owned(),
                Ok(n) => n,
                Err(e) => return format!("read error: {e}"),
            };
            if first {
                first = false;
                // A TLS ClientHello starts with record type 0x16. The one
                // misconfiguration a friend's machine will hit (§6).
                if buf[0] == 0x16 {
                    println!(
                        "chat: client attempted TLS -- prefs.ini lacks chatserver_disable_tls=1 ({})",
                        self.peer
                    );
                    return "TLS attempt".to_owned();
                }
            }
            framer.feed(&buf[..n]);
            loop {
                let event = match framer.next_event() {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(e) => {
                        self.close();
                        return format!("bad XML: {e}");
                    }
                };
                match self.handle(event, &mut framer) {
                    Ok(()) => {}
                    Err(why) => {
                        self.close();
                        return why;
                    }
                }
            }
        }
    }

    /// One event. `Err` ends the connection with that reason.
    fn handle(&mut self, event: Event, framer: &mut Framer) -> Result<(), String> {
        match (self.phase, event) {
            (_, Event::Close) => Err("stream closed".to_owned()),
            (Phase::Header, Event::Open(_)) => {
                self.out(stream_header(&format!("s{}", self.conn)));
                self.send(&features_sasl());
                self.phase = Phase::Auth;
                Ok(())
            }
            (Phase::Header2, Event::Open(_)) => {
                self.out(stream_header(&format!("s{}b", self.conn)));
                self.send(&features_bind());
                self.phase = Phase::Bind;
                Ok(())
            }
            (Phase::Auth, Event::Stanza(e)) if e.name == "auth" => {
                self.auth(&e)?;
                // Rule 11: nothing more is sent until the client's new
                // header, and the parser starts a new document.
                framer.reset();
                self.phase = Phase::Header2;
                Ok(())
            }
            (Phase::Bind, Event::Stanza(e)) if e.name == "iq" && first_child_is(&e, "bind") => {
                let id = e.attr("id").unwrap_or_default();
                let c = self.charid.ok_or("bind before auth")?;
                self.send(&bind_result(id, c));
                self.phase = Phase::Session;
                Ok(())
            }
            (Phase::Session, Event::Stanza(e))
                if e.name == "iq" && first_child_is(&e, "session") =>
            {
                self.send(&iq_result(e.attr("id").unwrap_or_default(), None));
                self.phase = Phase::Presence;
                Ok(())
            }
            (Phase::Presence, Event::Stanza(e))
                if e.name == "presence" && e.attr("to").is_none() =>
            {
                let c = self.charid.ok_or("presence before auth")?;
                // The echo goes out *before* the stream is registered, so it
                // is the first non-iq stanza after `ready` and nothing the hub
                // sends can overtake it (§2 step 6).
                self.send(&presence_echo(c));
                self.hub.register(c, self.conn, self.tx.clone());
                self.phase = Phase::Ready;
                println!("[chat] {c} logged in from {}", self.peer);
                Ok(())
            }
            (Phase::Ready, Event::Stanza(e)) => {
                self.stanza(&e);
                Ok(())
            }
            (_, Event::Stanza(e)) if e.name == "iq" => {
                // Rule 8: every iq gets an answer, whatever the phase.
                self.unanswered_iq(&e);
                Ok(())
            }
            (phase, other) => {
                println!("[chat] UNANSWERED in {phase:?}: {other:?}");
                Ok(())
            }
        }
    }

    fn auth(&mut self, e: &Element) -> Result<(), String> {
        let failure = || {
            Element::new("failure")
                .with_attr("xmlns", NS_SASL)
                .with_child(Element::new("not-authorized"))
        };
        let claimed = (e.attr("mechanism") == Some("PLAIN"))
            .then(|| eve_xmpp::sasl::plain_decode(&e.text))
            .flatten()
            .and_then(|(_, authcid, password)| authcid.parse::<i64>().ok().map(|c| (c, password)));
        let Some((c, token)) = claimed else {
            self.send(&failure());
            return Err("unreadable auth".to_owned());
        };
        // The published directory, then the hub.
        //
        // **A short grace for "not online yet"** (lane undock-invuln): a bot
        // dials the chat port straight after it asks for its token, and a
        // login storm (or a reconnect that swaps the old session for the new
        // one) can put the dial a beat ahead of the session table. The token
        // proves the machoNet side already asked, so wait a little for the
        // table before refusing; a session that really is gone stays refused.
        let is_online = || self.hub.directory().current().is_online(c);
        let mut online = is_online();
        for _ in 0..AUTH_ONLINE_POLLS {
            if online {
                break;
            }
            std::thread::sleep(AUTH_ONLINE_POLL);
            online = is_online();
        }
        if !(online && self.hub.take_token(&token, c)) {
            println!(
                "[chat] {c} failed auth from {} (online: {online})",
                self.peer
            );
            self.send(&failure());
            return Err("auth refused".to_owned());
        }
        self.charid = Some(c);
        self.send(&Element::new("success").with_attr("xmlns", NS_SASL));
        Ok(())
    }

    /// Routing once the stream is ready.
    fn stanza(&mut self, e: &Element) {
        let Some(c) = self.charid else { return };
        match e.name.as_str() {
            "presence" => match (e.attr("to"), e.attr("type")) {
                // A later broadcast presence (status change): echoed like the
                // first, as RFC 6121 does.
                (None, None) => self.send(&presence_echo(c)),
                (Some(to), None) if e.child("x").and_then(|x| x.attr("xmlns")) == Some(NS_MUC) => {
                    match room_of(to) {
                        Some(room) => {
                            let history = e
                                .child("x")
                                .and_then(|x| x.child("history"))
                                .and_then(|h| h.attr("seconds"))
                                .and_then(|s| s.parse::<u64>().ok());
                            let password = e
                                .child("x")
                                .and_then(|x| x.child("password"))
                                .map(|p| p.text.as_str());
                            self.hub.lock().join(
                                c,
                                &room,
                                e.attr("id").unwrap_or_default(),
                                history,
                                password,
                            )
                        }
                        None => println!("[chat] UNANSWERED join to {to}"),
                    }
                }
                (Some(to), Some("unavailable")) => {
                    if let Some(room) = room_of(to) {
                        self.hub.lock().leave(c, &room);
                    }
                }
                _ => println!("[chat] UNANSWERED {c}: {e}"),
            },
            "message" => self.message(c, e),
            "iq" => self.iq(c, e),
            _ => println!("[chat] UNANSWERED {c}: {e}"),
        }
    }

    fn message(&mut self, c: i64, e: &Element) {
        let Some(room) = e.attr("to").and_then(room_of) else {
            println!("[chat] UNANSWERED {c}: {e}");
            return;
        };
        let id = e.attr("id");
        match e.attr("type").unwrap_or("normal") {
            "groupchat" => {
                let mut hub = self.hub.lock();
                let Some(body) = e.child("body") else {
                    // A subject (MOTD) change: the owner of a player channel.
                    let Some(subject) = e.child("subject") else {
                        println!("[chat] UNANSWERED {c}: {e}");
                        return;
                    };
                    if !hub.set_subject(c, &room, &subject.text) {
                        hub.send(
                            c,
                            &message_error(&room, c, id, error_el(403, "auth", "forbidden"), None),
                        );
                    }
                    return;
                };
                if !hub.speak(c, &room, id, &body.text, true) {
                    hub.send(
                        c,
                        &message_error(&room, c, id, error_el(403, "auth", "forbidden"), None),
                    );
                }
            }
            "normal" => {
                let Some(x) = e
                    .child("x")
                    .filter(|x| x.attr("xmlns") == Some(NS_MUC_USER))
                else {
                    println!("[chat] UNANSWERED {c}: {e}");
                    return;
                };
                if let Some(inv) = x.child("invite") {
                    self.mediated_invite(c, &room, inv);
                } else if let Some(dec) = x.children.first().filter(|d| d.name == "decline") {
                    let reason = dec.child("reason").map(|r| r.text.as_str()).unwrap_or("");
                    if let Some(to) = dec.attr("to").and_then(charid_of) {
                        let hub = self.hub.lock();
                        hub.send(to, &decline(&room, to, c, reason));
                    }
                } else {
                    println!("[chat] UNANSWERED {c}: {e}");
                }
            }
            _ => println!("[chat] UNANSWERED {c}: {e}"),
        }
    }

    /// §4.7: allow the invitee into a private room the inviter is in, and
    /// forward. Anything else is refused with 405, which is the code the
    /// client maps to "you lack the rights" for an invite [V].
    fn mediated_invite(&mut self, c: i64, room: &str, inv: &Element) {
        let reason = inv
            .child("reason")
            .map(|r| r.text.clone())
            .unwrap_or_default();
        let target = inv.attr("to").and_then(charid_of);
        let mut hub = self.hub.lock();
        if hub.players.contains_key(room) {
            // A player channel: the owner or admin, or anyone in an open one.
            if !target.is_some_and(|o| hub.invite_player(c, false, room, o)) {
                hub.send(c, &invite_refused(room, c, inv));
            }
            return;
        }
        let allowed = hub
            .rooms
            .get(room)
            .is_some_and(|r| r.allow.is_some() && r.occupants.get(&c).copied().unwrap_or(false));
        match target {
            Some(o) if allowed => {
                if let Some(allow) = hub.rooms.get_mut(room).and_then(|r| r.allow.as_mut()) {
                    allow.insert(o);
                }
                hub.send(o, &invite(room, o, &bare_jid(c), &reason));
            }
            _ => hub.send(c, &invite_refused(room, c, inv)),
        }
    }

    /// §4.9.
    fn iq(&mut self, c: i64, e: &Element) {
        let id = e.attr("id").unwrap_or_default();
        let kind = e.attr("type").unwrap_or_default();
        if kind == "result" || kind == "error" {
            // We never ask the client anything, so there is nothing to match.
            return;
        }
        let q = e.children.first();
        let ns = q.and_then(|q| q.attr("xmlns")).unwrap_or_default();
        if let Some(q) = q
            && self.moderation_iq(c, e, id, kind, ns, q)
        {
            return;
        }
        match (kind, ns) {
            ("get", NS_EVE_USER_DATA) => {
                let who = q.and_then(|q| q.attr("jid")).and_then(charid_of);
                let data = match who {
                    Some(w) => self.hub.directory().current().user_data(w),
                    None => UserData::default(),
                };
                // A direct child of the iq: `_GetPresenceUserData` searches
                // one level only [V].
                self.send(&iq_result(id, Some(data.element())));
            }
            ("get", NS_DISCO_INFO) => {
                let to = e.attr("to").unwrap_or_default();
                // To the conference host with a `node`: the title of that
                // channel (`GetDisplayName`). To a room: its occupant count.
                let room = match (to == conference(), q.and_then(|q| q.attr("node"))) {
                    (true, Some(node)) => node.to_owned(),
                    _ => room_of(to).unwrap_or_default(),
                };
                let hub = self.hub.lock();
                let n = hub.rooms.get(&room).map_or(0, |r| r.occupants.len());
                let name = hub.display_name(&room);
                drop(hub);
                self.send(&iq_result(id, Some(disco_info(&name, n))));
            }
            ("get", NS_DISCO_ITEMS)
                if e.attr("to") == Some(conference().as_str())
                    && q.is_some_and(|q| q.attr("node").is_some()) =>
            {
                let node = q.and_then(|q| q.attr("node")).unwrap_or_default();
                let items = self.hub.lock().channel_items(c, node);
                self.send(&iq_result(id, Some(items_result(items))));
            }
            ("get", NS_DISCO_ITEMS | NS_MUC_ADMIN | NS_EXPIRING_SEARCH) => {
                self.send(&iq_result(id, Some(empty_query(ns))));
            }
            ("set", NS_MUC_OWNER) if q.is_some_and(|q| first_child_is(q, "destroy")) => {
                let room = e.attr("to").and_then(room_of).unwrap_or_default();
                if self.hub.lock().destroy_player(c, &room) {
                    self.send(&iq_result(id, None));
                } else {
                    self.send(&iq_error(id, error_el(403, "auth", "forbidden")));
                }
            }
            (_, NS_MUC_ADMIN | NS_MUC_OWNER | NS_EXPIRING | NS_EVE_USER_DATA) => {
                println!("[chat] refused {c}: {kind} {ns} (moderation is out of scope)");
                self.send(&iq_error(id, error_el(405, "cancel", "not-allowed")));
            }
            _ => self.unanswered_iq(e),
        }
    }

    /// The moderation iqs of a player channel (lane soc-chan2): lists, the
    /// settings form, affiliations, roles and timed bans and mutes. True when
    /// the iq was one of them and has been answered; anything about another
    /// kind of room falls through to the refusal below.
    fn moderation_iq(
        &mut self,
        c: i64,
        e: &Element,
        id: &str,
        kind: &str,
        ns: &str,
        q: &Element,
    ) -> bool {
        // The timed restrictions name their room in an attribute and go to
        // the host; the rest go to the room.
        let room = match ns {
            NS_EXPIRING | NS_EXPIRING_SEARCH => q.attr("room").and_then(room_of),
            NS_MUC_ADMIN | NS_MUC_OWNER => e.attr("to").and_then(room_of),
            _ => None,
        };
        let Some(room) = room else { return false };
        if !self.hub.lock().players.contains_key(&room) {
            return false;
        }
        let item = q.child("item");
        let who_of = |i: &Element| {
            i.attr("jid")
                .and_then(charid_of)
                .or_else(|| i.attr("nick").and_then(|n| n.parse().ok()))
        };
        let result: Result<Option<Element>, u16> = match (kind, ns) {
            ("get", NS_MUC_ADMIN) => {
                let it = item.ok_or(400u16);
                it.and_then(|i| {
                    let (what, value) = match (i.attr("affiliation"), i.attr("role")) {
                        (Some(a), _) => ("affiliation", a),
                        (_, Some(r)) => ("role", r),
                        _ => return Err(400),
                    };
                    let list = self.hub.lock().listed(c, &room, what, value);
                    list.map(|l| Some(items_of(NS_MUC_ADMIN, &l))).ok_or(403)
                })
            }
            ("get", NS_EXPIRING_SEARCH) => {
                let list = q
                    .attr("category")
                    .and_then(|k| self.hub.lock().listed(c, &room, "limit", k));
                list.map(|l| Some(items_of(NS_EXPIRING_SEARCH, &l)))
                    .ok_or(403)
            }
            ("get", NS_MUC_OWNER) => self.hub.lock().config_form(c, &room).map(Some).ok_or(403),
            ("set", NS_MUC_OWNER) if first_child_is(q, "destroy") => return false,
            ("set", NS_MUC_OWNER) => {
                let fields: Vec<(String, String)> = q
                    .child("x")
                    .map(|x| {
                        x.children
                            .iter()
                            .filter(|f| f.name == "field")
                            .filter_map(|f| {
                                Some((f.attr("var")?.to_owned(), f.child("value")?.text.clone()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.hub.lock().configure(c, &room, &fields).map(|()| None)
            }
            ("set", NS_MUC_ADMIN) => {
                let mut done = Ok(None);
                let too_many =
                    q.children.iter().filter(|i| i.name == "item").count() > ADMIN_ITEMS_CAP;
                if too_many {
                    done = Err(400);
                }
                for i in q.children.iter().filter(|i| i.name == "item") {
                    if too_many {
                        break;
                    }
                    let why = i.child("reason").map(|r| {
                        xml_safe(&r.text)
                            .chars()
                            .take(REASON_CAP)
                            .collect::<String>()
                    });
                    let step = match (who_of(i), i.attr("affiliation"), i.attr("role")) {
                        (Some(w), Some(a), _) => self.hub.lock().set_affiliation(c, &room, w, a),
                        (Some(w), None, Some(r)) => {
                            self.hub.lock().set_role(c, &room, w, r, why.as_deref())
                        }
                        _ => Err(400),
                    };
                    if let Err(code) = step {
                        done = Err(code);
                        break;
                    }
                }
                done
            }
            ("set", NS_EXPIRING) => {
                let who = q.attr("jid").and_then(charid_of);
                let secs = match q.attr("action") {
                    Some("delete") => Some(None),
                    Some("set") => q.attr("duration").and_then(|d| d.parse().ok()).map(Some),
                    _ => None,
                };
                match (who, secs, q.attr("category")) {
                    (Some(w), Some(secs), Some(k)) => self
                        .hub
                        .lock()
                        .set_limit(c, &room, w, k, secs)
                        .map(|()| None),
                    _ => Err(400),
                }
            }
            _ => return false,
        };
        match result {
            Ok(body) => self.send(&iq_result(id, body)),
            Err(code) => {
                println!("[chat] refused {c}: {kind} {ns} in {room} ({code})");
                let err = match code {
                    400 => error_el(400, "modify", "bad-request"),
                    404 => error_el(404, "cancel", "item-not-found"),
                    405 => error_el(405, "cancel", "not-allowed"),
                    409 => error_el(409, "cancel", "conflict"),
                    _ => error_el(403, "auth", "forbidden"),
                };
                self.send(&iq_error(id, err));
            }
        }
        true
    }

    /// Rule 8's default: an iq we do not implement still gets an answer.
    fn unanswered_iq(&self, e: &Element) {
        println!("[chat] UNANSWERED {} iq: {e}", self.who());
        let id = e.attr("id").unwrap_or_default();
        self.send(&iq_error(
            id,
            error_el(501, "cancel", "feature-not-implemented"),
        ));
    }
}

fn first_child_is(e: &Element, name: &str) -> bool {
    e.children.first().is_some_and(|c| c.name == name)
}

/// `room@conference.H[/nick]` -> `room`, or `None` if it is not one of ours.
fn room_of(jid: &str) -> Option<String> {
    let bare = jid.split('/').next()?;
    let (room, domain) = bare.split_once('@')?;
    (domain == conference()).then(|| room.to_owned())
}

/// [`room_of`] for a caller outside this module (`XmppChatMgr.SetMotd`).
pub fn room_name_of(jid: &str) -> Option<String> {
    room_of(jid)
}

/// `C@H[/res]` -> `C`.
fn charid_of(jid: &str) -> Option<i64> {
    jid.split('@').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::session_table::SessionEntry;
    use eve_xmpp::Client;
    use std::sync::RwLock;

    const A: i64 = 90_000_001;
    const B: i64 = 90_000_002;
    const C: i64 = 90_000_003;
    const SYS1: i64 = 30_000_001;
    const SYS2: i64 = 30_000_003;
    const QUIET: Duration = Duration::from_millis(300);

    struct World {
        world: Arc<RwLock<Registry>>,
        hub: Arc<ChatHub>,
        addr: SocketAddr,
    }

    fn pilot(id: i64, n: i64) -> Character {
        Character {
            character_id: id,
            ship_id: 100_000_000 + n,
            ..Character::default()
        }
    }

    /// **The reconnect race** (lane undock-invuln). A reconcile pass that read
    /// the world while the character was offline must not revoke the token a
    /// login minted after that read, nor close that login's stream; one minted
    /// before it still goes.
    #[test]
    fn a_stale_offline_verdict_spares_a_login_made_after_the_snapshot() {
        let hub = ChatHub::new(ChatConfig::default());
        let old = hub.mint_token(A);
        let snapshot = hub.lock().token_counter;
        let fresh = hub.mint_token(A);
        let tx = ChatOut::new(None, 1, None);
        hub.register(A, 1, tx);
        // The stale pass: "A is offline", from before `fresh` existed.
        hub.lock().reconcile(&BTreeMap::new(), snapshot);
        assert!(!hub.take_token(&old, A), "older than the snapshot: revoked");
        assert!(hub.take_token(&fresh, A), "newer: kept");
        assert!(hub.lock().streams.contains_key(&A), "the new stream stays");
        // The next pass is a fresh snapshot and does take them.
        let again = hub.mint_token(A);
        let snapshot = hub.lock().token_counter;
        hub.lock().reconcile(&BTreeMap::new(), snapshot);
        assert!(!hub.take_token(&again, A));
        assert!(!hub.lock().streams.contains_key(&A));
    }

    /// Three characters in one system and one NPC corp, all online.
    fn world() -> World {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![pilot(A, 1), pilot(B, 2), pilot(C, 3)],
            ..SandboxConfig::default()
        });
        for (i, c) in [A, B, C].into_iter().enumerate() {
            let client_id = 10 + i as i64;
            reg.session_opened(SessionEntry {
                client_id,
                session_id: client_id,
                user_id: 1,
                character_id: None,
            });
            reg.session_selected_character(client_id, c);
        }
        let hub = ChatHub::new(ChatConfig::default());
        reg.set_chat(Arc::clone(&hub));
        let world = Arc::new(RwLock::new(reg));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        spawn_listener(listener, Arc::clone(&hub)).unwrap();
        hub.reconcile();
        World { world, hub, addr }
    }

    impl World {
        fn login(&self, c: i64) -> Client {
            let token = self.hub.mint_token(c);
            let mut cl = Client::login(self.addr, HOSTNAME, c, &token).expect("login");
            cl.set_timeout(Duration::from_secs(3));
            cl
        }

        /// Log in and join `room`, consuming the join's answer.
        fn joined(&self, c: i64, room: &str) -> Client {
            let mut cl = self.login(c);
            let id = cl.join(room).unwrap();
            cl.wait_for(|e| e.name == "presence" && e.attr("id") == Some(id.as_str()))
                .unwrap();
            cl.drain(QUIET).unwrap();
            cl
        }

        /// Publish the world's chat directory, then run one reconcile pass.
        fn sync(&self) {
            crate::chat_dir::publish(&self.write());
            self.hub.reconcile();
        }

        fn write(&self) -> std::sync::RwLockWriteGuard<'_, Registry> {
            crate::service::world_write(&self.world)
        }
    }

    fn local(ss: i64) -> String {
        format!("local_{ss}")
    }

    fn froms(stanzas: &[Element]) -> Vec<String> {
        stanzas
            .iter()
            .map(|e| e.attr("from").unwrap_or_default().to_owned())
            .collect()
    }

    fn read_some(s: &mut TcpStream, wait: Duration) -> Vec<u8> {
        s.set_read_timeout(Some(wait)).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        out
    }

    /// No byte between top-level stanzas: every `>` that ends a depth-1
    /// element is followed by `<` or by nothing (§3 rule 1).
    fn assert_no_interstanza_bytes(bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        for (i, w) in text.as_bytes().windows(2).enumerate() {
            if w[0] == b'>' {
                assert!(
                    w[1] == b'<' || !(w[1] as char).is_ascii_whitespace(),
                    "whitespace after '>' at byte {i}: {text}"
                );
            }
        }
    }

    /// Test 1 and 2: the golden transcript of §2, byte for byte, then the
    /// initial-presence echo.
    #[test]
    fn the_login_transcript_is_byte_exact_and_ends_in_the_presence_echo() {
        let w = world();
        let token = w.hub.mint_token(A);
        let mut s = TcpStream::connect(w.addr).unwrap();
        s.write_all(b"<?xml version='1.0'?><stream:stream to='eveoffline' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").unwrap();
        let got = read_some(&mut s, QUIET);
        let text = String::from_utf8(got.clone()).unwrap();
        assert!(text.starts_with("<?xml version='1.0'?><stream:stream from='eveoffline' id='"));
        assert!(text.ends_with(
            "' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>\
<stream:features><mechanisms xmlns='urn:ietf:params:xml:ns:xmpp-sasl'><mechanism>PLAIN</mechanism></mechanisms></stream:features>"
        ), "{text}");
        assert_no_interstanza_bytes(&got);

        let auth = eve_xmpp::sasl::plain_encode(&A.to_string(), &token);
        s.write_all(
            format!(
                "<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{auth}</auth>"
            )
            .as_bytes(),
        )
        .unwrap();
        // Rule 11: success, and then nothing until our new header.
        assert_eq!(
            String::from_utf8(read_some(&mut s, QUIET)).unwrap(),
            "<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>"
        );

        s.write_all(b"<?xml version='1.0'?><stream:stream to='eveoffline' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>").unwrap();
        let text = String::from_utf8(read_some(&mut s, QUIET)).unwrap();
        assert!(text.ends_with(
            "<stream:features><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'/><session xmlns='urn:ietf:params:xml:ns:xmpp-session'/></stream:features>"
        ), "{text}");

        s.write_all(format!("<iq id='id1' type='set' from='{A}@eveoffline' ><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'><resource>eveclient</resource></bind></iq>").as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(read_some(&mut s, QUIET)).unwrap(),
            format!(
                "<iq type='result' id='id1'><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'><jid>{A}@eveoffline/eveclient</jid></bind></iq>"
            )
        );
        s.write_all(format!("<iq id='id2' type='set' from='{A}@eveoffline/eveclient' ><session xmlns='urn:ietf:params:xml:ns:xmpp-session'/></iq>").as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(read_some(&mut s, QUIET)).unwrap(),
            "<iq type='result' id='id2'/>"
        );
        s.write_all(b"<presence><show/></presence>").unwrap();
        let rest = read_some(&mut s, QUIET);
        assert_no_interstanza_bytes(&rest);
        let text = String::from_utf8(rest).unwrap();
        // Test 2: the first stanza after `ready` is the echo.
        let echo =
            format!("<presence from='{A}@eveoffline/eveclient' to='{A}@eveoffline/eveclient'/>");
        assert!(text.starts_with(&echo), "{text}");
    }

    /// Test 3.
    #[test]
    fn a_token_works_once_for_its_own_character_and_not_after_logoff() {
        let w = world();
        let deny = |c: i64, token: &str| {
            let e = Client::login(w.addr, HOSTNAME, c, token)
                .err()
                .expect("refused");
            assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        };
        let token = w.hub.mint_token(B);
        deny(A, &token);
        deny(A, "not-a-token");
        let mine = w.hub.mint_token(A);
        Client::login(w.addr, HOSTNAME, A, &mine).expect("its own token");
        deny(A, &mine);

        let later = w.hub.mint_token(C);
        w.write().session_closed(12);
        deny(C, &later);
    }

    /// Test 4: every join is answered with its own id and status 110, re-joins
    /// included, and the others hear about the joiner once.
    #[test]
    fn join_answers_carry_the_id_and_rejoins_do_not_rebroadcast() {
        let w = world();
        let mut a = w.joined(A, &local(SYS1));
        let mut b = w.login(B);
        for _ in 0..2 {
            let id = b.join(&local(SYS1)).unwrap();
            let answer = b
                .wait_for(|e| e.name == "presence" && e.attr("id") == Some(id.as_str()))
                .unwrap();
            assert_eq!(
                answer.attr("from"),
                Some(occupant_jid(&local(SYS1), B).as_str())
            );
            let x = answer.child("x").unwrap();
            assert_eq!(x.attr("xmlns"), Some(NS_MUC_USER));
            assert_eq!(x.child("status").and_then(|s| s.attr("code")), Some("110"));
            assert!(answer.child("eve_user_data").is_some());
        }
        let heard = a.drain(QUIET).unwrap();
        let about_b = occupant_jid(&local(SYS1), B);
        assert_eq!(
            heard
                .iter()
                .filter(|e| e.attr("from") == Some(about_b.as_str()))
                .count(),
            0,
            "B was already in A's roster; joining twice told A nothing new: {heard:?}"
        );
    }

    /// Test 10: derived membership needs no stream -- the roster a joiner gets
    /// lists every online character in the system, connected or not.
    #[test]
    fn the_roster_lists_members_without_a_chat_connection() {
        let w = world();
        let mut a = w.login(A);
        let id = a.join(&local(SYS1)).unwrap();
        let mut got = Vec::new();
        loop {
            let e = a.recv().unwrap();
            let done = e.attr("id") == Some(id.as_str());
            got.push(e);
            if done {
                break;
            }
        }
        let roster: BTreeSet<String> = froms(&got)
            .into_iter()
            .filter(|f| f.starts_with(&local(SYS1)))
            .collect();
        let want: BTreeSet<String> = [A, B, C]
            .iter()
            .map(|c| occupant_jid(&local(SYS1), *c))
            .collect();
        assert_eq!(roster, want);
        // The self-presence comes last, after the others (§4.2).
        assert_eq!(
            got.last().unwrap().attr("from"),
            Some(occupant_jid(&local(SYS1), A).as_str())
        );
    }

    /// Test 6, and the invariant: a Local line reaches every listening member,
    /// the sender included and with its id, and nobody outside.
    #[test]
    fn a_local_message_reaches_every_member_and_no_one_else() {
        let w = world();
        w.write().character_mut(C).unwrap().solar_system_id = Some(SYS2);
        w.sync();
        let mut a = w.joined(A, &local(SYS1));
        let mut b = w.joined(B, &local(SYS1));
        let mut c = w.joined(C, &local(SYS2));
        a.drain(QUIET).unwrap();

        let id = a.say(&local(SYS1), "o7 <&>").unwrap();
        for (who, cl) in [(A, &mut a), (B, &mut b)] {
            let m = cl.wait_for(|e| e.name == "message").unwrap();
            assert_eq!(m.attr("type"), Some("groupchat"), "to {who}");
            assert_eq!(m.attr("id"), Some(id.as_str()), "to {who}");
            assert_eq!(m.attr("from"), Some(occupant_jid(&local(SYS1), A).as_str()));
            assert_eq!(m.attr("to"), Some(full_jid(who).as_str()));
            assert_eq!(m.child("body").unwrap().text, "o7 <&>");
        }
        assert!(
            c.drain(QUIET).unwrap().iter().all(|e| e.name != "message"),
            "a pilot in another system heard Local"
        );
    }

    /// Test 7.
    #[test]
    fn speaking_in_a_room_you_are_not_in_is_a_403() {
        let w = world();
        let mut a = w.login(A);
        let id = a.say(&local(SYS2), "hello?").unwrap();
        let e = a.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(e.attr("type"), Some("error"));
        assert_eq!(e.attr("id"), Some(id.as_str()));
        assert_eq!(e.child("error").and_then(|e| e.attr("code")), Some("403"));
    }

    /// Test 5 and test 9: a system change takes the pilot out of the old
    /// Local before anything about the new one, the old Local hears a JSON
    /// `leave` with a numeric charid, and the pilot is invited to the new one.
    #[test]
    fn a_system_change_moves_local_membership() {
        let w = world();
        let mut a = w.joined(A, &local(SYS1));
        let mut b = w.joined(B, &local(SYS1));
        a.drain(QUIET).unwrap();

        w.write().character_mut(B).unwrap().solar_system_id = Some(SYS2);
        w.sync();

        let leave = a.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(leave.attr("type"), Some("groupchat"));
        assert_eq!(
            leave.attr("from"),
            Some(room_jid(&local(SYS1)).as_str()),
            "admin: no resource"
        );
        assert_eq!(
            leave.child("body").unwrap().text,
            format!("{{\"cmd\":\"leave\",\"charid\":{B}}}")
        );

        let seen = b.drain(QUIET).unwrap();
        let old = local(SYS1);
        assert!(
            seen.iter()
                .all(|e| !e.attr("from").unwrap_or_default().starts_with(&old)),
            "the mover still heard the old Local: {seen:?}"
        );
        let inv = seen
            .iter()
            .find(|e| e.name == "message" && e.child("x").is_some())
            .expect("an invite to the new Local");
        assert_eq!(inv.attr("from"), Some(room_jid(&local(SYS2)).as_str()));
        assert_eq!(inv.attr("type"), None, "an invite has no type");
        let invite_el = inv.child("x").unwrap().child("invite").unwrap();
        assert_eq!(
            invite_el.attr("from"),
            Some(format!("admin@{HOSTNAME}").as_str())
        );

        assert_eq!(w.hub.occupants(&local(SYS1)), vec![A, C]);
        assert_eq!(w.hub.occupants(&local(SYS2)), vec![B]);

        // And A's next line does not reach B.
        a.say(&local(SYS1), "still here").unwrap();
        assert!(b.drain(QUIET).unwrap().iter().all(|e| e.name != "message"));
    }

    /// Test 5's other half: outside Local a departure is an unavailable
    /// presence, not JSON.
    #[test]
    fn leaving_corp_is_an_unavailable_presence() {
        let w = world();
        let corp = format!("corp_{}", Character::default().corporation_id);
        let mut a = w.joined(A, &corp);
        let _b = w.joined(B, &corp);
        a.drain(QUIET).unwrap();
        w.write().character_mut(B).unwrap().corporation_id = 98_000_001;
        w.sync();
        let p = a.wait_for(|e| e.name == "presence").unwrap();
        assert_eq!(p.attr("type"), Some("unavailable"));
        assert_eq!(p.attr("from"), Some(occupant_jid(&corp, B).as_str()));
        assert!(a.drain(QUIET).unwrap().iter().all(|e| e.name != "message"));
    }

    /// Join `room` with an XEP-0045 `<history seconds>`, the reconnect gap
    /// fill; returns the join's id.
    fn join_with_history(cl: &mut Client, room: &str, secs: u64) -> String {
        let id = cl.next_id();
        let s = format!(
            "<presence from='{}' id='{id}' to='{}/{}'><x xmlns='http://jabber.org/protocol/muc'><history seconds='{secs}'/></x></presence>",
            cl.full_jid(),
            cl.room_jid(room),
            cl.charid()
        );
        cl.send_raw(&s).unwrap();
        id
    }

    fn text_of(e: &Element) -> String {
        e.child("body").map(|b| b.text.clone()).unwrap_or_default()
    }

    /// G1: the opener's first line in a private conversation was sent before
    /// the other pilot joined, and used to be lost.
    #[test]
    fn a_private_rooms_first_line_is_replayed_to_the_invited_pilot() {
        let w = world();
        let room = w.hub.create_private(A, B);
        let mut a = w.joined(A, &room);
        let mut b = w.login(B);
        let said = a.say(&room, "hi").unwrap();
        a.wait_for(|e| e.name == "message").unwrap();
        a.send_raw(&format!(
            "<message to='{}' type='normal'><x xmlns='{NS_MUC_USER}'><invite to='{}'/></x></message>",
            a.room_jid(&room),
            b.bare_jid()
        ))
        .unwrap();
        b.wait_for(|e| e.name == "message" && e.child("x").is_some())
            .unwrap();
        let id = b.join(&room).unwrap();
        b.wait_for(|e| e.name == "presence" && e.attr("id") == Some(id.as_str()))
            .unwrap();
        let line = b.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(line.attr("from"), Some(occupant_jid(&room, A).as_str()));
        assert_eq!(line.attr("type"), Some("groupchat"));
        assert_eq!(line.attr("id"), Some(said.as_str()));
        assert_eq!(text_of(&line), "hi");
        assert_eq!(
            line.child("delay").and_then(|d| d.attr("xmlns")),
            Some("urn:xmpp:delay")
        );
    }

    /// G1: Local never stores, and a non-private room replays only on an
    /// explicit `<history>`.
    #[test]
    fn local_is_never_replayed_and_other_rooms_only_on_request() {
        let w = world();
        let corp = format!("corp_{}", Character::default().corporation_id);
        let mut a = w.joined(A, &local(SYS1));
        let mut ab = w.joined(B, &corp);
        a.say(&local(SYS1), "in local").unwrap();
        ab.say(&corp, "in corp").unwrap();
        a.wait_for(|e| e.name == "message").unwrap();
        ab.wait_for(|e| e.name == "message").unwrap();

        let mut c = w.login(C);
        let id = join_with_history(&mut c, &local(SYS1), 600);
        c.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert!(c.drain(QUIET).unwrap().iter().all(|e| e.name != "message"));

        let id = c.join(&corp).unwrap();
        c.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert!(c.drain(QUIET).unwrap().iter().all(|e| e.name != "message"));

        let id = join_with_history(&mut c, &corp, 600);
        c.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        let line = c.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(text_of(&line), "in corp");
    }

    /// G2: a corp change tells Local's listeners without a leave.
    #[test]
    fn a_corp_change_sends_refresh_char_and_no_leave_in_local() {
        let w = world();
        let mut a = w.joined(A, &local(SYS1));
        let _b = w.joined(B, &local(SYS1));
        a.drain(QUIET).unwrap();
        w.write().character_mut(B).unwrap().corporation_id = 98_000_002;
        w.sync();
        let msgs: Vec<Element> = a
            .drain(QUIET)
            .unwrap()
            .into_iter()
            .filter(|e| e.name == "message")
            .collect();
        let refresh = msgs
            .iter()
            .find(|e| text_of(e).contains("refresh_char"))
            .expect("refresh_char");
        assert_eq!(refresh.attr("from"), Some(room_jid(&local(SYS1)).as_str()));
        let body = text_of(refresh);
        assert!(body.contains(&format!("\"charid\":{B}")), "{body}");
        assert!(body.contains("\"corpid\":98000002"), "{body}");
        assert!(msgs.iter().all(|e| !text_of(e).contains("\"leave\"")));
    }

    /// Logging off takes a pilot out of every room; their stream closes after
    /// a linger, and a presence written in between draws no error.
    #[test]
    fn logoff_removes_the_pilot_everywhere_and_closes_the_stream_late() {
        let w = world();
        let mut a = w.joined(A, &local(SYS1));
        let mut b = w.joined(B, &local(SYS1));
        a.drain(QUIET).unwrap();
        w.write().session_closed(11);
        w.sync();
        let leave = a.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(
            leave.child("body").unwrap().text,
            format!("{{\"cmd\":\"leave\",\"charid\":{B}}}")
        );
        assert_eq!(w.hub.occupants(&local(SYS1)), vec![A, C]);
        // Still open, and quiet: the client's presence on it is not answered
        // with an error.
        assert!(matches!(
            b.recv_timeout(Duration::from_millis(100)),
            Ok(None)
        ));
        b.leave(&local(SYS1)).unwrap();
        let deadline = Instant::now() + LOGOFF_LINGER * 5;
        let end = loop {
            match b.recv_timeout(QUIET) {
                Ok(Some(e)) => {
                    assert_ne!(e.attr("type"), Some("error"), "{e}");
                }
                Ok(None) => {
                    if Instant::now() > deadline {
                        break Ok(None::<Element>);
                    }
                }
                Err(e) => break Err(e),
            }
        };
        assert!(end.is_err(), "B's stream stayed open: {end:?}");
    }

    /// Test 11.
    #[test]
    fn resync_lists_local_then_corp_for_a_docked_pilot() {
        let w = world();
        let reg = w.world.read().unwrap();
        assert_eq!(
            system_rooms(&reg, A),
            vec![
                local(SYS1),
                format!("corp_{}", Character::default().corporation_id)
            ]
        );
    }

    /// Fleet membership adds `fleet_<id>` and gives the fleet its own room.
    #[test]
    fn a_fleet_member_is_put_in_the_fleet_room() {
        let w = world();
        let fleet = {
            let mut reg = w.write();
            let id = reg.fleets_mut().create(A).unwrap();
            reg.fleets_mut()
                .init(A, id, None, None, Some(SYS1))
                .unwrap();
            id
        };
        w.sync();
        let room = format!("fleet_{fleet}");
        assert!(system_rooms(&w.world.read().unwrap(), A).contains(&room));
        assert_eq!(w.hub.occupants(&room), vec![A]);
        let mut a = w.joined(A, &room);
        let id = a.say(&room, "x up").unwrap();
        let m = a.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(m.attr("id"), Some(id.as_str()));
        // B is not in the fleet, so B may not join it.
        let mut b = w.login(B);
        let id = b.join(&room).unwrap();
        let refused = b.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(refused.attr("type"), Some("error"));
        assert_eq!(
            refused.child("error").and_then(|e| e.attr("code")),
            Some("407")
        );
    }

    /// **PA-11.** Chat answers while another thread holds the world's write
    /// lock: logins, a join, a groupchat and a user-data iq.
    #[test]
    fn chat_serves_logins_and_rooms_while_the_world_is_write_locked() {
        let w = world();
        let mut b = w.joined(B, &local(SYS1));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (held, is_held) = std::sync::mpsc::channel::<()>();
        let holder = {
            let world = Arc::clone(&w.world);
            std::thread::spawn(move || {
                let _guard = world.write().expect("unpoisoned");
                held.send(()).expect("test alive");
                let _ = wait.recv();
            })
        };
        is_held.recv().expect("holder started");
        let started = Instant::now();
        let mut a = w.joined(A, &local(SYS1));
        b.drain(QUIET).unwrap();
        a.say(&local(SYS1), "while locked").unwrap();
        let m = b.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(m.child("body").unwrap().text, "while locked");
        let id = a
            .iq(
                "get",
                Some(HOSTNAME),
                &format!("<query xmlns='urn:xmpp:eve_user_data' jid='{C}@{HOSTNAME}'/>"),
            )
            .unwrap();
        let r = a.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(r.children.first().unwrap().name, "eve_user_data");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            w.world.try_read().is_err(),
            "the holder must still have held the lock when the answers came"
        );
        release.send(()).unwrap();
        holder.join().unwrap();
    }

    /// **PA-11.** A resync is applied to the hub at once, and a reconcile
    /// pass over the older published directory does not put the pilot back.
    #[test]
    fn a_resync_is_not_undone_by_an_older_directory() {
        use crate::service::{Caller, Request};
        let w = world();
        w.sync();
        let hub_local = |ss: i64| w.hub.occupants(&local(ss));
        // A is in Local of SYS1 through the first publish.
        assert!(hub_local(SYS1).contains(&A));
        // The world moves A; nothing publishes it.
        w.write().character_mut(A).unwrap().solar_system_id = Some(SYS2);
        let req = Request {
            service: "XmppChatMgr",
            method: "ResyncSystemChannelAccess",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(A),
        };
        let reply = w.write().dispatch_as(Caller::Internal, &req).unwrap();
        w.hub.reconcile();
        let eve_marshal::PyValue::List(rooms) = reply else {
            panic!("a list")
        };
        let names: Vec<String> = rooms
            .iter()
            .filter_map(|r| match r {
                eve_marshal::PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&local(SYS2)) && !names.contains(&local(SYS1)));
        assert!(hub_local(SYS2).contains(&A), "in the new Local");
        assert!(!hub_local(SYS1).contains(&A), "and out of the old one");
    }

    /// **PA-11.** Structural: the part of this file above its tests never
    /// names the world lock.
    #[test]
    fn chat_rs_never_takes_the_world_lock() {
        let src = include_str!("chat.rs");
        let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        for needle in [
            "world.read(",
            "world.write(",
            "RwLock<Registry>",
            "world_write(",
        ] {
            assert!(!code.contains(needle), "chat.rs names `{needle}`");
        }
    }

    /// **PA-11.** The reconcile loop wakes on a publish, not on its period.
    #[test]
    fn the_reconcile_thread_wakes_on_a_publish() {
        let w = world();
        w.write().character_mut(C).unwrap().solar_system_id = Some(SYS2);
        let hub = Arc::clone(&w.hub);
        std::thread::spawn(move || run_reconcile(&hub, Duration::from_secs(10)));
        std::thread::sleep(QUIET);
        crate::chat_dir::publish(&w.write());
        let start = Instant::now();
        while !w.hub.occupants(&local(SYS2)).contains(&C) {
            assert!(start.elapsed() < Duration::from_secs(1), "not applied");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Test 12.
    #[test]
    fn eve_user_data_is_a_direct_child_of_the_iq() {
        let w = world();
        let mut a = w.login(A);
        let id = a
            .iq(
                "get",
                Some(HOSTNAME),
                &format!("<query xmlns='urn:xmpp:eve_user_data' jid='{B}@{HOSTNAME}'/>"),
            )
            .unwrap();
        let r = a.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(r.attr("type"), Some("result"));
        let d = r.children.first().unwrap();
        assert_eq!(d.name, "eve_user_data");
        assert_eq!(
            d.attr("corpid"),
            Some(Character::default().corporation_id.to_string().as_str())
        );
    }

    /// Test 13.
    #[test]
    fn an_unknown_iq_is_an_error_with_the_same_id() {
        let w = world();
        let mut a = w.login(A);
        let id = a
            .iq(
                "get",
                Some(HOSTNAME),
                "<query xmlns='urn:example:nothing'/>",
            )
            .unwrap();
        let r = a.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(r.attr("type"), Some("error"));
        assert_eq!(r.child("error").and_then(|e| e.attr("code")), Some("501"));
    }

    /// Test 14.
    #[test]
    fn a_tls_hello_is_closed_without_a_byte_of_xml() {
        let w = world();
        let mut s = TcpStream::connect(w.addr).unwrap();
        s.write_all(&[0x16, 0x03, 0x01, 0x00, 0x05]).unwrap();
        assert_eq!(read_some(&mut s, Duration::from_secs(1)), Vec::<u8>::new());
    }

    /// Test 8: every error constructor carries a `code`.
    #[test]
    fn every_error_the_server_can_send_has_a_code() {
        let errors = [
            join_error("local_1", A, "id1", 407),
            join_error("private_1", A, "id1", 404),
            message_error(
                "local_1",
                A,
                Some("id1"),
                error_el(403, "auth", "forbidden"),
                None,
            ),
            message_error(
                "private_1",
                A,
                None,
                error_el(405, "cancel", "not-allowed"),
                None,
            ),
            iq_error("id1", error_el(501, "cancel", "feature-not-implemented")),
            iq_error("id1", error_el(405, "cancel", "not-allowed")),
        ];
        for e in errors {
            let err = e.child("error").expect("an error child");
            assert!(
                err.attr("code").is_some_and(|c| c.parse::<u16>().is_ok()),
                "{e}"
            );
        }
    }

    /// Private conversations: the machoNet path a GM-roled player takes.
    /// Create, add the other pilot (who gets an admin invite), both join,
    /// text goes both ways; a third pilot can neither join nor hear.
    #[test]
    fn a_private_conversation_round_trips_and_stays_private() {
        let w = world();
        let room = w.hub.create_private(A, B);
        let mut a = w.joined(A, &room);
        let mut b = w.login(B);
        let mut c = w.login(C);
        assert!(w.hub.add_to_channel(&room, B));
        let inv = b.wait_for(|e| e.name == "message").unwrap();
        let from = inv
            .child("x")
            .unwrap()
            .child("invite")
            .unwrap()
            .attr("from")
            .unwrap()
            .to_owned();
        assert_eq!(from, format!("admin@{HOSTNAME}"));
        let id = b.join(&room).unwrap();
        b.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        // A hears B arrive: a private room has no derived members.
        let arrived = a.wait_for(|e| e.name == "presence").unwrap();
        assert_eq!(arrived.attr("from"), Some(occupant_jid(&room, B).as_str()));

        b.say(&room, "hi A").unwrap();
        assert_eq!(
            a.wait_for(|e| e.name == "message")
                .unwrap()
                .child("body")
                .unwrap()
                .text,
            "hi A"
        );
        a.say(&room, "hi B").unwrap();
        assert_eq!(
            b.wait_for(
                |e| e.name == "message" && e.attr("from") == Some(occupant_jid(&room, A).as_str())
            )
            .unwrap()
            .child("body")
            .unwrap()
            .text,
            "hi B"
        );

        let id = c.join(&room).unwrap();
        let refused = c.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(
            refused.child("error").and_then(|e| e.attr("code")),
            Some("404")
        );
        assert!(c.drain(QUIET).unwrap().iter().all(|e| e.name != "message"));
    }

    /// A server-side speaker with no stream can talk in its Local.
    #[test]
    fn a_bot_without_a_stream_can_speak_in_local() {
        let w = world();
        let mut a = w.joined(A, &local(SYS1));
        assert!(w.hub.say(&local(SYS1), C, "beep"));
        let m = a.wait_for(|e| e.name == "message").unwrap();
        assert_eq!(m.attr("from"), Some(occupant_jid(&local(SYS1), C).as_str()));
        assert!(!w.hub.say(&local(SYS2), C, "not there"));
    }

    const CONF: &str = "conference.eveoffline";

    /// One `disco#items` to the conference host, returning the item attributes.
    fn items(cl: &mut Client, node: &str) -> Vec<(String, String, String)> {
        let id = cl
            .iq(
                "get",
                Some(CONF),
                &format!("<query xmlns='{NS_DISCO_ITEMS}' node='{node}'></query>"),
            )
            .unwrap();
        let r = cl
            .wait_for(|e| e.name == "iq" && e.attr("id") == Some(id.as_str()))
            .unwrap();
        r.child("query")
            .unwrap()
            .children
            .iter()
            .map(|i| {
                (
                    i.attr("jid").unwrap().to_owned(),
                    i.attr("name").unwrap().to_owned(),
                    i.attr("node").unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn join_room(cl: &mut Client, room: &str) -> Element {
        let id = cl.join(room).unwrap();
        cl.wait_for(|e| e.name == "presence" && e.attr("id") == Some(id.as_str()))
            .unwrap()
    }

    #[test]
    fn a_created_room_is_found_by_name() {
        let w = world();
        let room = w.hub.create_player(A, "  Mining Corp Friends ");
        assert!(room.starts_with("player_") && valid_room_name(&room));
        let mut b = w.login(B);
        // The name search is exact on the trimmed name; a part of it is empty.
        let found = items(&mut b, "byname/Mining Corp Friends");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, room_jid(&room));
        assert_eq!(found[0].1, "Mining Corp Friends");
        assert!(items(&mut b, "byname/mining").is_empty());
        // The title comes from disco#info with the room as the node.
        let id = b
            .iq(
                "get",
                Some(CONF),
                &format!("<query xmlns='{NS_DISCO_INFO}' node='{room}'></query>"),
            )
            .unwrap();
        let r = b.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        let name = r
            .child("query")
            .and_then(|q| q.child("identity"))
            .and_then(|i| i.attr("name"));
        assert_eq!(name, Some("Mining Corp Friends"));
    }

    #[test]
    fn forme_marks_the_owner() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        assert_eq!(
            items(&mut a, "forme"),
            vec![(room_jid(&room), "Haulers".to_owned(), "owner".to_owned())]
        );
        // A stranger's row is not marked as the owner's. Whether a passworded
        // channel is listed to strangers is `[A]`, so it is not pinned.
        assert_ne!(items(&mut b, "forme")[0].2, "owner");
        assert!(w.hub.set_player_password(&room, "pw"));
        assert_eq!(items(&mut a, "forme").len(), 1);
    }

    #[test]
    fn the_owner_is_set_apart_in_the_channel() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        let mine = join_room(&mut a, &room);
        let item = |e: &Element| {
            let i = e.child("x").unwrap().child("item").unwrap();
            (
                i.attr("affiliation").unwrap().to_owned(),
                i.attr("role").unwrap().to_owned(),
            )
        };
        // The role strings are `[A]` (the FCI scenario checks them against the
        // client); here only that the owner differs from a joiner.
        let theirs = join_room(&mut b, &room);
        assert_ne!(item(&mine), item(&theirs));
        // The roster line the join sent for the owner says so too.
        let roster = b
            .drain(QUIET)
            .unwrap()
            .into_iter()
            .find(|e| e.attr("from") == Some(occupant_jid(&room, A).as_str()));
        assert_eq!(
            item(&roster.expect("the owner is in the roster")).0,
            "owner"
        );
    }

    #[test]
    fn a_wrong_password_is_401() {
        let w = world();
        let room = w.hub.create_player(A, "Secret");
        assert!(w.hub.set_player_password(&room, "hunter"));
        let mut b = w.login(B);
        let join = |cl: &mut Client, pw: Option<&str>| {
            let id = cl.next_id();
            let pw = pw
                .map(|p| format!("<password>{p}</password>"))
                .unwrap_or_default();
            cl.send_raw(&format!(
                "<presence id='{id}' to='{}/{B}'><x xmlns='{NS_MUC}'>{pw}</x></presence>",
                room_jid(&room)
            ))
            .unwrap();
            cl.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap()
        };
        for pw in [None, Some("wrong")] {
            let r = join(&mut b, pw);
            assert_eq!(r.child("error").and_then(|e| e.attr("code")), Some("401"));
        }
        let ok = join(&mut b, Some("hunter"));
        assert!(ok.child("error").is_none());
        // The owner needs no password.
        let mut a = w.login(A);
        assert!(join_room(&mut a, &room).child("error").is_none());
    }

    #[test]
    fn only_the_owner_destroys() {
        let w = world();
        let room = w.hub.create_player(A, "Doomed");
        let mut a = w.login(A);
        let mut b = w.login(B);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        let destroy = |cl: &mut Client| {
            let id = cl
                .iq(
                    "set",
                    Some(&room_jid(&room)),
                    &format!("<query xmlns='{NS_MUC_OWNER}'><destroy/></query>"),
                )
                .unwrap();
            cl.wait_for(|e| e.name == "iq" && e.attr("id") == Some(id.as_str()))
                .unwrap()
        };
        let refused = destroy(&mut b);
        assert_eq!(refused.attr("type"), Some("error"));
        assert_eq!(
            refused.child("error").and_then(|e| e.attr("code")),
            Some("403")
        );
        assert_eq!(items(&mut b, "byname/doomed").len(), 1);
        let done = destroy(&mut a);
        assert_eq!(done.attr("type"), Some("result"));
        // B is told the room is gone, the name search no longer finds it, and
        // joining it is refused.
        let gone = b
            .wait_for(|e| {
                e.name == "presence"
                    && e.attr("type") == Some("unavailable")
                    && e.child("x").is_some_and(|x| x.child("destroy").is_some())
            })
            .unwrap();
        assert_eq!(gone.attr("from"), Some(occupant_jid(&room, B).as_str()));
        assert!(items(&mut b, "byname/doomed").is_empty());
        assert_eq!(
            join_room(&mut b, &room)
                .child("error")
                .and_then(|e| e.attr("code")),
            Some("404")
        );
        // The deletion is queued for the world file.
        let rows = w.hub.take_dirty();
        assert!(rows.iter().any(|(k, v)| k[0] == room && v.is_none()));
    }

    #[test]
    fn xml_unsafe_characters_never_reach_a_stored_subject_or_name() {
        let w = world();
        let room = w.hub.create_player(A, "Ha\u{1}u\u{FFFF}l\u{FFFE}ers");
        assert_eq!(w.hub.lock().display_name(&room), "Haulers");
        assert!(w.hub.set_motd(A, &room, "a\u{1}b\u{B}c\tok\u{FFFF}"));
        assert_eq!(w.hub.lock().players[&room].motd, "abc\tok");
        // The same filter covers a spoken body.
        assert_eq!(xml_safe("x\u{0}y\u{1F}z\n"), "xyz\n");
        // A long multibyte MOTD is not cut at the body cap.
        let long = "\u{e9}".repeat(MOTD_CAP);
        assert!(w.hub.set_motd(A, &room, &long));
        assert_eq!(w.hub.lock().players[&room].motd.chars().count(), MOTD_CAP);
    }

    #[test]
    fn a_name_with_markup_characters_is_found_by_name() {
        let w = world();
        let room = w.hub.create_player(A, "R&D <ops>");
        let mut b = w.login(B);
        // On the wire the client escapes the name once for the node and the
        // attribute escapes it again.
        let found = items(&mut b, "byname/R&amp;amp;D &amp;lt;ops&amp;gt;");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, room_jid(&room));
    }

    #[test]
    fn the_owner_is_named_in_the_admin_list_of_a_player_room() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut b = w.login(B);
        let id = b
            .iq(
                "get",
                Some(&room_jid(&room)),
                &format!("<query xmlns='{NS_MUC_ADMIN}'><item affiliation='owner'/></query>"),
            )
            .unwrap();
        let r = b
            .wait_for(|e| e.name == "iq" && e.attr("id") == Some(id.as_str()))
            .unwrap();
        let item = r.child("query").and_then(|q| q.child("item")).unwrap();
        assert_eq!(item.attr("jid"), Some(full_jid(A).as_str()));
    }

    #[test]
    fn an_owner_at_the_room_cap_is_refused_with_a_500_join() {
        let w = world();
        for _ in 0..PLAYER_ROOMS_PER_OWNER {
            assert_ne!(w.hub.create_player(A, "r"), FULL_ROOM);
        }
        let refused = w.hub.create_player(A, "one more");
        assert_eq!(refused, FULL_ROOM);
        // Another owner is not held back, and nothing was stored for the refusal.
        assert_ne!(w.hub.create_player(B, "r"), FULL_ROOM);
        assert_eq!(w.hub.lock().players.len(), PLAYER_ROOMS_PER_OWNER + 1);
        let mut a = w.login(A);
        assert_eq!(
            join_room(&mut a, &refused)
                .child("error")
                .and_then(|e| e.attr("code")),
            Some("500")
        );
    }

    /// One iq to `to`, returning the answer.
    fn ask(cl: &mut Client, to: &str, kind: &str, body: &str) -> Element {
        let id = cl.iq(kind, Some(to), body).unwrap();
        cl.wait_for(|e| e.name == "iq" && e.attr("id") == Some(id.as_str()))
            .unwrap()
    }

    fn code(e: &Element) -> Option<&str> {
        e.child("error").and_then(|x| x.attr("code"))
    }

    /// The pilots a `muc#admin` list names, or the error code.
    fn admin_list(
        cl: &mut Client,
        room: &str,
        attr: &str,
        value: &str,
    ) -> Result<Vec<i64>, String> {
        let r = ask(
            cl,
            &room_jid(room),
            "get",
            &format!("<query xmlns='{NS_MUC_ADMIN}'><item {attr}='{value}'/></query>"),
        );
        if r.attr("type") == Some("error") {
            return Err(code(&r).unwrap_or("?").to_owned());
        }
        Ok(r.child("query")
            .unwrap()
            .children
            .iter()
            .map(|i| charid_of(i.attr("jid").unwrap()).unwrap())
            .collect())
    }

    fn set_aff(cl: &mut Client, room: &str, who: i64, aff: &str) -> Element {
        ask(
            cl,
            &room_jid(room),
            "set",
            &format!(
                "<query xmlns='{NS_MUC_ADMIN}'><item affiliation='{aff}' jid='{who}@{HOSTNAME}'><reason>x</reason></item></query>"
            ),
        )
    }

    fn set_role(cl: &mut Client, room: &str, who: i64, role: &str) -> Element {
        ask(
            cl,
            &room_jid(room),
            "set",
            &format!("<query xmlns='{NS_MUC_ADMIN}'><item nick='{who}' role='{role}'/></query>"),
        )
    }

    /// The client's `RegisterTemporaryRestriction` (`set`) or the unregister.
    fn restrict(cl: &mut Client, room: &str, who: i64, kind: &str, secs: Option<u64>) -> Element {
        let body = match secs {
            Some(s) => format!(
                "<query xmlns='{NS_EXPIRING}' jid='{who}@{HOSTNAME}' room='{}' duration='{s}' action='set' category='{kind}'/>",
                room_jid(room)
            ),
            None => format!(
                "<query xmlns='{NS_EXPIRING}' jid='{who}@{HOSTNAME}' room='{}' action='delete' category='{kind}'/>",
                room_jid(room)
            ),
        };
        ask(cl, HOSTNAME, "set", &body)
    }

    fn say(cl: &mut Client, room: &str, text: &str) -> String {
        let id = cl.next_id();
        cl.send_raw(&format!(
            "<message id='{id}' to='{}' type='groupchat'><body>{text}</body></message>",
            room_jid(room)
        ))
        .unwrap();
        id
    }

    fn join_with(cl: &mut Client, room: &str, pw: &str) -> Element {
        let id = cl.next_id();
        cl.send_raw(&format!(
            "<presence id='{id}' to='{}/{}'><x xmlns='{NS_MUC}'><password>{pw}</password></x></presence>",
            room_jid(room),
            cl.charid()
        ))
        .unwrap();
        cl.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap()
    }

    #[test]
    fn an_outcast_cannot_join_and_is_put_out() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        a.drain(QUIET).unwrap();
        // Only a moderator bans.
        assert_eq!(code(&set_aff(&mut b, &room, C, "outcast")), Some("403"));
        assert_eq!(
            set_aff(&mut a, &room, B, "outcast").attr("type"),
            Some("result")
        );
        // B hears its own departure, A hears B go.
        let out = b
            .wait_for(|e| e.name == "presence" && e.attr("type") == Some("unavailable"))
            .unwrap();
        assert_eq!(out.attr("from"), Some(occupant_jid(&room, B).as_str()));
        assert!(!w.hub.occupants(&room).contains(&B));
        let told = a
            .wait_for(|e| e.attr("type") == Some("unavailable"))
            .unwrap();
        assert_eq!(told.attr("from"), Some(occupant_jid(&room, B).as_str()));
        // 407 is the client's "you have been banned" for a player channel.
        assert_eq!(code(&join_room(&mut b, &room)), Some("407"));
        assert_eq!(
            admin_list(&mut a, &room, "affiliation", "outcast"),
            Ok(vec![B])
        );
        // Lifting the ban lets B back in.
        assert_eq!(
            set_aff(&mut a, &room, B, "none").attr("type"),
            Some("result")
        );
        assert!(join_room(&mut b, &room).child("error").is_none());
        assert_eq!(
            admin_list(&mut a, &room, "affiliation", "outcast"),
            Ok(vec![])
        );
    }

    #[test]
    fn a_timed_ban_expires() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        // What `BanUserFromChannel` sends for a timed ban: outcast, the
        // record, then affiliation none.
        assert_eq!(
            set_aff(&mut a, &room, B, "outcast").attr("type"),
            Some("result")
        );
        assert_eq!(
            restrict(&mut a, &room, B, "ban", Some(600)).attr("type"),
            Some("result")
        );
        assert_eq!(
            set_aff(&mut a, &room, B, "none").attr("type"),
            Some("result")
        );
        assert_eq!(code(&join_room(&mut b, &room)), Some("407"));
        let found = ask(
            &mut a,
            HOSTNAME,
            "get",
            &format!(
                "<query xmlns='{NS_EXPIRING_SEARCH}' room='{}' category='ban'/>",
                room_jid(&room)
            ),
        );
        assert_eq!(
            found
                .child("query")
                .and_then(|q| q.child("item"))
                .and_then(|i| i.attr("jid")),
            Some(full_jid(B).as_str())
        );
        // The clock reaches the end: the record is gone and B may join.
        for l in &mut w.hub.lock().players.get_mut(&room).unwrap().limits {
            l.until = now_secs() - 1;
        }
        assert!(join_room(&mut b, &room).child("error").is_none());
        let found = ask(
            &mut a,
            HOSTNAME,
            "get",
            &format!(
                "<query xmlns='{NS_EXPIRING_SEARCH}' room='{}' category='ban'/>",
                room_jid(&room)
            ),
        );
        assert!(found.child("query").unwrap().children.is_empty());
        // An unregister lifts a running ban at once.
        let mut c = w.login(C);
        assert_eq!(
            restrict(&mut a, &room, C, "ban", Some(600)).attr("type"),
            Some("result")
        );
        assert_eq!(code(&join_room(&mut c, &room)), Some("407"));
        assert_eq!(
            restrict(&mut a, &room, C, "ban", None).attr("type"),
            Some("result")
        );
        assert!(join_room(&mut c, &room).child("error").is_none());
    }

    #[test]
    fn a_visitor_cannot_speak() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        a.drain(QUIET).unwrap();
        // `MuteUser`: the role, then the record.
        assert_eq!(
            set_role(&mut a, &room, B, "visitor").attr("type"),
            Some("result")
        );
        assert_eq!(
            restrict(&mut a, &room, B, "mute", Some(600)).attr("type"),
            Some("result")
        );
        assert_eq!(admin_list(&mut b, &room, "role", "visitor"), Ok(vec![B]));
        let id = say(&mut b, &room, "hello");
        let refused = b.wait_for(|e| e.attr("id") == Some(id.as_str())).unwrap();
        assert_eq!(refused.attr("type"), Some("error"));
        assert!(
            a.drain(QUIET)
                .unwrap()
                .iter()
                .all(|e| e.child("body").is_none())
        );
        // Other people speak, and a moderator cannot be muted.
        say(&mut a, &room, "still here");
        assert!(b.wait_for(|e| e.child("body").is_some()).is_ok());
        assert_eq!(code(&set_role(&mut b, &room, A, "visitor")), Some("403"));
        assert_eq!(code(&set_role(&mut a, &room, A, "visitor")), Some("403"));
        // Unmute.
        set_role(&mut a, &room, B, "participant");
        restrict(&mut a, &room, B, "mute", None);
        say(&mut b, &room, "back");
        assert!(a.wait_for(|e| e.child("body").is_some()).is_ok());
    }

    #[test]
    fn an_operator_shows_as_moderator() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let mut a = w.login(A);
        let mut b = w.login(B);
        let mut c = w.login(C);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        let before = join_room(&mut c, &room);
        let role = |e: &Element| {
            e.child("x")
                .and_then(|x| x.child("item"))
                .and_then(|i| i.attr("role"))
                .map(str::to_owned)
        };
        assert_eq!(
            set_aff(&mut a, &room, B, "admin").attr("type"),
            Some("result")
        );
        // The admin list names B and the change reaches the room.
        assert_eq!(
            admin_list(&mut c, &room, "affiliation", "admin"),
            Ok(vec![B])
        );
        let told = c
            .wait_for(|e| {
                e.name == "presence" && e.attr("from") == Some(occupant_jid(&room, B).as_str())
            })
            .unwrap();
        // The admin has the owner's role and C, who has none, has another.
        let owner_role = role(&join_room(&mut a, &room));
        assert_eq!(role(&told), owner_role);
        assert_ne!(role(&before), owner_role);
        // An admin sets the subject and mutes C, but cannot touch the owner,
        // name another admin or ban an admin.
        assert!(w.hub.set_motd(B, &room, "from the admin"));
        assert_eq!(
            set_role(&mut b, &room, C, "visitor").attr("type"),
            Some("result")
        );
        assert_eq!(code(&set_aff(&mut b, &room, A, "outcast")), Some("403"));
        assert_eq!(code(&set_aff(&mut b, &room, C, "admin")), Some("403"));
        assert_eq!(code(&set_aff(&mut b, &room, C, "owner")), Some("403"));
        // The creator cannot be demoted by an owner they named.
        assert_eq!(
            set_aff(&mut a, &room, C, "owner").attr("type"),
            Some("result")
        );
        assert_eq!(code(&set_aff(&mut c, &room, A, "none")), Some("403"));
        // Only owners delete.
        assert!(!w.hub.lock().destroy_player(B, &room));
        assert!(w.hub.lock().destroy_player(C, &room));
    }

    #[test]
    fn config_round_trips() {
        let w = world();
        let room = w.hub.create_player(A, "Haulers");
        let other = w.hub.create_player(C, "Taken");
        let mut a = w.login(A);
        let mut b = w.login(B);
        let form = |cl: &mut Client| {
            let r = ask(
                cl,
                &room_jid(&room),
                "get",
                &format!("<query xmlns='{NS_MUC_OWNER}'/>"),
            );
            let fields: Vec<(String, String, String)> = r
                .child("query")
                .and_then(|q| q.child("x"))
                .map(|x| {
                    x.children
                        .iter()
                        .map(|f| {
                            (
                                f.attr("var").unwrap().to_owned(),
                                f.attr("type").unwrap_or("").to_owned(),
                                f.child("value").map(|v| v.text.clone()).unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            fields
        };
        let get = |f: &[(String, String, String)], var: &str| {
            f.iter()
                .find(|x| x.0 == var)
                .map(|x| (x.1.clone(), x.2.clone()))
        };
        // A stranger gets no form (the window is then read-only).
        assert!(form(&mut b).is_empty());
        let f = form(&mut a);
        assert_eq!(get(&f, "muc#roomconfig_roomname").unwrap().1, "Haulers");
        // The client reads booleans by their type, and `0` as false.
        assert_eq!(
            get(&f, "muc#roomconfig_passwordprotectedroom"),
            Some(("boolean".into(), "0".into()))
        );
        let submit = |cl: &mut Client, fields: &str| {
            ask(
                cl,
                &room_jid(&room),
                "set",
                &format!(
                    "<query xmlns='{NS_MUC_OWNER}'><x xmlns='jabber:x:data' type='submit'>{fields}</x></query>"
                ),
            )
        };
        let field = |var: &str, v: &str| format!("<field var='{var}'><value>{v}</value></field>");
        let both = format!(
            "{}{}{}",
            field("muc#roomconfig_roomname", "Fast &amp; Loud"),
            field("muc#roomconfig_passwordprotectedroom", "1"),
            field("muc#roomconfig_roomsecret", "s3cret")
        );
        assert_eq!(code(&submit(&mut b, &both)), Some("403"));
        assert_eq!(submit(&mut a, &both).attr("type"), Some("result"));
        {
            let hub = w.hub.lock();
            let p = &hub.players[&room];
            assert_eq!(
                (p.name.as_str(), p.password.as_str()),
                ("Fast & Loud", "s3cret")
            );
        }
        let f = form(&mut a);
        assert_eq!(get(&f, "muc#roomconfig_roomname").unwrap().1, "Fast & Loud");
        assert_eq!(
            get(&f, "muc#roomconfig_passwordprotectedroom").unwrap().1,
            "1"
        );
        // The secret is never sent back.
        assert!(get(&f, "muc#roomconfig_roomsecret").is_none());
        assert!(f.iter().all(|x| x.2 != "s3cret"));
        // The new password is asked of strangers, and the old name is gone.
        assert_eq!(code(&join_with(&mut b, &room, "wrong")), Some("401"));
        assert!(join_with(&mut b, &room, "s3cret").child("error").is_none());
        assert!(items(&mut b, "byname/Haulers").is_empty());
        assert_eq!(items(&mut b, "byname/Fast &amp;amp; Loud").len(), 1);
        // A name another channel has is refused, and nothing changes.
        let name = w.hub.lock().players[&other].name.clone();
        let r = submit(&mut a, &field("muc#roomconfig_roomname", &name));
        assert_eq!(code(&r), Some("409"));
        assert_eq!(w.hub.lock().players[&room].name, "Fast & Loud");
        // Clearing the password and setting the default access.
        let clear = format!(
            "{}{}{}",
            field("muc#roomconfig_passwordprotectedroom", "0"),
            field("muc#roomconfig_membersonly", "1"),
            field("members_by_default", "1")
        );
        assert_eq!(submit(&mut a, &clear).attr("type"), Some("result"));
        let f = form(&mut a);
        assert_eq!(
            get(&f, "muc#roomconfig_passwordprotectedroom").unwrap().1,
            "0"
        );
        assert_eq!(get(&f, "muc#roomconfig_membersonly").unwrap().1, "1");
        // And it is stored: a restart reads it back.
        let rows = w.hub.take_dirty();
        let (_, row) = rows.iter().find(|(k, _)| k[0] == room).unwrap();
        let back = PlayerRoom::parse(row.as_ref().unwrap()).unwrap();
        assert_eq!(back, w.hub.lock().players[&room]);
    }

    #[test]
    fn a_blocked_channel_opens_to_the_allowed_list_and_an_invite() {
        let w = world();
        let room = w.hub.create_player(A, "Closed");
        w.hub.lock().players.get_mut(&room).unwrap().access = Access::Blocked;
        let mut a = w.login(A);
        let mut b = w.login(B);
        let mut c = w.login(C);
        join_room(&mut a, &room);
        assert!(join_room(&mut b, &room).child("error").is_some());
        // The allowed list (`SetAffiliation` member) lets B in.
        assert_eq!(
            set_aff(&mut a, &room, B, "member").attr("type"),
            Some("result")
        );
        assert!(join_room(&mut b, &room).child("error").is_none());
        // B, only a member, cannot read the allowed list or invite. `[A]`
        // lane policy: the allowed list is for moderators; falsifier: the
        // client's Allowed tab opening for a member.
        assert_eq!(
            admin_list(&mut b, &room, "affiliation", "member"),
            Err("403".into())
        );
        assert_eq!(
            admin_list(&mut a, &room, "affiliation", "member"),
            Ok(vec![B])
        );
        b.send_raw(&format!(
            "<message to='{}'><x xmlns='{NS_MUC_USER}'><invite to='{C}@{HOSTNAME}'/></x></message>",
            room_jid(&room)
        ))
        .unwrap();
        assert_eq!(
            b.wait_for(|e| e.attr("type") == Some("error"))
                .unwrap()
                .child("error")
                .and_then(|e| e.attr("code")),
            Some("405")
        );
        // The owner's invite reaches C and lets C in.
        a.send_raw(&format!(
            "<message to='{}'><x xmlns='{NS_MUC_USER}'><invite to='{C}@{HOSTNAME}'><reason>hi</reason></invite></x></message>",
            room_jid(&room)
        ))
        .unwrap();
        let inv = c
            .wait_for(|e| e.child("x").is_some_and(|x| x.child("invite").is_some()))
            .unwrap();
        assert_eq!(inv.attr("from"), Some(room_jid(&room).as_str()));
        assert!(join_room(&mut c, &room).child("error").is_none());
    }

    #[test]
    fn a_plain_pilot_gets_no_bypass_and_a_grant_makes_an_owner() {
        let w = world();
        let room = w.hub.create_player(A, "Secret");
        assert!(w.hub.set_player_password(&room, "hunter"));
        let mut b = w.login(B);
        // The client's retry after `GrantChannelOwnership`: it sends a
        // literal that is not the password.
        assert_eq!(
            code(&join_with(&mut b, &room, "bypass_with_roles")),
            Some("401")
        );
        assert!(w.hub.grant_ownership(&room, B));
        // B is now an owner: listed, able to configure and moderate. Whether
        // an owner passes the password is `[A]` and not pinned here.
        assert_eq!(
            admin_list(&mut b, &room, "affiliation", "owner").map(|v| v.len()),
            Ok(2)
        );
        assert!(w.hub.lock().config_form(B, &room).is_some());
        assert!(!w.hub.grant_ownership("player_nowhere", B));
        // A grant clears a ban the pilot was under.
        let mut c = w.login(C);
        assert!(
            w.hub
                .lock()
                .set_limit(A, &room, C, "ban", Some(600))
                .is_ok()
        );
        assert_eq!(code(&join_with(&mut c, &room, "hunter")), Some("407"));
        assert!(w.hub.grant_ownership(&room, C));
        assert!(!w.hub.lock().players[&room].banned(C, now_secs()));
    }

    #[test]
    fn a_password_with_unsafe_characters_still_matches() {
        let w = world();
        let room = w.hub.create_player(A, "Odd");
        assert!(w.hub.set_player_password(&room, "pa\u{1}ss"));
        assert_eq!(w.hub.lock().players[&room].password, "pass");
        let mut b = w.login(B);
        assert!(join_with(&mut b, &room, "pass").child("error").is_none());
    }

    #[test]
    fn moderation_rows_survive_a_restart_and_old_rows_still_parse() {
        let mut p = PlayerRoom::new("Haulers".into(), A);
        p.acl.insert(B, Aff::Admin);
        p.acl.insert(C, Aff::Outcast);
        p.access = Access::Moderated;
        p.set_limit(LimitKind::Mute, B + 10, 600, now_secs());
        assert_eq!(PlayerRoom::parse(&p.row()), Some(p.clone()));
        // A row from before the moderation columns has none of them.
        let old: Vec<String> = ["Haulers", "90000001", "pw", "motd"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let q = PlayerRoom::parse(&old).unwrap();
        assert_eq!((q.password.as_str(), q.motd.as_str()), ("pw", "motd"));
        assert!(q.acl.is_empty() && q.limits.is_empty());
        // A limit that has ended is not written back.
        p.limits[0].until = now_secs() - 1;
        assert!(PlayerRoom::parse(&p.row()).unwrap().limits.is_empty());
        // Garbage in the new columns is skipped, not fatal.
        let mut junk = old;
        junk.extend(["x".into(), "1:nope,zz".into(), "ban:1".into()]);
        assert!(PlayerRoom::parse(&junk).is_some());
    }

    /// A corporation or alliance id (an `[A]` range) is refused, not listed.
    const CORP: i64 = 1_500_000;

    #[test]
    fn a_corporation_entry_is_refused_with_400() {
        let w = world();
        let room = w.hub.create_player(A, "Corp");
        let mut h = w.hub.lock();
        assert_eq!(h.set_affiliation(A, &room, CORP, "member"), Err(400));
        assert_eq!(h.set_limit(A, &room, CORP, "ban", Some(60)), Err(400));
        assert!(h.players[&room].acl.is_empty());
    }

    #[test]
    fn the_lists_stop_growing_and_ended_limits_make_room() {
        let w = world();
        let room = w.hub.create_player(A, "Full");
        let mut h = w.hub.lock();
        for i in 0..ACL_CAP as i64 {
            assert_eq!(h.set_affiliation(A, &room, 10_000 + i, "member"), Ok(()));
        }
        assert_eq!(h.set_affiliation(A, &room, B, "member"), Err(409));
        // Someone already listed can still change.
        assert_eq!(h.set_affiliation(A, &room, 10_000, "outcast"), Ok(()));
        let p = h.players.get_mut(&room).unwrap();
        let now = now_secs();
        for i in 0..LIMITS_CAP as i64 {
            assert!(p.set_limit(LimitKind::Mute, 20_000 + i, 600, now));
        }
        assert!(!p.set_limit(LimitKind::Mute, B, 600, now));
        // Renewing a listed one, and a full list of ended ones, both fit.
        assert!(p.set_limit(LimitKind::Mute, 20_000, 600, now));
        assert!(p.set_limit(LimitKind::Mute, B, 600, now + 1_000));
        assert_eq!(p.limits.len(), 1);
    }

    #[test]
    fn an_invite_does_not_give_a_visitor_a_voice_in_a_moderated_channel() {
        let w = world();
        let room = w.hub.create_player(A, "Quiet");
        assert!(w.hub.set_player_password(&room, "pw"));
        w.hub.lock().players.get_mut(&room).unwrap().access = Access::Moderated;
        let mut b = w.login(B);
        assert!(join_with(&mut b, &room, "pw").child("error").is_none());
        let mut h = w.hub.lock();
        // Not oneself, and not a friend either: B is a visitor.
        assert!(!h.invite_player(B, false, &room, B));
        assert!(h.invite_player(B, false, &room, C));
        let p = &h.players[&room];
        assert_eq!((p.aff(B), p.aff(C)), (None, None));
        assert_eq!(p.role(B, now_secs()), "visitor");
        // The owner's invite does list the pilot.
        assert!(h.invite_player(A, false, &room, C));
        assert_eq!(h.players[&room].aff(C), Some(Aff::Member));
    }

    #[test]
    fn promoting_a_banned_pilot_lifts_the_timed_ban() {
        let w = world();
        let room = w.hub.create_player(A, "Promote");
        let mut h = w.hub.lock();
        assert_eq!(h.set_limit(A, &room, B, "ban", Some(600)), Ok(()));
        assert!(h.players[&room].banned(B, now_secs()));
        assert_eq!(h.set_affiliation(A, &room, B, "admin"), Ok(()));
        assert!(!h.players[&room].banned(B, now_secs()));
    }

    #[test]
    fn a_mute_carries_the_moderators_reason_to_the_muted_pilot() {
        let w = world();
        let room = w.hub.create_player(A, "Reasons");
        let mut a = w.login(A);
        let mut b = w.login(B);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        let r = ask(
            &mut a,
            &room_jid(&room),
            "set",
            &format!(
                "<query xmlns='{NS_MUC_ADMIN}'><item nick='{B}' role='visitor'><reason>too loud</reason></item></query>"
            ),
        );
        assert_eq!(r.attr("type"), Some("result"));
        let told = b
            .wait_for(|e| {
                e.name == "presence"
                    && e.attr("from") == Some(occupant_jid(&room, B).as_str())
                    && e.child("x")
                        .and_then(|x| x.child("item"))
                        .and_then(|i| i.attr("role"))
                        == Some("visitor")
            })
            .unwrap();
        let why = told
            .child("x")
            .and_then(|x| x.child("item"))
            .and_then(|i| i.child("reason"))
            .map(|r| r.text.clone());
        assert_eq!(why.as_deref(), Some("too loud"));
    }

    #[test]
    fn shutdown_saves_a_change_made_after_the_last_tick() {
        let w = world();
        let room = w.hub.create_player(A, "Late");
        let mut reg = w.write();
        crate::chat_dir::drain_dirty(&mut reg);
        assert!(reg.deltas().of(KIND).contains_key(&vec![room]));
    }

    #[test]
    fn the_motd_reaches_a_later_joiner() {
        let w = world();
        let room = w.hub.create_player(A, "News");
        let mut a = w.login(A);
        join_room(&mut a, &room);
        a.send_raw(&format!(
            "<message to='{}' type='groupchat'><subject>Belt 3 is rich</subject></message>",
            room_jid(&room)
        ))
        .unwrap();
        let subject = |e: &Element| e.child("subject").map(|s| s.text.clone());
        // The owner hears it set (sent as the setter).
        let heard = a.wait_for(|e| subject(e).is_some()).unwrap();
        assert_eq!(heard.attr("from"), Some(occupant_jid(&room, A).as_str()));
        // A stranger cannot change it.
        let mut b = w.login(B);
        join_room(&mut b, &room);
        let m = b.wait_for(|e| subject(e).is_some()).unwrap();
        assert_eq!(subject(&m).as_deref(), Some("Belt 3 is rich"));
        b.send_raw(&format!(
            "<message id='x1' to='{}' type='groupchat'><subject>mine now</subject></message>",
            room_jid(&room)
        ))
        .unwrap();
        let err = b.wait_for(|e| e.attr("type") == Some("error")).unwrap();
        assert_eq!(err.child("error").and_then(|e| e.attr("code")), Some("403"));
        assert!(a.drain(QUIET).unwrap().iter().all(|e| subject(e).is_none()));
        // The macho path (`XmppChatMgr.SetMotd`) is the same rule.
        assert!(w.hub.set_motd(A, &room, "Belt 4"));
        assert!(!w.hub.set_motd(B, &room, "no"));
    }

    #[test]
    fn a_member_speaks_both_ways_and_leaving_keeps_the_room() {
        let w = world();
        let room = w.hub.create_player(A, "Chat");
        let mut a = w.login(A);
        let mut b = w.login(B);
        join_room(&mut a, &room);
        join_room(&mut b, &room);
        b.say(&room, "hello").unwrap();
        let m = a.wait_for(|e| e.child("body").is_some()).unwrap();
        assert_eq!(m.child("body").unwrap().text, "hello");
        a.say(&room, "hi").unwrap();
        let m = b
            .wait_for(|e| e.child("body").is_some_and(|b| b.text == "hi"))
            .unwrap();
        assert_eq!(m.attr("from"), Some(occupant_jid(&room, A).as_str()));
        // Both leave; the room is still there to join.
        a.leave(&room).unwrap();
        b.leave(&room).unwrap();
        a.drain(QUIET).unwrap();
        assert!(w.hub.occupants(&room).is_empty());
        assert!(join_room(&mut a, &room).child("error").is_none());
    }

    #[test]
    fn rooms_survive_a_restart() {
        let hub = ChatHub::new(ChatConfig::default());
        let room = hub.create_player(A, "Kept");
        assert!(hub.set_motd(A, &room, "welcome"));
        let gone = hub.create_player(A, "Dropped");
        // The world side stores what is dirty; the second room was destroyed
        // before the tick, so only its (absent) row is queued.
        assert!(hub.lock().destroy_player(A, &gone));
        let mut rows = crate::delta::Rows::new();
        for (k, v) in hub.take_dirty() {
            if let Some(v) = v {
                rows.insert(k, v);
            }
        }
        assert!(hub.take_dirty().is_empty(), "drained");
        // A new hub (a restart) is seeded from the rows.
        let again = ChatHub::new(ChatConfig::default());
        again.seed_players(&rows);
        let mut st = again.lock();
        let p = st.players.get(&room).expect("the room is back");
        assert_eq!(
            (p.name.as_str(), p.owner, p.motd.as_str()),
            ("Kept", A, "welcome")
        );
        assert!(!st.players.contains_key(&gone));
        // The counter only goes up: the next channel is not a name in use.
        let fresh = st.create_player(B, "New");
        assert_ne!(fresh, room);
        assert_ne!(fresh, gone);
    }

    #[test]
    fn the_counter_row_raises_a_new_hubs_counter() {
        let hub = ChatHub::new(ChatConfig::default());
        let mut rows = crate::delta::Rows::new();
        rows.insert(
            vec![COUNTER_KEY.to_owned()],
            vec![(u64::MAX / 2).to_string()],
        );
        hub.seed_players(&rows);
        let made = hub.create_player(A, "x");
        let n: u64 = made.strip_prefix("player_").unwrap().parse().unwrap();
        assert!(n > u64::MAX / 2);
    }

    #[test]
    fn the_tick_writes_dirty_rooms_into_the_world_deltas() {
        let w = world();
        let room = w.hub.create_player(A, "Stored");
        assert!(w.hub.set_motd(A, &room, "m"));
        let mut reg = w.write();
        assert!(reg.deltas().of(KIND).is_empty(), "chat threads write none");
        crate::chat_dir::tick(&mut reg, Instant::now(), crate::damage::BlueTime::default());
        let row = reg.deltas().of(KIND).get(&vec![room.clone()]).cloned();
        assert_eq!(row.unwrap()[0], "Stored");
        assert!(
            reg.deltas()
                .of(KIND)
                .contains_key(&vec![COUNTER_KEY.to_owned()])
        );
        // A later hub seeded from those deltas finds the room.
        let hub2 = ChatHub::new(ChatConfig::default());
        hub2.seed_players(reg.deltas().of(KIND));
        assert!(hub2.lock().players.contains_key(&room));
    }

    #[test]
    fn room_names_follow_the_clients_regex() {
        for ok in ["local_30000001", "corp_1000044", "private_17", "fleet_1-2"] {
            assert!(valid_room_name(ok), "{ok}");
        }
        for bad in ["local", "_x", "Local_1", "local_", "a b_c"] {
            assert!(!valid_room_name(bad), "{bad}");
        }
        assert_eq!(
            room_of("local_1@conference.eveoffline/90000001"),
            Some("local_1".to_owned())
        );
        assert_eq!(room_of("local_1@elsewhere"), None);
    }

    /// PA-5: a chat connection owns no writer thread. When the client goes,
    /// nothing keeps its queue alive: the hub, the reader and the pool shard
    /// have all let go of it.
    #[test]
    fn a_disconnect_leaves_no_thread_and_no_handle_behind() {
        let w = world();
        let a = w.joined(A, &local(SYS1));
        let weak = Arc::downgrade(&w.hub.lock().streams[&A].tx);
        let has_writer_thread = || {
            std::fs::read_dir("/proc/self/task")
                .map(|d| {
                    d.flatten().any(|t| {
                        std::fs::read_to_string(t.path().join("comm"))
                            .is_ok_and(|n| n.trim() == "chat-write")
                    })
                })
                .unwrap_or(false)
        };
        assert!(!has_writer_thread(), "the per-connection writer is gone");
        drop(a);
        let until = Instant::now() + Duration::from_secs(5);
        while weak.upgrade().is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(weak.upgrade().is_none(), "the connection's queue leaked");
        assert!(!w.hub.lock().streams.contains_key(&A));
        assert!(!has_writer_thread());
    }

    /// PA-5: through a four-shard pool, each client's stanzas arrive in the
    /// order they were queued, whatever the other clients are doing, and a
    /// `Close` is the last thing written.
    #[test]
    fn chat_order_is_kept_per_client_through_the_pool() {
        const CLIENTS: u64 = 9;
        const N: usize = 400;
        let pool = Arc::new(Writers::default());
        crate::push::spawn_writers(Arc::clone(&pool), 4).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut readers = Vec::new();
        let mut outs = Vec::new();
        for key in 0..CLIENTS {
            let client = TcpStream::connect(addr).unwrap();
            let (server, _) = listener.accept().unwrap();
            outs.push(ChatOut::new(Some(server), key, Some(Arc::clone(&pool))));
            readers.push(std::thread::spawn(move || {
                let mut client = client;
                client
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut all = Vec::new();
                client.read_to_end(&mut all).unwrap();
                String::from_utf8(all).unwrap()
            }));
        }
        let posters: Vec<_> = outs
            .iter()
            .map(|o| {
                let o = Arc::clone(o);
                std::thread::spawn(move || {
                    for n in 0..N {
                        o.send(Out::Data(format!("<m n='{n}'/>")));
                    }
                    o.send(Out::Close);
                })
            })
            .collect();
        for p in posters {
            p.join().unwrap();
        }
        for (key, r) in readers.into_iter().enumerate() {
            let got = r.join().unwrap();
            let want: String =
                (0..N).map(|n| format!("<m n='{n}'/>")).collect::<String>() + "</stream:stream>";
            assert_eq!(got, want, "client {key}");
        }
    }

    fn status_kb(key: &str) -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
            .unwrap()
    }

    /// PA-5 measurement, not a check: 200 chat connections' thread count and
    /// RSS. `cargo test -p eve-server --lib measure_200 -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn measure_200_chat_connections() {
        const N: i64 = 200;
        let chars: Vec<Character> = (0..N).map(|i| pilot(80_000_000 + i, i + 1)).collect();
        let mut reg = Registry::new(SandboxConfig {
            characters: chars,
            ..SandboxConfig::default()
        });
        for i in 0..N {
            let client_id = 1000 + i;
            reg.session_opened(SessionEntry {
                client_id,
                session_id: client_id,
                user_id: 1,
                character_id: None,
            });
            reg.session_selected_character(client_id, 80_000_000 + i);
        }
        let hub = ChatHub::new(ChatConfig::default());
        reg.set_chat(Arc::clone(&hub));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        spawn_listener(listener, Arc::clone(&hub)).unwrap();
        hub.reconcile();
        std::thread::sleep(Duration::from_millis(300));
        let (t0, r0) = (status_kb("Threads:"), status_kb("VmRSS:"));
        let mut clients = Vec::new();
        for i in 0..N {
            let c = 80_000_000 + i;
            let token = hub.mint_token(c);
            let mut cl = Client::login(addr, HOSTNAME, c, &token).expect("login");
            cl.set_timeout(Duration::from_secs(3));
            clients.push(cl);
        }
        std::thread::sleep(Duration::from_millis(500));
        let (t1, r1) = (status_kb("Threads:"), status_kb("VmRSS:"));
        println!(
            "MEASURE {N} chat connections: threads +{} ({:.2} per connection), rss +{} KiB",
            t1 - t0,
            (t1 - t0) as f64 / N as f64,
            r1 - r0
        );
    }
}
