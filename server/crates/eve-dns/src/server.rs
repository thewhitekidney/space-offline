//! The message handler and the two listeners that feed it.
//!
//! [`handle_message`] is the whole server as a pure function: bytes in,
//! optional bytes out, no I/O and no state. The socket loops below are thin
//! wrappers around it, which is what lets the interesting behaviour be tested
//! without a socket at all.
//!
//! Both transports are served because both are reachable from the jail —
//! `tools/jail-setup.sh` opens `udp dport 53` *and* `tcp dport 53`. A resolver
//! that receives a truncated UDP answer retries over TCP (RFC 1035 §4.2.2),
//! and glibc's resolver does so automatically; answering costs one more loop
//! over the same codec, whereas not answering would stall the client for its
//! full resolver timeout.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::policy::{Decision, Policy};
use crate::wire::{
    Answer, Header, OPCODE_QUERY, Question, RCODE_FORMAT_ERROR, RCODE_NAME_ERROR, RCODE_NO_ERROR,
    RCODE_NOT_IMPLEMENTED, TCP_LENGTH_PREFIX_LEN, build_bare_response, build_response,
};

/// Receive buffer for one UDP query.
///
/// A query without EDNS0 cannot exceed 512 bytes (RFC 1035 §2.3.4), but a
/// resolver may send a larger one if it believes the server speaks EDNS0, and
/// a short read would silently mangle it. 4 KiB is comfortably above anything
/// a resolver sends and is one page.
pub const UDP_RECV_BUF: usize = 4096;

/// Idle timeout on a DNS-over-TCP connection.
///
/// RFC 7766 §6.2 has servers close idle connections rather than hold them
/// open. A resolver sends its query immediately after connecting, so this
/// only ever fires on a connection that is going nowhere — and it is what
/// stops such a connection from holding a thread forever.
pub const TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Write timeout, so a peer that stops reading cannot pin a thread either.
pub const TCP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Concurrent TCP connections served at once. The jail holds one client; this
/// is only here so that a misbehaving one cannot spawn threads without bound.
pub const MAX_TCP_CONNECTIONS: usize = 32;

/// Consecutive accept/receive failures tolerated before a loop gives up.
///
/// A single failure is normal — on Linux an unconnected UDP socket can surface
/// a previous send's ICMP error on the next receive — so one error must not
/// stop the server. A loop spinning on a permanent error must stop, though,
/// rather than burn a core silently.
const MAX_CONSECUTIVE_ERRORS: usize = 64;

/// Exit status when a query line cannot be written to the `--log` file.
///
/// Distinct from 1 (a socket failed) and 2 (usage) so a launcher can tell an
/// audit failure from the rest.
pub const EXIT_QUERY_LOG_FAILED: i32 = 3;

/// Permissions on a query log this process creates: the owner only. The
/// names the client asked for are the owner's business.
const QUERY_LOG_MODE: u32 = 0o600;

/// Where the per-query lines go: stderr (the default), or a file.
///
/// The file is the phone-home audit (`tools/jail-drops.sh` reads it as
/// `logs/eve-dns.log`), so it must never lose a line silently. Two rules
/// follow, and both are deliberate:
///
/// - **The line is written before the answer is sent**, one unbuffered
///   `O_APPEND` write per line, so a query the client got an answer to is
///   already in the kernel's hands; a crash after that cannot lose it.
/// - **A line that cannot be written stops the resolver** (exit
///   [`EXIT_QUERY_LOG_FAILED`]) rather than answering unaudited. With no
///   resolver the jailed client cannot resolve anything -- fail closed --
///   and `tools/up.sh` reports the resolver as not running. The lost line
///   is written to stderr first, so it survives wherever stderr goes.
///
/// With a file, query lines go to the file only, never to stderr as well, so
/// a launcher that also points stderr at the same file does not count every
/// query twice. Diagnostics (bind, listen, socket errors) stay on stderr.
#[derive(Debug)]
pub struct QueryLog {
    sink: Sink,
}

