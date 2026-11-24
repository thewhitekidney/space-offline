//! **The one clock seam** (#145 FF-0a). Every time source the server reads
//! goes through a function here, so a later mode (an affine or lockstep
//! fast-forward, #147/#148) can change what "now" means in one place.
//!
//! **Only [`Mode::Wall`] exists today.** In Wall mode each function returns
//! exactly what the raw call it replaced returned: this module is a rename,
//! not a behaviour change. The other modes can be named but not installed
//! ([`SEAM_COMPLETE`] is `false`), so nothing can run under one by accident.
//!
//! ## Which function
//!
//! - **Game time** (a deadline, a cooldown, a cycle, a due time that a
//!   faster clock should shorten): [`instant`], [`since`], [`system_now`],
//!   [`unix_secs`], [`sleep`].
//! - **Real time** (a network deadline, a keepalive, a timing measurement, a
//!   log stamp, a save cadence: things that must stay real under a faster
//!   clock): [`wall`], [`wall_since`].
//! - **Entropy and ids** (a seed, a uuid: not a duration at all):
//!   [`wall_system`].
//! - **A socket timeout** (`set_read_timeout` and friends): wrap the
//!   duration in [`real_timeout`], or, for a kept-real site, list it in the
//!   lint table below.
//!
//! ## The lint
//!
//! `tests::every_time_source_is_sanctioned` counts every raw time source in
//! `src/` (production code only: `#[cfg(test)]`/`#[test]` items and
//! test-only files are not counted) and compares each file with
//! [`SANCTIONED`]. A new raw call fails it; the message names the file.
//!
//! ## Where it lives
//!
//! `game_clock.rs` hosts this file with `#[path = "clock.rs"] pub mod clock;`
//! (Option P of `specs/ff-now.md` §0), so no `lib.rs` line was needed. Code
//! writes `crate::game_clock::clock::..`. After the maintainer adds
//! `pub mod clock;` to `lib.rs`, the `#[path]` line goes and the paths
//! shorten.
//!
//! Lane note: `docs/lanes/clock.md`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "clock/marker.rs"]
pub mod marker;
#[path = "clock/sim.rs"]
pub mod sim;

/// Whether every game-time site reads this module, so that a mode other than
/// Wall may be installed. The hot-file sites (`push.rs`, `service.rs`,
/// `dispatch.rs`, `conn.rs`, `server.rs`, `store.rs`) are not converted yet
/// (see the pending block of [`SANCTIONED`]), so this stays `false`.
pub const SEAM_COMPLETE: bool = false;

/// How the clock runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The real clock. The only mode that can be installed today.
    Wall,
    /// Game time runs `k` times faster than the wall (#147). Not installable.
    Affine {
        /// The speed-up factor.
        k: u32,
    },
    /// Game time advances only when every actor waits (#148). Not installable.
    Lockstep,
}

/// One clock: its mode and whether anything has read it yet. The process has
/// one ([`GLOBAL`]); a test builds its own so it never changes the process's.
pub struct Slot {
    mode: Mutex<Mode>,
    /// Set by the first read. `install` is refused after it, because a
    /// reader that already saw the old mode would see time jump.
    read: AtomicBool,
    installed: AtomicBool,
    /// The Affine origin, set once by an Affine install: `(inst0, epoch, k)`.
    /// Unset means every reader answers the wall clock (Wall, and for now
    /// Lockstep). A `OnceLock` read takes no lock on the hot path.
    affine: OnceLock<Origin>,
}

/// The fixed point of an Affine clock (#147): the real `Instant` it started
/// at, the Unix time that instant stands for, and the speed-up factor.
#[derive(Debug, Clone, Copy)]
struct Origin {
    inst0: Instant,
    epoch: Duration,
    k: u32,
}

/// The pure Affine maths: game `(Instant, SystemTime)` at real time
/// `real_now`, for a clock that was at `(inst0, UNIX_EPOCH + epoch)` at
/// `inst0` and runs `k` times faster than the wall. `real_now` before `inst0`
/// counts as `inst0`. Pure so a test passes `real_now` explicitly.
pub fn affine_at(
    inst0: Instant,
    epoch: Duration,
    k: u32,
    real_now: Instant,
) -> (Instant, SystemTime) {
    let virt = real_now.saturating_duration_since(inst0).saturating_mul(k);
    (inst0 + virt, UNIX_EPOCH + epoch + virt)
}

/// The real wait for a game duration `d` under factor `k`: `d / k`, never
/// under 1 ms (a zero socket timeout means "block forever").
pub fn affine_real_timeout(d: Duration, k: u32) -> Duration {
    (d / k.max(1)).max(Duration::from_millis(1))
}

impl Slot {
    /// A fresh slot in Wall mode, never read, nothing installed.
    pub const fn new() -> Self {
        Slot {
            mode: Mutex::new(Mode::Wall),
            read: AtomicBool::new(false),
            installed: AtomicBool::new(false),
            affine: OnceLock::new(),
        }
    }

    /// Choose the mode. Refused after a read, on a second call, and for any
    /// mode but Wall while [`SEAM_COMPLETE`] is `false`.
    pub fn install(&self, mode: Mode) -> Result<(), String> {
        self.install_gated(mode, SEAM_COMPLETE)
    }

