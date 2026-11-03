//! A non-blocking log channel (TT-A, part b).
//!
//! `println!` under the world write lock takes the process-wide stdout mutex
//! and does a `write(2)` on every newline. A stalled disk, a jbd2 commit or a
//! paused terminal then stops the whole world, because the thread holding the
//! lock waits on the write. [`outln!`] and [`errln!`] format the line on the
//! caller's thread and hand it to a writer thread over a bounded channel with
//! `try_send`: **the caller never blocks.** A full queue drops the line and
//! counts it; the next line to get in is preceded by
//! `[log] dropped N lines (queue full)`, exactly where the gap is.
//!
//! * Not initialised (unit tests, `eve-bot` using the lib, a bin before
//!   [`init`]): the macros print directly with `println!` / `eprintln!`.
//! * `--log-sync` ([`init`] with `sync`): the caller writes under a mutex and
//!   flushes, the old behaviour, still timed. It exists for the A/B run.
//! * Otherwise: one `log-writer` thread. One channel carries both streams, so
//!   there is one total order (enqueue order); the writer flushes on every
//!   stream switch, so a `2>&1` file keeps the cross-stream order.
//!
//! Every call adds its own time (format included) to a per-thread tally that
//! `dispatch.rs` reads at the end of a turn ([`take_thread_stats`]).
//!
//! A panic flushes the queue first (a hook [`init`] installs), so the panic
//! message comes after the lines that led to it. `SIGKILL` loses at most the
//! queue, which is milliseconds of lines.

use std::cell::Cell;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Which process stream a line goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
}

enum Msg {
    /// Without the trailing newline.
    Line(Stream, String),
    /// A TSV-style append; the header goes first when the file is new or
    /// empty. Done on the writer thread so the caller never touches the file.
    Append {
        path: PathBuf,
        header: Option<String>,
        text: String,
    },
    Flush(std::sync::mpsc::Sender<()>),
}

/// Queue depth. [A] about 2 MB; falsifier: `[log] dropped` in a run whose
/// p99 write is under 1 ms.
pub const QUEUE_LINES: usize = 16_384;

/// Lines taken from the queue per batch, and so per stdout lock.
const BATCH: usize = 1024;

/// One thread's logging since the last [`take_thread_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStats {
    /// Time in `emit`, the format included.
    pub ns: u64,
    pub lines: u32,
    /// Lines this thread lost to a full queue.
    pub dropped: u32,
}

thread_local! {
    static STATS: Cell<LogStats> = const { Cell::new(LogStats { ns: 0, lines: 0, dropped: 0 }) };
}

fn note(started: Instant, dropped: u32) {
    let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    STATS.with(|s| {
        let mut v = s.get();
        v.ns = v.ns.saturating_add(ns);
        v.lines = v.lines.saturating_add(1);
        v.dropped = v.dropped.saturating_add(dropped);
        s.set(v);
    });
}

/// This thread's logging since the last call, and reset.
pub fn take_thread_stats() -> LogStats {
    STATS.with(|s| s.replace(LogStats::default()))
}

/// The two sinks plus the stream written last, so a switch flushes.
struct Sinks {
    out: BufWriter<Box<dyn Write + Send>>,
    err: BufWriter<Box<dyn Write + Send>>,
    last: Option<Stream>,
}

impl Sinks {
    fn new(out: Box<dyn Write + Send>, err: Box<dyn Write + Send>) -> Sinks {
        Sinks {
            out: BufWriter::with_capacity(64 * 1024, out),
            err: BufWriter::with_capacity(8 * 1024, err),
            last: None,
        }
    }

    fn line(&mut self, stream: Stream, text: &str) {
        if self.last.is_some_and(|l| l != stream) {
            self.flush();
        }
        self.last = Some(stream);
        let w = match stream {
            Stream::Out => &mut self.out,
            Stream::Err => &mut self.err,
        };
        // A sink that fails has nowhere to report to; the line is lost.
        let _ = w.write_all(text.as_bytes());
        let _ = w.write_all(b"\n");
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
        let _ = self.err.flush();
    }

    fn append(&mut self, path: &std::path::Path, header: Option<&str>, text: &str) {
        self.flush();
        if let Err(e) = append_file(path, header, text) {
            self.line(
                Stream::Err,
                &format!("[log] append to {} failed: {e}", path.display()),
            );
        }
    }
}

