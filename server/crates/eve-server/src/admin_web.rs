//! **Admin web** (lane `admin-web`): a small read-only HTTP listener for the
//! server's owner. A page with the cluster map and the pilots online, and a
//! JSON API behind a token.
//!
//! Off by default: `--admin ADDR` (e.g. `--admin 0.0.0.0:8090`) turns it on.
//! std sockets and threads only, no dependency.
//!
//! # Auth
//!
//! [`Shared::open`] reads `<dir>/admin.token` (the bot-control directory,
//! which is the world file's directory: `/data` on the server host) and keeps it
//! when it holds a token of at least 32 characters; otherwise it writes a
//! fresh one (32 bytes of `/dev/urandom`, hex, mode 0600). Every `/api/`
//! request must carry it in the `X-Admin-Token` header, or it gets 401. The
//! static page itself is served without the token: it holds no data and
//! asks the operator for the token.
//!
//! # World state, without hurting the tick
//!
//! An HTTP thread never takes the world lock. The clock (`push.rs`) calls
//! [`take`] under the lock every [`SNAPSHOT_EVERY`] ticks -- O(pilots online),
//! the same order as the `[live]` line's work -- and publishes the result
//! after the lock with [`Shared::publish`]; each report window it adds the
//! tick health with [`Shared::note_health`]. A request renders JSON from the
//! last snapshot under the admin's own small mutex. The map is built once at
//! bind, from the loaded bundle tables, and kept as a string.
//!
//! # Hardening
//!
//! At most [`MAX_CONNS`] connections at once (the rest are dropped), a
//! request head of at most [`MAX_HEAD`] bytes, 5 s read and write timeouts,
//! `GET` plus `POST /api/action/*` (see [`route_post`] and
//! `admin_actions`), no CORS headers (a page from another origin cannot read the
//! answers), `Cache-Control: no-store`.

use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The token file, in the control directory.
pub const TOKEN_FILE: &str = "admin.token";
/// The request header carrying the token (matched case-insensitively).
pub const TOKEN_HEADER: &str = "x-admin-token";
/// The clock takes a snapshot every this many ticks (2 s at 250 ms).
pub const SNAPSHOT_EVERY: u64 = 8;
/// Connections served at once; more are closed unanswered.
pub const MAX_CONNS: usize = 16;
/// The longest request head read.
pub const MAX_HEAD: usize = 8192;
const IO_WAIT: Duration = Duration::from_secs(5);

const INDEX_HTML: &str = include_str!("../web/index.html");
const ADMIN_JS: &str = include_str!("../web/admin.js");
const ADMIN_CSS: &str = include_str!("../web/admin.css");

/// One pilot online, as the clock saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub character_id: i64,
    pub name: String,
    /// The owning account (`Character::owner_user_id`).
    pub user_id: Option<i64>,
    pub bot: bool,
    /// `Registry::session_system`: in space, the roster's; docked, the
    /// station's.
    pub system_id: Option<i64>,
    /// `space`, `docked` or `jumping`.
    pub state: &'static str,
    /// Docked only.
    pub station_id: Option<i64>,
    pub ship_type_id: i64,
    pub ship: String,
    /// In space only: metres in the system frame, as last written by the
    /// roster (at most one tick old) [A].
    pub position: Option<[f64; 3]>,
}

/// What the clock hands over each snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub taken_unix: u64,
    pub pilots: Vec<Pilot>,
    pub npcs: usize,
}

/// The last report window's tick health.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Health {
    pub unix: u64,
    pub ticks: u64,
    pub p95_ms: f64,
    pub held_max_ms: f64,
    pub skipped: u64,
}

#[derive(Default)]
struct State {
    snap: Option<Snapshot>,
    health: Option<Health>,
}

/// The listener's shared state: the token, the map, the last snapshot.
pub struct Shared {
    started: Instant,
    token: String,
    map_json: String,
    system_names: BTreeMap<i64, String>,
    state: Mutex<State>,
    active: AtomicUsize,
    /// The control directory: where `bot-groups.tsv` is read and
    /// `admin-actions.log` appended.
    dir: Option<PathBuf>,
    /// Bot groups by character name, and when the file was last read.
    groups: Mutex<(Option<Instant>, BTreeMap<String, BotGroup>)>,
    actions: crate::admin_actions::Queue,
}

/// One bot's group, side and role (a `bot-groups.tsv` row).
#[derive(Debug, Clone, PartialEq)]
pub struct BotGroup {
    pub group: String,
    pub side: String,
    pub role: String,
}

/// `bot-groups.tsv` (eve-bots writes it every few seconds): `#group` header,
/// then `group<TAB>side<TAB>role<TAB>character name` per bot. Bad rows are
/// skipped.
pub fn parse_groups(text: &str) -> BTreeMap<String, BotGroup> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (group, side, role, name) = (f.next()?, f.next()?, f.next()?, f.next()?);
            (!name.is_empty() && f.next().is_none()).then(|| {
                (
                    name.to_owned(),
                    BotGroup {
                        group: group.to_owned(),
                        side: side.to_owned(),
                        role: role.to_owned(),
                    },
                )
            })
        })
        .collect()
}

