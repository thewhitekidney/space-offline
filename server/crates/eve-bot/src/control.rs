//! **Runtime bot control, the `eve-bots` end** (lane `bot-ctl`).
//!
//! The server's `/bots` command (`eve_server::bot_control`) relays one line
//! at a time over a loopback link; this module is the process that answers.
//! [`Controller::handle`] takes the line and returns the short reply the GM
//! sees; [`run_link`] is the thread that keeps the link up.
//!
//! # The seam with DEV-MODE's roster
//!
//! [`Spawn`] is one roster row (group, side, role, count, skills, ship,
//! optional system). Starting bots for a row is **not done here**: the
//! process that owns the start-up supplies a [`SpawnHook`], and
//! [`Controller`] only keeps what the hook returned (the [`Member`]s) so it
//! can list, bring, re-side and stop them. `eve-bots` implements the hook by
//! building `BotConfig`s the way its start-up does; the roster loader calls
//! the same hook for each file row. What the hook must do beyond starting
//! the threads (make sure the characters named `<group>-<n>` exist, grant
//! `skills`, put the group in one fleet with the first as boss, make
//! opposite sides hostile) is DEV-MODE's, and [`Controller::side_of`] is how
//! it reads a group's current side.
//!
//! # The commands (words; the server has already checked them)
//!
//! - `spawn <group> <side> <role> <count> <skills> <ship> [system]`
//! - `list`
//! - `despawn <group|all>`
//! - `bring <group> <system> <station> <character>`: the server resolved
//!   the caller; the group's bots fly to that station (their way home,
//!   [`crate::driver::Handle::bring`]), which needs the gate graph
//!   (`--content` or `--celestials`) to cross systems.
//! - BP-CAMP: `camp <group|all> <system>`: the gang holds that system's gate
//!   ([`crate::roam_gang::Plan::Camp`]).
//! - GANG-SUMMON: `summon <group|all> <system> <character>`: the server
//!   resolved the caller; every bot of the group flies to that character
//!   ([`crate::driver::Handle::summon`]): a roaming gang's FC leads them
//!   there, any other role undocks, flies and warps to him.
//! - GANG-SUMMON: a `spawn` may end in `quarry=<character>:<system>` (before
//!   `tokens=`): a roaming gang then hunts that character, or for an allied
//!   one joins him, instead of roaming ([`Spawn::quarry`]).
//! - FC-3: `order <character> <words...>` (every wing bot that follows him),
//!   `order-group <group> <words...>` and `join <character>`: the words of
//!   [`eve_server::wing_words`], queued for the wing bots' next turn.
//! - `side <group> <side>`
//! - BOT-ADMIN: `pause <group|bot|all>` (parks at the next docked turn),
//!   `resume <group|bot|all>`, `inspect <bot>` (state and the last
//!   [`INSPECT_ROWS`] act-log rows), `stop-all` (every bot, the ones
//!   started at launch too). A `KILL` file in the control directory does
//!   the same without the link (`eve-bots`' main loop polls it).
//!
//! Group and bot names match without regard to case (the GM's natural
//! `/bots` words arrive as typed).
//!
//! The link is std sockets only; the token and address come from the files
//! the server writes (`bot-control.token`, `bot-control.addr`).

use crate::brain::Place;
use crate::driver::Handle;
use eve_server::wing_words::WingCmd;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// One roster row: what `/bots spawn` and a roster file both ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub group: String,
    /// `red`, `blue`, ...: opposite sides are hostile (DEV-MODE's rule).
    pub side: String,
    /// A role name as `--loop` spells it.
    pub role: String,
    pub count: u32,
    /// `all5`, `role` or a plan name; DEV-MODE's dev skill grant reads it.
    pub skills: String,
    /// A hull name or id.
    pub ship: String,
    /// Start in this system; `None`: wherever the character is.
    pub system: Option<i64>,
    /// BOT-ACCOUNTS: the logins the server made for this spawn's bots
    /// (empty for a roster file's rows, whose logins are in `--tokens`).
    pub tokens: Tokens,
    /// GANG-SUMMON: `(character, system)` a roaming gang hunts (hostile) or
    /// joins (allied); the system is the character's, not the spawn's.
    pub quarry: Option<(i64, i64)>,
    /// FC-1: the human FC a `wing` follows (the spawn's `fc=`); `None` for
    /// every other role.
    pub fc: Option<i64>,
}

/// `NAME -> TOKEN`, as the server sent them. **Secret**: `Debug` shows only
/// the count, so no log line or panic message can print a token.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tokens(pub BTreeMap<String, String>);

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tokens({} hidden)", self.0.len())
    }
}

