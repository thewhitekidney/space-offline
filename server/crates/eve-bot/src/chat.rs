//! A bot's chat: the client's own XMPP login, when the server has chat on.
//!
//! **The client's sequence, call for call** (`xmppchatsvc.py`, quoted at
//! `eve-server/src/service/base.rs`'s `XmppChatMgr` arm) **[V]**:
//!
//! 1. `XmppChatMgr.Hostname()` -- the XMPP domain;
//! 2. the chat server's address, `XmppChatMgr.GetDeprecatedPrefsFallback()`
//!    -- **`None` means chat is off**, and the client then never dials, so
//!    neither does a bot: it says nothing and plays on;
//! 3. `chatAuthenticationService.GetAuthenticationToken()` -- the password;
//! 4. a TCP connection to `(address, 5222)` and the login, which
//!    `eve_xmpp::Client` sends byte for byte as the client does;
//! 5. `XmppChatMgr.ResyncSystemChannelAccess()` -- the rooms it is in
//!    (Local, corp, fleet) -- and a MUC join of each. Asked again when the
//!    fleet or the system changes, as the client does on a session change.
//!
//! **An IP literal, never a name** (`docs/guides/jail.md`): the address is parsed
//! as an [`IpAddr`] and refused otherwise, so this opens one more TCP socket
//! and resolves nothing -- the same rule as the machoNet connection.
//!
//! A login the server *refuses* is retried twice with a fresh token (the
//! server can lose a token to its own reconcile pass, or see the session a
//! beat late); any other failure -- closed, timed out -- is dropped, not
//! retried, and never takes the machoNet connection with it: a bot that
//! cannot talk still flies.

use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_xmpp::{Client, Element};
use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// `s.connect((ejabberd_address, 5222))` **[V]** (`xmppchatsvc.py`, the
/// `base.rs` quote): the port the client dials. Tests run the chat server
/// on another one.
pub const PORT: u16 = 5222;

/// How long a chat read waits: the login's own steps, and nothing longer.
/// A bot is unattended; a silent chat server must be an error, not a hang.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The pause before each login attempt: none, then a short and a longer one.
const RETRY_WAITS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_millis(300),
    Duration::from_millis(1200),
];

/// An open chat.
pub struct Chat {
    client: Client,
    /// The rooms `ResyncSystemChannelAccess` answered, joined.
    rooms: Vec<String>,
    /// `(fleet, solar system)` the rooms were asked for.
    synced_for: (Option<i64>, Option<i64>),
    /// Who is in Local, as the room's presences say (DOC-3).
    local: Roster,
    /// The character this chat is for: its own echoes are not heard.
    me: i64,
}

/// One pilot in Local, from the room's occupant presence (`eve_user_data`,
/// `chat.rs` of the server, section 4.1) **[V]**: the owner ids the client
/// itself shows beside a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMember {
    pub char_id: i64,
    pub corp: i64,
    pub alliance: Option<i64>,
    /// The `typeid` the presence states (the character's type, not a ship's).
    pub type_id: i64,
}

/// Local's occupants (DOC-3): the client keeps the same list from the same
/// stanzas. "unavailable" from others is ignored for Local; a departure is
/// the admin groupchat `{"cmd":"leave","charid":N}` **[V]** (`xmppchatsvc.py`
/// `CHANNELS_WITH_SPECIAL_PRESENCE_HANDLING`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roster {
    room: Option<String>,
    members: BTreeMap<i64, LocalMember>,
    /// When the current room was first seen: presences in the first
    /// [`INITIAL_FILL`] are the room's occupants, not arrivals **[A]**.
    since: Option<Instant>,
    /// Lines said in Local or a fleet room, oldest first, at most
    /// [`HEARD_CAP`] (S1).
    heard: VecDeque<Heard>,
    /// Pilots who entered Local after the initial fill, at most
    /// [`HEARD_CAP`].
    arrived: VecDeque<Arrived>,
    /// Numbers [`Heard::seq`] and [`Arrived::seq`].
    next_seq: u64,
}

/// Which room a line was said in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomKind {
    Local,
    Fleet,
}