/// How often the groups file is read again, at most.
const GROUPS_EVERY: Duration = Duration::from_secs(2);
/// The groups file, in the control directory (written by `eve-bots`).
pub const GROUPS_FILE: &str = "bot-groups.tsv";
/// The action log, in the control directory.
pub const ACTION_LOG: &str = "admin-actions.log";
/// The longest request body read.
pub const MAX_BODY: usize = 4096;

impl Shared {
    /// A state with this token and map (tests; [`Shared::open`] is the real
    /// path).
    pub fn new(token: String, map: &MapData) -> Shared {
        Shared {
            started: Instant::now(),
            token,
            map_json: map.to_json(),
            system_names: map.systems.iter().map(|s| (s.id, s.name.clone())).collect(),
            state: Mutex::new(State::default()),
            active: AtomicUsize::new(0),
            dir: None,
            groups: Mutex::new((None, BTreeMap::new())),
            actions: crate::admin_actions::Queue::default(),
        }
    }

    /// The action queue the clock drains.
    pub(crate) fn actions(&self) -> &crate::admin_actions::Queue {
        &self.actions
    }

    /// The bot groups, by character name (the file, re-read every 2 s).
    fn bot_groups(&self) -> BTreeMap<String, BotGroup> {
        let Ok(mut g) = self.groups.lock() else {
            return BTreeMap::new();
        };
        let stale = g.0.is_none_or(|t| t.elapsed() >= GROUPS_EVERY);
        if stale && let Some(dir) = &self.dir {
            g.0 = Some(Instant::now());
            g.1 = std::fs::read_to_string(dir.join(GROUPS_FILE))
                .map(|t| parse_groups(&t))
                .unwrap_or_default();
        }
        g.1.clone()
    }

    /// Set the groups directly (tests).
    #[cfg(test)]
    fn set_groups(&self, text: &str) {
        if let Ok(mut g) = self.groups.lock() {
            g.0 = Some(Instant::now());
            g.1 = parse_groups(text);
        }
    }

    /// Read or write the token in `dir`, bind `addr` and start serving.
    pub fn open(addr: SocketAddr, dir: &Path, map: &MapData) -> io::Result<Arc<Shared>> {
        let token = token_in(dir)?;
        let listener = TcpListener::bind(addr)?;
        let mut shared = Shared::new(token, map);
        shared.dir = Some(dir.to_path_buf());
        let shared = Arc::new(shared);
        serve(listener, Arc::clone(&shared))?;
        Ok(shared)
    }

    /// The clock's snapshot, taken under the world lock.
    pub fn publish(&self, snap: Snapshot) {
        if let Ok(mut s) = self.state.lock() {
            s.snap = Some(snap);
        }
    }

    /// The report window's tick numbers.
    pub fn note_health(&self, h: Health) {
        if let Ok(mut s) = self.state.lock() {
            s.health = Some(h);
        }
    }

    fn token_ok(&self, given: Option<&str>) -> bool {
        given.is_some_and(|t| same(t.trim(), &self.token))
    }
}

/// The token in `<dir>/admin.token`, kept when it is long enough, else a
/// fresh one written there (0600).
fn token_in(dir: &Path) -> io::Result<String> {
    let path = dir.join(TOKEN_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let t = text.trim();
        if t.len() >= 32 && t.bytes().all(|b| b.is_ascii_graphic()) {
            return Ok(t.to_owned());
        }
    }
    std::fs::create_dir_all(dir)?;
    let token: String = crate::accounts::urandom32()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// Equal without stopping at the first different byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ---------------------------------------------------------------- world slot

/// The world slot: the listener's state, when `--admin` is on.
#[derive(Default)]
pub struct AdminSlot {
    shared: Option<Arc<Shared>>,
}

/// The slot, for `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "admin_web",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(AdminSlot::default())
}

/// Hand the world the listener (`Server::bind`).
pub fn attach(reg: &mut Registry, shared: Arc<Shared>) {
    reg.ext_mut::<AdminSlot>().shared = Some(shared);
}

/// The listener's state, when `--admin` is on (the clock's action drain).
pub(crate) fn shared_of(w: &Registry) -> Option<Arc<Shared>> {
    w.ext::<AdminSlot>().shared.clone()
}

/// The clock's snapshot, under the lock; `None` without `--admin`.
pub fn take(w: &Registry) -> Option<(Arc<Shared>, Snapshot)> {
    let shared = Arc::clone(w.ext::<AdminSlot>().shared.as_ref()?);
    Some((shared, snapshot(w)))
}