impl Tokens {
    /// The token of bot `name`, if the server made one for it.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Parse the wire's `NAME:PASSWORD,...`; a bot's token is the whole
    /// `NAME:PASSWORD`. A malformed entry is dropped.
    fn parse(list: &str) -> Tokens {
        Tokens(
            list.split(',')
                .filter_map(|t| {
                    let (name, pw) = t.split_once(':')?;
                    (!name.is_empty() && !pw.is_empty()).then(|| (name.to_owned(), t.to_owned()))
                })
                .collect(),
        )
    }
}

/// One running bot a hook started.
pub struct Member {
    pub name: String,
    pub handle: Handle,
    pub thread: JoinHandle<()>,
}

/// Starts the bots of one row; the process supplies it (module header).
pub type SpawnHook = Box<dyn FnMut(&Spawn) -> Result<Vec<Member>, String> + Send>;

struct Group {
    spec: Spawn,
    members: Vec<Member>,
}

/// The most act-log rows `inspect` shows.
pub const INSPECT_ROWS: usize = 20;

/// The groups this process runs on the GM's word, and the hook that starts
/// more.
pub struct Controller {
    groups: BTreeMap<String, Group>,
    hook: SpawnHook,
    /// BOT-ADMIN: bots started at launch (`--loop`, a roster file), by name:
    /// not in a group, but they can be paused, inspected and stopped.
    loose: Vec<(String, Handle)>,
}

/// The most bots `list` names one by one in a group.
const LIST_BOTS: usize = 8;

impl Controller {
    pub fn new(hook: SpawnHook) -> Controller {
        Controller {
            groups: BTreeMap::new(),
            hook,
            loose: Vec::new(),
        }
    }

    /// BOT-ADMIN: the bots started at launch, so `pause`, `resume`,
    /// `inspect` and `stop-all` reach them by name.
    pub fn adopt_loose(&mut self, bots: Vec<(String, Handle)>) {
        self.loose = bots;
    }

    /// The group key that matches `name` without regard to case.
    fn group_key(&self, name: &str) -> Option<String> {
        self.groups
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// The handles `which` names: `all`, a group, or one bot.
    fn targets(&self, which: &str) -> Vec<(String, Handle)> {
        if which.eq_ignore_ascii_case("all") {
            return self.everyone();
        }
        if let Some(k) = self.group_key(which) {
            return self.groups[&k]
                .members
                .iter()
                .map(|m| (m.name.clone(), m.handle.clone()))
                .collect();
        }
        self.everyone()
            .into_iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(which))
            .collect()
    }

