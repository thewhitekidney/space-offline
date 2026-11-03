//! **A clean stop** (tracker §83, lane `shutdown`): `/shutdown`, the stop
//! file, and the one path both take.
//!
//! `eve-server` used to serve until it was killed, so `Drop for Server` --
//! the flush that writes the last dirty world through the saver
//! (`dispatch::flush_world`, CP52) -- never ran in production, and every
//! stop lost up to `push::SAVE_EVERY` of play. std has no signal handling,
//! and the workspace has no dependencies and no `unsafe`, so SIGTERM cannot
//! be caught. Instead the server is *asked* to stop, two ways:
//!
//! - **`/shutdown [seconds] [message]`**, a GM slash command: a countdown,
//!   announced to every online character with the client's own cluster
//!   shutdown notification. `/shutdown cancel` aborts it.
//! - **The stop file** (`--stop-file PATH`, default `<world>.stop`): the
//!   world ticker polls it, and its appearance stops the server at once.
//!   For scripts (`tools/up.sh`, `serve.sh`, `soak.sh`), which `touch` it
//!   and wait, with SIGTERM only as the fallback.
//!
//! Both end in [`Stop::request`]: the flag is set, and each listener is
//! woken by a connection to itself so its blocking `accept` returns and
//! `Server::run` returns (no new login is accepted from then on). The
//! binary then hangs up every session (`Server::hang_up`), waits up to
//! [`SESSION_DRAIN`] for their disconnect hooks, drops the `Server` -- whose
//! `Drop` renders the last world and closes the saver, which writes it and
//! releases the `--world` lock -- and exits with status 0.
//!
//! ## The client's notification [V]
//!
//! `machoNet.__BroadcastShutdownInformation` (`carbon/common/script/net/
//! machoNet.py`, lines 2599-2619) broadcasts
//! `OnClusterShutdownInitiated(explanationLabel, when, duration)`; its
//! `GracefulShutdown` (2548-2578) broadcasts
//! `OnClusterShutdownCancelled(explanationLabel)` on a cancel. The client's
//! `ShutdownNotificationAdapter` (`notifications/client/
//! shutdownNotificationAdapter.py`, 18-80) shows a "Cluster Shutdown Alert"
//! notification whose text is `GetByLabel(explanationLabel,
//! intervalRemaining=max(when - now, 500), shutdownTime=FmtDate(when))`, with
//! a downtime suffix only when `duration` is truthy; `gameui` (358-379)
//! stores `when` and, inside its last five minutes, prints "Cluster Shutdown
//! in N" every five seconds by itself. `when` is wallclock FILETIME.
//!
//! The labels are the client's own (`localization_fsd_main`):
//! [`LABEL_UNSCHEDULED`] (280382, takes `intervalRemaining` and
//! `shutdownTime`), [`LABEL_EMERGENCY`] (280383, `shutdownTime` only) and
//! [`LABEL_CANCELLED`] (281154). The notice schedule is CCP's: the first one
//! an hour before (or at once, when less than an hour remains), then 15 and 5
//! minutes before, each one sent when `now + 20 s` has passed its mark
//! (`sortOfNow`, twice the worker's 10 s poll).

use crate::notify::Push;
use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One second in blue (FILETIME) ticks.
const SEC: i64 = 10_000_000;
/// One minute in blue ticks.
const MIN: i64 = 60 * SEC;
/// One hour in blue ticks.
const HOUR: i64 = 60 * MIN;
/// CCP's `sortOfNow` lead: twice the shutdown worker's 10 s poll [V].
const NOTICE_LEAD: i64 = 20 * SEC;

/// The explanation for a countdown: "the servers will be closing in
/// {intervalRemaining} for a short unscheduled reboot at {shutdownTime}" [V].
pub const LABEL_UNSCHEDULED: &str = "/EVE-Universe/ClusterBroadcast/UnscheduledDowntime";
/// The explanation for `/shutdown 0`: "closing immediately for an emergency
/// reboot at {shutdownTime}" [V].
pub const LABEL_EMERGENCY: &str = "/EVE-Universe/ClusterBroadcast/EmergencyDowntime";
/// The explanation for a cancel: "The cluster shutdown has been temporarily
/// cancelled." [V]
pub const LABEL_CANCELLED: &str = "/EVE-Universe/ClusterBroadcast/ShutdownCancelled";

/// `/shutdown` with no seconds. **[A]** a minute: long enough to dock or log
/// off, short enough for an operator at the keyboard. Falsified by players
/// asking for longer by default (then the GM types the seconds).
pub const DEFAULT_SECONDS: u64 = 60;