/// One groupchat line a bot heard (S1): the room, the speaking character
/// (the room nick is the character id, `eve-server/src/chat.rs`'s
/// `groupchat`), the text and when it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub seq: u64,
    pub room: RoomKind,
    pub from_char: i64,
    pub text: String,
    pub at: Instant,
}

/// A pilot who came into Local (S1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrived {
    pub seq: u64,
    pub char_id: i64,
    pub at: Instant,
}

/// Lines and arrivals kept per drain: a flood cannot grow a bot.
pub const HEARD_CAP: usize = 64;

/// Presences this soon after a room is first seen are its occupants, not
/// arrivals **[A]**. Falsified by a human who is greeted on logging in as
/// if new (too short) or never greeted (too long).
pub const INITIAL_FILL: Duration = Duration::from_secs(4);

/// The longest line kept: a pasted wall is cut, not stored.
pub const LINE_CAP: usize = 240;

/// The most occupants one roster keeps: a hostile Local cannot grow a bot.
pub const ROSTER_CAP: usize = 4096;

impl Roster {
    /// Fold one stanza in (pure). Anything that is not about a `local_`
    /// room is ignored; a new room replaces the old roster (a system change).
    pub fn on_stanza(&mut self, el: &Element) {
        self.on_stanza_at(el, Instant::now());
    }

    /// [`Self::on_stanza`] at a given time (tests, and one clock per drain).
    pub fn on_stanza_at(&mut self, el: &Element, now: Instant) {
        let Some(from) = el.attr("from") else { return };
        let (room, nick) = match from.split_once('/') {
            Some((r, n)) => (r, Some(n)),
            None => (from, None),
        };
        let name = room.split('@').next().unwrap_or("");
        // A line in a fleet room is heard, and is nothing else to the roster.
        if name.starts_with("fleet_") {
            if el.name == "message" {
                self.hear(el, nick, RoomKind::Fleet, now);
            }
            return;
        }
        if !name.starts_with("local_") {
            return;
        }
        if self.room.as_deref() != Some(name) {
            self.room = Some(name.to_owned());
            self.members.clear();
            self.since = Some(now);
            self.heard.clear();
            self.arrived.clear();
        }
        match el.name.as_str() {
            "presence" => {
                if el.attr("type") == Some("unavailable") || el.attr("type") == Some("error") {
                    return;
                }
                let (Some(nick), Some(data)) = (nick, el.child("eve_user_data")) else {
                    return;
                };
                let (Ok(char_id), Some(corp)) = (nick.parse::<i64>(), int_attr(data, "corpid"))
                else {
                    return;
                };
                if self.members.len() >= ROSTER_CAP && !self.members.contains_key(&char_id) {
                    return;
                }
                let alliance = int_attr(data, "allianceid").filter(|a| *a > 0);
                let new = !self.members.contains_key(&char_id);
                let settled = self
                    .since
                    .is_some_and(|t| now.saturating_duration_since(t) >= INITIAL_FILL);
                if new && settled {
                    let seq = self.seq();
                    push_capped(
                        &mut self.arrived,
                        Arrived {
                            seq,
                            char_id,
                            at: now,
                        },
                    );
                }
                self.members.insert(
                    char_id,
                    LocalMember {
                        char_id,
                        corp,
                        alliance,
                        type_id: int_attr(data, "typeid").unwrap_or(0),
                    },
                );
            }
            "message" if nick.is_none() => {
                if let Some(c) = el.child("body").and_then(|b| leave_of(&b.text)) {
                    self.members.remove(&c);
                }
            }
            "message" => self.hear(el, nick, RoomKind::Local, now),
            _ => {}
        }
    }

    fn seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Keep one groupchat line from a character (S1). History a room
    /// replays on joining carries a `delay` child and is not "now".
    fn hear(&mut self, el: &Element, nick: Option<&str>, room: RoomKind, now: Instant) {
        if el.attr("type") != Some("groupchat") || el.child("delay").is_some() {
            return;
        }
        let Some(from_char) = nick.and_then(|n| n.parse::<i64>().ok()) else {
            return;
        };
        let Some(body) = el.child("body") else { return };
        let text: String = body.text.trim().chars().take(LINE_CAP).collect();
        if text.is_empty() {
            return;
        }
        let seq = self.seq();
        push_capped(
            &mut self.heard,
            Heard {
                seq,
                room,
                from_char,
                text,
                at: now,
            },
        );
    }