/// Every pilot online, in connection order.
pub fn snapshot(w: &Registry) -> Snapshot {
    let roster = w.roster();
    let mut pilots = Vec::new();
    for id in w.sessions().online_characters() {
        let Some(c) = w.character(id) else { continue };
        let presence = roster.get(id);
        let state = if presence.is_some() {
            "space"
        } else if w.jumps().contains(id) {
            "jumping"
        } else {
            "docked"
        };
        pilots.push(Pilot {
            character_id: id,
            name: c.name.clone(),
            user_id: c.owner_user_id,
            bot: c.is_bot,
            system_id: w.session_system(id),
            state,
            station_id: if state == "docked" {
                c.station_id
            } else {
                None
            },
            ship_type_id: c.ship_type_id,
            ship: crate::item::find_in(w.content(), c.ship_type_id)
                .map(|t| t.name.to_owned())
                .unwrap_or_default(),
            position: presence.map(|p| p.motion.position),
        });
    }
    Snapshot {
        taken_unix: unix_now(),
        pilots,
        npcs: w.npcs().iter().count(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------- map

/// One system on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSystem {
    pub id: i64,
    pub name: String,
    pub region_id: i64,
    pub security: f64,
    /// Galaxy-frame centre, metres.
    pub center: [f64; 3],
}

/// The cluster for the canvas: systems and gate links.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapData {
    pub systems: Vec<MapSystem>,
    /// `(a, b)` with `a < b`, each link once.
    pub links: Vec<(i64, i64)>,
}

impl MapData {
    /// From rows: `(id, name, region, security)` for each system, the
    /// centres, and gate pairs `(from_system, to_system)`. A system without
    /// a centre is left out. With any gate, only systems a gate touches are
    /// kept (known space; wormhole and hidden systems have no gates);
    /// without gates (no celestials loaded) every system with a centre in
    /// known space is.
    pub fn build(
        systems: impl IntoIterator<Item = (i64, String, i64, f64)>,
        centers: &crate::celestial::Centers,
        gates: impl IntoIterator<Item = (i64, i64)>,
    ) -> MapData {
        let links: BTreeSet<(i64, i64)> = gates
            .into_iter()
            .filter(|(a, b)| a != b && *a != 0 && *b != 0)
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        let gated: BTreeSet<i64> = links.iter().flat_map(|(a, b)| [*a, *b]).collect();
        let mut out: Vec<MapSystem> = systems
            .into_iter()
            .filter(|(id, ..)| {
                if gated.is_empty() {
                    crate::celestial::KNOWN_SPACE.contains(id)
                } else {
                    gated.contains(id)
                }
            })
            .filter_map(|(id, name, region_id, security)| {
                Some(MapSystem {
                    id,
                    name,
                    region_id,
                    security,
                    center: centers.system(id)?,
                })
            })
            .collect();
        out.sort_by_key(|s| s.id);
        let kept: BTreeSet<i64> = out.iter().map(|s| s.id).collect();
        MapData {
            systems: out,
            links: links
                .into_iter()
                .filter(|(a, b)| kept.contains(a) && kept.contains(b))
                .collect(),
        }
    }

    /// From the world's loaded tables (once, at bind).
    pub fn from_registry(reg: &Registry) -> MapData {
        let content = reg.content();
        let empty_c = crate::celestial::Centers::default();
        let centers = content
            .try_get::<crate::celestial::Centers>()
            .unwrap_or(&empty_c);
        let systems: Vec<(i64, String, i64, f64)> = content
            .try_get::<crate::celestial::Universe>()
            .map(|u| {
                u.systems()
                    .map(|s| (s.id, s.name.clone(), s.region_id, s.security))
                    .collect()
            })
            .unwrap_or_default();
        let catalogue = reg.celestials();
        let gates: Vec<(i64, i64)> = catalogue
            .served()
            .filter(|c| c.kind == crate::celestial::Kind::Gate)
            .filter_map(|c| Some((catalogue.system_of(c.id)?, catalogue.jump_of(c.id)?.1)))
            .collect();
        MapData::build(systems, centers, gates)
    }

    /// `{"systems":[[id,"name",region,sec,x,z],...],"links":[[a,b],...]}`,
    /// x and z in light years to three places. The page draws x right and
    /// z up (`-z` down the screen) [A].
    pub fn to_json(&self) -> String {
        let ly = crate::celestial::LIGHTYEAR;
        let mut s = String::from("{\"systems\":[");
        for (i, sys) in self.systems.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "[{},{},{},{},{},{}]",
                sys.id,
                json_str(&sys.name),
                sys.region_id,
                num(round(sys.security, 2)),
                num(round(sys.center[0] / ly, 3)),
                num(round(sys.center[2] / ly, 3)),
            );
        }
        s.push_str("],\"links\":[");
        for (i, (a, b)) in self.links.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "[{a},{b}]");
        }
        s.push_str("]}");
        s
    }
}

fn round(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

// --------------------------------------------------------------------- JSON

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `<` too, so a name can never close a script tag.
            c if (c as u32) < 0x20 || c == '<' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A finite number, or `null`.
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".to_owned()
    }
}

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

/// `/api/status`.
pub fn status_json(shared: &Shared) -> String {
    let st = shared.state.lock().map(|s| (s.snap.clone(), s.health));
    let (snap, health) = st.unwrap_or((None, None));
    let now = unix_now();
    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"uptime_s\":{},\"now_unix\":{now}",
        shared.started.elapsed().as_secs()
    );
    match &snap {
        Some(snap) => {
            let humans = snap.pilots.iter().filter(|p| !p.bot).count();
            let in_space = snap.pilots.iter().filter(|p| p.state == "space").count();
            let docked = snap.pilots.iter().filter(|p| p.state == "docked").count();
            let systems: BTreeSet<i64> = snap.pilots.iter().filter_map(|p| p.system_id).collect();
            let _ = write!(
                s,
                ",\"snapshot_age_s\":{},\"pilots\":{},\"humans\":{humans},\"bots\":{},\
                 \"in_space\":{in_space},\"docked\":{docked},\"systems\":{},\"npcs\":{}",
                now.saturating_sub(snap.taken_unix),
                snap.pilots.len(),
                snap.pilots.len() - humans,
                systems.len(),
                snap.npcs
            );
        }
        None => s.push_str(",\"snapshot_age_s\":null"),
    }
    match health {
        Some(h) => {
            let _ = write!(
                s,
                ",\"tick\":{{\"window_unix\":{},\"ticks\":{},\"p95_ms\":{},\"held_max_ms\":{},\
                 \"skipped\":{}}}",
                h.unix,
                h.ticks,
                num(round(h.p95_ms, 2)),
                num(round(h.held_max_ms, 2)),
                h.skipped
            );
        }
        None => s.push_str(",\"tick\":null"),
    }
    s.push('}');
    s
}

