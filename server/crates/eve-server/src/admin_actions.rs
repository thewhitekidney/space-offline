//! **Admin web actions** (lane `admin-web-2`): `POST /api/action/<name>`.
//!
//! The HTTP thread never touches the world. It checks the request
//! ([`crate::admin_web::route_post`]), parses the body, enqueues a [`Job`]
//! and waits for the answer. The clock drains the queue under its own lock
//! ([`drain`], once per tick) and runs each job as the synthetic GM caller
//! (`Caller::Internal`, which `/bots` accepts as a GM) through the same
//! slash path a GM's console line takes ([`crate::bot_panel::issue`]).
//! A `/bots` relay waits on eve-bots over TCP, so it is handed out as a
//! [`crate::bot_control::Deferred`] and run on its own thread after the
//! lock, exactly as the wire path does (TT-B C1).
//!
//! Actions: `bots {words}`, `broadcast {text}`, `save {}`. Each is limited to
//! one per second ([`Queue::allow`]); every one is logged by the caller.

use crate::bot_control::Deferred;
use crate::service::Registry;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// At most this many jobs wait for the clock; more get 503.
pub const MAX_PENDING: usize = 8;
/// One action per name per this long.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// The longest broadcast text, in characters.
pub const MAX_TEXT: usize = 300;
/// The most words one `bots` action may carry.
pub const MAX_WORDS: usize = 24;

/// What an action asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `/bots <words>`.
    Bots(Vec<String>),
    /// A toast to every pilot online.
    Broadcast(String),
    /// Save the world now.
    Save,
}

impl Action {
    /// The name in the URL.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Bots(_) => "bots",
            Action::Broadcast(_) => "broadcast",
            Action::Save => "save",
        }
    }

    /// The arguments, for the log line.
    pub fn args(&self) -> String {
        match self {
            Action::Bots(w) => w.join(" "),
            Action::Broadcast(t) => t.clone(),
            Action::Save => String::new(),
        }
    }

    /// The action `name` with its JSON `body` (a flat object), or why not.
    pub fn parse(name: &str, body: &str) -> Result<Action, String> {
        let fields = parse_flat_json(body).ok_or("the body is not a flat JSON object")?;
        let text = |key: &str| fields.get(key).map(String::as_str).unwrap_or("").trim();
        match name {
            "bots" => {
                let words: Vec<String> = text("words")
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                if words.is_empty() || words.len() > MAX_WORDS {
                    return Err(format!("\"words\" must be 1 to {MAX_WORDS} words"));
                }
                Ok(Action::Bots(words))
            }
            "broadcast" => {
                let t = text("text");
                if t.is_empty() || t.chars().count() > MAX_TEXT {
                    return Err(format!("\"text\" must be 1 to {MAX_TEXT} characters"));
                }
                if t.chars().any(char::is_control) {
                    return Err("\"text\" has a control character".to_owned());
                }
                Ok(Action::Broadcast(t.to_owned()))
            }
            "save" => Ok(Action::Save),
            _ => Err("no such action".to_owned()),
        }
    }
}

/// One queued action and where its answer goes.
pub struct Job {
    pub action: Action,
    pub reply: Sender<String>,
}

/// What running a job under the lock left.
pub enum Outcome {
    /// The answer, ready.
    Now(Sender<String>, String),
    /// A `/bots` relay to run after the lock.
    Relay(Sender<String>, Deferred),
    /// The clock saves, then answers.
    Save(Sender<String>),
}

/// The jobs waiting for the clock, and the rate limit.
#[derive(Default)]
pub struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    last: Mutex<BTreeMap<&'static str, Instant>>,
}

impl Queue {
    /// Whether `name` may run at `now`; records it when so.
    pub fn allow(&self, name: &'static str, now: Instant) -> bool {
        let Ok(mut last) = self.last.lock() else {
            return false;
        };
        if last
            .get(name)
            .is_some_and(|t| now.saturating_duration_since(*t) < MIN_GAP)
        {
            return false;
        }
        last.insert(name, now);
        true
    }

    /// Queue a job; `false` when the queue is full.
    pub fn push(&self, job: Job) -> bool {
        match self.jobs.lock() {
            Ok(mut q) if q.len() < MAX_PENDING => {
                q.push_back(job);
                true
            }
            _ => false,
        }
    }

    /// The clock's drain, for a test that plays the clock.
    #[cfg(test)]
    pub(crate) fn take_all_for_test(&self) -> Vec<Job> {
        self.take_all()
    }