#[derive(Debug)]
enum Sink {
    Stderr,
    File { path: PathBuf, file: Mutex<File> },
}

impl QueryLog {
    /// Query lines to stderr, as before `--log` existed.
    pub const fn stderr() -> Self {
        Self { sink: Sink::Stderr }
    }

    /// Open (or create, mode 0600) `path` for appending.
    ///
    /// Never truncates: the file is the durable record across runs, and a
    /// launcher that wants a fresh one truncates it itself. Call this after
    /// the privilege drop -- `eve-dns` refuses `--log` as root -- so the
    /// open is performed as the user who owns the path.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(QUERY_LOG_MODE)
            .open(path)?;
        Ok(Self {
            sink: Sink::File {
                path: path.to_path_buf(),
                file: Mutex::new(file),
            },
        })
    }

    /// The file this writes to, if any.
    pub fn path(&self) -> Option<&Path> {
        match &self.sink {
            Sink::Stderr => None,
            Sink::File { path, .. } => Some(path),
        }
    }

    /// Write one line (a newline is added), reporting any failure.
    ///
    /// The whole line goes out in one `write_all` under the lock, so lines
    /// from the UDP and TCP threads never interleave.
    pub fn write_line(&self, line: &str) -> io::Result<()> {
        match &self.sink {
            Sink::Stderr => {
                crate::log!("{line}");
                Ok(())
            }
            Sink::File { file, .. } => {
                let mut buf = Vec::with_capacity(line.len() + 1);
                buf.extend_from_slice(line.as_bytes());
                buf.push(b'\n');
                // A poisoned lock means another thread panicked mid-write;
                // the file itself is still fine to append to.
                let mut file = file.lock().unwrap_or_else(|p| p.into_inner());
                file.write_all(&buf)
            }
        }
    }

    /// Write one line, or stop the process (see the type's doc).
    fn record(&self, line: &str) {
        if let Err(err) = self.write_line(line) {
            let path = self.path().map(Path::display);
            crate::log!("eve-dns: cannot write the query log {path:?}: {err}");
            crate::log!("eve-dns: unlogged: {line}");
            crate::log!("eve-dns: stopping rather than answer queries nobody records");
            process::exit(EXIT_QUERY_LOG_FAILED);
        }
    }
}

/// The audit line for one query, in the format `tools/jail-drops.sh` parses:
/// `query NAME. TYPE -> IP`, `query NAME. TYPE NXDOMAIN`, or
/// `query NAME. TYPE NODATA`, then the census fields (CEN-3,
/// `docs/guides/census.md`): ` peer=IP:PORT` when the transport knows who asked,
/// and ` t=UNIXMS`, the wall time in milliseconds. They are appended, so the
/// script's fields (`$1`, `$2`, and the `-> ` test) are unchanged, and
/// `tools/census_report.py` slices a run's lookups out of the shared log by
/// `t=`. The name is absolute (trailing dot) and escaped by
/// [`crate::wire::Name`]'s `Display`, so it never contains a space or a
/// newline and the line splits on whitespace as the script expects.
fn query_line(
    question: &Question,
    decision: &Decision,
    peer: Option<SocketAddr>,
    t_ms: u128,
) -> String {
    let verdict = match decision {
        Decision::Address(addr) => format!("-> {addr}"),
        Decision::NoData => "NODATA".to_owned(),
        Decision::NameError => "NXDOMAIN".to_owned(),
    };
    let peer = peer.map(|p| format!(" peer={p}")).unwrap_or_default();
    format!(
        "query {} {} {}{peer} t={t_ms}",
        question.name,
        crate::wire::type_name(question.qtype),
        verdict
    )
}