/// `/api/pilots`: `{"taken_unix":N,"pilots":[{...}]}`.
pub fn pilots_json(shared: &Shared) -> String {
    let snap = shared
        .state
        .lock()
        .ok()
        .and_then(|s| s.snap.clone())
        .unwrap_or_default();
    let groups = shared.bot_groups();
    let mut s = format!("{{\"taken_unix\":{},\"pilots\":[", snap.taken_unix);
    for (i, p) in snap.pilots.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let system_name = p
            .system_id
            .and_then(|id| shared.system_names.get(&id))
            .map_or_else(|| "null".to_owned(), |n| json_str(n));
        let pos = p.position.map_or_else(
            || "null".to_owned(),
            |[x, y, z]| format!("[{},{},{}]", num(x.round()), num(y.round()), num(z.round())),
        );
        let bg = groups.get(&p.name).filter(|_| p.bot);
        let field =
            |f: fn(&BotGroup) -> &String| bg.map_or_else(|| "null".to_owned(), |g| json_str(f(g)));
        let (group, side, role) = (field(|g| &g.group), field(|g| &g.side), field(|g| &g.role));
        let _ = write!(
            s,
            "{{\"character_id\":{},\"name\":{},\"user_id\":{},\"bot\":{},\"system_id\":{},\
             \"system\":{system_name},\"state\":\"{}\",\"station_id\":{},\"ship_type_id\":{},\
             \"ship\":{},\"position\":{pos},\"group\":{group},\"side\":{side},\"role\":{role}}}",
            p.character_id,
            json_str(&p.name),
            opt(p.user_id),
            p.bot,
            opt(p.system_id),
            p.state,
            opt(p.station_id),
            p.ship_type_id,
            json_str(&p.ship),
        );
    }
    s.push_str("]}");
    s
}

/// `/api/systems`: the occupied systems with counts.
pub fn systems_json(shared: &Shared) -> String {
    let snap = shared
        .state
        .lock()
        .ok()
        .and_then(|s| s.snap.clone())
        .unwrap_or_default();
    // system -> (humans, bots, in space)
    let mut by: BTreeMap<i64, (u32, u32, u32)> = BTreeMap::new();
    for p in &snap.pilots {
        let Some(id) = p.system_id else { continue };
        let e = by.entry(id).or_default();
        if p.bot {
            e.1 += 1;
        } else {
            e.0 += 1;
        }
        if p.state == "space" {
            e.2 += 1;
        }
    }
    let mut s = String::from("{\"systems\":[");
    for (i, (id, (h, b, sp))) in by.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let name = shared
            .system_names
            .get(id)
            .map_or_else(|| "null".to_owned(), |n| json_str(n));
        let _ = write!(
            s,
            "{{\"system_id\":{id},\"name\":{name},\"humans\":{h},\"bots\":{b},\"in_space\":{sp}}}"
        );
    }
    s.push_str("]}");
    s
}

// --------------------------------------------------------------------- HTTP

/// An answer: status line, content type, body.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: String) -> Response {
        Response {
            status,
            content_type,
            body,
        }
    }

    fn error(status: u16, msg: &str) -> Response {
        Response::new(
            status,
            "application/json",
            format!("{{\"error\":{}}}", json_str(msg)),
        )
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            429 => "Too Many Requests",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Error",
        }
    }

    /// The whole HTTP/1.1 answer, connection closed after it.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             X-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
             Content-Security-Policy: default-src 'self'; style-src 'self'; script-src 'self'\r\n\
             Connection: close\r\n",
            self.status,
            self.reason(),
            self.content_type,
            self.body.len()
        );
        if self.status == 204 {
            // RFC 9110 §8.6: no Content-Length on a 204.
            head = head.replace("Content-Length: 0\r\n", "");
        }
        if self.status == 401 {
            head.push_str("WWW-Authenticate: X-Admin-Token\r\n");
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }
}

