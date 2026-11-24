//! **A local image server for character portraits** (#205), opt-in with
//! `--image-server ADDR`.
//!
//! The client draws every 2D portrait (character select, NEOCOM, chat,
//! show-info) through `EvePhoto`'s `RemoteImageCacher`, which fetches
//! `<imageserverurl>Character/<charID>_<size>.jpg` and caches it
//! **[V decompiled `evePhotosvc.pyc`, `InitializePortraits`]**. The URL is
//! `machoNet.GetGlobalConfig().get('imageserverurl')`, else
//! `defaultImageServerForUser`, which is `None` in a packaged client
//! (`blue.pyos.packaged`). With no URL the cacher never initializes, so
//! every portrait is the default head, even one the character creator
//! saved to the client's cache.
//!
//! Two halves:
//!
//! - **The URL.** `machoNet.OnGlobalConfigUpdated(key, value)` is in
//!   machoNet's `__notifyevents__` and sets one global config key
//!   **[V decompiled `machoNet.pyc`]**. `dispatch` pushes it as a
//!   notification right after the session's initial state, so it lands
//!   before the select screen's first portrait (`InitializePortraits` reads
//!   the URL once). Only with `--image-server`: without it the login frames
//!   are unchanged.
//! - **The pictures.** `GET /Character/<id>_<size>.jpg` serves
//!   `<dir>/<id>_<size>.jpg`, else the largest `<dir>/<id>_<n>.jpg` there
//!   is. `dir` is `--portraits DIR`, default `portraits/` beside the world
//!   file. **[A]** this server renders nothing, so a picture must already
//!   exist: pointing `--portraits` at the client's own
//!   `cache/Pictures/Characters` serves the 512 px snapshot the character
//!   creator saves there. Falsified by a character made in that client whose
//!   select-screen portrait stays the default head. Anything else is 404, and
//!   the client keeps its default.
//!
//! # Hardening
//!
//! std sockets and threads only, as `admin_web`. At most [`MAX_CONNS`]
//! connections are served at once, each on its own thread; one past that is
//! closed unanswered on the accept thread, so no thread is made for it. GET
//! only, a fixed path shape (no traversal), a request head of at most
//! [`MAX_HEAD`] bytes that must arrive within [`HEAD_WAIT`] (a head dripped a
//! byte at a time cannot hold a slot past it), 5 s socket timeouts, and no
//! file over [`MAX_FILE`] bytes is read.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The world slot: the URL `dispatch` advertises at login, if any.
#[derive(Debug, Default)]
pub struct Config {
    pub url: Option<String>,
}

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "images",
    make: |_| Box::new(Config::default()),
};

/// The global config key the URL goes in **[V]**.
pub const URL_KEY: &str = "imageserverurl";

/// Connections served at once; one more is closed unanswered.
pub const MAX_CONNS: usize = 16;
/// The longest request head read.
pub const MAX_HEAD: usize = 4096;
/// The whole request head must have arrived within this.
pub const HEAD_WAIT: Duration = Duration::from_secs(10);
/// The largest picture served (a 512 px portrait is tens of KiB).
pub const MAX_FILE: u64 = 4 * 1024 * 1024;
/// One socket read or write.
const IO_WAIT: Duration = Duration::from_secs(5);