/// Milliseconds since the Unix epoch, for the `t=` field. A clock before
/// 1970 writes `t=0` rather than failing the query.
fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Answer one message.
///
/// Returns the bytes to send back, or `None` when the right response is
/// silence. We stay silent only for a message that is not a query at all: a
/// datagram with QR set is a response, and replying to one invites a loop
/// between two servers, each answering the other's error reply forever. A
/// message too short to hold an ID gets nothing either, because a response
/// without an ID matches no outstanding query.
///
/// Query lines go to stderr; [`handle_message_logged`] takes a [`QueryLog`].
pub fn handle_message(policy: &Policy, msg: &[u8]) -> Option<Vec<u8>> {
    handle_message_logged(policy, &QueryLog::stderr(), msg)
}

/// [`handle_message`], writing its query line to `log` (with no `peer=`).
pub fn handle_message_logged(policy: &Policy, log: &QueryLog, msg: &[u8]) -> Option<Vec<u8>> {
    handle_message_from(policy, log, msg, None)
}

/// [`handle_message_logged`], naming the peer that sent `msg` in the line.
pub fn handle_message_from(
    policy: &Policy,
    log: &QueryLog,
    msg: &[u8],
    peer: Option<SocketAddr>,
) -> Option<Vec<u8>> {
    let header = Header::parse(msg).ok()?;
    if header.qr() {
        return None;
    }

    // We implement only the standard query. IQUERY was obsoleted by RFC 3425
    // and the client never sends it, but a definite NOTIMP beats a timeout.
    if header.opcode() != OPCODE_QUERY {
        return Some(build_bare_response(&header, RCODE_NOT_IMPLEMENTED));
    }
    if header.qdcount != 1 {
        return Some(build_bare_response(&header, RCODE_FORMAT_ERROR));
    }

    // Any parse failure here is the peer's fault, not ours -- a truncated
    // question, a reserved label kind, a bad or looping compression pointer,
    // or an over-long name -- and they all mean the same thing to the peer.
    let Ok((question, _)) = Question::parse(msg, crate::wire::HEADER_LEN) else {
        return Some(build_bare_response(&header, RCODE_FORMAT_ERROR));
    };

    let decision = policy.decide(&question);

    // Log every query and what we decided.
    //
    // This costs a line per lookup and is worth it: when the client failed
    // with "Could not connect to the specified address", nothing recorded
    // whether it had even asked, let alone for what name. A resolver that
    // answers silently is a resolver you cannot debug, and the names a real
    // client asks for are themselves a finding -- `docs/guides/jail.md` lists the
    // outbound surface we expect, and this is how we check it.
    log.record(&query_line(&question, &decision, peer, unix_ms()));

    let (rcode, answers) = match decision {
        Decision::Address(addr) => (
            RCODE_NO_ERROR,
            vec![Answer {
                ttl: policy.ttl(),
                addr,
            }],
        ),
        Decision::NoData => (RCODE_NO_ERROR, Vec::new()),
        Decision::NameError => (RCODE_NAME_ERROR, Vec::new()),
    };
    Some(build_response(&header, &question, rcode, &answers))
}

/// Bind a UDP socket for the resolver.
///
/// Port 53 needs `CAP_NET_BIND_SERVICE`; any port above 1023 does not, which
/// is how the tests run.
pub fn bind_udp(addr: SocketAddr) -> io::Result<UdpSocket> {
    UdpSocket::bind(addr)
}

/// Bind the TCP listener for the resolver.
pub fn bind_tcp(addr: SocketAddr) -> io::Result<TcpListener> {
    TcpListener::bind(addr)
}

/// Serve UDP queries until the socket fails persistently.
///
/// Never returns in normal operation. A send that fails affects one client and
/// is logged, not fatal.
pub fn serve_udp(socket: &UdpSocket, policy: &Policy) -> io::Result<()> {
    serve_udp_logged(socket, policy, &QueryLog::stderr())
}