/// Answer one request. `token` is the `X-Admin-Token` header's value.
pub fn route(shared: &Shared, method: &str, target: &str, token: Option<&str>) -> Response {
    if method != "GET" {
        return Response::error(405, "GET only");
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    match path {
        "/" | "/index.html" => {
            return Response::new(200, "text/html; charset=utf-8", INDEX_HTML.to_owned());
        }
        "/admin.js" => {
            return Response::new(200, "text/javascript; charset=utf-8", ADMIN_JS.to_owned());
        }
        "/admin.css" => {
            return Response::new(200, "text/css; charset=utf-8", ADMIN_CSS.to_owned());
        }
        // No icon: answered empty, so a browser does not log a 404.
        "/favicon.ico" => return Response::new(204, "image/x-icon", String::new()),
        _ => {}
    }
    if !path.starts_with("/api/") {
        return Response::error(404, "not found");
    }
    if !shared.token_ok(token) {
        return Response::error(401, "token required");
    }
    let json = |body| Response::new(200, "application/json", body);
    match path {
        "/api/status" => json(status_json(shared)),
        "/api/pilots" => json(pilots_json(shared)),
        "/api/systems" => json(systems_json(shared)),
        "/api/map" => json(shared.map_json.clone()),
        _ => Response::error(404, "no such API"),
    }
}

/// A request head, as far as the listener reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub token: Option<String>,
    pub content_type: Option<String>,
    pub origin: Option<String>,
    pub host: Option<String>,
    pub content_length: Option<usize>,
}