    /// Hand over the lines heard since the last call.
    pub fn take_heard(&mut self) -> Vec<Heard> {
        self.heard.drain(..).collect()
    }

    /// Hand over the arrivals since the last call.
    pub fn take_arrived(&mut self) -> Vec<Arrived> {
        self.arrived.drain(..).collect()
    }

    /// The occupants, by character id.
    pub fn members(&self) -> impl Iterator<Item = &LocalMember> {
        self.members.values()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Forget everyone (a system change; the next presences refill it).
    pub fn clear(&mut self) {
        self.room = None;
        self.members.clear();
        self.since = None;
        self.arrived.clear();
    }
}

fn push_capped<T>(q: &mut VecDeque<T>, v: T) {
    if q.len() >= HEARD_CAP {
        q.pop_front();
    }
    q.push_back(v);
}

fn int_attr(e: &Element, key: &str) -> Option<i64> {
    e.attr(key)?.trim().parse().ok()
}

/// The character in a `{"cmd":"leave","charid":N}` body (no JSON library:
/// the server writes exactly this shape).
fn leave_of(body: &str) -> Option<i64> {
    if !body.contains("\"leave\"") {
        return None;
    }
    let rest = body.split("\"charid\"").nth(1)?;
    let digits: String = rest
        .trim_start_matches([':', ' '])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The address the bot dials for chat: `--chat-host` when given (a bot
/// that shares the server's network namespace cannot reach the address the
/// server tells a jailed client), else the advertised one.
pub fn chat_ip(
    advertised: &str,
    host_override: Option<IpAddr>,
) -> std::result::Result<IpAddr, String> {
    if let Some(ip) = host_override {
        return Ok(ip);
    }
    advertised.parse::<IpAddr>().map_err(|_| {
        format!("the chat address {advertised:?} is not an IP literal; names are not resolved")
    })
}

impl Chat {
    /// Steps 1-5. `Ok(Err(why))` is "chat is not available", which is not a
    /// fault -- the server's `--chat` is off, or its chat port is closed;
    /// `Err` is the machoNet connection failing under a step-1-3 call.
    pub fn open(
        conn: &mut Conn,
        port: u16,
        host_override: Option<IpAddr>,
    ) -> Result<std::result::Result<Chat, String>> {
        let Some(char_id) = conn.session().char_id() else {
            return Ok(Err("no character selected".to_owned()));
        };
        let Some(host) = text_answer(conn, "XmppChatMgr", "Hostname")? else {
            return Ok(Err("Hostname is None: the server's chat is off".to_owned()));
        };
        let Some(address) = text_answer(conn, "XmppChatMgr", "GetDeprecatedPrefsFallback")? else {
            return Ok(Err(
                "GetDeprecatedPrefsFallback is None: the server's chat is off".to_owned(),
            ));
        };
        let ip = match chat_ip(&address, host_override) {
            Ok(ip) => ip,
            Err(why) => return Ok(Err(why)),
        };
        // **A refused login asks again with a fresh token** (lane
        // undock-invuln): a token is single-use and the server may revoke it
        // in a race with its own reconcile pass, or see the session a beat
        // late; a second token a moment later gets through. Only a refusal
        // (`PermissionDenied`) is retried -- a closed port is not a race.
        let mut client = None;
        let mut last = String::new();
        for (attempt, wait) in RETRY_WAITS.iter().enumerate() {
            if attempt > 0 {
                std::thread::sleep(*wait);
            }
            let Some(token) =
                text_answer(conn, "chatAuthenticationService", "GetAuthenticationToken")?
            else {
                return Ok(Err("no chat token".to_owned()));
            };
            match Client::login(SocketAddr::new(ip, port), &host, char_id, &token) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(e) => {
                    let refused = e.kind() == std::io::ErrorKind::PermissionDenied;
                    last = format!("login to {ip}:{port}: {e}");
                    if !refused {
                        break;
                    }
                }
            }
        }
        let Some(mut client) = client else {
            return Ok(Err(last));
        };
        client.set_timeout(TIMEOUT);
        let mut chat = Chat {
            client,
            rooms: Vec::new(),
            synced_for: (None, None),
            local: Roster::default(),
            me: char_id,
        };
        if let Err(why) = chat.resync(conn)? {
            return Ok(Err(why));
        }
        Ok(Ok(chat))
    }

    /// Who is in Local now (DOC-3): filled by [`Chat::drain`].
    pub fn local(&self) -> &Roster {
        &self.local
    }

    /// The lines heard since the last call, never the bot's own echo (S1).
    pub fn take_heard(&mut self) -> Vec<Heard> {
        let me = self.me;
        let mut v = self.local.take_heard();
        v.retain(|h| h.from_char != me);
        v
    }

    /// The pilots who came into Local since the last call (S1).
    pub fn take_arrived(&mut self) -> Vec<Arrived> {
        let me = self.me;
        let mut v = self.local.take_arrived();
        v.retain(|a| a.char_id != me);
        v
    }

    /// The rooms joined.
    pub fn rooms(&self) -> &[String] {
        &self.rooms
    }

    /// Step 5: ask which rooms, join the new ones.
    fn resync(&mut self, conn: &mut Conn) -> Result<std::result::Result<(), String>> {
        let s = conn.session();
        let moved = self.synced_for.1 != s.solar_system_id2().or(s.solar_system_id());
        self.synced_for = (s.fleet_id(), s.solar_system_id2().or(s.solar_system_id()));
        if moved {
            self.local.clear();
        }
        let answer = conn.call("XmppChatMgr", "ResyncSystemChannelAccess", vec![])?;
        let Some(PyValue::List(names)) = answer.value() else {
            return Ok(Err(format!(
                "ResyncSystemChannelAccess answered {:?}",
                answer.value()
            )));
        };
        let wanted: Vec<String> = names.iter().filter_map(crate::text).collect();
        for room in &wanted {
            if !self.rooms.contains(room)
                && let Err(e) = self.client.join(room)
            {
                return Ok(Err(format!("join {room}: {e}")));
            }
        }
        self.rooms = wanted;
        Ok(Ok(()))
    }

    /// **Say a line**: in the fleet's room when in a fleet, else in Local.
    /// Returns the room. `Ok(Err(why))` means the chat is gone and should be
    /// dropped.
    pub fn say(
        &mut self,
        conn: &mut Conn,
        line: &str,
    ) -> Result<std::result::Result<String, String>> {
        let s = conn.session();
        let now = (s.fleet_id(), s.solar_system_id2().or(s.solar_system_id()));
        if now != self.synced_for
            && let Err(why) = self.resync(conn)?
        {
            return Ok(Err(why));
        }
        let fleet = conn.session().fleet_id().is_some();
        let room = self
            .rooms
            .iter()
            .find(|r| fleet && r.starts_with("fleet_"))
            .or_else(|| self.rooms.iter().find(|r| r.starts_with("local_")))
            .cloned();
        let Some(room) = room else {
            return Ok(Err(format!("no room to say it in ({:?})", self.rooms)));
        };
        if let Err(e) = self.client.say(&room, line) {
            return Ok(Err(format!("say in {room}: {e}")));
        }
        Ok(Ok(room))
    }

    /// Read whatever the chat server sent, so its queue to us never backs
    /// up. Local's occupants and the lines said in Local and the fleet are
    /// kept ([`Roster`]), for the bot's chatter to read. `Err` is a dead chat.
    pub fn drain(&mut self) -> std::result::Result<usize, String> {
        let mut n = 0;
        while n < 256 {
            match self.client.recv_timeout(Duration::from_millis(1)) {
                Ok(Some(el)) => {
                    self.local.on_stanza(&el);
                    n += 1;
                }
                Ok(None) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(n)
    }
}

/// A service call whose answer is a string or `None`.
fn text_answer(conn: &mut Conn, service: &str, method: &str) -> Result<Option<String>> {
    let outcome = conn.call(service, method, vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("{service}.{method}: {key}")));
    }
    Ok(outcome
        .value()
        .as_ref()
        .and_then(crate::text)
        .filter(|t| !t.is_empty()))
}

/// Character names from a `config.GetMultiOwnersEx` answer, the
/// `(header, data)` tuple (`eve_rowset::cfg::owners_header_data`): the
/// `ownerID` and `ownerName` columns, by header name. A row without both is
/// skipped. Nothing the server says about a name is trusted past length:
/// a name is cut at [`NAME_CAP`] chars and one holding a control character
/// is dropped.
pub fn owner_names(v: &PyValue) -> Vec<(i64, String)> {
    let PyValue::Tuple(t) = v else {
        return Vec::new();
    };
    let (Some(PyValue::List(head)), Some(PyValue::List(rows))) = (t.first(), t.get(1)) else {
        return Vec::new();
    };
    let col = |name: &[u8]| {
        head.iter().position(|h| match h {
            PyValue::Bytes(b) => b == name,
            PyValue::Unicode(u) => u.as_bytes() == name,
            _ => false,
        })
    };
    let (Some(id_at), Some(name_at)) = (col(b"ownerID"), col(b"ownerName")) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let PyValue::List(cells) = r else {
                return None;
            };
            let PyValue::Int(id) = cells.get(id_at)? else {
                return None;
            };
            let name = match cells.get(name_at)? {
                PyValue::Unicode(u) => u.clone(),
                PyValue::Bytes(b) => String::from_utf8(b.clone()).ok()?,
                _ => return None,
            };
            let name: String = name.trim().chars().take(NAME_CAP).collect();
            (!name.is_empty() && !name.chars().any(char::is_control)).then_some((*id, name))
        })
        .collect()
}