    pub(crate) fn take_all(&self) -> Vec<Job> {
        self.jobs
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Run one job in the world (under the lock).
pub fn execute(w: &mut Registry, job: Job) -> Outcome {
    let Job { action, reply } = job;
    match action {
        Action::Bots(words) => {
            let line = format!("/bots {}", words.join(" "));
            crate::bot_control::allow_deferral(w, true);
            let text = crate::bot_panel::issue(w, None, &line);
            crate::bot_control::allow_deferral(w, false);
            match crate::bot_control::take_deferred(w) {
                Some(job) => Outcome::Relay(reply, job),
                None => Outcome::Now(reply, text),
            }
        }
        Action::Broadcast(text) => {
            let to = w.sessions().online_characters();
            let n = to.len();
            crate::world_events::notice(w, to, "Server announcement", "", &text);
            Outcome::Now(reply, format!("sent to {n} pilot(s)"))
        }
        Action::Save => Outcome::Save(reply),
    }
}

/// Every queued job, run (the clock, under the world lock, each tick).
pub fn drain(w: &mut Registry) -> Vec<Outcome> {
    let Some(shared) = crate::admin_web::shared_of(w) else {
        return Vec::new();
    };
    let jobs = shared.actions().take_all();
    jobs.into_iter().map(|j| execute(w, j)).collect()
}

/// Send the answers (after the lock); a relay waits on its own thread.
pub fn finish(outcomes: Vec<Outcome>) {
    for o in outcomes {
        match o {
            Outcome::Now(tx, text) => {
                let _ = tx.send(text);
            }
            Outcome::Relay(tx, job) => {
                let _ = std::thread::Builder::new()
                    .name("admin-relay".to_owned())
                    .spawn(move || {
                        let _ = tx.send(job.run());
                    });
            }
            Outcome::Save(tx) => {
                let _ = tx.send("save was not run".to_owned());
            }
        }
    }
}

/// A flat JSON object (`{"k":"v","n":3,"b":true,"z":null}`) as text values.
/// No nesting: an object or array value is refused.
pub fn parse_flat_json(s: &str) -> Option<BTreeMap<String, String>> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let skip = |i: &mut usize| {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1;
        }
    };
    let string = |i: &mut usize| -> Option<String> {
        if b.get(*i) != Some(&'"') {
            return None;
        }
        *i += 1;
        let mut out = String::new();
        loop {
            let c = *b.get(*i)?;
            *i += 1;
            match c {
                '"' => return Some(out),
                '\\' => {
                    let e = *b.get(*i)?;
                    *i += 1;
                    match e {
                        '"' | '\\' | '/' => out.push(e),
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' | 'f' => out.push(' '),
                        'u' => {
                            let hex: String = b.get(*i..*i + 4)?.iter().collect();
                            *i += 4;
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                        }
                        _ => return None,
                    }
                }
                c => out.push(c),
            }
        }
    };
    skip(&mut i);
    if b.get(i) != Some(&'{') {
        return None;
    }
    i += 1;
    let mut map = BTreeMap::new();
    skip(&mut i);
    if b.get(i) == Some(&'}') {
        i += 1;
    } else {
        loop {
            skip(&mut i);
            let key = string(&mut i)?;
            skip(&mut i);
            if b.get(i) != Some(&':') {
                return None;
            }
            i += 1;
            skip(&mut i);
            let value = if b.get(i) == Some(&'"') {
                string(&mut i)?
            } else {
                let start = i;
                while i < b.len() && !matches!(b[i], ',' | '}') && !b[i].is_whitespace() {
                    if matches!(b[i], '{' | '[') {
                        return None;
                    }
                    i += 1;
                }
                let lit: String = b[start..i].iter().collect();
                let ok =
                    matches!(lit.as_str(), "true" | "false" | "null") || lit.parse::<f64>().is_ok();
                if !ok {
                    return None;
                }
                lit
            };
            map.insert(key, value);
            skip(&mut i);
            match b.get(i) {
                Some(',') => i += 1,
                Some('}') => {
                    i += 1;
                    break;
                }
                _ => return None,
            }
        }
    }
    skip(&mut i);
    (i == b.len()).then_some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_control::Link;
    use crate::model::SandboxConfig;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    #[test]
    fn flat_json_reads_strings_numbers_and_refuses_nesting() {
        let m = parse_flat_json(" {\"words\":\"spawn a\\\"b\", \"n\": 3, \"t\":true} ").unwrap();
        assert_eq!(m["words"], "spawn a\"b");
        assert_eq!(m["n"], "3");
        assert_eq!(m["t"], "true");
        assert_eq!(parse_flat_json("{}").unwrap().len(), 0);
        for bad in [
            "",
            "[]",
            "{\"a\":{}}",
            "{\"a\":[1]}",
            "{\"a\":1",
            "{\"a\":1} x",
            "{a:1}",
        ] {
            assert!(parse_flat_json(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn actions_validate_their_fields() {
        assert_eq!(
            Action::parse("bots", "{\"words\":\" list \"}"),
            Ok(Action::Bots(vec!["list".to_owned()]))
        );
        assert!(Action::parse("bots", "{\"words\":\"\"}").is_err());
        assert!(Action::parse("bots", "{}").is_err());
        let long = format!("{{\"words\":\"{}\"}}", "w ".repeat(MAX_WORDS + 1));
        assert!(Action::parse("bots", &long).is_err());
        assert!(Action::parse("broadcast", "{\"text\":\"hi\\nthere\"}").is_err());
        assert_eq!(
            Action::parse("broadcast", "{\"text\":\"hello\"}"),
            Ok(Action::Broadcast("hello".to_owned()))
        );
        assert_eq!(Action::parse("save", "{}"), Ok(Action::Save));
        assert!(Action::parse("stop", "{}").is_err());
    }

    #[test]
    fn rate_limit_is_one_per_second_per_action() {
        let q = Queue::default();
        let t0 = Instant::now();
        assert!(q.allow("bots", t0));
        assert!(!q.allow("bots", t0 + Duration::from_millis(900)));
        assert!(q.allow("save", t0 + Duration::from_millis(900)));
        assert!(q.allow("bots", t0 + MIN_GAP));
    }

    #[test]
    fn the_queue_is_bounded() {
        let q = Queue::default();
        for _ in 0..MAX_PENDING {
            let (tx, _rx) = channel();
            assert!(q.push(Job {
                action: Action::Save,
                reply: tx
            }));
        }
        let (tx, _rx) = channel();
        assert!(!q.push(Job {
            action: Action::Save,
            reply: tx
        }));
        assert_eq!(q.take_all().len(), MAX_PENDING);
    }

    /// The bots action reaches the bot-control path: a fake eve-bots peer
    /// gets the command, and the relay is handed out past the lock.
    #[test]
    fn the_bots_action_reaches_the_control_link() {
        let link = Link::open(None).unwrap();
        let mut s = TcpStream::connect(link.addr()).unwrap();
        s.write_all(format!("HELLO {}\n", link.token()).as_bytes())
            .unwrap();
        let mut first = String::new();
        let mut r = BufReader::new(s.try_clone().unwrap());
        r.read_line(&mut first).unwrap();
        assert_eq!(first.trim(), "OK");
        let fake = std::thread::spawn(move || {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let rest = line.trim_end().strip_prefix("CMD ").unwrap().to_owned();
            let (id, cmd) = rest.split_once(' ').unwrap();
            s.write_all(format!("REPLY {id} fake saw {cmd}\n").as_bytes())
                .unwrap();
        });
        let mut w = Registry::new(SandboxConfig::default());
        crate::bot_control::attach(&mut w, true, Some(Arc::clone(&link)));
        let (tx, rx) = channel();
        let out = execute(
            &mut w,
            Job {
                action: Action::Bots(vec!["list".to_owned()]),
                reply: tx,
            },
        );
        assert!(matches!(out, Outcome::Relay(..)));
        finish(vec![out]);
        let answer = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(answer, "fake saw list");
        fake.join().unwrap();
    }

    #[test]
    fn bots_without_a_dev_server_says_so_and_broadcast_counts_pilots() {
        let mut w = Registry::new(SandboxConfig::default());
        let (tx, _rx) = channel();
        let Outcome::Now(_, text) = execute(
            &mut w,
            Job {
                action: Action::Bots(vec!["list".to_owned()]),
                reply: tx,
            },
        ) else {
            panic!("answered inline");
        };
        assert!(text.contains("dev server"), "{text}");
        let (tx, _rx) = channel();
        let Outcome::Now(_, text) = execute(
            &mut w,
            Job {
                action: Action::Broadcast("hi".to_owned()),
                reply: tx,
            },
        ) else {
            panic!("answered inline");
        };
        assert_eq!(text, "sent to 0 pilot(s)");
    }
}