/// The head, or `None` when it is not one.
pub fn parse_head_full(head: &str) -> Option<Head> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    if !first.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let mut h = Head {
        method,
        target,
        ..Head::default()
    };
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case(TOKEN_HEADER) {
            h.token = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("content-type") {
            h.content_type = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("origin") {
            h.origin = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("host") {
            h.host = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("content-length") {
            h.content_length = v.parse().ok();
        }
    }
    Some(h)
}

/// `(method, target, token)` from a request head, or `None` when it is
/// not one.
pub fn parse_head(head: &str) -> Option<(String, String, Option<String>)> {
    let h = parse_head_full(head)?;
    Some((h.method, h.target, h.token))
}

/// How long a `POST` waits for the clock to run its action.
const ACTION_WAIT: Duration = Duration::from_secs(10);

/// `POST /api/action/<name>`: the token, a JSON content type, no foreign
/// `Origin`, then the rate limit; the action runs on the clock and its text
/// comes back as `{"ok":true,"text":"..."}`. `peer` is for the log.
pub fn route_post(shared: &Shared, h: &Head, body: &str, peer: &str) -> Response {
    let path = h.target.split(['?', '#']).next().unwrap_or("");
    let Some(name) = path.strip_prefix("/api/action/") else {
        return Response::error(404, "not found");
    };
    if !shared.token_ok(h.token.as_deref()) {
        return Response::error(401, "token required");
    }
    let json = h
        .content_type
        .as_deref()
        .and_then(|c| c.split(';').next())
        .is_some_and(|c| c.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Response::error(415, "Content-Type must be application/json");
    }
    if let Some(origin) = &h.origin {
        let own = h.host.as_deref().is_some_and(|host| {
            origin == &format!("http://{host}") || origin == &format!("https://{host}")
        });
        if !own {
            return Response::error(403, "foreign Origin");
        }
    }
    let action = match crate::admin_actions::Action::parse(name, body) {
        Ok(a) => a,
        Err(why) if why == "no such action" => return Response::error(404, &why),
        Err(why) => return Response::error(400, &why),
    };
    if !shared.actions.allow(action.name(), Instant::now()) {
        return Response::error(429, "one per second per action");
    }
    let (name, args) = (action.name(), action.args());
    let (tx, rx) = std::sync::mpsc::channel();
    if !shared
        .actions
        .push(crate::admin_actions::Job { action, reply: tx })
    {
        return Response::error(503, "too many actions waiting");
    }
    let Ok(text) = rx.recv_timeout(ACTION_WAIT) else {
        return Response::error(504, "no answer in time");
    };
    shared.log_action(peer, name, &args, &text);
    Response::new(
        200,
        "application/json",
        format!("{{\"ok\":true,\"text\":{}}}", json_str(&text)),
    )
}

impl Shared {
    /// `[admin] <unix> <peer> <action> <args> -> <result>` on stdout and
    /// appended to `<dir>/admin-actions.log`, one line each.
    fn log_action(&self, peer: &str, action: &str, args: &str, result: &str) {
        let one = |t: &str| t.replace(['\n', '\r'], " / ");
        let line = format!(
            "[admin] {} {peer} {action} {} -> {}",
            unix_now(),
            one(args),
            one(result)
        );
        println!("{line}");
        if let Some(dir) = &self.dir
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(ACTION_LOG))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Accept connections on `listener`, one short-lived thread each.
pub fn serve(listener: TcpListener, shared: Arc<Shared>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("admin-web".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if shared.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
                    shared.active.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let s = Arc::clone(&shared);
                let spawned = std::thread::Builder::new()
                    .name("admin-conn".to_owned())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        answer(stream, &s);
                        s.active.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    shared.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
    Ok(())
}

fn answer(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(IO_WAIT));
    let _ = stream.set_write_timeout(Some(IO_WAIT));
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(i);
        }
        if buf.len() >= MAX_HEAD {
            break None;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = head_end
        .and_then(|i| std::str::from_utf8(&buf[..i]).ok())
        .and_then(parse_head_full);
    let resp = match (head, head_end) {
        (Some(h), Some(end)) if h.method == "POST" => {
            let want = h.content_length.unwrap_or(0);
            if want > MAX_BODY {
                Response::error(413, "body too large")
            } else {
                let mut body = buf[(end + 4).min(buf.len())..].to_vec();
                while body.len() < want {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                    }
                }
                body.truncate(want);
                match String::from_utf8(body) {
                    Ok(body) if body.len() == want => {
                        let peer = stream
                            .peer_addr()
                            .map_or_else(|_| "?".to_owned(), |a| a.to_string());
                        route_post(shared, &h, &body, &peer)
                    }
                    _ => Response::error(400, "bad body"),
                }
            }
        }
        (Some(h), _) => route(shared, &h.method, &h.target, h.token.as_deref()),
        (None, _) => Response::error(400, "bad request"),
    };
    let _ = stream.write_all(&resp.to_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centers() -> crate::celestial::Centers {
        let ly = crate::celestial::LIGHTYEAR;
        let lines: Vec<String> = [
            format!("s\t39000001\t{}\t0\t{}", ly, -2.0 * ly),
            format!("s\t39000002\t{}\t0\t0", 3.0 * ly),
            "s\t39000003\t0\t0\t0".to_owned(),
            "c\t29000001\t0\t0\t0".to_owned(),
        ]
        .into();
        crate::celestial::Centers::parse(&lines).expect("invented centres parse")
    }

    fn rows() -> Vec<(i64, String, i64, f64)> {
        vec![
            (39_000_001, "Alpha \"A\"".to_owned(), 19_000_001, 0.95),
            (39_000_002, "Beta".to_owned(), 19_000_001, -0.2),
            (39_000_003, "Gamma".to_owned(), 19_000_002, 0.4),
            // No centre: left out.
            (39_000_004, "Delta".to_owned(), 19_000_002, 0.4),
        ]
    }

    fn map() -> MapData {
        MapData::build(
            rows(),
            &centers(),
            vec![
                (39_000_001, 39_000_002),
                (39_000_002, 39_000_001),
                (39_000_002, 39_000_004),
            ],
        )
    }

    fn shared_with(pilots: Vec<Pilot>) -> Shared {
        let s = Shared::new("t0k3n".repeat(8), &map());
        s.publish(Snapshot {
            taken_unix: 1,
            pilots,
            npcs: 3,
        });
        s
    }

    fn pilot(id: i64, bot: bool, state: &'static str, system: i64) -> Pilot {
        Pilot {
            character_id: id,
            name: format!("Pilot {id}"),
            user_id: Some(7),
            bot,
            system_id: Some(system),
            state,
            station_id: (state == "docked").then_some(69_000_001),
            ship_type_id: 99_000_001,
            ship: "Invented Hull".to_owned(),
            position: (state == "space").then_some([1.4, -2.6, 3.0]),
        }
    }

    #[test]
    fn map_keeps_gated_systems_with_centres_and_each_link_once() {
        let m = map();
        let ids: Vec<i64> = m.systems.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![39_000_001, 39_000_002]);
        assert_eq!(m.links, vec![(39_000_001, 39_000_002)]);
        let json = m.to_json();
        assert_eq!(
            json,
            "{\"systems\":[[39000001,\"Alpha \\\"A\\\"\",19000001,0.95,1,-2],\
             [39000002,\"Beta\",19000001,-0.2,3,0]],\"links\":[[39000001,39000002]]}"
        );
    }

    #[test]
    fn map_without_gates_keeps_known_space_only() {
        let ly = crate::celestial::LIGHTYEAR;
        let lines = vec![
            format!("s\t30999990\t{ly}\t0\t0"),
            "s\t39000001\t0\t0\t0".to_owned(),
        ];
        let c = crate::celestial::Centers::parse(&lines).expect("parse");
        let m = MapData::build(
            vec![
                (30_999_990, "K".to_owned(), 1, 0.5),
                (39_000_001, "J".to_owned(), 1, -1.0),
            ],
            &c,
            Vec::new(),
        );
        assert_eq!(m.systems.len(), 1);
        assert_eq!(m.systems[0].id, 30_999_990);
    }

    #[test]
    fn api_refuses_without_or_with_a_wrong_token() {
        let s = shared_with(Vec::new());
        for path in [
            "/api/status",
            "/api/pilots",
            "/api/map",
            "/api/systems",
            "/api/x",
        ] {
            assert_eq!(route(&s, "GET", path, None).status, 401, "{path}");
            assert_eq!(route(&s, "GET", path, Some("nope")).status, 401, "{path}");
        }
        assert_eq!(
            route(&s, "GET", "/api/status", Some(&"t0k3n".repeat(8))).status,
            200
        );
        assert_eq!(route(&s, "POST", "/api/status", None).status, 405);
        assert_eq!(route(&s, "GET", "/", None).status, 200);
        assert_eq!(route(&s, "GET", "/favicon.ico", None).status, 204);
        assert_eq!(route(&s, "GET", "/etc/passwd", None).status, 404);
    }

    #[test]
    fn answers_carry_no_cors_header() {
        let s = shared_with(Vec::new());
        let bytes = route(&s, "GET", "/api/status", Some(&"t0k3n".repeat(8))).to_bytes();
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(!text.to_ascii_lowercase().contains("access-control"));
        assert!(text.contains("Cache-Control: no-store"));
    }

    #[test]
    fn pilots_json_shape_on_an_invented_world() {
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_002),
        ]);
        let json = pilots_json(&s);
        assert_eq!(
            json,
            "{\"taken_unix\":1,\"pilots\":[\
             {\"character_id\":90000001,\"name\":\"Pilot 90000001\",\"user_id\":7,\"bot\":false,\
             \"system_id\":39000001,\"system\":\"Alpha \\\"A\\\"\",\"state\":\"space\",\
             \"station_id\":null,\"ship_type_id\":99000001,\"ship\":\"Invented Hull\",\
             \"position\":[1,-3,3],\"group\":null,\"side\":null,\"role\":null},\
             {\"character_id\":90000002,\"name\":\"Pilot 90000002\",\"user_id\":7,\"bot\":true,\
             \"system_id\":39000002,\"system\":\"Beta\",\"state\":\"docked\",\
             \"station_id\":69000001,\"ship_type_id\":99000001,\"ship\":\"Invented Hull\",\
             \"position\":null,\"group\":null,\"side\":null,\"role\":null}]}"
        );
    }

    #[test]
    fn status_and_systems_count_the_snapshot() {
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_001),
            pilot(90_000_003, true, "space", 39_000_002),
        ]);
        s.note_health(Health {
            unix: 5,
            ticks: 40,
            p95_ms: 1.234,
            held_max_ms: 2.0,
            skipped: 0,
        });
        let st = status_json(&s);
        for part in [
            "\"pilots\":3",
            "\"humans\":1",
            "\"bots\":2",
            "\"in_space\":2",
            "\"docked\":1",
            "\"systems\":2",
            "\"npcs\":3",
            "\"tick\":{\"window_unix\":5,\"ticks\":40,\"p95_ms\":1.23,\"held_max_ms\":2,\"skipped\":0}",
        ] {
            assert!(st.contains(part), "{part} in {st}");
        }
        assert_eq!(
            systems_json(&s),
            "{\"systems\":[\
             {\"system_id\":39000001,\"name\":\"Alpha \\\"A\\\"\",\"humans\":1,\"bots\":1,\"in_space\":1},\
             {\"system_id\":39000002,\"name\":\"Beta\",\"humans\":0,\"bots\":1,\"in_space\":1}]}"
        );
    }

    #[test]
    fn json_str_escapes_quotes_controls_and_angle() {
        assert_eq!(
            json_str("a\"b\\c\n</script>"),
            "\"a\\\"b\\\\c\\n\\u003c/script>\""
        );
    }

    #[test]
    fn parse_head_reads_the_token_case_insensitively() {
        let (m, t, tok) =
            parse_head("GET /api/status?x=1 HTTP/1.1\r\nHost: a\r\nx-ADMIN-token:  abc \r\n")
                .expect("head");
        assert_eq!(
            (m.as_str(), t.as_str(), tok.as_deref()),
            ("GET", "/api/status?x=1", Some("abc"))
        );
        assert!(parse_head("garbage").is_none());
    }

    /// An invented world: one human docked, one bot in space; `take` is
    /// `None` until a listener is attached.
    #[test]
    fn snapshot_reads_docked_and_in_space_pilots() {
        use crate::space_state::{Motion, Presence};
        let pilot = |id, name: &str, ship, is_bot| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            is_bot,
            solar_system_id: Some(39_000_001),
            station_id: Some(69_000_001),
            ship_type_id: 99_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(91_000_001, "Human", 92_000_001, false),
                pilot(91_000_002, "Bot", 92_000_002, true),
                // Offline: not in the snapshot.
                pilot(91_000_003, "Away", 92_000_003, false),
            ],
            ..crate::model::SandboxConfig::default()
        });
        for (n, id) in [(1, 91_000_001), (2, 91_000_002)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        reg.roster_mut().arrive(Presence::arriving(
            91_000_002,
            92_000_002,
            39_000_002,
            Motion::arrived([10.0, 20.0, 30.0], [0.0; 3]),
        ));
        assert!(take(&reg).is_none());
        attach(&mut reg, Arc::new(Shared::new("x".repeat(32), &map())));
        type Row = (
            i64,
            bool,
            Option<i64>,
            &'static str,
            Option<i64>,
            Option<[f64; 3]>,
        );
        let (_, snap) = take(&reg).expect("attached");
        let got: Vec<Row> = snap
            .pilots
            .iter()
            .map(|p| {
                (
                    p.character_id,
                    p.bot,
                    p.system_id,
                    p.state,
                    p.station_id,
                    p.position,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    91_000_001,
                    false,
                    Some(39_000_001),
                    "docked",
                    Some(69_000_001),
                    None
                ),
                (
                    91_000_002,
                    true,
                    Some(39_000_002),
                    "space",
                    None,
                    Some([10.0, 20.0, 30.0])
                ),
            ]
        );
    }

    fn post_head(path: &str, token: Option<&str>) -> Head {
        Head {
            method: "POST".to_owned(),
            target: path.to_owned(),
            token: token.map(str::to_owned),
            content_type: Some("application/json".to_owned()),
            origin: None,
            host: Some("admin.example:8090".to_owned()),
            content_length: None,
        }
    }

    #[test]
    fn actions_refuse_without_token_wrong_type_or_foreign_origin() {
        let s = shared_with(Vec::new());
        let tok = "t0k3n".repeat(8);
        let body = "{\"words\":\"list\"}";
        let post = |h: &Head| route_post(&s, h, body, "peer").status;
        assert_eq!(post(&post_head("/api/action/bots", None)), 401);
        assert_eq!(post(&post_head("/api/action/bots", Some("nope"))), 401);
        let mut h = post_head("/api/action/bots", Some(&tok));
        h.content_type = Some("text/plain".to_owned());
        assert_eq!(post(&h), 415);
        h.content_type = None;
        assert_eq!(post(&h), 415);
        let mut h = post_head("/api/action/bots", Some(&tok));
        h.origin = Some("http://evil.example".to_owned());
        assert_eq!(post(&h), 403);
        h.origin = Some("http://admin.example:8090.evil.example".to_owned());
        assert_eq!(post(&h), 403);
        // Nothing reached the queue.
        assert!(s.actions().allow("bots", Instant::now()));
        assert_eq!(
            route_post(&s, &post_head("/api/action/nope", Some(&tok)), "{}", "p").status,
            404
        );
        assert_eq!(
            route_post(&s, &post_head("/api/action/bots", Some(&tok)), "x", "p").status,
            400
        );
    }

    /// Over a socket: a POST with the token and the listener's own Origin is
    /// queued; a thread playing the clock answers it; the second one inside a
    /// second is 429.
    #[test]
    fn an_action_is_queued_answered_and_rate_limited() {
        let dir = std::env::temp_dir().join(format!("admin-act-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let tok = "t0k3n".repeat(8);
        let mut shared = Shared::new(tok.clone(), &map());
        shared.dir = Some(dir.clone());
        let shared = Arc::new(shared);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        serve(listener, Arc::clone(&shared)).expect("serve");
        let clock = Arc::clone(&shared);
        let fake_clock = std::thread::spawn(move || {
            for _ in 0..200 {
                for job in clock.actions().take_all_for_test() {
                    let _ =
                        job.reply
                            .send(format!("ran {} {}", job.action.name(), job.action.args()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let ask = |origin: &str| {
            let body = "{\"words\":\"list now\"}";
            let mut c = TcpStream::connect(addr).expect("connect");
            c.set_read_timeout(Some(Duration::from_secs(8))).expect("t");
            write!(
                c,
                "POST /api/action/bots HTTP/1.1\r\nHost: {addr}\r\n{origin}\
                 Content-Type: application/json\r\nX-Admin-Token: {tok}\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .expect("write");
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        let foreign = ask("Origin: http://evil.example\r\n");
        assert!(foreign.starts_with("HTTP/1.1 403 "), "{foreign}");
        let own = ask(&format!("Origin: http://{addr}\r\n"));
        assert!(own.starts_with("HTTP/1.1 200 "), "{own}");
        assert!(
            own.contains("{\"ok\":true,\"text\":\"ran bots list now\"}"),
            "{own}"
        );
        let again = ask("");
        assert!(again.starts_with("HTTP/1.1 429 "), "{again}");
        let log = std::fs::read_to_string(dir.join(ACTION_LOG)).expect("log");
        assert!(log.contains("bots list now -> ran bots list now"), "{log}");
        assert!(log.starts_with("[admin] "), "{log}");
        let _ = fake_clock.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bot_groups_join_the_pilots_by_name_and_only_for_bots() {
        let text = "#group\tside\trole\tname\n\
                    g1\tred\tmine\tPilot 90000002\n\
                    bad row\n\
                    g2\tblue\tescort\tPilot 90000001\n";
        let got = parse_groups(text);
        assert_eq!(got.len(), 2, "{got:?}");
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_002),
        ]);
        s.set_groups(text);
        let json = pilots_json(&s);
        // The human named in the file gets nothing; the bot gets its row.
        assert!(
            json.contains("\"position\":[1,-3,3],\"group\":null,\"side\":null,\"role\":null}"),
            "{json}"
        );
        assert!(
            json.contains("\"group\":\"g1\",\"side\":\"red\",\"role\":\"mine\""),
            "{json}"
        );
    }

    /// Over a real socket on loopback: no token, 401; the token, 200.
    #[test]
    fn listener_refuses_without_the_token() {
        let dir = std::env::temp_dir().join(format!("admin-web-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Shared::open("127.0.0.1:0".parse().expect("addr"), &dir, &map());
        // `open` binds an ephemeral port we cannot read back; bind our own.
        drop(shared);
        let token = std::fs::read_to_string(dir.join(TOKEN_FILE)).expect("token file");
        let token = token.trim().to_owned();
        assert_eq!(token.len(), 64);
        // A second open keeps the token.
        assert_eq!(token_in(&dir).expect("again"), token);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let s = Arc::new(Shared::new(token.clone(), &map()));
        serve(listener, s).expect("serve");
        let ask = |extra: &str| {
            let mut c = TcpStream::connect(addr).expect("connect");
            c.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            write!(c, "GET /api/status HTTP/1.1\r\nHost: x\r\n{extra}\r\n").expect("write");
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        assert!(ask("").starts_with("HTTP/1.1 401 "));
        assert!(ask(&format!("X-Admin-Token: {token}\r\n")).starts_with("HTTP/1.1 200 "));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