/// The longest character name kept.
pub const NAME_CAP: usize = 48;

#[cfg(test)]
mod tests {
    /// `--chat-host` wins over the address the server advertises.
    #[test]
    fn chat_host_override_wins_over_the_advertised_address() {
        use super::chat_ip;
        let lo: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(chat_ip("192.0.2.9", Some(lo)), Ok(lo));
        assert_eq!(chat_ip("not-an-ip", Some(lo)), Ok(lo));
        assert_eq!(chat_ip("192.0.2.9", None), Ok("192.0.2.9".parse().unwrap()));
        assert!(chat_ip("not-an-ip", None).is_err());
    }

    use super::*;

    const ROOM: &str = "local_30000999@conference.h";

    fn presence(room: &str, char_id: i64, corp: i64, alliance: i64) -> Element {
        Element::new("presence")
            .with_attr("from", format!("{room}/{char_id}"))
            .with_child(
                Element::new("eve_user_data")
                    .with_attr("corpid", corp.to_string())
                    .with_attr("allianceid", alliance.to_string())
                    .with_attr("typeid", "1373"),
            )
    }

    fn leave(room: &str, char_id: i64) -> Element {
        Element::new("message")
            .with_attr("from", room)
            .with_attr("type", "groupchat")
            .with_child(
                Element::new("body")
                    .with_text(format!("{{\"cmd\":\"leave\",\"charid\":{char_id}}}")),
            )
    }