fn append_file(path: &std::path::Path, header: Option<&str>, text: &str) -> io::Result<()> {
    let fresh = !std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    let mut body = String::new();
    if let (true, Some(h)) = (fresh, header) {
        body.push_str(h);
        body.push('\n');
    }
    body.push_str(text);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(body.as_bytes())
}

/// The real streams. A write takes the stream's lock for that call only, and
/// `Sinks`' `BufWriter` makes that one call per batch.
struct StdOut;
struct StdErr;

impl Write for StdOut {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        io::stdout().lock().write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stdout().lock().flush()
    }
}
impl Write for StdErr {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        io::stderr().lock().write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stderr().lock().flush()
    }
}

/// The channel, its counters and (in sync mode) its sinks.
pub struct Logger {
    tx: SyncSender<Msg>,
    dropped: AtomicU64,
    /// Sync mode: the caller writes here and the channel is unused.
    direct: Option<Mutex<Sinks>>,
}

fn writer(rx: Receiver<Msg>, mut sinks: Sinks) {
    let mut batch: Vec<Msg> = Vec::with_capacity(BATCH);
    while let Ok(first) = rx.recv() {
        batch.push(first);
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        for m in batch.drain(..) {
            match m {
                Msg::Line(s, text) => sinks.line(s, &text),
                Msg::Append { path, header, text } => {
                    sinks.append(&path, header.as_deref(), &text);
                }
                Msg::Flush(ack) => {
                    sinks.flush();
                    let _ = ack.send(());
                }
            }
        }
        sinks.flush();
    }
    sinks.flush();
}

impl Logger {
    /// A logger over any two sinks: what the tests use, and [`init`].
    pub fn with_sinks(
        out: Box<dyn Write + Send>,
        err: Box<dyn Write + Send>,
        cap: usize,
        sync: bool,
    ) -> Logger {
        let sinks = Sinks::new(out, err);
        let (tx, rx) = sync_channel(cap.max(1));
        if sync {
            return Logger {
                tx,
                dropped: AtomicU64::new(0),
                direct: Some(Mutex::new(sinks)),
            };
        }
        // Detached: it ends when every sender is gone, and [`shutdown`] flushes.
        let _ = std::thread::Builder::new()
            .name("log-writer".to_owned())
            .spawn(move || writer(rx, sinks))
            .ok();
        Logger {
            tx,
            dropped: AtomicU64::new(0),
            direct: None,
        }
    }