/// [`serve_udp`], writing query lines to `log`.
pub fn serve_udp_logged(socket: &UdpSocket, policy: &Policy, log: &QueryLog) -> io::Result<()> {
    let mut buf = [0u8; UDP_RECV_BUF];
    let mut errors = 0usize;
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                errors = 0;
                // recv_from cannot report more bytes than the buffer holds,
                // but slice defensively rather than trust that.
                let msg = buf.get(..len).unwrap_or(&buf[..]);
                if let Some(response) = handle_message_from(policy, log, msg, Some(from))
                    && let Err(err) = socket.send_to(&response, from)
                {
                    crate::log!("eve-dns: udp send to {from}: {err}");
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => {
                errors += 1;
                crate::log!("eve-dns: udp recv: {err}");
                if errors >= MAX_CONSECUTIVE_ERRORS {
                    return Err(err);
                }
            }
        }
    }
}

/// Serve DNS over TCP until the listener fails persistently.
///
/// One scoped thread per connection, capped at [`MAX_TCP_CONNECTIONS`], so a
/// slow connection cannot block the next query and a flood cannot spawn
/// threads without limit. Scoped threads borrow `policy` directly, so there is
/// no shared-ownership machinery to get wrong.
pub fn serve_tcp(listener: &TcpListener, policy: &Policy) -> io::Result<()> {
    serve_tcp_logged(listener, policy, &QueryLog::stderr())
}

/// [`serve_tcp`], writing query lines to `log`.
pub fn serve_tcp_logged(listener: &TcpListener, policy: &Policy, log: &QueryLog) -> io::Result<()> {
    let live = AtomicUsize::new(0);
    thread::scope(|scope| -> io::Result<()> {
        let mut errors = 0usize;
        loop {
            match listener.accept() {
                Ok((stream, peer)) => {
                    errors = 0;
                    if live.fetch_add(1, Ordering::Relaxed) >= MAX_TCP_CONNECTIONS {
                        live.fetch_sub(1, Ordering::Relaxed);
                        // Dropping the stream closes it; the resolver falls
                        // back to UDP or retries.
                        crate::log!("eve-dns: tcp {peer}: refused, connection limit reached");
                        continue;
                    }
                    let live = &live;
                    scope.spawn(move || {
                        match serve_tcp_connection(stream, peer, policy, log) {
                            Ok(()) => {}
                            // A resolver that got its answer usually just
                            // closes; that is not worth a line of log.
                            Err(err) if err.kind() == ErrorKind::UnexpectedEof => {}
                            Err(err) => crate::log!("eve-dns: tcp {peer}: {err}"),
                        }
                        live.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => {
                    errors += 1;
                    crate::log!("eve-dns: tcp accept: {err}");
                    if errors >= MAX_CONSECUTIVE_ERRORS {
                        return Err(err);
                    }
                }
            }
        }
    })
}

/// Serve queries on one TCP connection until the peer closes it.
///
/// Each message is a 2-byte big-endian length followed by that many bytes
/// (RFC 1035 §4.2.2), and a connection may carry several queries in sequence
/// (RFC 7766 §6.2), so this loops rather than handling one and closing.
fn serve_tcp_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    policy: &Policy,
    log: &QueryLog,
) -> io::Result<()> {
    stream.set_read_timeout(Some(TCP_IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(TCP_WRITE_TIMEOUT))?;
    stream.set_nodelay(true)?;

    let mut prefix = [0u8; TCP_LENGTH_PREFIX_LEN];
    let mut msg = Vec::new();
    loop {
        if !read_exact_or_eof(&mut stream, &mut prefix)? {
            return Ok(());
        }
        let len = usize::from(u16::from_be_bytes(prefix));
        msg.clear();
        msg.resize(len, 0);
        stream.read_exact(&mut msg)?;

        if let Some(response) = handle_message_from(policy, log, &msg, Some(peer)) {
            // The length prefix is 2 bytes, so a response that does not fit in
            // a u16 cannot be framed. build_response caps every response far
            // below this, so it is a bug rather than a peer's fault.
            let framed = u16::try_from(response.len()).map_err(|_| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "response of {} bytes exceeds the TCP prefix",
                        response.len()
                    ),
                )
            })?;
            stream.write_all(&framed.to_be_bytes())?;
            stream.write_all(&response)?;
            stream.flush()?;
        }
    }
}

