//! A minimal XMPP client that speaks **exactly** what the EVE client speaks.
//!
//! Every string it writes is the client's own template from
//! `xmpp/xmpphandler.py` (build 3619879, `docs/lanes/chat.md` §2 [V]), byte
//! for byte, including the stray space in `from='...' >` of an iq. That is
//! the point of it: a test that passes against this client passes against
//! the real one's bytes, not against a friendlier dialect.
//!
//! Used by `eve-server`'s chat tests, and meant for `eve-bot`:
//!
//! ```no_run
//! use eve_xmpp::Client;
//! # fn main() -> std::io::Result<()> {
//! // The token comes from machoNet `chatAuthenticationService.GetAuthenticationToken`.
//! let mut c = Client::login("10.137.0.1:5222", "eveoffline", 90_000_001, "token")?;
//! c.join("local_30000001")?;
//! c.say("local_30000001", "o7")?;
//! while let Some(stanza) = c.recv_timeout(std::time::Duration::from_secs(1))? {
//!     println!("{stanza}");
//! }
//! # Ok(()) }
//! ```

use crate::sasl::plain_encode;
use crate::xml::{Element, Event, Framer, escape};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// The resource the EVE client binds (`xmppchatsvc.Login` [V]).
pub const RESOURCE: &str = "eveclient";

/// How long [`Client::recv`] waits before calling it a timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Client {
    stream: TcpStream,
    framer: Framer,
    /// Stanzas read while looking for something else; served first.
    backlog: VecDeque<Element>,
    host: String,
    charid: i64,
    next_id: u64,
    timeout: Duration,
}

fn io_err(kind: io::ErrorKind, msg: impl Into<String>) -> io::Error {
    io::Error::new(kind, msg.into())
}