/// The longest countdown `/shutdown` takes: a day. Past it is a typo.
pub const MAX_SECONDS: u64 = 24 * 3600;

/// How long the binary waits, after hanging up every session, for their
/// connection threads to end (and their disconnect hooks to run) before it
/// flushes anyway. **[A]** five seconds; falsified by a `[shutdown]` line
/// reporting sessions still live after the drain on an idle machine.
pub const SESSION_DRAIN: Duration = Duration::from_secs(5);

/// How long a wake-up connection to our own listener may take. Local, so
/// anything near this is a listener that is not there.
const WAKE_TIMEOUT: Duration = Duration::from_secs(1);

/// The stop handle: one per `Server`, shared by the world ticker (which
/// requests) and the accept loops (which watch). No world lock inside.
#[derive(Debug, Default)]
pub struct Stop {
    requested: AtomicBool,
    why: Mutex<Option<String>>,
    /// Where each listener can be reached from this host.
    wake: Vec<SocketAddr>,
    /// The operator's stop file, if any.
    file: Option<PathBuf>,
}

impl Stop {
    /// A handle for listeners at `listening` (their bound addresses) and an
    /// optional stop file.
    pub fn new(listening: &[SocketAddr], file: Option<PathBuf>) -> Self {
        Stop {
            requested: AtomicBool::new(false),
            why: Mutex::new(None),
            wake: listening.iter().map(|a| wake_addr(*a)).collect(),
            file,
        }
    }

    /// Whether a stop has been asked for.
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Why, once asked.
    pub fn reason(&self) -> Option<String> {
        self.why.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The stop file this handle polls.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Ask for the stop. The first call sets the flag, then wakes every
    /// listener (a connection to itself) so its `accept` returns; later calls
    /// do nothing. `true` for the call that stopped it. Never call it holding
    /// the world lock: it dials.
    pub fn request(&self, why: &str) -> bool {
        if self.requested.swap(true, Ordering::SeqCst) {
            return false;
        }
        *self.why.lock().unwrap_or_else(|e| e.into_inner()) = Some(why.to_owned());
        crate::outln!("[shutdown] stop requested: {why}; no new connection is accepted");
        for addr in &self.wake {
            // The connection itself is the message: the accept loop sees the
            // flag and drops it. A failure only means that listener's thread
            // stays in `accept` -- the process still exits from `main`.
            if let Err(e) = TcpStream::connect_timeout(addr, WAKE_TIMEOUT) {
                crate::errln!("[shutdown] could not wake the listener at {addr}: {e}");
            }
        }
        true
    }

    /// Whether the stop file is there. A stat, with no lock held.
    pub fn file_says_stop(&self) -> bool {
        self.file.as_deref().is_some_and(|p| p.exists())
    }

    /// Remove the stop file, if it is there: a stale one at startup would
    /// stop the server on its first tick, and one left after a stop would
    /// stop the next start.
    pub fn clear_file(&self) {
        if let Some(p) = &self.file
            && p.exists()
        {
            match std::fs::remove_file(p) {
                Ok(()) => crate::outln!("[shutdown] removed the stop file {}", p.display()),
                Err(e) => crate::errln!(
                    "[shutdown] cannot remove the stop file {}: {e}",
                    p.display()
                ),
            }
        }
    }
}

/// Where to dial to reach a listener bound at `bound`: its own address, or
/// loopback for a wildcard.
fn wake_addr(bound: SocketAddr) -> SocketAddr {
    let ip = match bound.ip() {
        IpAddr::V4(v) if v.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v) if v.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, bound.port())
}

/// The default stop file for a `--world FILE`: `FILE.stop`.
pub fn default_stop_file(world: &Path) -> PathBuf {
    let mut s = world.as_os_str().to_owned();
    s.push(".stop");
    PathBuf::from(s)
}

/// A running countdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Countdown {
    /// When the server stops, wallclock FILETIME.
    pub when: i64,
    /// The explanation label the notices carry.
    pub label: &'static str,
    /// The GM's own words, if any.
    pub message: String,
    /// The next notice's mark, or `None` when the last one has gone.
    notify: Option<i64>,
}

/// The world slot: the countdown, and the server's stop handle.
#[derive(Debug, Default)]
pub struct Shutdown {
    countdown: Option<Countdown>,
    stop: Option<Arc<Stop>>,
}

impl Shutdown {
    /// The running countdown, if any.
    pub fn countdown(&self) -> Option<&Countdown> {
        self.countdown.as_ref()
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "shutdown",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Shutdown::default())
}