/// Fill `buf` completely, or report a clean end of stream.
///
/// `Ok(false)` means the peer closed between messages, which is normal.
/// Closing part-way through a length prefix is not, and comes back as
/// `UnexpectedEof`.
fn read_exact_or_eof(stream: &mut TcpStream, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "peer closed mid length-prefix",
                ));
            }
            Ok(n) => filled += n,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        CLASS_IN, FLAG_QR, FLAG_RD, HEADER_LEN, Name, TYPE_A, TYPE_AAAA, parse_query,
    };
    use std::net::Ipv4Addr;

    const TARGET: Ipv4Addr = Ipv4Addr::new(10, 137, 0, 1);

    fn policy() -> Policy {
        Policy::for_jail(TARGET)
    }

    fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut out = Header {
            id,
            flags: FLAG_RD,
            qdcount: 1,
            ancount: 0,
            nscount: 0,
            arcount: 0,
        }
        .to_bytes()
        .to_vec();
        Question {
            name: Name::from_ascii(name).unwrap(),
            qtype,
            qclass: CLASS_IN,
        }
        .write(&mut out);
        out
    }

    #[test]
    fn a_matched_a_query_is_answered_with_the_target() {
        let response = handle_message(
            &policy(),
            &query(0x1111, "tranquility.servers.eveonline.com", TYPE_A),
        )
        .expect("a query deserves a response");
        let header = Header::parse(&response).unwrap();
        assert_eq!(header.id, 0x1111);
        assert_eq!(header.rcode(), RCODE_NO_ERROR);
        assert_eq!(header.ancount, 1);
        assert_eq!(&response[response.len() - 4..], &TARGET.octets());
    }

    #[test]
    fn an_unmatched_name_is_nxdomain() {
        for name in ["sentry.io", "clientresources.eveonline.com"] {
            let response = handle_message(&policy(), &query(2, name, TYPE_A)).unwrap();
            let header = Header::parse(&response).unwrap();
            assert_eq!(header.rcode(), RCODE_NAME_ERROR, "{name}");
            assert_eq!(header.ancount, 0, "{name}");
            assert_eq!(
                response.len(),
                HEADER_LEN + 4 + Name::from_ascii(name).unwrap().encoded_len()
            );
        }
    }

    #[test]
    fn aaaa_on_a_matched_name_is_noerror_with_no_answers() {
        let response = handle_message(
            &policy(),
            &query(3, "tranquility.servers.eveonline.com", TYPE_AAAA),
        )
        .unwrap();
        let header = Header::parse(&response).unwrap();
        assert_eq!(
            header.rcode(),
            RCODE_NO_ERROR,
            "NXDOMAIN here would let the resolver cache the name as absent"
        );
        assert_eq!(header.ancount, 0);
        assert_eq!(header.qdcount, 1, "the question is still echoed");
    }

    #[test]
    fn a_response_is_never_answered() {
        let mut msg = query(4, "tranquility.servers.eveonline.com", TYPE_A);
        let flags = u16::from_be_bytes([msg[2], msg[3]]) | FLAG_QR;
        msg[2..4].copy_from_slice(&flags.to_be_bytes());
        assert_eq!(
            handle_message(&policy(), &msg),
            None,
            "answering a response is how two servers loop forever"
        );
    }

    #[test]
    fn a_runt_datagram_is_dropped() {
        for n in 0..HEADER_LEN {
            assert_eq!(
                handle_message(&policy(), &[0u8; HEADER_LEN][..n]),
                None,
                "{n} bytes"
            );
        }
    }

    #[test]
    fn a_malformed_question_gets_formerr_not_a_panic() {
        let base = query(5, "tranquility.servers.eveonline.com", TYPE_A);
        // Every truncation that still has a header must produce a response,
        // and that response must be a well-formed message.
        for n in HEADER_LEN..base.len() {
            let response = handle_message(&policy(), &base[..n])
                .unwrap_or_else(|| panic!("{n} bytes should get a reply"));
            let header = Header::parse(&response).unwrap();
            assert_eq!(header.id, 5);
            assert_eq!(header.rcode(), RCODE_FORMAT_ERROR, "{n} bytes");
            assert_eq!(header.qdcount, 0, "{n} bytes");
        }
    }

    #[test]
    fn a_pointer_loop_in_a_query_gets_formerr() {
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1; // qdcount
        msg.extend_from_slice(&[0xC0, HEADER_LEN as u8]); // pointer to itself
        let response = handle_message(&policy(), &msg).unwrap();
        assert_eq!(
            Header::parse(&response).unwrap().rcode(),
            RCODE_FORMAT_ERROR
        );
    }

    #[test]
    fn a_non_query_opcode_gets_notimp() {
        let mut msg = query(6, "tranquility.servers.eveonline.com", TYPE_A);
        // Opcode 1 (IQUERY), bits 14..11.
        let flags = u16::from_be_bytes([msg[2], msg[3]]) | (1 << 11);
        msg[2..4].copy_from_slice(&flags.to_be_bytes());
        let response = handle_message(&policy(), &msg).unwrap();
        let header = Header::parse(&response).unwrap();
        assert_eq!(header.rcode(), RCODE_NOT_IMPLEMENTED);
        assert_eq!(header.opcode(), 1, "the opcode is echoed");
    }

    #[test]
    fn a_zero_question_count_gets_formerr() {
        let mut msg = query(7, "tranquility.servers.eveonline.com", TYPE_A);
        msg[4..6].copy_from_slice(&0u16.to_be_bytes());
        let response = handle_message(&policy(), &msg).unwrap();
        assert_eq!(
            Header::parse(&response).unwrap().rcode(),
            RCODE_FORMAT_ERROR
        );
    }

    #[test]
    fn every_response_we_emit_parses_as_a_dns_message() {
        // Fuzz-ish sweep over structurally odd inputs: whatever comes back
        // must be a message, and nothing may panic.
        let policy = policy();
        let mut inputs: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![0xFF; 1],
            vec![0xFF; HEADER_LEN],
            vec![0x00; UDP_RECV_BUF],
            vec![0xC0; 64],
        ];
        let mut all_types = query(8, "tranquility.servers.eveonline.com", TYPE_A);
        for t in [0u16, 1, 28, 255, 65535] {
            let n = all_types.len();
            all_types[n - 4..n - 2].copy_from_slice(&t.to_be_bytes());
            inputs.push(all_types.clone());
        }
        for (i, input) in inputs.iter().enumerate() {
            if let Some(response) = handle_message(&policy, input) {
                let header = Header::parse(&response)
                    .unwrap_or_else(|e| panic!("input {i} produced a bad header: {e}"));
                assert!(header.qr(), "input {i} produced a non-response");
                if header.qdcount == 1 {
                    parse_query(&response[..]).expect_err("it is a response, not a query");
                    Question::parse(&response, HEADER_LEN)
                        .unwrap_or_else(|e| panic!("input {i} echoed a bad question: {e}"));
                }
            }
        }
    }

    /// A unique scratch file for one test, removed on drop.
    struct TempLog(std::path::PathBuf);

    impl TempLog {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("eve-dns-{tag}-{}.log", std::process::id()));
            let _ = std::fs::remove_file(&path);
            Self(path)
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// The fields `tools/jail-drops.sh`'s awk reads from one line: `$1` must
    /// be `query`, `$2` is the name (trailing dot stripped), and the line
    /// counts as answered iff it contains `-> `.
    fn as_jail_drops_reads_it(line: &str) -> Option<(String, bool)> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"query") {
            return None;
        }
        let name = fields.get(1)?.strip_suffix('.').unwrap_or(fields[1]);
        Some((name.to_owned(), line.contains("-> ")))
    }

    #[test]
    fn log_appends_one_line_per_query() {
        let tmp = TempLog::new("append");
        std::fs::write(&tmp.0, "an earlier run's line\n").unwrap();
        let log = QueryLog::open(&tmp.0).unwrap();
        assert_eq!(log.path(), Some(tmp.0.as_path()));
        let policy = policy();

        let matched = "tranquility.servers.eveonline.com";
        let queries = [
            (matched, TYPE_A),
            ("sentry.io", TYPE_A),
            (matched, TYPE_AAAA),
        ];
        for (i, (name, qtype)) in queries.iter().enumerate() {
            let id = u16::try_from(i).unwrap();
            assert!(handle_message_logged(&policy, &log, &query(id, name, *qtype)).is_some());
        }
        // Not queries, so not lines: a runt and a response.
        assert!(handle_message_logged(&policy, &log, &[0u8; 3]).is_none());
        let mut response = query(9, matched, TYPE_A);
        let flags = u16::from_be_bytes([response[2], response[3]]) | FLAG_QR;
        response[2..4].copy_from_slice(&flags.to_be_bytes());
        assert!(handle_message_logged(&policy, &log, &response).is_none());
        drop(log);

        let text = std::fs::read_to_string(&tmp.0).unwrap();
        assert!(text.ends_with('\n'), "every line is terminated");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "an earlier run's line", "open never truncates");
        assert_eq!(lines.len(), 1 + queries.len(), "one line per query: {text}");

        let read: Vec<(String, bool)> = lines[1..]
            .iter()
            .map(|l| as_jail_drops_reads_it(l).unwrap_or_else(|| panic!("unparsed: {l}")))
            .collect();
        assert_eq!(read[0], (matched.to_owned(), true));
        assert_eq!(read[1], ("sentry.io".to_owned(), false));
        assert_eq!(read[2], (matched.to_owned(), false));
        // No transport, so no peer=; the time is always there (CEN-3).
        assert_eq!(
            without_t(lines[1]).0,
            format!("query {matched}. A -> {TARGET}")
        );
        assert_eq!(without_t(lines[2]).0, "query sentry.io. A NXDOMAIN");
    }

    /// A line split into everything before ` t=` and the `t=` value, which
    /// must be the last field and all digits.
    fn without_t(line: &str) -> (&str, u128) {
        let (head, t) = line
            .rsplit_once(" t=")
            .unwrap_or_else(|| panic!("no t= field: {line}"));
        let t = t
            .parse()
            .unwrap_or_else(|_| panic!("t= is not a number: {line}"));
        (head, t)
    }

    #[test]
    fn the_census_fields_name_the_peer_and_the_time() {
        let tmp = TempLog::new("census");
        let log = QueryLog::open(&tmp.0).unwrap();
        let policy = policy();
        // Documentation addresses (RFC 5737, RFC 3849): invented peers.
        let v4: SocketAddr = "192.0.2.2:40001".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::2]:40002".parse().unwrap();
        let before = unix_ms();
        for (id, peer) in [(1u16, Some(v4)), (2, Some(v6)), (3, None)] {
            let msg = query(id, "canary.census.invalid", TYPE_A);
            assert!(handle_message_from(&policy, &log, &msg, peer).is_some());
        }
        let after = unix_ms();
        drop(log);

        let text = std::fs::read_to_string(&tmp.0).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        let want = [
            "query canary.census.invalid. A NXDOMAIN peer=192.0.2.2:40001",
            "query canary.census.invalid. A NXDOMAIN peer=[2001:db8::2]:40002",
            "query canary.census.invalid. A NXDOMAIN",
        ];
        for (line, want) in lines.iter().zip(want) {
            let (head, t) = without_t(line);
            assert_eq!(head, want);
            assert!(
                (before..=after).contains(&t),
                "t={t} outside {before}..={after}"
            );
            // jail-drops.sh's fields are unchanged by the appended ones.
            assert_eq!(
                as_jail_drops_reads_it(line),
                Some(("canary.census.invalid".to_owned(), false))
            );
        }
    }

    #[test]
    fn the_listeners_log_who_asked() {
        // Over real loopback sockets: the UDP loop names the datagram's
        // source, the TCP loop the connection's peer.
        let tmp = TempLog::new("listeners");
        let log = QueryLog::open(&tmp.0).unwrap();
        let policy = policy();
        let udp = bind_udp("127.0.0.1:0".parse().unwrap()).unwrap();
        let tcp = bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
        let (udp_addr, tcp_addr) = (udp.local_addr().unwrap(), tcp.local_addr().unwrap());
        thread::scope(|scope| {
            let log = &log;
            let policy = &policy;
            scope.spawn(move || {
                let mut buf = [0u8; UDP_RECV_BUF];
                let (len, from) = udp.recv_from(&mut buf).unwrap();
                let reply = handle_message_from(policy, log, &buf[..len], Some(from)).unwrap();
                udp.send_to(&reply, from).unwrap();
            });
            scope.spawn(move || {
                let (stream, peer) = tcp.accept().unwrap();
                serve_tcp_connection(stream, peer, policy, log).unwrap();
            });
            let client = UdpSocket::bind("127.0.0.1:0").unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            client
                .send_to(&query(1, "u.census.invalid", TYPE_A), udp_addr)
                .unwrap();
            let mut buf = [0u8; UDP_RECV_BUF];
            client.recv_from(&mut buf).unwrap();
            let mut stream = TcpStream::connect(tcp_addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let msg = query(2, "t.census.invalid", TYPE_A);
            let len = u16::try_from(msg.len()).unwrap();
            stream.write_all(&len.to_be_bytes()).unwrap();
            stream.write_all(&msg).unwrap();
            let mut prefix = [0u8; TCP_LENGTH_PREFIX_LEN];
            stream.read_exact(&mut prefix).unwrap();
            // Read the whole answer: closing with unread bytes is a reset,
            // which the server side would report as an error.
            let mut body = vec![0u8; usize::from(u16::from_be_bytes(prefix))];
            stream.read_exact(&mut body).unwrap();
            drop(stream);
        });
        drop(log);
        let text = std::fs::read_to_string(&tmp.0).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        for (name, line) in [("u", text.lines().next()), ("t", text.lines().nth(1))] {
            let line = line.unwrap();
            assert!(
                line.starts_with(&format!(
                    "query {name}.census.invalid. A NXDOMAIN peer=127.0.0.1:"
                )),
                "{line}"
            );
            without_t(line);
        }
    }

    #[test]
    fn a_hostile_name_cannot_forge_a_log_line() {
        // A label holding a space and a newline: unescaped, the newline
        // would start a second line that jail-drops.sh counts as a query,
        // and the space would shift the fields it reads.
        let mut msg = Header {
            id: 10,
            flags: FLAG_RD,
            qdcount: 1,
            ancount: 0,
            nscount: 0,
            arcount: 0,
        }
        .to_bytes()
        .to_vec();
        msg.extend_from_slice(b"\x0ba b\nquery x");
        msg.extend_from_slice(&[0, 0, 1, 0, 1]);

        let tmp = TempLog::new("hostile");
        let log = QueryLog::open(&tmp.0).unwrap();
        assert!(handle_message_logged(&policy(), &log, &msg).is_some());
        drop(log);

        let text = std::fs::read_to_string(&tmp.0).unwrap();
        assert_eq!(text.lines().count(), 1, "{text:?}");
        let fields: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(fields.len(), 5, "{text:?}");
        assert_eq!(fields[3], "NXDOMAIN");
        assert!(fields[4].starts_with("t="), "{text:?}");
    }

    #[test]
    fn a_failed_log_write_is_an_error_not_silence() {
        // /dev/full accepts the open and fails every write with ENOSPC.
        let log = QueryLog::open(Path::new("/dev/full")).unwrap();
        assert!(log.write_line("query sentry.io. A NXDOMAIN").is_err());
    }
}