    /// One bot's handle by name (tests, and the admin verbs).
    pub fn handle_of(&self, bot: &str) -> Option<Handle> {
        self.targets(bot)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(bot))
            .map(|(_, h)| h)
    }

    fn everyone(&self) -> Vec<(String, Handle)> {
        self.groups
            .values()
            .flat_map(|g| g.members.iter().map(|m| (m.name.clone(), m.handle.clone())))
            .chain(self.loose.iter().cloned())
            .collect()
    }

    fn pause(&mut self, which: &str, on: bool) -> String {
        let hit = self.targets(which);
        if hit.is_empty() {
            return format!("no group or bot {which}");
        }
        for (_, h) in &hit {
            if on {
                h.pause();
            } else {
                h.resume();
            }
        }
        if on {
            format!(
                "{} bot(s) of {which} will park at their next docked turn (a bot in space \
                 finishes its trip first); /bots resume {which} lets them go",
                hit.len()
            )
        } else {
            format!("{} bot(s) of {which} are running again", hit.len())
        }
    }

    fn inspect(&self, which: &str) -> String {
        let hit = self.targets(which);
        let [(name, h)] = hit.as_slice() else {
            return if hit.is_empty() {
                format!("no bot {which}; /bots list names them")
            } else {
                format!("inspect takes one bot; {which} is {} bots", hit.len())
            };
        };
        let state = match (h.is_paused(), h.is_parked()) {
            (true, true) => "paused (parked)",
            (true, false) => "pause asked, still on its way to a dock",
            _ => "running",
        };
        let mut out = vec![format!("{name}: {state}; {}", doing_of(h, false))];
        let log = h.log();
        let from = log.len().saturating_sub(INSPECT_ROWS);
        out.push(format!("last {} act-log rows:", log.len() - from));
        for e in &log[from..] {
            out.push(format!(
                "  {:>6}s {:?} {} [{}]",
                e.at.as_secs(),
                e.kind,
                e.what,
                match &e.verdict {
                    crate::driver::Verdict::Ok => "ok".to_owned(),
                    crate::driver::Verdict::NotSent(w) => format!("not sent: {w}"),
                    crate::driver::Verdict::Refused(w) => format!("refused: {w}"),
                    crate::driver::Verdict::Failed(w) => format!("failed: {w}"),
                }
            ));
        }
        out.join("\n")
    }

    fn stop_all(&mut self) -> String {
        let groups = self.groups.len();
        let mut bots = 0;
        for g in std::mem::take(&mut self.groups).into_values() {
            bots += g.members.len();
            for m in &g.members {
                m.handle.stop();
            }
        }
        let loose = std::mem::take(&mut self.loose);
        for (_, h) in &loose {
            h.stop();
        }
        format!(
            "stopped {groups} group(s), {bots} bot(s), and {} bot(s) started at launch",
            loose.len()
        )
    }

    /// Adopt bots started at launch (a roster file's rows) so `/bots list`
    /// and `despawn` see them too.
    pub fn adopt(&mut self, spec: Spawn, members: Vec<Member>) {
        self.groups
            .insert(spec.group.clone(), Group { spec, members });
    }

    /// A group's side now, for the hostility rule.
    pub fn side_of(&self, group: &str) -> Option<&str> {
        self.groups.get(group).map(|g| g.spec.side.as_str())
    }

    /// Every group's side.
    pub fn sides(&self) -> BTreeMap<String, String> {
        self.groups
            .iter()
            .map(|(k, g)| (k.clone(), g.spec.side.clone()))
            .collect()
    }

    /// ADMIN-WEB: one row per bot of every group, `group<TAB>side<TAB>role<TAB>
    /// character name`, for the server's admin page to join by name. A name
    /// is whatever the bot logged on as (its key before the first login).
    pub fn groups_tsv(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let mut out = String::from("#group\tside\trole\tname\n");
        for g in self.groups.values() {
            for m in &g.members {
                let name = m.handle.character().unwrap_or_else(|| m.name.clone());
                out.push_str(&format!(
                    "{}\t{}\t{}\t{}\n",
                    clean(&g.spec.group),
                    clean(&g.spec.side),
                    clean(&g.spec.role),
                    clean(&name)
                ));
            }
        }
        out
    }

    /// Run one command line; the answer is what the GM reads.
    pub fn handle(&mut self, line: &str) -> String {
        let mut words: Vec<&str> = line.split_whitespace().collect();
        // BOT-ACCOUNTS: the server's `tokens=NAME:PW,...` closes a spawn.
        let tokens = match words.last().and_then(|w| w.strip_prefix("tokens=")) {
            Some(list) => {
                let t = Tokens::parse(list);
                words.pop();
                t
            }
            None => Tokens::default(),
        };
        // GANG-SUMMON: `quarry=<character>:<system>` and FC-1's
        // `fc=<character>` come before it, in either order.
        let mut quarry = None;
        let mut fc = None;
        while let Some(last) = words.last().copied() {
            if let Some(q) = last.strip_prefix("quarry=") {
                let parsed = q
                    .split_once(':')
                    .and_then(|(c, s)| Some((c.parse::<i64>().ok()?, s.parse::<i64>().ok()?)));
                let Some(parsed) = parsed else {
                    return format!("quarry '{q}' is not <character>:<system>");
                };
                quarry = Some(parsed);
            } else if let Some(f) = last.strip_prefix("fc=") {
                let Ok(f) = f.parse::<i64>() else {
                    return format!("fc '{f}' is not a numeric character id");
                };
                fc = Some(f);
            } else {
                break;
            }
            words.pop();
        }
        match words.as_slice() {
            ["spawn", group, side, role, count, skills, ship, rest @ ..] if rest.len() <= 1 => {
                let Ok(count) = count.parse::<u32>() else {
                    return format!("count '{count}' is not a number");
                };
                let system = match rest {
                    [s] => match s.parse::<i64>() {
                        Ok(s) => Some(s),
                        Err(_) => return format!("system '{s}' is not a numeric id"),
                    },
                    _ => None,
                };
                self.spawn(Spawn {
                    group: (*group).to_owned(),
                    side: (*side).to_owned(),
                    role: (*role).to_owned(),
                    count,
                    skills: (*skills).to_owned(),
                    ship: (*ship).to_owned(),
                    system,
                    tokens,
                    quarry,
                    fc,
                })
            }
            ["list"] => self.list(),
            ["despawn", which] => self.despawn(which),
            ["pause", which] => self.pause(which, true),
            ["resume", which] => self.pause(which, false),
            ["inspect", which] => self.inspect(which),
            ["stop-all"] => self.stop_all(),
            ["bring", group, system, station, who] => {
                match (system.parse::<i64>(), station.parse::<i64>()) {
                    (Ok(system), Ok(station)) => self.bring(group, system, station, who),
                    _ => "bring: system and station are numeric ids".to_owned(),
                }
            }
            ["summon", group, system, who] => match (system.parse::<i64>(), who.parse::<i64>()) {
                (Ok(system), Ok(who)) => self.summon(group, system, who),
                _ => "summon: system and character are numeric ids".to_owned(),
            },
            // BP-CAMP: `camp <group|all> <system>`: the gang holds that
            // system's gate (null or wormhole; the bot checks the band).
            ["camp", group, system] => match system.parse::<i64>() {
                Ok(system) => self.camp(group, system),
                Err(_) => "camp: the system is a numeric id".to_owned(),
            },
            // FC-3: `order <fc> <words>` reaches every wing bot that follows
            // the character; `order-group <group> <words>` one group.
            ["order", fc, words @ ..] if !words.is_empty() => match fc.parse::<i64>() {
                Ok(fc) => self.order(Some(fc), None, words),
                Err(_) => "order: the character is a numeric id".to_owned(),
            },
            ["order-group", group, words @ ..] if !words.is_empty() => {
                self.order(None, Some(group), words)
            }
            ["join", fc] => match fc.parse::<i64>() {
                Ok(fc) => self.order_cmd(Some(fc), None, WingCmd::Join, "join"),
                Err(_) => "join: the character is a numeric id".to_owned(),
            },
            ["side", group, side] => self.set_side(group, side),
            _ => format!("eve-bots does not understand: {}", words.join(" ")),
        }
    }

    fn spawn(&mut self, spec: Spawn) -> String {
        if self.group_key(&spec.group).is_some() {
            return format!(
                "group {} already exists; /bots despawn {} first",
                spec.group, spec.group
            );
        }
        match (self.hook)(&spec) {
            Ok(members) => {
                let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
                let reply = format!(
                    "spawned {} x {} in group {} ({}): {}",
                    members.len(),
                    spec.role,
                    spec.group,
                    spec.side,
                    names.join(" ")
                );
                self.groups
                    .insert(spec.group.clone(), Group { spec, members });
                reply
            }
            Err(why) => format!("spawn {}: {why}", spec.group),
        }
    }

    fn list(&mut self) -> String {
        if self.groups.is_empty() {
            return "no bot groups".to_owned();
        }
        let mut out = Vec::new();
        for g in self.groups.values() {
            let alive = g.members.iter().filter(|m| !m.thread.is_finished()).count();
            out.push(format!(
                "{} side={} role={} count={} alive={}",
                g.spec.group,
                g.spec.side,
                g.spec.role,
                g.members.len(),
                alive
            ));
            for m in g.members.iter().take(LIST_BOTS) {
                out.push(format!("  {}: {}", who(m), doing(m)));
            }
            if g.members.len() > LIST_BOTS {
                out.push(format!("  ... {} more", g.members.len() - LIST_BOTS));
            }
        }
        out.join("\n")
    }

    fn despawn(&mut self, which: &str) -> String {
        let names: Vec<String> = if which == "all" {
            self.groups.keys().cloned().collect()
        } else if let Some(k) = self.group_key(which) {
            vec![k]
        } else {
            return format!("no group {which}");
        };
        let mut bots = 0;
        for n in &names {
            if let Some(g) = self.groups.remove(n) {
                bots += g.members.len();
                // Stopped, not joined: a bot leaves at its next turn, and
                // the link must keep answering meanwhile.
                for m in &g.members {
                    m.handle.stop();
                }
            }
        }
        format!("despawned {} group(s), {bots} bot(s)", names.len())
    }

    fn bring(&mut self, group: &str, system: i64, station: i64, who: &str) -> String {
        let keys: Vec<String> = if group.eq_ignore_ascii_case("all") {
            self.groups.keys().cloned().collect()
        } else {
            match self.group_key(group) {
                Some(k) => vec![k],
                None => return format!("no group {group}"),
            }
        };
        let mut n = 0;
        for k in &keys {
            for m in &self.groups[k].members {
                m.handle.bring(station);
                n += 1;
            }
        }
        format!(
            "{n} bot(s) of {group} are heading for station {station} in system {system} to meet {who}"
        )
    }

    /// GANG-SUMMON: every bot of `group` (or of every group) goes to the
    /// character `who`, in `system`.
    fn summon(&mut self, group: &str, system: i64, who: i64) -> String {
        let keys: Vec<String> = if group.eq_ignore_ascii_case("all") {
            self.groups.keys().cloned().collect()
        } else {
            match self.group_key(group) {
                Some(k) => vec![k],
                None => return format!("no group {group}"),
            }
        };
        let mut n = 0;
        for k in &keys {
            for m in &self.groups[k].members {
                m.handle.summon(system, who);
                n += 1;
            }
        }
        format!("{n} bot(s) of {group} are coming to {who} in system {system}")
    }

    /// BP-CAMP: every bot of `group` (or of every group) camps the gate of
    /// `system`.
    fn camp(&mut self, group: &str, system: i64) -> String {
        let keys: Vec<String> = if group.eq_ignore_ascii_case("all") {
            self.groups.keys().cloned().collect()
        } else {
            match self.group_key(group) {
                Some(k) => vec![k],
                None => return format!("no group {group}"),
            }
        };
        let mut n = 0;
        for k in &keys {
            for m in &self.groups[k].members {
                m.handle.camp(system);
                n += 1;
            }
        }
        format!("{n} bot(s) of {group} are camping the gate in system {system}")
    }

    /// FC-3: parse the words and hand the command on.
    fn order(&mut self, fc: Option<i64>, group: Option<&str>, words: &[&str]) -> String {
        match eve_server::wing_words::parse(words) {
            Some(cmd) => self.order_cmd(fc, group, cmd, &words.join(" ")),
            None => format!("order: '{}' is not a word for the wing", words.join(" ")),
        }
    }

    /// FC-3: queue `cmd` for every wing bot that follows `fc`, or every wing
    /// bot of `group`. Hostile gangs and other roles are never reached: a
    /// bot follows an FC only through its wing seat.
    fn order_cmd(
        &mut self,
        fc: Option<i64>,
        group: Option<&str>,
        cmd: WingCmd,
        said: &str,
    ) -> String {
        let keys: Vec<String> = match group {
            Some(g) => match self.group_key(g) {
                Some(k) => vec![k],
                None => return format!("no group {g}"),
            },
            None => self.groups.keys().cloned().collect(),
        };
        let mut n = 0;
        for k in &keys {
            for m in &self.groups[k].members {
                let hit = match fc {
                    Some(fc) => m.handle.follows(fc),
                    None => m.handle.is_wing(),
                };
                if hit {
                    m.handle.order(cmd.clone());
                    n += 1;
                }
            }
        }
        if n == 0 {
            return match fc {
                Some(_) => "no wing follows you: /bots 6 wing first".to_owned(),
                None => "no wing bots in that group".to_owned(),
            };
        }
        format!("{n} wing bot(s) were told: {said}")
    }

    fn set_side(&mut self, group: &str, side: &str) -> String {
        let key = self.group_key(group);
        match key.and_then(|k| self.groups.get_mut(&k)) {
            Some(g) => {
                let was = std::mem::replace(&mut g.spec.side, side.to_owned());
                format!("group {group}: side {was} -> {side}")
            }
            None => format!("no group {group}"),
        }
    }
}