/// Hand the world its server's stop handle (`Server::bind`). A world with
/// none (a test registry) runs countdowns but has nothing to stop.
pub fn attach(reg: &mut Registry, stop: Arc<Stop>) {
    reg.ext_mut::<Shutdown>().stop = Some(stop);
}

/// `OnClusterShutdownInitiated(explanationLabel, when, duration)` [V], to
/// every character online when it was built. `duration` is `None`: we do
/// not know how long the server stays down, and a falsy duration is the
/// client's "no delay" branch.
#[derive(Debug, Clone)]
pub struct ShutdownInitiated {
    pub label: &'static str,
    pub when: i64,
    pub to: Vec<i64>,
}

impl Push for ShutdownInitiated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnClusterShutdownInitiated",
            vec![bstr(self.label), PyValue::Int(self.when), PyValue::None],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnClusterShutdownCancelled(explanationLabel)` [V].
#[derive(Debug, Clone)]
pub struct ShutdownCancelled {
    pub to: Vec<i64>,
}

impl Push for ShutdownCancelled {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnClusterShutdownCancelled", vec![bstr(LABEL_CANCELLED)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The GM's own words, as `OnRemoteMessage('CustomNotify', {'notify': text})`.
///
/// **[A]** `gameui.OnRemoteMessage(msgID, dict, kw)` (lines 458-466 [V])
/// hands the pair to `prompt_player_fsd_dialog`, and `CustomNotify` is the
/// generic message every refusal of ours already raises; that the pushed
/// form shows the same text is assumed. Falsified by a client that shows
/// nothing, or an error, when a `/shutdown` carries a message.
#[derive(Debug, Clone)]
pub struct ShutdownMessage {
    pub text: String,
    pub to: Vec<i64>,
}

impl Push for ShutdownMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRemoteMessage",
            vec![
                bstr("CustomNotify"),
                crate::pyutil::pydict(vec![("notify", bstr(&self.text))]),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The first notice's mark for a countdown ending at `when`, as CCP's
/// `GracefulShutdown` sets it: now when less than an hour remains, else an
/// hour before.
fn first_notice(now: i64, when: i64) -> i64 {
    if when < now + HOUR { now } else { when - HOUR }
}

/// The mark after a notice sent at `sort_of_now`: 15 minutes before, else 5,
/// else none (CCP's `__BroadcastShutdownInformation`).
fn next_notice(when: i64, sort_of_now: i64) -> Option<i64> {
    if when - sort_of_now > 15 * MIN {
        Some(when - 15 * MIN)
    } else if when - sort_of_now > 5 * MIN {
        Some(when - 5 * MIN)
    } else {
        None
    }
}

/// Start (or replace) a countdown of `seconds` from `now`. The notices go
/// from the clock ([`tick`]); the GM's message, if any, goes now. Returns
/// the console's answer.
pub fn start(reg: &mut Registry, now: i64, seconds: u64, message: &str) -> String {
    let when = now + i64::try_from(seconds).unwrap_or(0).saturating_mul(SEC);
    let label = if seconds == 0 {
        LABEL_EMERGENCY
    } else {
        LABEL_UNSCHEDULED
    };
    let replaced = reg.ext::<Shutdown>().countdown.is_some();
    reg.ext_mut::<Shutdown>().countdown = Some(Countdown {
        when,
        label,
        message: message.to_owned(),
        notify: Some(first_notice(now, when)),
    });
    let to = reg.sessions().online_characters();
    if !message.is_empty() {
        reg.push(ShutdownMessage {
            text: message.to_owned(),
            to: to.clone(),
        });
    }
    crate::outln!(
        "[shutdown] countdown: {seconds} s{}{}",
        if replaced {
            " (replaces the last one)"
        } else {
            ""
        },
        if message.is_empty() {
            String::new()
        } else {
            format!(", message {message:?}")
        }
    );
    format!(
        "shutdown in {seconds} s{}: {} online character(s) are told; the world is \
         written and the server exits when it runs out. /shutdown cancel aborts it.",
        if replaced {
            " (the earlier countdown is replaced)"
        } else {
            ""
        },
        to.len()
    )
}

/// Abort the countdown, telling everyone who was online. The answer says
/// whether there was one.
pub fn cancel(reg: &mut Registry) -> String {
    if reg
        .ext::<Shutdown>()
        .stop
        .as_ref()
        .is_some_and(|s| s.requested())
    {
        return "too late: the server is already stopping".to_owned();
    }
    if reg.ext_mut::<Shutdown>().countdown.take().is_none() {
        return "no shutdown countdown is running".to_owned();
    }
    let to = reg.sessions().online_characters();
    reg.push(ShutdownCancelled { to });
    crate::outln!("[shutdown] countdown cancelled");
    "shutdown cancelled; everyone online is told".to_owned()
}

/// What the clock should do after this tick, decided under the world lock and
/// acted on after it ([`Stop::request`] dials).
#[derive(Debug, Default)]
pub struct Due {
    /// The server's stop handle, if the world has one.
    pub stop: Option<Arc<Stop>>,
    /// Why to stop now, when the countdown ran out on this tick.
    pub fire: Option<String>,
}

impl Due {
    /// Act on it, with no world lock held: stop when the countdown ran out
    /// or the stop file is there. Polled every tick; a stat is cheap.
    pub fn act(self) {
        let Some(stop) = self.stop else { return };
        if stop.requested() {
            return;
        }
        if let Some(why) = self.fire {
            stop.request(&why);
        } else if stop.file_says_stop() {
            let why = format!(
                "stop file {}",
                stop.file()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
            stop.request(&why);
        }
    }
}

/// The clock's part, under the world lock: send a notice whose mark has
/// come, and end a countdown whose time has. Fires **once**: the countdown
/// is gone after it.
pub fn tick(reg: &mut Registry, now: i64) -> Due {
    let slot = reg.ext_mut::<Shutdown>();
    let stop = slot.stop.clone();
    let Some(c) = slot.countdown.as_mut() else {
        return Due { stop, fire: None };
    };
    let sort_of_now = now + NOTICE_LEAD;
    let mut notice = None;
    if let Some(mark) = c.notify
        && sort_of_now > mark
    {
        c.notify = next_notice(c.when, sort_of_now);
        notice = Some((c.label, c.when));
    }
    let fire = (c.notify.is_none() && c.when <= now).then(|| {
        if c.message.is_empty() {
            "/shutdown countdown ended".to_owned()
        } else {
            format!("/shutdown countdown ended ({})", c.message)
        }
    });
    if fire.is_some() {
        slot.countdown = None;
    }
    if let Some((label, when)) = notice {
        let to = reg.sessions().online_characters();
        crate::outln!(
            "[shutdown] notice to {} character(s): {} s left",
            to.len(),
            ((when - now) / SEC).max(0)
        );
        reg.schedule(Duration::ZERO, ShutdownInitiated { label, when, to });
    }
    Due { stop, fire }
}

/// `/shutdown [seconds] [message]` and `/shutdown cancel`.
///
/// The GM gate is the slash console's own (`service/gm.rs`, `ROLE_GML`).
/// A first word that is not a number or `cancel` starts the default
/// countdown with every word as the message.
pub(crate) fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(&slash_text(reg, crate::pyutil::filetime_now(), args)))
}

/// What `/shutdown` answers at `now`.
pub fn slash_text(reg: &mut Registry, now: i64, args: &[&str]) -> String {
    match args {
        ["cancel", ..] => cancel(reg),
        ["status"] => match reg.ext::<Shutdown>().countdown() {
            Some(c) => format!("shutdown in {} s", ((c.when - now) / SEC).max(0)),
            None => "no shutdown countdown is running".to_owned(),
        },
        [first, rest @ ..] => match first.parse::<u64>() {
            Ok(s) if s > MAX_SECONDS => {
                format!("at most {MAX_SECONDS} s (a day); /shutdown cancel aborts")
            }
            Ok(s) => start(reg, now, s, &rest.join(" ")),
            Err(_) => start(reg, now, DEFAULT_SECONDS, &args.join(" ")),
        },
        [] => start(reg, now, DEFAULT_SECONDS, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    /// An invented wallclock, far from any real one.
    const NOW: i64 = 1_000 * HOUR;

    fn world() -> Registry {
        Registry::new(SandboxConfig::default())
    }

    fn notices(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_due(std::time::Instant::now() + Duration::from_secs(1))
            .iter()
            .map(|p| p.to_push())
            .collect()
    }

    #[test]
    fn a_countdown_notifies_then_fires_once() {
        let mut reg = world();
        let stop = Arc::new(Stop::new(&[], None));
        attach(&mut reg, Arc::clone(&stop));
        slash_text(&mut reg, NOW, &["30", "back", "soon"]);
        let due = tick(&mut reg, NOW);
        assert!(due.fire.is_none(), "not yet");
        let sent = notices(&mut reg);
        assert_eq!(sent.len(), 1, "one notice at the start: {sent:?}");
        assert_eq!(sent[0].0, "OnClusterShutdownInitiated");
        assert_eq!(
            sent[0].1,
            vec![
                bstr(LABEL_UNSCHEDULED),
                PyValue::Int(NOW + 30 * SEC),
                PyValue::None
            ]
        );
        // No second notice inside the last five minutes: the client counts.
        assert!(tick(&mut reg, NOW + 10 * SEC).fire.is_none());
        assert!(notices(&mut reg).is_empty());
        let due = tick(&mut reg, NOW + 30 * SEC);
        assert!(due.fire.as_deref().is_some_and(|w| w.contains("back soon")));
        due.act();
        assert!(stop.requested(), "the countdown stopped the server");
        assert!(
            tick(&mut reg, NOW + 31 * SEC).fire.is_none(),
            "it fires once"
        );
    }

    #[test]
    fn a_long_countdown_follows_ccps_marks() {
        let mut reg = world();
        slash_text(&mut reg, NOW, &["7200"]);
        let when = NOW + 2 * HOUR;
        let mut sent = Vec::new();
        let mut t = NOW;
        while t <= when {
            let due = tick(&mut reg, t);
            if !notices(&mut reg).is_empty() {
                sent.push((when - t) / MIN);
            }
            if due.fire.is_some() {
                break;
            }
            t += 10 * SEC;
        }
        // An hour before (20 s early), then 15 and 5 minutes before.
        assert_eq!(sent, vec![60, 15, 5], "minutes left at each notice");
    }

    #[test]
    fn cancel_aborts_and_tells_everyone() {
        let mut reg = world();
        let stop = Arc::new(Stop::new(&[], None));
        attach(&mut reg, Arc::clone(&stop));
        slash_text(&mut reg, NOW, &["10"]);
        let answer = slash_text(&mut reg, NOW, &["cancel"]);
        assert!(answer.contains("cancelled"), "{answer}");
        let pushes: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push())
            .collect();
        assert_eq!(
            pushes,
            vec![("OnClusterShutdownCancelled", vec![bstr(LABEL_CANCELLED)])]
        );
        let due = tick(&mut reg, NOW + 60 * SEC);
        assert!(due.fire.is_none());
        due.act();
        assert!(!stop.requested(), "a cancelled countdown stops nothing");
        assert!(slash_text(&mut reg, NOW, &["cancel"]).contains("no shutdown"));
    }

    #[test]
    fn zero_seconds_is_the_emergency_label_and_fires_on_the_next_tick() {
        let mut reg = world();
        slash_text(&mut reg, NOW, &["0"]);
        let due = tick(&mut reg, NOW);
        assert!(due.fire.is_some());
        let sent = notices(&mut reg);
        assert_eq!(sent[0].1[0], bstr(LABEL_EMERGENCY));
    }

    #[test]
    fn the_arguments_parse() {
        let mut reg = world();
        assert!(slash_text(&mut reg, NOW, &["status"]).contains("no shutdown"));
        assert!(slash_text(&mut reg, NOW, &["99999999"]).starts_with("at most"));
        assert!(reg.ext::<Shutdown>().countdown().is_none());
        slash_text(&mut reg, NOW, &["patch", "time"]);
        let c = reg.ext::<Shutdown>().countdown().cloned().expect("started");
        assert_eq!(
            c.when,
            NOW + i64::try_from(DEFAULT_SECONDS).expect("small") * SEC
        );
        assert_eq!(c.message, "patch time");
        assert!(slash_text(&mut reg, NOW, &["status"]).contains(&format!("{DEFAULT_SECONDS} s")));
        let pushes: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert_eq!(pushes, vec!["OnRemoteMessage"], "the message goes at once");
    }

    #[test]
    fn the_stop_file_stops_through_the_same_handle() {
        let dir = std::env::temp_dir().join(format!("eve-stopfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let file = default_stop_file(&dir.join("world.dat"));
        assert_eq!(file, dir.join("world.dat.stop"));
        let mut reg = world();
        let stop = Arc::new(Stop::new(&[], Some(file.clone())));
        attach(&mut reg, Arc::clone(&stop));
        tick(&mut reg, NOW).act();
        assert!(!stop.requested());
        std::fs::write(&file, b"").expect("touch");
        tick(&mut reg, NOW).act();
        assert!(stop.requested());
        assert!(stop.reason().is_some_and(|r| r.contains("stop file")));
        stop.clear_file();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wildcard_listener_is_woken_on_loopback() {
        let any: SocketAddr = "0.0.0.0:26000".parse().expect("literal");
        assert_eq!(wake_addr(any), "127.0.0.1:26000".parse().expect("literal"));
        let own: SocketAddr = "10.137.0.1:26000".parse().expect("literal");
        assert_eq!(wake_addr(own), own);
    }
}