    /// [`Slot::install`] with the seam flag as a parameter, so a test can
    /// exercise the Affine path while [`SEAM_COMPLETE`] is `false`.
    fn install_gated(&self, mode: Mode, seam_complete: bool) -> Result<(), String> {
        if mode != Mode::Wall && !seam_complete {
            return Err(format!(
                "clock mode {mode:?} refused: not every time source reads the clock seam yet"
            ));
        }
        if matches!(mode, Mode::Affine { k: 0 }) {
            return Err("clock mode Affine needs k >= 1".to_owned());
        }
        if self.read.load(Ordering::Relaxed) {
            return Err("the clock was already read; install it before anything runs".to_owned());
        }
        if self.installed.swap(true, Ordering::Relaxed) {
            return Err("the clock is already installed".to_owned());
        }
        if let Mode::Affine { k } = mode {
            // The origin pair is read once, here: the game clock starts at
            // the wall clock and runs k times faster from this instant.
            let epoch = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let _ = self.affine.set(Origin {
                inst0: Instant::now(),
                epoch,
                k,
            });
        }
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        Ok(())
    }

    /// The installed mode.
    pub fn mode(&self) -> Mode {
        self.touch();
        *self.mode.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Note a read (a Relaxed load, and a store only the first time).
    fn touch(&self) {
        if !self.read.load(Ordering::Relaxed) {
            self.read.store(true, Ordering::Relaxed);
        }
    }

    /// Game-time `Instant::now()`.
    pub fn instant(&self) -> Instant {
        self.touch();
        match self.affine.get() {
            None => Instant::now(),
            Some(o) => affine_at(o.inst0, o.epoch, o.k, Instant::now()).0,
        }
    }

    /// Game time since `earlier` (saturating, as `Instant::elapsed` is).
    pub fn since(&self, earlier: Instant) -> Duration {
        self.instant().saturating_duration_since(earlier)
    }

    /// Game-time `SystemTime::now()`. Under `cfg(test)` a thread's frozen
    /// test clock (`test_clock`) wins, so the PA-8 golden tests keep working.
    pub fn system_now(&self) -> SystemTime {
        self.touch();
        if let Some(o) = self.affine.get() {
            return affine_at(o.inst0, o.epoch, o.k, Instant::now()).1;
        }
        #[cfg(test)]
        let t = crate::test_clock::system_now();
        #[cfg(not(test))]
        let t = SystemTime::now();
        t
    }

    /// Whole seconds since the Unix epoch, 0 before it. Not frozen by
    /// `test_clock` (the sites it replaced never were).
    pub fn unix_secs(&self) -> u64 {
        self.touch();
        if let Some(o) = self.affine.get() {
            return affine_at(o.inst0, o.epoch, o.k, Instant::now())
                .1
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// Game-time `thread::sleep`.
    pub fn sleep(&self, d: Duration) {
        self.touch();
        match self.affine.get() {
            None => std::thread::sleep(d),
            Some(o) => std::thread::sleep(d / o.k),
        }
    }

    /// The real duration to wait for a game duration `d` (a socket timeout).
    pub fn real_timeout(&self, d: Duration) -> Duration {
        self.touch();
        match self.affine.get() {
            None => d,
            Some(o) => affine_real_timeout(d, o.k),
        }
    }

    /// Real-time `Instant::now()`.
    pub fn wall(&self) -> Instant {
        self.touch();
        Instant::now()
    }

    /// Real time since `earlier` (saturating, as `Instant::elapsed` is).
    pub fn wall_since(&self, earlier: Instant) -> Duration {
        self.wall().saturating_duration_since(earlier)
    }

    /// Real-time `SystemTime::now()`, for seeds and ids.
    pub fn wall_system(&self) -> SystemTime {
        self.touch();
        SystemTime::now()
    }

    /// Whole seconds of the real clock since the Unix epoch (0 before it).
    /// For stamps another process reads against the wall, such as
    /// `bubble.tsv` (#147 H1).
    pub fn wall_unix_secs(&self) -> u64 {
        self.wall_system()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL: Slot = Slot::new();

/// Install the process's clock mode. See [`Slot::install`].
pub fn install(mode: Mode) -> Result<(), String> {
    GLOBAL.install(mode)
}

/// The process's clock mode.
pub fn mode() -> Mode {
    GLOBAL.mode()
}

/// Game-time `Instant::now()`.
pub fn instant() -> Instant {
    GLOBAL.instant()
}

/// Game time since `earlier`.
pub fn since(earlier: Instant) -> Duration {
    GLOBAL.since(earlier)
}

/// Game-time `SystemTime::now()`.
pub fn system_now() -> SystemTime {
    GLOBAL.system_now()
}

/// Whole seconds since the Unix epoch (0 before it).
pub fn unix_secs() -> u64 {
    GLOBAL.unix_secs()
}

/// Game-time sleep.
pub fn sleep(d: Duration) {
    GLOBAL.sleep(d);
}

/// The real wait for a game duration (a socket timeout).
pub fn real_timeout(d: Duration) -> Duration {
    GLOBAL.real_timeout(d)
}

/// Real-time `Instant::now()`.
pub fn wall() -> Instant {
    GLOBAL.wall()
}

/// Real time since `earlier`.
pub fn wall_since(earlier: Instant) -> Duration {
    GLOBAL.wall_since(earlier)
}

/// Real-time `SystemTime::now()`, for seeds and ids.
pub fn wall_system() -> SystemTime {
    GLOBAL.wall_system()
}

/// Whole seconds of the real clock since the Unix epoch.
pub fn wall_unix_secs() -> u64 {
    GLOBAL.wall_unix_secs()
}

/// The raw time sources the lint counts.
pub const PATTERNS: &[&str] = &[
    "Instant::now",
    "SystemTime::now",
    ".elapsed(",
    "thread::sleep(",
    "set_read_timeout(",
    "set_write_timeout(",
    "park_timeout(",
    "recv_timeout(",
    "wait_timeout",
    "connect_timeout(",
    "Instant as ",
    "SystemTime as ",
    USE_SLEEP,
];

/// Synthetic pattern: a `use` that imports `thread::sleep`, plain, aliased or
/// inside a group (`use std::thread::{self, sleep}`), so a bare `sleep(d)`
/// cannot hide from `thread::sleep(`. Known limit: a glob import
/// (`use std::thread::*`) and a renamed re-export from another module are not seen.
pub const USE_SLEEP: &str = "use thread::sleep";

/// Every raw time source left in production code, per file and pattern:
/// `(file under src/, pattern, exact count, why)`. A file or pattern not
/// listed must count 0. Counted per file, not per line, so an edit that only
/// shifts lines does not trip it. `clock.rs` itself is exempt.
pub const SANCTIONED: &[(&str, &str, usize, &str)] = &[
    (
        "admin_web.rs",
        "recv_timeout(",
        1,
        "kept real: the HTTP handler waits for the clock thread to answer an admin action (a client-facing deadline)",
    ),
    (
        "admin_web.rs",
        "set_read_timeout(",
        1,
        "kept real: the admin HTTP socket read budget",
    ),
    (
        "admin_web.rs",
        "set_write_timeout(",
        1,
        "kept real: the admin HTTP socket write budget",
    ),
    (
        "bot_control.rs",
        "set_read_timeout(",
        2,
        "kept real: bot-control socket timeouts",
    ),
    (
        "chat_dir.rs",
        "wait_timeout",
        1,
        "kept real: the reconcile thread's fallback period (a publish wakes it)",
    ),
    (
        "cli.rs",
        "connect_timeout(",
        1,
        "kept real: a socket connect limit",
    ),
    (
        "images.rs",
        "set_read_timeout(",
        1,
        "kept real: the portrait image server's HTTP socket read budget",
    ),
    (
        "images.rs",
        "set_write_timeout(",
        1,
        "kept real: the portrait image server's HTTP socket write budget",
    ),
    (
        "shutdown.rs",
        "connect_timeout(",
        1,
        "kept real: the shutdown wake-up connect",
    ),
    (
        "chat.rs",
        "set_write_timeout(",
        1,
        "kept real: the per-socket write budget",
    ),
    (
        "chat.rs",
        "wait_timeout",
        1,
        "kept real: the log-off write deadline",
    ),
    (
        "cli.rs",
        "set_read_timeout(",
        1,
        "kept real: the probe's socket timeout",
    ),
    ("conn.rs", ".elapsed(", 7, "pending #145 hot-ok: hot file"),
    (
        "conn.rs",
        "Instant::now",
        4,
        "pending #145 hot-ok: hot file",
    ),
    (
        "conn.rs",
        "set_read_timeout(",
        2,
        "pending #145 hot-ok: hot file",
    ),
    (
        "conn.rs",
        "set_write_timeout(",
        1,
        "pending #145 hot-ok: hot file",
    ),
    (
        "dispatch.rs",
        ".elapsed(",
        7,
        "pending #145 hot-ok: hot file",
    ),
    (
        "dispatch.rs",
        "Instant::now",
        10,
        "pending #145 hot-ok: hot file",
    ),
    (
        "gateway.rs",
        "set_read_timeout(",
        2,
        "kept real: the idle read timeouts of a gateway connection (before and after its first served request)",
    ),
    (
        "gateway.rs",
        "set_write_timeout(",
        1,
        "kept real: the write deadline of a gateway connection",
    ),
    (
        "gateway.rs",
        "thread::sleep(",
        1,
        "kept real: accept-error back-off",
    ),
    (
        "logline.rs",
        "recv_timeout(",
        1,
        "kept real: the log writer's flush wait",
    ),
    (
        "logline.rs",
        "thread::sleep(",
        1,
        "kept real: the log writer's flush poll",
    ),
    (
        "push.rs",
        "wait_timeout",
        1,
        "pending #145 hot-ok: hot file; the writer wait at push.rs:2006, for #147/#148",
    ),
    ("push.rs", ".elapsed(", 16, "pending #145 hot-ok: hot file"),
    (
        "push.rs",
        "Instant::now",
        29,
        "pending #145 hot-ok: hot file",
    ),
    (
        "push.rs",
        "SystemTime::now",
        2,
        "pending #145 hot-ok: hot file; one is the bubble.tsv stamp (push.rs:2625), which must stay wall time (clock::wall_unix_secs) because eve-bot bubble.rs reads it against the wall (#147 H1), so the hot batch does not convert it to game time",
    ),
    (
        "push.rs",
        "park_timeout(",
        1,
        "pending #145 hot-ok: hot file",
    ),
    (
        "push.rs",
        "set_write_timeout(",
        2,
        "pending #145 hot-ok: hot file",
    ),
    (
        "push.rs",
        "thread::sleep(",
        1,
        "pending #145 hot-ok: hot file",
    ),
    (
        "server.rs",
        "Instant::now",
        3,
        "pending #145 hot-ok: hot file",
    ),
    (
        "server.rs",
        "SystemTime::now",
        2,
        "pending #145 hot-ok: hot file",
    ),
    (
        "server.rs",
        "set_write_timeout(",
        1,
        "pending #145 hot-ok: hot file",
    ),
    (
        "server.rs",
        "thread::sleep(",
        2,
        "pending #145 hot-ok: hot file",
    ),
    (
        "service.rs",
        ".elapsed(",
        4,
        "pending #145 hot-ok: hot file",
    ),
    (
        "service.rs",
        "Instant::now",
        7,
        "pending #145 hot-ok: hot file",
    ),
    ("store.rs", ".elapsed(", 4, "pending #145 hot-ok: hot file"),
    (
        "store.rs",
        "Instant::now",
        4,
        "pending #145 hot-ok: hot file",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn is_ident(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    /// `src` with comments and the insides of string and char literals
    /// blanked (newlines kept), so braces and patterns are only seen in code.
    fn blank(src: &str) -> Vec<u8> {
        let c = src.as_bytes();
        let n = c.len();
        let mut o = c.to_vec();
        let wipe = |o: &mut Vec<u8>, a: usize, b: usize| {
            for x in &mut o[a..b.min(n)] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        };
        let mut i = 0;
        while i < n {
            match c[i] {
                b'/' if c.get(i + 1) == Some(&b'/') => {
                    let s = i;
                    while i < n && c[i] != b'\n' {
                        i += 1;
                    }
                    wipe(&mut o, s, i);
                }
                b'/' if c.get(i + 1) == Some(&b'*') => {
                    let s = i;
                    let mut depth = 0;
                    while i < n {
                        if c[i] == b'/' && c.get(i + 1) == Some(&b'*') {
                            depth += 1;
                            i += 2;
                        } else if c[i] == b'*' && c.get(i + 1) == Some(&b'/') {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            i += 1;
                        }
                    }
                    wipe(&mut o, s, i);
                }
                b'"' => {
                    let s = i;
                    i += 1;
                    while i < n && c[i] != b'"' {
                        i += if c[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                    wipe(&mut o, s, i);
                }
                b'r' | b'b' if raw_start(c, i).is_some() => {
                    let (hashes, body) = raw_start(c, i).unwrap_or((0, i + 1));
                    let mut j = body;
                    while j < n {
                        if c[j] == b'"' && (0..hashes).all(|h| c.get(j + 1 + h) == Some(&b'#')) {
                            j += 1 + hashes;
                            break;
                        }
                        j += 1;
                    }
                    wipe(&mut o, i, j);
                    i = j;
                }
                b'\'' => {
                    let lit_end = if c.get(i + 1) == Some(&b'\\') {
                        let mut j = i + 3;
                        while j < n && c[j] != b'\'' {
                            j += 1;
                        }
                        Some(j + 1)
                    } else if c.get(i + 1).is_some_and(|b| *b >= 0x80) {
                        let mut j = i + 2;
                        while j < n && c[j] != b'\'' {
                            j += 1;
                        }
                        Some(j + 1)
                    } else if c.get(i + 2) == Some(&b'\'') {
                        Some(i + 3)
                    } else {
                        None
                    };
                    match lit_end {
                        Some(e) => {
                            wipe(&mut o, i, e);
                            i = e;
                        }
                        None => i += 1,
                    }
                }
                _ => i += 1,
            }
        }
        o
    }

    /// If a raw string literal starts at `i` (`r"`, `r#"`, `br"`): its hash
    /// count and the index after the opening quote.
    fn raw_start(c: &[u8], i: usize) -> Option<(usize, usize)> {
        if i > 0 && is_ident(c[i - 1]) {
            return None;
        }
        let mut j = i;
        if c[j] == b'b' {
            j += 1;
        }
        if c.get(j) != Some(&b'r') {
            return None;
        }
        j += 1;
        let mut hashes = 0;
        while c.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
        (c.get(j) == Some(&b'"')).then_some((hashes, j + 1))
    }

    fn starts_with_at(c: &[u8], at: usize, pat: &str) -> bool {
        c.get(at..at + pat.len()) == Some(pat.as_bytes())
    }

    /// Blank every `#[cfg(test)]` / `#[test]` item in `c` (an item ends at a
    /// `;` or at the `}` matching its first `{`). Returns the names of
    /// out-of-line test modules (`#[cfg(test)] mod x;`).
    fn strip_tests(c: &mut [u8]) -> Vec<String> {
        let mut out_of_line = Vec::new();
        let mut pos = 0;
        while pos < c.len() {
            let Some(at) = ["#[cfg(test)]", "#[test]"]
                .iter()
                .filter_map(|a| {
                    (pos..c.len())
                        .find(|&k| starts_with_at(c, k, a))
                        .map(|k| (k, a.len()))
                })
                .min()
            else {
                break;
            };
            let (start, alen) = at;
            let mut q = start + alen;
            // further attributes on the same item
            loop {
                while q < c.len() && c[q].is_ascii_whitespace() {
                    q += 1;
                }
                if starts_with_at(c, q, "#[") {
                    let mut d = 0;
                    while q < c.len() {
                        match c[q] {
                            b'[' => d += 1,
                            b']' => {
                                d -= 1;
                                if d == 0 {
                                    q += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        q += 1;
                    }
                } else {
                    break;
                }
            }
            let item = q;
            // Items whose head may hold a top-level `,` (generics); anything
            // else (a field, a variant, a match arm) ends at `,` or at the
            // parent's closing `}`.
            let rest = String::from_utf8_lossy(&c[item..(item + 40).min(c.len())]).into_owned();
            let rest = strip_vis(rest.trim_start());
            let is_item = [
                "fn ",
                "mod ",
                "struct ",
                "enum ",
                "impl",
                "trait ",
                "use ",
                "const ",
                "static ",
                "type ",
                "unsafe ",
                "async ",
                "extern ",
                "macro_rules!",
            ]
            .iter()
            .any(|k| rest.starts_with(k));
            let mut depth = 0i32;
            let mut end = c.len();
            // a `=>` at depth 0 means a match arm, whose `|` and `>` are
            // operators, not the end of a parameter. A bare `=` is not
            // enough: it also appears in `<Item = u8>` and `<T = u8>`.
            // [A] only `=>` marks an arm; a test-only `let x = |a| ..;`
            // now over-counts, which is the loud direction.
            let mut seen_eq = false;
            while q < c.len() {
                match c[q] {
                    b'=' if depth <= 0 && c.get(q + 1) == Some(&b'>') => seen_eq = true,
                    b'|' if depth <= 0 && !is_item && !seen_eq => {
                        // the closing bar of a closure's parameters
                        // (`|#[cfg(test)] a: u8| { .. }`): the body is not ours
                        end = q;
                        break;
                    }
                    b'>' if depth <= 0
                        && !is_item
                        && !seen_eq
                        && !matches!(c[q - 1], b'=' | b'-') =>
                    {
                        // the closing bracket of a generic list
                        // (`fn f<#[cfg(test)] T>() { .. }`)
                        end = q;
                        break;
                    }
                    b'(' | b'[' => depth += 1,
                    b')' | b']' if depth <= 0 => {
                        // an unmatched closer (the last parameter of a
                        // signature, `fn f(a: u8, #[cfg(test)] b: u8)`)
                        // ends the item: not ours to blank
                        end = q;
                        break;
                    }
                    b')' | b']' => depth -= 1,
                    b';' if depth <= 0 => {
                        end = q + 1;
                        break;
                    }
                    b',' if depth <= 0 && !is_item => {
                        end = q + 1;
                        break;
                    }
                    b'}' if depth <= 0 => {
                        // the parent's closing brace: not ours to blank
                        end = q;
                        break;
                    }
                    b'{' if depth <= 0 => {
                        let mut d = 0;
                        while q < c.len() {
                            match c[q] {
                                b'{' => d += 1,
                                b'}' => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            q += 1;
                        }
                        end = (q + 1).min(c.len());
                        break;
                    }
                    _ => {}
                }
                q += 1;
            }
            let head = String::from_utf8_lossy(&c[item..end.min(item + 80)]).into_owned();
            let decl = strip_vis(&head)
                .strip_prefix("mod ")
                .and_then(|rest| rest.split_once(';'))
                .map(|(name, _)| name.trim());
            if let Some(name) = decl.filter(|n| !n.is_empty() && n.bytes().all(is_ident)) {
                out_of_line.push(name.to_owned());
            }
            for x in &mut c[start..end] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
            pos = end;
        }
        out_of_line
    }

    /// `s` without a leading visibility (`pub `, `pub(crate) `,
    /// `pub(super) `, `pub(in a::b) `).
    fn strip_vis(s: &str) -> &str {
        let s = s.trim_start();
        let Some(r) = s.strip_prefix("pub") else {
            return s;
        };
        if let Some(r) = r.strip_prefix('(') {
            return r.split_once(')').map_or(s, |(_, t)| t.trim_start());
        }
        if r.starts_with(char::is_whitespace) {
            return r.trim_start();
        }
        s
    }

    fn count(code: &str, pat: &str) -> usize {
        if pat == USE_SLEEP {
            // every `use ` at a token boundary (after a block, inside a fn
            // body, after `pub`), up to its `;`
            let word = |ch: char| ch.is_alphanumeric() || ch == '_';
            return code
                .match_indices("use")
                .filter(|(i, _)| code[i + 3..].starts_with(char::is_whitespace))
                .filter(|(i, _)| !code[..*i].ends_with(word))
                .filter(|(i, _)| {
                    let t = code[*i..].split(';').next().unwrap_or("");
                    t.contains("thread") && t.split(|ch: char| !word(ch)).any(|w| w == "sleep")
                })
                .count();
        }
        code.matches(pat).count()
    }

    /// Where an out-of-line module `name` declared in `file` lives.
    fn child_dir(file: &str) -> String {
        let p = Path::new(file);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let parent = p.parent().and_then(|d| d.to_str()).unwrap_or("");
        let join = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_owned()
            } else {
                format!("{a}/{b}")
            }
        };
        if matches!(stem, "lib" | "main" | "mod") {
            parent.to_owned()
        } else {
            join(parent, stem)
        }
    }

    /// The production-code counts: `file -> pattern -> n`, for the given
    /// `(path under src, source)` pairs. Files that only compile under
    /// `cfg(test)` count nothing.
    fn production_counts(
        files: &[(String, String)],
    ) -> BTreeMap<String, BTreeMap<&'static str, usize>> {
        let mut code: BTreeMap<&str, String> = BTreeMap::new();
        let mut test_only: BTreeSet<String> = BTreeSet::new();
        for (path, src) in files {
            let mut b = blank(src);
            for m in strip_tests(&mut b) {
                let dir = child_dir(path);
                let base = if dir.is_empty() {
                    m
                } else {
                    format!("{dir}/{m}")
                };
                test_only.insert(format!("{base}.rs"));
                test_only.insert(format!("{base}/mod.rs"));
            }
            code.insert(path, String::from_utf8_lossy(&b).into_owned());
        }
        let skip_dirs: Vec<String> = test_only
            .iter()
            .filter(|p| code.contains_key(p.as_str()))
            .map(|p| child_dir(p))
            .collect();
        let mut out = BTreeMap::new();
        for (path, text) in &code {
            let skipped = test_only.contains(*path)
                || skip_dirs
                    .iter()
                    .any(|d| !d.is_empty() && path.starts_with(&format!("{d}/")));
            if skipped || *path == "clock.rs" || path.starts_with("clock/") {
                continue;
            }
            let row: BTreeMap<&'static str, usize> = PATTERNS
                .iter()
                .map(|p| (*p, count(text, p)))
                .filter(|(_, n)| *n > 0)
                .collect();
            if !row.is_empty() {
                out.insert((*path).to_owned(), row);
            }
        }
        out
    }

    /// The lint: every difference between `files` and `sanctioned`.
    fn problems(
        files: &[(String, String)],
        sanctioned: &[(&str, &str, usize, &str)],
    ) -> Vec<String> {
        let found = production_counts(files);
        let mut want: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for (f, p, n, _) in sanctioned {
            want.insert((*f, *p), *n);
        }
        let mut bad = Vec::new();
        for (f, row) in &found {
            for (p, n) in row {
                let w = want.get(&(f.as_str(), *p)).copied().unwrap_or(0);
                if *n != w {
                    bad.push(format!(
                        "{f}: `{p}` found {n}, sanctioned {w}: use clock::instant() for game time or clock::wall() for real time; see clock.rs"
                    ));
                }
            }
        }
        for ((f, p), w) in &want {
            let n = found.get(*f).and_then(|r| r.get(p)).copied().unwrap_or(0);
            if n == 0 && *w > 0 {
                bad.push(format!(
                    "{f}: `{p}` found 0, sanctioned {w}: lower or drop its SANCTIONED row in clock.rs"
                ));
            }
        }
        bad
    }

    fn read_src(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                read_src(&p, root, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let rel = p
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if let Ok(s) = std::fs::read_to_string(&p) {
                    out.push((rel, s));
                }
            }
        }
    }

    fn crate_sources() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut v = Vec::new();
        read_src(&root, &root, &mut v);
        v
    }

    #[test]
    fn every_time_source_is_sanctioned() {
        let bad = problems(&crate_sources(), SANCTIONED);
        assert!(bad.is_empty(), "raw time sources:\n{}", bad.join("\n"));
    }

    fn one(path: &str, src: &str) -> Vec<(String, String)> {
        vec![(path.to_owned(), src.to_owned())]
    }

    #[test]
    fn the_lint_can_fail_and_counts_only_production_code() {
        let raw = "fn f() { let _ = std::time::Instant::now(); }\n";
        assert_eq!(problems(&one("x.rs", raw), &[]).len(), 1);
        // sanctioned with the exact count: clean; with another count: not
        let ok = [("x.rs", "Instant::now", 1, "why")];
        assert!(problems(&one("x.rs", raw), &ok).is_empty());
        let stale = [("x.rs", "Instant::now", 2, "why")];
        assert_eq!(problems(&one("x.rs", raw), &stale).len(), 1);
        // comments, strings, chars and test items are not production code
        let quiet = r##"
// Instant::now()
/* SystemTime::now() /* nested */ still a comment */
const S: &str = "Instant::now() {";
const R: &str = r#"thread::sleep( "quoted" "#;
const C: char = '{';
fn lifetime<'a>(x: &'a str) -> &'a str { x }
#[cfg(test)]
mod tests { fn t() { let _ = std::time::Instant::now(); } }
#[test]
fn also_a_test() { let _ = std::time::SystemTime::now(); }
#[cfg(test)]
fn helper() -> [u8; 2] { let _ = std::thread::sleep(d); [0; 2] }
fn production() { let _ = Instant::now(); }
"##;
        let counts = production_counts(&one("y.rs", quiet));
        let row = &counts["y.rs"];
        assert_eq!(row.get("Instant::now"), Some(&1));
        assert_eq!(row.len(), 1, "{row:?}");
    }

    #[test]
    fn a_test_only_field_variant_or_arm_does_not_blank_the_next_item() {
        let cases = [
            "struct S {\n #[cfg(test)]\n probe: u8,\n live: u8,\n}\n",
            "enum E {\n #[cfg(test)]\n Probe,\n Live,\n}\n",
            "fn arm(x: u8) -> u8 { match x {\n #[cfg(test)]\n 0 => 1,\n _ => 2,\n } }\n",
            "struct T {\n live: u8,\n #[cfg(test)]\n last: u8\n}\n",
        ];
        for head in cases {
            let src = format!("{head}fn after() {{ let _ = Instant::now(); }}\n");
            let counts = production_counts(&one("z.rs", &src));
            assert_eq!(counts["z.rs"].get("Instant::now"), Some(&1), "{head}");
        }
        // a test-only generic fn keeps its whole body blanked
        let g = "#[cfg(test)]\nfn generic<A, B>(a: A, b: B) { let _ = Instant::now(); }\n";
        assert!(production_counts(&one("g.rs", g)).is_empty());
    }

    #[test]
    fn an_attribute_on_the_last_parameter_does_not_hide_the_body() {
        let src = "fn f(a: u8, #[cfg(test)] b: u8) { let _ = Instant::now(); }\n\
                   fn g(a: [u8; 2], #[cfg(test)] b: [u8; 1]) { let _ = Instant::now(); }\n";
        let row = &production_counts(&one("p.rs", src))["p.rs"];
        assert_eq!(row.get("Instant::now"), Some(&2), "{row:?}");
    }

    #[test]
    fn a_pub_super_generic_fn_under_cfg_test_is_blanked() {
        let src = "#[cfg(test)]\npub(super) fn g<A, B>(a: A) { let _ = Instant::now(); }\n\
                   fn p() {}\n";
        assert!(production_counts(&one("g.rs", src)).is_empty());
    }

    #[test]
    fn an_attribute_on_a_closure_parameter_does_not_hide_the_body() {
        let src = "fn f() { let c = |#[cfg(test)] a: u8| { let _ = Instant::now(); }; }\n";
        let row = &production_counts(&one("c.rs", src))["c.rs"];
        assert_eq!(row.get("Instant::now"), Some(&1), "{row:?}");
    }

    #[test]
    fn an_attribute_on_a_generic_parameter_does_not_hide_the_body() {
        let src = "fn f<#[cfg(test)] T>() { let _ = Instant::now(); }\n\
                   fn g<#[cfg(test)] T: Fn() -> u8>() { let _ = Instant::now(); }\n";
        let row = &production_counts(&one("g.rs", src))["g.rs"];
        assert_eq!(row.get("Instant::now"), Some(&2), "{row:?}");
    }

    #[test]
    fn an_equals_inside_angle_brackets_does_not_hide_the_body() {
        let srcs = [
            "fn f<#[cfg(test)] T: Iterator<Item = u8>>() { Instant::now() }\n",
            "impl<#[cfg(test)] T: Tr<A = u8>> X for T { fn f() { Instant::now() } }\n",
            "trait Tr<#[cfg(test)] T = u8> { fn f() { Instant::now() } }\n",
            "fn g() { let c = |#[cfg(test)] a: Box<dyn Iterator<Item = u8>>| { Instant::now() }; }\n",
        ];
        for src in srcs {
            let row = &production_counts(&one("e.rs", src))["e.rs"];
            assert_eq!(row.get("Instant::now"), Some(&1), "{src} {row:?}");
        }
    }

    #[test]
    fn a_use_split_over_lines_is_counted() {
        let src = "use\n    std::thread::sleep;\nuse\tstd::thread::{self, sleep};\n";
        let row = &production_counts(&one("w.rs", src))["w.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&2), "{row:?}");
    }

    #[test]
    fn a_use_after_a_block_or_in_a_body_is_counted() {
        let src = "fn a() {}\nuse std::thread::sleep;\n\
                   fn b() { use std::thread::sleep as nap; nap(d); }\n\
                   mod m { pub(super) use std::thread::sleep; }\n\
                   pub(in crate::x) use std::thread::{self, sleep};\n\
                   fn reuse() {}\nuse std::thread::spawn;\n";
        let row = &production_counts(&one("u.rs", src))["u.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&4), "{row:?}");
    }

    #[test]
    fn a_test_only_pub_super_or_pub_in_use_is_blanked() {
        let src = "#[cfg(test)]\npub(super) use std::thread::sleep;\n\
                   #[cfg(test)]\npub(in crate::x) use std::thread::sleep;\n\
                   #[cfg(test)]\npub(in crate::x) mod probe;\nfn p() {}\n";
        let files = vec![
            ("a.rs".to_owned(), src.to_owned()),
            (
                "a/probe.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
        ];
        assert!(production_counts(&files).is_empty());
    }

    #[test]
    fn imported_and_aliased_time_sources_are_counted() {
        let src = "use std::thread::sleep;\nuse std::thread::{self, sleep as nap};\n\
                   use std::time::{Instant as I, SystemTime as S};\nuse std::thread::spawn;\n\
                   fn f() { let _ = (g.wait_timeout(m, d), TcpStream::connect_timeout(&a, d)); }\n";
        let row = &production_counts(&one("w.rs", src))["w.rs"];
        assert_eq!(row.get(USE_SLEEP), Some(&2), "{row:?}");
        assert_eq!(row.get("Instant as "), Some(&1));
        assert_eq!(row.get("SystemTime as "), Some(&1));
        assert_eq!(row.get("wait_timeout"), Some(&1));
        assert_eq!(row.get("connect_timeout("), Some(&1));
    }

    #[test]
    fn a_test_only_module_file_counts_nothing() {
        let files = vec![
            (
                "a.rs".to_owned(),
                "#[cfg(test)]\nmod probe;\nfn p() {}\n".to_owned(),
            ),
            (
                "a/probe.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
            (
                "a/probe/deeper.rs".to_owned(),
                "fn t() { let _ = Instant::now(); }\n".to_owned(),
            ),
        ];
        assert!(production_counts(&files).is_empty());
    }

    #[test]
    fn wall_mode_is_the_wall() {
        let s = Slot::new();
        let (a, b) = (Instant::now(), SystemTime::now());
        let i = s.instant();
        let w = s.wall();
        let st = s.system_now();
        let ws = s.wall_system();
        let (c, d) = (Instant::now(), SystemTime::now());
        assert!(a <= i && i <= c && a <= w && w <= c);
        assert!(b <= st && st <= d && b <= ws && ws <= d);
        assert_eq!(s.mode(), Mode::Wall);
        assert_eq!(
            s.real_timeout(Duration::from_secs(3)),
            Duration::from_secs(3)
        );
        assert!(s.unix_secs() >= b.duration_since(UNIX_EPOCH).unwrap().as_secs());
        assert!(s.since(a) >= i.saturating_duration_since(a));
        assert!(s.wall_since(a) >= w.saturating_duration_since(a));
        let t = Instant::now();
        s.sleep(Duration::from_millis(5));
        assert!(t.elapsed() >= Duration::from_millis(5));
    }

    #[test]
    fn install_refuses_after_a_read_and_twice() {
        let s = Slot::new();
        assert_eq!(s.install(Mode::Wall), Ok(()));
        assert!(s.install(Mode::Wall).is_err());
        let read = Slot::new();
        let _ = read.instant();
        assert!(read.install(Mode::Wall).is_err());
    }

    #[test]
    fn affine_runs_k_times_the_wall() {
        let inst0 = Instant::now();
        let epoch = Duration::from_secs(1_000);
        let (i, st) = affine_at(inst0, epoch, 20, inst0 + Duration::from_secs(1));
        assert_eq!(i, inst0 + Duration::from_secs(20));
        assert_eq!(st, UNIX_EPOCH + Duration::from_secs(1_020));
        // at the origin nothing has moved; before it counts as the origin
        let (i0, s0) = affine_at(inst0, epoch, 20, inst0);
        assert_eq!((i0, s0), (inst0, UNIX_EPOCH + epoch));
        // k = 1 is the wall
        let (i1, s1) = affine_at(inst0, epoch, 1, inst0 + Duration::from_millis(250));
        assert_eq!(i1, inst0 + Duration::from_millis(250));
        assert_eq!(s1, UNIX_EPOCH + epoch + Duration::from_millis(250));
    }

    #[test]
    fn affine_real_timeout_has_a_floor() {
        assert_eq!(
            affine_real_timeout(Duration::from_secs(10), 20),
            Duration::from_millis(500)
        );
        assert_eq!(
            affine_real_timeout(Duration::from_millis(5), 20),
            Duration::from_millis(1)
        );
        assert_eq!(
            affine_real_timeout(Duration::ZERO, 20),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn a_gated_affine_slot_reads_and_waits_scaled() {
        let s = Slot::new();
        assert!(s.install_gated(Mode::Affine { k: 0 }, true).is_err());
        assert_eq!(s.install_gated(Mode::Affine { k: 50 }, true), Ok(()));
        assert_eq!(s.mode(), Mode::Affine { k: 50 });
        let a = s.instant();
        s.sleep(Duration::from_millis(100)); // 2 ms real
        assert!(s.since(a) >= Duration::from_millis(100));
        // 5 game seconds are 100 ms real, not 5 s: an unscaled sleep never
        // returns early, so anything under 5 s proves the scaling without a
        // tight bound a loaded machine could miss.
        let t = Instant::now();
        s.sleep(Duration::from_secs(5));
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(100), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        // the game clock runs well ahead of the wall's own
        let w = s.wall();
        std::thread::sleep(Duration::from_millis(20));
        assert!(s.instant().saturating_duration_since(a) >= (w.elapsed() * 25));
        assert_eq!(
            s.real_timeout(Duration::from_secs(5)),
            Duration::from_millis(100)
        );
        // a Wall slot is untouched by any of it
        assert_eq!(
            Slot::new().real_timeout(Duration::from_secs(5)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn install_refuses_fast_forward_until_the_seam_is_complete() {
        const { assert!(!SEAM_COMPLETE) };
        let s = Slot::new();
        assert!(s.install(Mode::Affine { k: 2 }).is_err());
        assert!(s.install(Mode::Lockstep).is_err());
        // a refusal changes nothing
        assert_eq!(s.mode(), Mode::Wall);
    }
}