/// ADMIN-WEB: the file [`Controller::groups_tsv`] is written to, in the
/// control directory; the server's admin page reads it.
pub const GROUPS_FILE: &str = "bot-groups.tsv";

/// A bot as `/bots list` names it: its key, and its character's name when
/// that differs (`hulka3 (Kael Verran)`).
fn who(m: &Member) -> String {
    match m.handle.character() {
        Some(c) if c != m.name => format!("{} ({c})", m.name),
        _ => m.name.clone(),
    }
}

/// Where a bot is and what it did last, in a few words.
fn doing(m: &Member) -> String {
    if m.thread.is_finished() {
        return "gone".to_owned();
    }
    doing_of(&m.handle, true)
}

/// [`doing`] for a handle: where it is and what it did last.
fn doing_of(h: &Handle, brief: bool) -> String {
    let paused = match (h.is_paused(), h.is_parked()) {
        (true, true) if brief => " [paused]",
        (true, false) if brief => " [pause pending]",
        _ => "",
    };
    let place = match h.observation().map(|o| o.place) {
        Some(Place::Docked { station }) => format!("docked at {station}"),
        Some(Place::InSpace { system, .. }) => format!("in space, system {system}"),
        Some(Place::Transit) => "in transit".to_owned(),
        None => "logging in".to_owned(),
    };
    let place = format!("{place}{paused}");
    match h.last_entry() {
        Some(e) => format!("{place}; {:?} {}", e.kind, e.what),
        None => place,
    }
}