    #[test]
    fn local_presences_fill_the_roster_and_a_leave_empties_a_seat() {
        let mut r = Roster::default();
        r.on_stanza(&presence(ROOM, 970_000_001, 970_100_001, 0));
        r.on_stanza(&presence(ROOM, 970_000_002, 970_100_002, 970_200_001));
        assert_eq!(r.len(), 2);
        let m: Vec<_> = r.members().collect();
        assert_eq!(m[0].alliance, None, "allianceid 0 is none");
        assert_eq!(m[1].alliance, Some(970_200_001));
        assert_eq!(m[1].corp, 970_100_002);
        // "unavailable" from others is ignored for Local.
        let mut gone = presence(ROOM, 970_000_001, 970_100_001, 0);
        gone.attrs.push(("type".into(), "unavailable".into()));
        r.on_stanza(&gone);
        assert_eq!(r.len(), 2);
        r.on_stanza(&leave(ROOM, 970_000_001));
        assert_eq!(r.len(), 1);
    }

    fn line(room: &str, who: i64, text: &str) -> Element {
        Element::new("message")
            .with_attr("from", format!("{room}/{who}"))
            .with_attr("type", "groupchat")
            .with_child(Element::new("body").with_text(text))
    }

    #[test]
    fn lines_are_kept_in_order_and_history_is_not_heard() {
        let mut r = Roster::default();
        let t = Instant::now();
        r.on_stanza_at(&line(ROOM, 970_000_001, "hello"), t);
        r.on_stanza_at(&line("fleet_970300001@conference.h", 970_000_002, "o7"), t);
        let mut old = line(ROOM, 970_000_003, "ancient");
        old.children.push(Element::new("delay"));
        r.on_stanza_at(&old, t);
        r.on_stanza_at(&line(ROOM, 970_000_004, "   "), t);
        let h = r.take_heard();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].room, h[0].from_char), (RoomKind::Local, 970_000_001));
        assert_eq!(h[1].room, RoomKind::Fleet);
        assert!(h[0].seq < h[1].seq);
        assert!(r.take_heard().is_empty());
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut r = Roster::default();
        let t = Instant::now();
        for i in 0..(HEARD_CAP + 10) {
            r.on_stanza_at(&line(ROOM, 970_000_001, &format!("line {i}")), t);
        }
        let h = r.take_heard();
        assert_eq!(h.len(), HEARD_CAP);
        assert_eq!(h.last().unwrap().text, format!("line {}", HEARD_CAP + 9));
    }

    #[test]
    fn only_presences_after_the_initial_fill_are_arrivals() {
        let mut r = Roster::default();
        let t = Instant::now();
        r.on_stanza_at(&presence(ROOM, 970_000_001, 970_100_001, 0), t);
        assert!(r.take_arrived().is_empty(), "the room's occupants");
        let later = t + INITIAL_FILL + Duration::from_secs(1);
        r.on_stanza_at(&presence(ROOM, 970_000_001, 970_100_001, 0), later);
        assert!(r.take_arrived().is_empty(), "a repeat presence is not new");
        r.on_stanza_at(&presence(ROOM, 970_000_002, 970_100_002, 0), later);
        let a = r.take_arrived();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].char_id, 970_000_002);
    }

    #[test]
    fn another_room_or_system_replaces_the_roster() {
        let mut r = Roster::default();
        r.on_stanza(&presence(ROOM, 970_000_001, 970_100_001, 0));
        r.on_stanza(&presence(
            "corp_970100001@conference.h",
            970_000_009,
            970_100_001,
            0,
        ));
        assert_eq!(r.len(), 1, "a corp room is not Local");
        r.on_stanza(&presence(
            "local_30000998@conference.h",
            970_000_003,
            970_100_003,
            0,
        ));
        assert_eq!(r.len(), 1);
        assert_eq!(r.members().next().unwrap().char_id, 970_000_003);
    }

    #[test]
    fn owner_names_read_the_columns_by_name_and_skip_bad_rows() {
        let col = |n: &str| PyValue::Bytes(n.as_bytes().to_vec());
        let row =
            |id: i64, name: PyValue| PyValue::List(vec![PyValue::Int(id), name, PyValue::Int(5)]);
        let v = PyValue::Tuple(vec![
            PyValue::List(vec![col("ownerID"), col("ownerName"), col("typeID")]),
            PyValue::List(vec![
                row(7, PyValue::Unicode("Ada Test".to_owned())),
                row(8, PyValue::Bytes(b"Bo".to_vec())),
                row(9, PyValue::Unicode("bad\nname".to_owned())),
                row(10, PyValue::None),
            ]),
        ]);
        assert_eq!(
            owner_names(&v),
            vec![(7, "Ada Test".to_owned()), (8, "Bo".to_owned())]
        );
        assert!(owner_names(&PyValue::None).is_empty());
        let no_name = PyValue::Tuple(vec![
            PyValue::List(vec![col("ownerID")]),
            PyValue::List(vec![]),
        ]);
        assert!(owner_names(&no_name).is_empty());
    }
}