/// `(charID, size)` of a `/Character/<id>_<size>.jpg` path, else `None`.
pub fn parse_path(path: &str) -> Option<(i64, u32)> {
    let name = path.strip_prefix("/Character/")?.strip_suffix(".jpg")?;
    let (id, size) = name.split_once('_')?;
    if id.is_empty()
        || size.is_empty()
        || !id.bytes().chain(size.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((id.parse().ok()?, size.parse().ok()?))
}

/// The file to serve for `(id, size)` in `dir`: the exact size, else the
/// largest size on disk.
pub fn pick(dir: &Path, id: i64, size: u32) -> Option<PathBuf> {
    let exact = dir.join(format!("{id}_{size}.jpg"));
    if exact.is_file() {
        return Some(exact);
    }
    let prefix = format!("{id}_");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n: u32 = name
                .strip_prefix(&prefix)?
                .strip_suffix(".jpg")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

fn respond(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)
}

/// The request line, once the whole head is in: `None` for a head that is
/// too long, too slow, cut off or not text.
fn read_head(s: &mut TcpStream) -> Option<String> {
    let start = crate::game_clock::clock::wall();
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = std::str::from_utf8(&buf[..i]).ok()?;
            return head.lines().next().map(str::to_owned);
        }
        if buf.len() >= MAX_HEAD || crate::game_clock::clock::wall_since(start) > HEAD_WAIT {
            return None;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn serve(mut s: TcpStream, dir: &Path) -> io::Result<()> {
    s.set_read_timeout(Some(IO_WAIT))?;
    s.set_write_timeout(Some(IO_WAIT))?;
    let Some(line) = read_head(&mut s) else {
        return respond(&mut s, "400 Bad Request", "text/plain", b"bad request\n");
    };
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" {
        return respond(
            &mut s,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only\n",
        );
    }
    let path = path.split('?').next().unwrap_or("");
    let file = parse_path(path)
        .and_then(|(id, size)| pick(dir, id, size))
        .filter(|f| std::fs::metadata(f).is_ok_and(|m| m.len() <= MAX_FILE));
    match file {
        Some(file) => {
            let body = std::fs::read(&file)?;
            respond(&mut s, "200 OK", "image/jpeg", &body)
        }
        None => respond(&mut s, "404 Not Found", "text/plain", b"no such portrait\n"),
    }
}

/// One of the [`MAX_CONNS`] slots, given back when dropped (a connection's
/// thread ending, panicking included).
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>) -> Option<Slot> {
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(Arc::clone(active)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bind `addr` and serve `dir`, a thread per connection and at most
/// [`MAX_CONNS`] at once. Answers the bound address (`addr` with port 0
/// picks one).
pub fn open(addr: SocketAddr, dir: PathBuf) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("images".to_owned())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                // Past the cap: dropped here, closed unanswered.
                let Some(slot) = Slot::take(&active) else {
                    continue;
                };
                let dir = dir.clone();
                let _ = std::thread::Builder::new()
                    .name("images-conn".to_owned())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        let _slot = slot;
                        // A peer that hung up is not worth a line: one that
                        // connects and leaves would otherwise write one each.
                        match serve(conn, &dir) {
                            Err(e)
                                if !matches!(
                                    e.kind(),
                                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                                ) =>
                            {
                                crate::outln!("[images] request failed: {e}");
                            }
                            _ => {}
                        }
                    });
            }
        })?;
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eve-images-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn get(addr: SocketAddr, path: &str) -> String {
        let mut s = TcpStream::connect(addr).expect("connect");
        // A refused connection may be closed, or reset, before the request
        // is written or read: no answer either way.
        let _ = write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n");
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn only_the_portrait_path_shape_is_accepted() {
        assert_eq!(
            parse_path("/Character/90000005_128.jpg"),
            Some((90_000_005, 128))
        );
        assert_eq!(parse_path("/Character/../x_1.jpg"), None);
        assert_eq!(parse_path("/Character/9_.jpg"), None);
        assert_eq!(parse_path("/Corporation/1_64.png"), None);
        assert_eq!(parse_path("/Character/1_64.jpg/../../etc"), None);
    }

    #[test]
    fn the_exact_size_wins_else_the_largest() {
        let dir = temp_dir("pick");
        for n in ["7_64.jpg", "7_512.jpg", "8_32.jpg"] {
            std::fs::write(dir.join(n), b"x").expect("write");
        }
        assert_eq!(pick(&dir, 7, 64), Some(dir.join("7_64.jpg")));
        assert_eq!(pick(&dir, 7, 128), Some(dir.join("7_512.jpg")));
        assert_eq!(pick(&dir, 9, 128), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_request_gets_the_file_or_404() {
        let dir = temp_dir("srv");
        std::fs::write(dir.join("5_512.jpg"), b"JPEGBYTES").expect("write");
        let addr = open("127.0.0.1:0".parse().expect("addr"), dir.clone()).expect("open");
        let ok = get(addr, "/Character/5_128.jpg");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.ends_with("JPEGBYTES"));
        assert!(get(addr, "/Character/6_128.jpg").starts_with("HTTP/1.1 404"));
        let mut s = TcpStream::connect(addr).expect("connect");
        write!(s, "POST /Character/5_128.jpg HTTP/1.1\r\n\r\n").expect("send");
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 405"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A head that reaches [`MAX_HEAD`] bytes without ending is answered 400
    /// without being read on.
    #[test]
    fn an_overlong_head_is_refused() {
        let dir = temp_dir("head");
        let addr = open("127.0.0.1:0".parse().expect("addr"), dir.clone()).expect("open");
        let mut s = TcpStream::connect(addr).expect("connect");
        // Exactly MAX_HEAD bytes, so the server has read all of them before
        // it answers (unread bytes at close would reset the connection).
        let mut long = b"GET /Character/5_1.jpg HTTP/1.1\r\nX: ".to_vec();
        long.resize(MAX_HEAD, b'a');
        let _ = s.write_all(&long);
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// [`MAX_CONNS`] connections that send nothing hold every slot: one more
    /// is closed unanswered, and once one of them goes its slot serves again.
    #[test]
    fn connections_past_the_cap_are_closed_unanswered() {
        let dir = temp_dir("cap");
        std::fs::write(dir.join("5_64.jpg"), b"JPEGBYTES").expect("write");
        let addr = open("127.0.0.1:0".parse().expect("addr"), dir.clone()).expect("open");
        let mut idle: Vec<TcpStream> = (0..MAX_CONNS)
            .map(|_| TcpStream::connect(addr).expect("connect"))
            .collect();
        // Accepted in order, so these are all counted before the next one.
        assert_eq!(get(addr, "/Character/5_64.jpg"), "");
        drop(idle.pop());
        // The freed slot comes back once its thread sees the close.
        let mut tries = 0;
        let served = loop {
            let r = get(addr, "/Character/5_64.jpg");
            if r.starts_with("HTTP/1.1 200") || tries == 50 {
                break r;
            }
            tries += 1;
            crate::game_clock::clock::sleep(Duration::from_millis(20));
        };
        assert!(served.ends_with("JPEGBYTES"), "{served}");
        drop(idle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