// ---------------------------------------------------------------------
// The link
// ---------------------------------------------------------------------

/// How often the link thread looks at its stop flag while idle.
const POLL: Duration = Duration::from_millis(300);
/// How long to wait before dialing again.
const REDIAL: Duration = Duration::from_secs(2);

/// Read where the server's link is and its token from `dir`.
pub fn read_link(dir: &Path) -> Result<(SocketAddr, String), String> {
    let addr = std::fs::read_to_string(dir.join("bot-control.addr"))
        .map_err(|e| format!("{}/bot-control.addr: {e}", dir.display()))?;
    let addr: SocketAddr = addr
        .trim()
        .parse()
        .map_err(|_| "bot-control.addr is not an address".to_owned())?;
    if !addr.ip().is_loopback() {
        return Err("bot-control.addr is not loopback; refusing".to_owned());
    }
    let token = std::fs::read_to_string(dir.join("bot-control.token"))
        .map_err(|e| format!("{}/bot-control.token: {e}", dir.display()))?;
    Ok((addr, token.trim().to_owned()))
}

/// Keep a control link to the server in `dir` up on its own thread until
/// `stop` is set: dial, say `HELLO`, answer each `CMD` with `controller`.
/// It redials after a hang-up, and while the files are not there yet.
pub fn run_link(
    dir: std::path::PathBuf,
    controller: Arc<Mutex<Controller>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("bot-control".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok((addr, token)) = read_link(&dir) {
                    let _ = serve_once(addr, &token, &controller, &stop);
                }
                sleep_unless(&stop, REDIAL);
            }
        })
        .expect("a thread for the control link")
}