impl Client {
    /// Connect and run the whole login (§2 steps 1-6), returning once the
    /// server has echoed the initial presence.
    ///
    /// A SASL `<failure>` is `PermissionDenied`.
    pub fn login(
        addr: impl ToSocketAddrs,
        host: &str,
        charid: i64,
        token: &str,
    ) -> io::Result<Client> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        let mut c = Client {
            stream,
            framer: Framer::new(),
            backlog: VecDeque::new(),
            host: host.to_owned(),
            charid,
            next_id: 1,
            timeout: DEFAULT_TIMEOUT,
        };
        c.start_stream()?;
        c.expect_header()?;
        let features = c.recv()?;
        if features.name != "stream:features" {
            return Err(io_err(
                io::ErrorKind::InvalidData,
                format!("expected features, got {features}"),
            ));
        }
        c.send_raw(&format!(
            "<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>{}</auth>",
            plain_encode(&charid.to_string(), token)
        ))?;
        let answer = c.recv()?;
        match answer.name.as_str() {
            "success" => {}
            "failure" => return Err(io_err(io::ErrorKind::PermissionDenied, answer.to_xml())),
            _ => return Err(io_err(io::ErrorKind::InvalidData, answer.to_xml())),
        }
        c.framer.reset();
        c.start_stream()?;
        c.expect_header()?;
        let _features = c.recv()?;
        let bind_id = c.iq_from(
            "set",
            None,
            &c.bare_jid(),
            &format!(
                "<bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'><resource>{RESOURCE}</resource></bind>"
            ),
        )?;
        c.wait_for(|e| e.name == "iq" && e.attr("id") == Some(bind_id.as_str()))?;
        let session_id = c.iq_from(
            "set",
            None,
            &c.full_jid(),
            "<session xmlns='urn:ietf:params:xml:ns:xmpp-session'/>",
        )?;
        c.wait_for(|e| e.name == "iq" && e.attr("id") == Some(session_id.as_str()))?;
        c.send_raw("<presence><show/></presence>")?;
        let me = c.full_jid();
        c.wait_for(|e| e.name == "presence" && e.attr("from") == Some(me.as_str()))?;
        Ok(c)
    }

    fn start_stream(&mut self) -> io::Result<()> {
        let header = format!(
            "<?xml version='1.0'?><stream:stream to='{}' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>",
            self.host
        );
        self.send_raw(&header)
    }

    fn expect_header(&mut self) -> io::Result<()> {
        match self.read_event(self.timeout)? {
            Some(Event::Open(_)) => Ok(()),
            Some(other) => Err(io_err(
                io::ErrorKind::InvalidData,
                format!("expected a stream header, got {other:?}"),
            )),
            None => Err(io_err(io::ErrorKind::TimedOut, "no stream header")),
        }
    }

    pub fn charid(&self) -> i64 {
        self.charid
    }

    /// `charid@host`.
    pub fn bare_jid(&self) -> String {
        format!("{}@{}", self.charid, self.host)
    }

    /// `charid@host/eveclient`.
    pub fn full_jid(&self) -> String {
        format!("{}/{RESOURCE}", self.bare_jid())
    }

    /// `room@conference.host`.
    pub fn room_jid(&self, room: &str) -> String {
        format!("{room}@conference.{}", self.host)
    }

    /// How long `recv` waits. Five seconds by default.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// The next `idN`, from the same counter the client uses for iqs, joins,
    /// leaves and groupchats.
    pub fn next_id(&mut self) -> String {
        let id = format!("id{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Write bytes as they are. For tests that need something the client
    /// would never send.
    pub fn send_raw(&mut self, s: &str) -> io::Result<()> {
        self.stream.write_all(s.as_bytes())
    }

    /// `join_room`: a MUC join presence. Returns its id; the server's answer
    /// is the self-presence carrying it.
    pub fn join(&mut self, room: &str) -> io::Result<String> {
        let id = self.next_id();
        let s = format!(
            "<presence from='{}' id='{id}' to='{}/{}'><x xmlns='http://jabber.org/protocol/muc'></x></presence>",
            self.full_jid(),
            self.room_jid(room),
            self.charid
        );
        self.send_raw(&s)?;
        Ok(id)
    }

    /// `leave_room`. The client does not wait for an answer.
    pub fn leave(&mut self, room: &str) -> io::Result<String> {
        let id = self.next_id();
        let s = format!(
            "<presence from='{}' id='{id}' to='{}/{}' type='unavailable'/>",
            self.full_jid(),
            self.room_jid(room),
            self.charid
        );
        self.send_raw(&s)?;
        Ok(id)
    }

    /// `groupchat`: a message to a room. Returns its id, which the server
    /// echoes back to the sender.
    pub fn say(&mut self, room: &str, text: &str) -> io::Result<String> {
        let id = self.next_id();
        let s = format!(
            "<message from='{}' to='{}' type='groupchat' xml:lang='en' id='{id}'><body>{}</body></message>",
            self.full_jid(),
            self.room_jid(room),
            escape(text)
        );
        self.send_raw(&s)?;
        Ok(id)
    }

    /// `issue_request`: an iq of `kind` (`get`/`set`) with `body` inside,
    /// optionally addressed. Returns its id.
    pub fn iq(&mut self, kind: &str, to: Option<&str>, body: &str) -> io::Result<String> {
        let from = self.full_jid();
        self.iq_from(kind, to, &from, body)
    }

    fn iq_from(
        &mut self,
        kind: &str,
        to: Option<&str>,
        from: &str,
        body: &str,
    ) -> io::Result<String> {
        let id = self.next_id();
        let to_clause = to.map(|t| format!("to='{t}'")).unwrap_or_default();
        let s = format!("<iq id='{id}' type='{kind}' from='{from}' {to_clause}>{body}</iq>");
        self.send_raw(&s)?;
        Ok(id)
    }

    /// The next stanza, or `TimedOut`.
    pub fn recv(&mut self) -> io::Result<Element> {
        self.recv_timeout(self.timeout)?
            .ok_or_else(|| io_err(io::ErrorKind::TimedOut, "no stanza"))
    }

    /// The next stanza, or `None` if nothing arrived within `wait`.
    /// A closed stream is `UnexpectedEof`.
    pub fn recv_timeout(&mut self, wait: Duration) -> io::Result<Option<Element>> {
        if let Some(e) = self.backlog.pop_front() {
            return Ok(Some(e));
        }
        match self.read_event(wait)? {
            None => Ok(None),
            Some(Event::Stanza(e)) => Ok(Some(e)),
            Some(Event::Close) => Err(io_err(io::ErrorKind::UnexpectedEof, "stream closed")),
            Some(Event::Open(_)) => {
                Err(io_err(io::ErrorKind::InvalidData, "a second stream header"))
            }
        }
    }

    /// Read until a stanza matches, keeping the others for later `recv`s.
    pub fn wait_for(&mut self, pred: impl Fn(&Element) -> bool) -> io::Result<Element> {
        let deadline = Instant::now() + self.timeout;
        let mut skipped = Vec::new();
        let found = loop {
            if let Some(i) = self.backlog.iter().position(&pred) {
                break self.backlog.remove(i);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break None;
            }
            match self.read_event(left)? {
                Some(Event::Stanza(e)) if pred(&e) => break Some(e),
                Some(Event::Stanza(e)) => skipped.push(e),
                Some(Event::Close) => {
                    return Err(io_err(io::ErrorKind::UnexpectedEof, "stream closed"));
                }
                Some(Event::Open(_)) | None => {}
            }
        };
        for e in skipped.into_iter().rev() {
            self.backlog.push_front(e);
        }
        found.ok_or_else(|| io_err(io::ErrorKind::TimedOut, "nothing matched"))
    }

    /// Everything that arrives until the line has been quiet for `quiet`.
    pub fn drain(&mut self, quiet: Duration) -> io::Result<Vec<Element>> {
        let mut out: Vec<Element> = self.backlog.drain(..).collect();
        while let Some(e) = self.recv_timeout(quiet)? {
            out.push(e);
        }
        Ok(out)
    }

    /// Send `</stream:stream>` and drop the connection.
    pub fn close(mut self) -> io::Result<()> {
        self.send_raw("</stream:stream>")
    }

    fn read_event(&mut self, wait: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + wait;
        loop {
            match self.framer.next_event() {
                Ok(Some(e)) => return Ok(Some(e)),
                Ok(None) => {}
                Err(e) => return Err(io_err(io::ErrorKind::InvalidData, e.to_string())),
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(left))?;
            let mut buf = [0u8; 4096];
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(io_err(io::ErrorKind::UnexpectedEof, "connection closed")),
                Ok(n) => self.framer.feed(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
    }
}