    /// Queue one line (or write it, in sync mode). Never blocks.
    pub fn emit(&self, stream: Stream, started: Instant, line: String) {
        let mut lost = 0u32;
        if let Some(direct) = &self.direct {
            let mut s = direct.lock().unwrap_or_else(|e| e.into_inner());
            s.line(stream, &line);
            s.flush();
        } else {
            let n = self.dropped.swap(0, Ordering::Relaxed);
            if n > 0 {
                let marker =
                    Msg::Line(Stream::Err, format!("[log] dropped {n} lines (queue full)"));
                if self.tx.try_send(marker).is_err() {
                    self.dropped.fetch_add(n, Ordering::Relaxed);
                }
            }
            match self.tx.try_send(Msg::Line(stream, line)) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    lost = 1;
                }
                Err(TrySendError::Disconnected(Msg::Line(s, text))) => {
                    // After `shutdown`: nobody drains, so write directly.
                    match s {
                        Stream::Out => println!("{text}"),
                        Stream::Err => eprintln!("{text}"),
                    }
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
        note(started, lost);
    }

    /// Queue an append to `path`; see [`append`].
    pub fn append(&self, path: PathBuf, header: Option<String>, text: String) {
        if let Some(direct) = &self.direct {
            let mut s = direct.lock().unwrap_or_else(|e| e.into_inner());
            s.append(&path, header.as_deref(), &text);
            return;
        }
        let msg = Msg::Append { path, header, text };
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(Msg::Append { path, header, text })) => {
                if let Err(e) = append_file(&path, header.as_deref(), &text) {
                    eprintln!("[log] append to {} failed: {e}", path.display());
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Wait until everything queued so far is written, up to `within`.
    /// True when it was. Never blocks past `within`, even on a full queue.
    pub fn flush_for(&self, within: Duration) -> bool {
        if let Some(direct) = &self.direct {
            direct.lock().unwrap_or_else(|e| e.into_inner()).flush();
            return true;
        }
        let deadline = Instant::now() + within;
        let (ack, wait) = std::sync::mpsc::channel();
        let mut msg = Msg::Flush(ack);
        loop {
            match self.tx.try_send(msg) {
                Ok(()) => break,
                Err(TrySendError::Full(m)) => msg = m,
                Err(TrySendError::Disconnected(_)) => return true,
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let rest = deadline.saturating_duration_since(Instant::now());
        match wait.recv_timeout(rest) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        }
    }

    /// Lines lost to a full queue and not yet reported by a marker.
    pub fn pending_dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Start the process logger over stdout and stderr and install the panic
/// hook. A second call is a no-op. `sync` is `--log-sync`.
pub fn init(sync: bool) -> io::Result<()> {
    if GLOBAL.get().is_some() {
        return Ok(());
    }
    let logger = Logger::with_sinks(Box::new(StdOut), Box::new(StdErr), QUEUE_LINES, sync);
    if GLOBAL.set(logger).is_err() {
        return Ok(());
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let writer_thread = std::thread::current().name() == Some("log-writer");
        if !writer_thread && let Some(l) = GLOBAL.get() {
            l.flush_for(Duration::from_millis(500));
        }
        previous(info);
    }));
    Ok(())
}

/// Flush, wait up to `wait` for the writer to finish, and send later lines
/// direct. Call it before the process ends.
pub fn shutdown(wait: Duration) {
    let Some(l) = GLOBAL.get() else {
        return;
    };
    l.flush_for(wait);
}

/// What the macros call.
pub fn emit(stream: Stream, started: Instant, line: String) {
    match GLOBAL.get() {
        Some(l) => l.emit(stream, started, line),
        None => {
            match stream {
                Stream::Out => println!("{line}"),
                Stream::Err => eprintln!("{line}"),
            }
            note(started, 0);
        }
    }
}

/// Append `text` to `path` on the writer thread (the header first when the
/// file is new or empty). Uninitialised: inline, as before.
pub fn append(path: PathBuf, header: Option<String>, text: String) {
    match GLOBAL.get() {
        Some(l) => l.append(path, header, text),
        None => {
            if let Err(e) = append_file(&path, header.as_deref(), &text) {
                eprintln!("[log] append to {} failed: {e}", path.display());
            }
        }
    }
}

/// `println!`, queued. See the module header.
#[macro_export]
macro_rules! outln {
    () => {
        $crate::logline::emit(
            $crate::logline::Stream::Out,
            ::std::time::Instant::now(),
            ::std::string::String::new(),
        )
    };
    ($($a:tt)*) => {{
        let t = ::std::time::Instant::now();
        $crate::logline::emit($crate::logline::Stream::Out, t, ::std::format!($($a)*))
    }};
}

/// `eprintln!`, queued. See the module header.
#[macro_export]
macro_rules! errln {
    () => {
        $crate::logline::emit(
            $crate::logline::Stream::Err,
            ::std::time::Instant::now(),
            ::std::string::String::new(),
        )
    };
    ($($a:tt)*) => {{
        let t = ::std::time::Instant::now();
        $crate::logline::emit($crate::logline::Stream::Err, t, ::std::format!($($a)*))
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier};

    /// A sink into a shared buffer.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn logger(out: &Buf, err: &Buf, cap: usize, sync: bool) -> Logger {
        Logger::with_sinks(Box::new(out.clone()), Box::new(err.clone()), cap, sync)
    }
    fn out(l: &Logger, s: &str) {
        l.emit(Stream::Out, Instant::now(), s.to_owned());
    }
    fn err(l: &Logger, s: &str) {
        l.emit(Stream::Err, Instant::now(), s.to_owned());
    }

    #[test]
    fn per_thread_order_holds() {
        let o = Buf::default();
        let l = Arc::new(logger(&o, &Buf::default(), 100_000, false));
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let l = Arc::clone(&l);
                std::thread::spawn(move || {
                    for n in 0..10_000 {
                        out(&l, &format!("t{i} {n}"));
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert!(l.flush_for(Duration::from_secs(20)));
        let text = o.text();
        assert_eq!(text.lines().count(), 80_000);
        let mut last = [-1i64; 8];
        for line in text.lines() {
            let (t, n) = line.split_once(' ').unwrap();
            let i: usize = t[1..].parse().unwrap();
            let n: i64 = n.parse().unwrap();
            assert!(n > last[i], "thread {i} out of order at {n}");
            last[i] = n;
        }
    }

    #[test]
    fn happens_before_is_kept() {
        let o = Buf::default();
        let l = Arc::new(logger(&o, &Buf::default(), 10_000, false));
        for k in 0..1000 {
            let (tx, rx) = std::sync::mpsc::channel();
            let la = Arc::clone(&l);
            let a = std::thread::spawn(move || {
                out(&la, &format!("a{k}"));
                tx.send(()).unwrap();
            });
            let lb = Arc::clone(&l);
            let b = std::thread::spawn(move || {
                rx.recv().unwrap();
                out(&lb, &format!("b{k}"));
            });
            a.join().unwrap();
            b.join().unwrap();
        }
        assert!(l.flush_for(Duration::from_secs(10)));
        let text = o.text();
        let lines: Vec<&str> = text.lines().collect();
        for k in 0..1000 {
            let pa = lines.iter().position(|x| *x == format!("a{k}")).unwrap();
            let pb = lines.iter().position(|x| *x == format!("b{k}")).unwrap();
            assert!(pa < pb, "a{k} after b{k}");
        }
    }

    #[test]
    fn streams_keep_their_order() {
        let shared = Buf::default();
        let l = logger(&shared, &shared, 100, false);
        out(&l, "out1");
        err(&l, "err1");
        out(&l, "out2");
        err(&l, "err2");
        assert!(l.flush_for(Duration::from_secs(5)));
        assert_eq!(shared.text(), "out1\nerr1\nout2\nerr2\n");
    }

    /// A sink whose first write waits on a barrier, after saying it is there.
    struct Blocked(Buf, Option<Arc<Barrier>>, Arc<AtomicBool>);
    impl Write for Blocked {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if let Some(g) = self.1.take() {
                self.2.store(true, Ordering::SeqCst);
                g.wait();
            }
            self.0.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A logger whose Out sink is stuck on its first write, with one primed
    /// line already taken by the writer.
    fn stuck(out_sink: Buf, err_sink: Buf, cap: usize) -> (Logger, Arc<Barrier>) {
        let gate = Arc::new(Barrier::new(2));
        let entered = Arc::new(AtomicBool::new(false));
        let l = Logger::with_sinks(
            Box::new(Blocked(
                out_sink,
                Some(Arc::clone(&gate)),
                Arc::clone(&entered),
            )),
            Box::new(err_sink),
            cap,
            false,
        );
        out(&l, "prime");
        let t = Instant::now();
        while !entered.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(5), "writer never blocked");
            std::thread::sleep(Duration::from_millis(1));
        }
        (l, gate)
    }

    #[test]
    fn a_blocked_sink_never_blocks_emit() {
        let o = Buf::default();
        let (l, gate) = stuck(o.clone(), Buf::default(), 64);
        let _ = take_thread_stats();
        let t = Instant::now();
        for n in 0..1000 {
            out(&l, &format!("line {n}"));
        }
        let took = t.elapsed();
        assert!(took < Duration::from_millis(50), "emit blocked: {took:?}");
        let st = take_thread_stats();
        assert_eq!(st.lines, 1000);
        // The queue holds `cap`; everything else is dropped and counted.
        assert_eq!(st.dropped, 1000 - 64);
        assert_eq!(l.pending_dropped(), u64::from(st.dropped));
        gate.wait();
        assert!(l.flush_for(Duration::from_secs(5)));
        out(&l, "after");
        assert!(l.flush_for(Duration::from_secs(5)));
        assert!(o.text().starts_with("prime\nline 0\n"));
        assert!(o.text().ends_with("after\n"));
    }

    #[test]
    fn a_blocked_sink_leaves_the_marker_at_the_gap() {
        let shared = Buf::default();
        let (l, gate) = stuck(shared.clone(), shared.clone(), 64);
        for n in 0..1000 {
            out(&l, &format!("line {n}"));
        }
        let dropped = l.pending_dropped();
        assert_eq!(dropped, 1000 - 64);
        gate.wait();
        assert!(l.flush_for(Duration::from_secs(5)));
        out(&l, "after");
        assert!(l.flush_for(Duration::from_secs(5)));
        let text = shared.text();
        assert!(
            text.ends_with(&format!(
                "[log] dropped {dropped} lines (queue full)\nafter\n"
            )),
            "{}",
            &text[text.len().saturating_sub(120)..]
        );
    }

    #[test]
    fn uninitialised_prints_directly() {
        // The global is never set by a test.
        assert!(GLOBAL.get().is_none());
        let _ = take_thread_stats();
        crate::outln!("direct {}", 1);
        crate::errln!();
        let st = take_thread_stats();
        assert_eq!(st.lines, 2);
        assert_eq!(st.dropped, 0);
    }

    #[test]
    fn sync_mode_matches_async_bytes() {
        let script = |l: &Logger| {
            for n in 0..5000 {
                match n % 7 {
                    0 => err(l, &format!("e{n}")),
                    3 => out(l, ""),
                    5 => out(l, &format!("multi\nline {n}")),
                    _ => out(l, &format!("o{n}")),
                }
            }
        };
        let run = |sync: bool| {
            let shared = Buf::default();
            let l = logger(&shared, &shared, 100_000, sync);
            script(&l);
            assert!(l.flush_for(Duration::from_secs(10)));
            shared.text()
        };
        let (a, b) = (run(true), run(false));
        assert!(!a.is_empty());
        assert_eq!(a, b);
    }

    #[test]
    fn stats_are_per_thread_and_reset() {
        let l = Arc::new(logger(&Buf::default(), &Buf::default(), 100, false));
        let hs: Vec<_> = (0..2)
            .map(|_| {
                let l = Arc::clone(&l);
                std::thread::spawn(move || {
                    for n in 0..3 {
                        out(&l, &format!("x{n}"));
                    }
                    let first = take_thread_stats();
                    (first.lines, take_thread_stats().lines)
                })
            })
            .collect();
        for h in hs {
            assert_eq!(h.join().unwrap(), (3, 0));
        }
    }

    #[test]
    fn append_writes_header_once() {
        let dir = std::env::temp_dir().join(format!("logline-append-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rows.tsv");
        let _ = std::fs::remove_file(&path);
        let l = logger(&Buf::default(), &Buf::default(), 100, false);
        l.append(path.clone(), Some("h1\th2".to_owned()), "a\t1\n".to_owned());
        l.append(path.clone(), Some("h1\th2".to_owned()), "b\t2\n".to_owned());
        assert!(l.flush_for(Duration::from_secs(5)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "h1\th2\na\t1\nb\t2\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 4 threads x 250k lines into a null sink. Release only.
    #[test]
    #[ignore]
    fn logline_bench() {
        for blocked in [false, true] {
            let gate = Arc::new(Barrier::new(2));
            let sink: Box<dyn Write + Send> = if blocked {
                Box::new(Blocked(
                    Buf::default(),
                    Some(Arc::clone(&gate)),
                    Arc::new(AtomicBool::new(false)),
                ))
            } else {
                Box::new(io::sink())
            };
            let l = Arc::new(Logger::with_sinks(
                sink,
                Box::new(io::sink()),
                QUEUE_LINES,
                false,
            ));
            let hs: Vec<_> = (0..4)
                .map(|_| {
                    let l = Arc::clone(&l);
                    std::thread::spawn(move || {
                        let mut ns = Vec::with_capacity(250_000);
                        for n in 0..250_000u32 {
                            let t = Instant::now();
                            l.emit(Stream::Out, t, format!("[svc] handed out object {n}"));
                            ns.push(t.elapsed().as_nanos() as u64);
                        }
                        ns
                    })
                })
                .collect();
            let mut all: Vec<u64> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
            all.sort_unstable();
            let p = |q: f64| all[((all.len() - 1) as f64 * q) as usize];
            println!(
                "blocked={blocked} p50={}ns p99={}ns max={}ns",
                p(0.5),
                p(0.99),
                all[all.len() - 1]
            );
            if blocked {
                gate.wait();
            }
        }
    }
}