fn sleep_unless(stop: &AtomicBool, total: Duration) {
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let step = left.min(POLL);
        std::thread::sleep(step);
        left -= step;
    }
}

/// One connection's life. `Err` only says the connection ended.
fn serve_once(
    addr: SocketAddr,
    token: &str,
    controller: &Mutex<Controller>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(format!("HELLO {token}\n").as_bytes())?;
    stream.set_read_timeout(Some(POLL))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // A half line stays in `line`; read_line appends next time.
                continue;
            }
            Err(e) => return Err(e),
        }
        let text = std::mem::take(&mut line);
        let text = text.trim_end();
        if text == "OK" {
            continue;
        }
        if text == "NO" {
            return Ok(());
        }
        let Some(rest) = text.strip_prefix("CMD ") else {
            continue;
        };
        let (id, command) = rest.split_once(' ').unwrap_or((rest, ""));
        let reply = match controller.lock() {
            Ok(mut c) => c.handle(command),
            Err(_) => "eve-bots: the controller is poisoned".to_owned(),
        };
        let reply = reply.replace('\n', "\\n");
        writer.write_all(format!("REPLY {id} {reply}\n").as_bytes())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_words() -> &'static str {
        "spawn alpha red mine 2 all5 merlin 30000001"
    }

    fn empty_hook() -> SpawnHook {
        Box::new(|_| Ok(Vec::new()))
    }

    #[test]
    fn spawn_reaches_the_hook_with_the_row() {
        let seen: Arc<Mutex<Vec<Spawn>>> = Arc::default();
        let log = Arc::clone(&seen);
        let mut c = Controller::new(Box::new(move |s| {
            log.lock().unwrap().push(s.clone());
            Ok(Vec::new())
        }));
        let reply = c.handle(spec_words());
        assert!(reply.starts_with("spawned 0"), "{reply}");
        let got = seen.lock().unwrap();
        assert_eq!(
            got[0],
            Spawn {
                group: "alpha".into(),
                side: "red".into(),
                role: "mine".into(),
                count: 2,
                skills: "all5".into(),
                ship: "merlin".into(),
                system: Some(30_000_001),
                tokens: Tokens::default(),
                quarry: None,
                fc: None,
            }
        );
        drop(got);
        assert!(c.handle(spec_words()).contains("already exists"));
    }

    #[test]
    fn a_spawns_tokens_reach_the_hook_and_never_a_debug_line() {
        let seen: Arc<Mutex<Vec<Spawn>>> = Arc::default();
        let log = Arc::clone(&seen);
        let mut c = Controller::new(Box::new(move |s| {
            log.lock().unwrap().push(s.clone());
            Ok(Vec::new())
        }));
        let secret = "0123abcd0123abcd0123abcd0123abcd";
        c.handle(&format!(
            "spawn alpha red mine 2 all5 merlin tokens=alpha1:{secret},alpha2:{secret}"
        ));
        let got = seen.lock().unwrap();
        assert_eq!(got[0].system, None);
        assert_eq!(
            got[0].tokens.get("alpha2"),
            Some(format!("alpha2:{secret}").as_str())
        );
        assert!(!format!("{:?}", got[0]).contains(secret));
        drop(got);
        assert!(
            !c.handle(&format!("nonsense tokens=a:{secret}"))
                .contains(secret)
        );
    }

    #[test]
    fn a_hook_refusal_comes_back_and_leaves_no_group() {
        let mut c = Controller::new(Box::new(|_| Err("no such role".to_owned())));
        assert_eq!(
            c.handle("spawn a red x 1 all5 merlin"),
            "spawn a: no such role"
        );
        assert_eq!(c.handle("list"), "no bot groups");
    }

    #[test]
    fn groups_tsv_names_each_bot_with_its_group_side_and_role() {
        let mut c = Controller::new(empty_hook());
        c.adopt(
            Spawn {
                group: "g1".to_owned(),
                side: "red".to_owned(),
                role: "mine".to_owned(),
                count: 1,
                skills: "all5".to_owned(),
                ship: "x".to_owned(),
                system: None,
                tokens: Tokens::default(),
                quarry: None,
                fc: None,
            },
            vec![lonely("bot a\tb")],
        );
        assert_eq!(
            c.groups_tsv(),
            "#group\tside\trole\tname\ng1\tred\tmine\tbot a b\n"
        );
    }

    #[test]
    fn side_list_and_despawn_work_on_groups() {
        let mut c = Controller::new(empty_hook());
        c.handle("spawn g red mine 1 all5 merlin");
        assert_eq!(c.side_of("g"), Some("red"));
        assert_eq!(c.handle("side g blue"), "group g: side red -> blue");
        assert_eq!(c.side_of("g"), Some("blue"));
        assert!(
            c.handle("list")
                .starts_with("g side=blue role=mine count=0 alive=0")
        );
        assert_eq!(c.handle("side nope blue"), "no group nope");
        assert_eq!(c.handle("despawn nope"), "no group nope");
        assert_eq!(c.handle("despawn all"), "despawned 1 group(s), 0 bot(s)");
        assert!(c.handle("frobnicate").contains("does not understand"));
    }

    /// A member whose bot never gets a server (nothing listens there): it
    /// has a real [`Handle`], which is all the admin verbs touch.
    fn lonely(name: &str) -> Member {
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut cfg = crate::driver::BotConfig::new(name, addr, crate::driver::Pace::Fast);
        cfg.start_delay = Duration::from_secs(600);
        let (handle, thread) = crate::driver::spawn(cfg);
        Member {
            name: name.to_owned(),
            handle,
            thread,
        }
    }

    /// A wing member: a real handle with a wing seat for `fc`.
    fn lonely_wing(name: &str, fc: Option<i64>) -> Member {
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut cfg = crate::driver::BotConfig::new(name, addr, crate::driver::Pace::Fast);
        cfg.start_delay = Duration::from_secs(600);
        cfg.policy.wing = Some(crate::wing::Seat::new(
            Arc::new(crate::wing_hall::WingHall::new()),
            fc,
        ));
        let (handle, thread) = crate::driver::spawn(cfg);
        Member {
            name: name.to_owned(),
            handle,
            thread,
        }
    }

    /// FC-3: `order <fc> <words>` reaches only the wing bots that follow
    /// that character (never another role, another FC's wing or a hostile
    /// gang); `order-group` one group; an unknown word is refused.
    #[test]
    fn an_order_reaches_only_the_bots_that_follow_the_caller() {
        let mut c = Controller::new(Box::new(|s| {
            Ok((1..=s.count)
                .map(|i| {
                    let name = format!("{}{i}", s.group);
                    match (s.role.as_str(), s.fc) {
                        ("wing", fc) => lonely_wing(&name, fc),
                        _ => lonely(&name),
                    }
                })
                .collect())
        }));
        c.handle("spawn Mine red mine 1 all5 merlin");
        c.handle("spawn Mine2 blue wing 2 all5 - 77 fc=41");
        c.handle("spawn Other blue wing 1 all5 - 77 fc=42");
        let queued = |c: &Controller, group: &str| -> usize {
            c.groups[&c.group_key(group).unwrap()]
                .members
                .iter()
                .map(|m| m.handle.take_orders().len())
                .sum()
        };
        assert_eq!(c.handle("order 41 hold"), "2 wing bot(s) were told: hold");
        assert_eq!(queued(&c, "Mine"), 0);
        assert_eq!(queued(&c, "Other"), 0);
        assert_eq!(queued(&c, "Mine2"), 2);
        assert_eq!(
            c.handle("order 41 weapons free"),
            "2 wing bot(s) were told: weapons free"
        );
        assert!(c.handle("order 99 hold").contains("no wing follows you"));
        assert!(c.handle("order 41 dance").contains("not a word"));
        assert!(c.handle("order x hold").contains("numeric"));
        assert_eq!(
            c.handle("order-group Other follow me"),
            "1 wing bot(s) were told: follow me"
        );
        assert_eq!(queued(&c, "Other"), 1);
        assert!(c.handle("order-group Mine follow").contains("no wing bots"));
        assert!(c.handle("order-group nobody hold").contains("no group"));
        // `join` is not a word, but the control relays it.
        assert_eq!(c.handle("join 42"), "1 wing bot(s) were told: join");
        assert_eq!(queued(&c, "Other"), 1);
        c.handle("stop-all");
    }

    fn crew_controller() -> Controller {
        Controller::new(Box::new(|s| {
            Ok((1..=s.count)
                .map(|i| lonely(&format!("{}{i}", s.group)))
                .collect())
        }))
    }

    #[test]
    fn pause_resume_inspect_and_stop_all_reach_the_bots() {
        let mut c = crew_controller();
        c.handle("spawn Crew red mine 2 all5 merlin");
        // Names match without regard to case.
        let r = c.handle("pause CREW");
        assert!(r.starts_with("2 bot(s) of CREW will park"), "{r}");
        let hs = c.targets("crew");
        assert!(hs.iter().all(|(_, h)| h.is_paused() && !h.is_parked()));
        assert!(c.handle("list").contains("[pause pending]"));
        // One bot by name; a bot nobody knows.
        assert!(c.handle("resume crew1").starts_with("1 bot(s) of crew1"));
        assert!(!c.targets("crew1")[0].1.is_paused());
        assert!(c.targets("crew2")[0].1.is_paused());
        assert_eq!(c.handle("pause nobody"), "no group or bot nobody");
        let r = c.handle("inspect Crew2");
        assert!(
            r.starts_with("Crew2: pause asked, still on its way to a dock;")
                && r.contains("last 0 act-log rows:"),
            "{r}"
        );
        assert!(c.handle("inspect crew").contains("takes one bot"));
        assert_eq!(
            c.handle("inspect nobody"),
            "no bot nobody; /bots list names them"
        );
        assert_eq!(c.handle("resume all"), "2 bot(s) of all are running again");
        assert!(c.targets("all").iter().all(|(_, h)| !h.is_paused()));
        let r = c.handle("stop-all");
        assert_eq!(
            r,
            "stopped 1 group(s), 2 bot(s), and 0 bot(s) started at launch"
        );
        assert_eq!(c.handle("list"), "no bot groups");
    }

    #[test]
    fn bots_started_at_launch_are_reached_by_name() {
        let mut c = crew_controller();
        let m = lonely("Launch1");
        c.adopt_loose(vec![(m.name.clone(), m.handle.clone())]);
        assert!(c.handle("pause launch1").starts_with("1 bot(s)"));
        assert!(m.handle.is_paused());
        assert!(
            c.handle("inspect LAUNCH1")
                .starts_with("Launch1: pause asked")
        );
        assert!(
            c.handle("stop-all")
                .ends_with("and 1 bot(s) started at launch")
        );
        // `bring all` with no groups says so rather than failing.
        assert_eq!(
            c.handle("bring all 1 2 3"),
            "0 bot(s) of all are heading for station 2 in system 1 to meet 3"
        );
    }

    /// GANG-SUMMON: a spawn's `quarry=` (before `tokens=`) reaches the hook;
    /// `summon` is counted and refused on a bad id or an unknown group.
    #[test]
    fn a_spawn_carries_its_quarry_and_summon_counts_the_bots() {
        let seen: Arc<Mutex<Vec<Spawn>>> = Arc::default();
        let log = Arc::clone(&seen);
        let mut c = Controller::new(Box::new(move |s| {
            log.lock().unwrap().push(s.clone());
            Ok(vec![lonely("Quarry1")])
        }));
        let r = c.handle("spawn rg red roamgang 1 role Rifter 77 quarry=5:77 tokens=Quarry1:tk");
        assert!(r.starts_with("spawned 1"), "{r}");
        assert_eq!(seen.lock().unwrap()[0].quarry, Some((5, 77)));
        assert!(seen.lock().unwrap()[0].tokens.get("Quarry1").is_some());
        assert!(
            c.handle("spawn rg2 red roamgang 1 role Rifter 77 quarry=x:77")
                .contains("not <character>:<system>")
        );
        assert_eq!(
            c.handle("summon RG 77 5"),
            "1 bot(s) of RG are coming to 5 in system 77"
        );
        assert_eq!(c.handle("summon nobody 77 5"), "no group nobody");
        assert!(c.handle("summon rg x 5").contains("numeric"));
        assert_eq!(
            c.handle("summon all 77 5"),
            "1 bot(s) of all are coming to 5 in system 77"
        );
    }

    /// FC-1: a spawn's `fc=` reaches the hook, with or without a quarry,
    /// and a bad one is refused before anything spawns.
    #[test]
    fn a_spawn_carries_its_fc() {
        let seen: Arc<Mutex<Vec<Spawn>>> = Arc::default();
        let log = Arc::clone(&seen);
        let mut c = Controller::new(Box::new(move |s| {
            log.lock().unwrap().push(s.clone());
            Ok(vec![lonely("Wing1")])
        }));
        let r = c.handle("spawn wg red wing 1 all5 - 77 fc=41 tokens=Wing1:tk");
        assert!(r.starts_with("spawned 1"), "{r}");
        let got = seen.lock().unwrap();
        assert_eq!(
            (got[0].fc, got[0].quarry, got[0].system),
            (Some(41), None, Some(77))
        );
        assert!(got[0].tokens.get("Wing1").is_some());
        drop(got);
        assert!(
            c.handle("spawn wg2 red wing 1 all5 - 77 fc=x")
                .contains("numeric")
        );
        c.handle("spawn wg3 red wing 1 all5 - 77 quarry=5:77 fc=42");
        let got = seen.lock().unwrap();
        assert_eq!((got[1].fc, got[1].quarry), (Some(42), Some((5, 77))));
    }

    #[test]
    fn the_link_files_must_be_loopback() {
        let dir = std::env::temp_dir().join(format!("botctl-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bot-control.addr"), "10.0.0.5:9\n").unwrap();
        std::fs::write(dir.join("bot-control.token"), "t\n").unwrap();
        assert!(read_link(&dir).unwrap_err().contains("loopback"));
        std::fs::write(dir.join("bot-control.addr"), "127.0.0.1:9\n").unwrap();
        assert_eq!(read_link(&dir).unwrap().1, "t");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
