//! End-to-end tests over real sockets.
//!
//! Everything binds `127.0.0.1:0` and reads the port back, because these run
//! concurrently with each other and with whatever else is on the machine; a
//! hardcoded port is a flaky test waiting to happen. Loopback is also all the
//! build container has — `tools/dev.sh` runs it with `--network none` — which
//! is exactly enough.
//!
//! The server threads are never joined. Each loop runs until its socket fails,
//! which does not happen here, so they end when the test process does.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use eve_dns::wire::{
    CLASS_IN, FLAG_AA, FLAG_QR, FLAG_RD, HEADER_LEN, Header, Name, Question, RCODE_NAME_ERROR,
    RCODE_NO_ERROR, TYPE_A, TYPE_AAAA,
};
use eve_dns::{Policy, bind_tcp, bind_udp, serve_tcp, serve_udp};

const TARGET: Ipv4Addr = Ipv4Addr::new(10, 137, 0, 1);
const MATCHED: &str = "tranquility.servers.eveonline.com";
const TTL: u32 = 60;
/// Generous: these are loopback round trips that normally take microseconds.
/// It exists so a bug stalls the suite for a second rather than forever.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

fn policy() -> Arc<Policy> {
    let mut policy = Policy::for_jail(TARGET);
    policy.set_ttl(TTL);
    Arc::new(policy)
}

/// A query as a stub resolver builds it: one question, RD set, nothing else.
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

/// Start the UDP server on an OS-chosen loopback port.
fn start_udp() -> SocketAddr {
    let socket = bind_udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let addr = socket.local_addr().unwrap();
    let policy = policy();
    thread::spawn(move || serve_udp(&socket, &policy));
    addr
}

/// Start the TCP server on an OS-chosen loopback port.
fn start_tcp() -> SocketAddr {
    let listener = bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let addr = listener.local_addr().unwrap();
    let policy = policy();
    thread::spawn(move || serve_tcp(&listener, &policy));
    addr
}

fn ask_udp(server: SocketAddr, message: &[u8]) -> Vec<u8> {
    let client = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    client.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    client.send_to(message, server).unwrap();
    let mut buf = [0u8; 2048];
    let (len, from) = client
        .recv_from(&mut buf)
        .expect("no reply from the server");
    assert_eq!(from, server, "the reply must come from the server we asked");
    buf[..len].to_vec()
}

fn ask_tcp(stream: &mut TcpStream, message: &[u8]) -> Vec<u8> {
    let len = u16::try_from(message.len()).unwrap();
    stream.write_all(&len.to_be_bytes()).unwrap();
    stream.write_all(message).unwrap();
    stream.flush().unwrap();

    let mut prefix = [0u8; 2];
    stream.read_exact(&mut prefix).expect("no reply length");
    let mut response = vec![0u8; usize::from(u16::from_be_bytes(prefix))];
    stream.read_exact(&mut response).expect("short reply");
    response
}

fn connect(server: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(server).unwrap();
    stream.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(CLIENT_TIMEOUT)).unwrap();
    stream
}

/// The exact bytes a matched `A` query must come back as.
///
/// Spelled out rather than recomputed with our own encoder, so that a change
/// to the encoder has to be defended here instead of silently agreeing with
/// itself.
fn expected_matched_response(id: u16) -> Vec<u8> {
    let mut expect = Vec::new();
    expect.extend_from_slice(&id.to_be_bytes());
    // QR + AA + RD echoed, RA clear, Z clear, RCODE 0.
    expect.extend_from_slice(&(FLAG_QR | FLAG_AA | FLAG_RD | RCODE_NO_ERROR).to_be_bytes());
    expect.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    expect.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    expect.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    expect.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    // Question, name uncompressed.
    expect.extend_from_slice(b"\x0btranquility\x07servers\x09eveonline\x03com\x00");
    expect.extend_from_slice(&TYPE_A.to_be_bytes());
    expect.extend_from_slice(&CLASS_IN.to_be_bytes());
    // Answer: owner name is a pointer to offset 12, the question's name.
    expect.extend_from_slice(&[0xC0, 0x0C]);
    expect.extend_from_slice(&TYPE_A.to_be_bytes());
    expect.extend_from_slice(&CLASS_IN.to_be_bytes());
    expect.extend_from_slice(&TTL.to_be_bytes());
    expect.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    expect.extend_from_slice(&TARGET.octets());
    expect
}

#[test]
fn udp_answers_a_matched_name_with_the_target_address() {
    let server = start_udp();
    let response = ask_udp(server, &query(0xABCD, MATCHED, TYPE_A));
    assert_eq!(response, expected_matched_response(0xABCD));
}

#[test]
fn udp_nxdomains_an_unmatched_name() {
    let server = start_udp();
    // Both of these are names tools/jail-verify.sh asserts must not resolve.
    for (id, name) in [(1u16, "sentry.io"), (2, "clientresources.eveonline.com")] {
        let response = ask_udp(server, &query(id, name, TYPE_A));
        let header = Header::parse(&response).unwrap();
        assert_eq!(header.id, id, "{name}");
        assert!(header.qr(), "{name}");
        assert_eq!(header.rcode(), RCODE_NAME_ERROR, "{name} must be NXDOMAIN");
        assert_eq!(header.ancount, 0, "{name} must carry no address");
        assert!(
            !response[HEADER_LEN..]
                .windows(4)
                .any(|w| w == TARGET.octets()),
            "{name} must not leak the target address anywhere in the message"
        );
    }
}

#[test]
fn udp_answers_aaaa_on_a_matched_name_with_noerror_and_no_records() {
    // The pinned subtlety: NXDOMAIN here would let a dual-stack resolver cache
    // the whole name as nonexistent and never ask for the A record that works.
    let server = start_udp();
    let response = ask_udp(server, &query(0x0F0F, MATCHED, TYPE_AAAA));
    let header = Header::parse(&response).unwrap();
    assert_eq!(header.id, 0x0F0F);
    assert_eq!(
        header.rcode(),
        RCODE_NO_ERROR,
        "AAAA on a name we serve is NODATA, not NXDOMAIN"
    );
    assert_eq!(header.ancount, 0);
    assert_eq!(header.qdcount, 1);
    // Question echoed verbatim, and nothing after it.
    let (question, end) = Question::parse(&response, HEADER_LEN).unwrap();
    assert_eq!(question.name, Name::from_ascii(MATCHED).unwrap());
    assert_eq!(question.qtype, TYPE_AAAA);
    assert_eq!(end, response.len(), "no records follow the question");
}

#[test]
fn udp_would_fail_if_the_policy_were_inverted() {
    // The directional pair, over a real socket: the matched name must get an
    // address and the unmatched one must not. Swapping the policy's match test
    // fails both assertions.
    let server = start_udp();
    let matched = ask_udp(server, &query(10, MATCHED, TYPE_A));
    let unmatched = ask_udp(server, &query(11, "sentry.io", TYPE_A));
    assert_eq!(Header::parse(&matched).unwrap().rcode(), RCODE_NO_ERROR);
    assert_eq!(Header::parse(&matched).unwrap().ancount, 1);
    assert_eq!(Header::parse(&unmatched).unwrap().rcode(), RCODE_NAME_ERROR);
    assert_eq!(Header::parse(&unmatched).unwrap().ancount, 0);
}

#[test]
fn udp_survives_a_hostile_datagram_and_keeps_serving() {
    let server = start_udp();
    let client = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    client.set_read_timeout(Some(CLIENT_TIMEOUT)).unwrap();

    // A pointer loop, a truncated question, and random noise. None of these
    // may take the server down; some get FORMERR, some get nothing.
    let hostile: Vec<Vec<u8>> = vec![
        vec![0x00, 0x01, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 0x0C],
        vec![0x00, 0x02, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 63],
        vec![0xFF; 600],
        Vec::new(),
        vec![0x41],
    ];
    for message in &hostile {
        client.send_to(message, server).unwrap();
    }
    // Drain whatever replies came back, then prove the server still works.
    client
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut buf = [0u8; 2048];
    while client.recv_from(&mut buf).is_ok() {}

    let response = ask_udp(server, &query(0x5555, MATCHED, TYPE_A));
    assert_eq!(
        response,
        expected_matched_response(0x5555),
        "the server must still answer after being fed garbage"
    );
}

#[test]
fn tcp_answers_with_a_length_prefix() {
    let server = start_tcp();
    let mut stream = connect(server);
    let response = ask_tcp(&mut stream, &query(0xABCD, MATCHED, TYPE_A));
    assert_eq!(
        response,
        expected_matched_response(0xABCD),
        "the TCP payload is the same message UDP carries"
    );
}

#[test]
fn tcp_serves_several_queries_on_one_connection() {
    // RFC 7766 §6.2: a resolver may reuse a kept-open connection.
    let server = start_tcp();
    let mut stream = connect(server);

    let matched = ask_tcp(&mut stream, &query(1, MATCHED, TYPE_A));
    assert_eq!(Header::parse(&matched).unwrap().rcode(), RCODE_NO_ERROR);

    let unmatched = ask_tcp(&mut stream, &query(2, "sentry.io", TYPE_A));
    let header = Header::parse(&unmatched).unwrap();
    assert_eq!(header.id, 2);
    assert_eq!(header.rcode(), RCODE_NAME_ERROR);

    let aaaa = ask_tcp(&mut stream, &query(3, MATCHED, TYPE_AAAA));
    let header = Header::parse(&aaaa).unwrap();
    assert_eq!(header.rcode(), RCODE_NO_ERROR);
    assert_eq!(header.ancount, 0);
}

#[test]
fn tcp_accepts_a_new_connection_after_one_closes() {
    let server = start_tcp();
    for id in 0..4u16 {
        let mut stream = connect(server);
        let response = ask_tcp(&mut stream, &query(id, MATCHED, TYPE_A));
        assert_eq!(Header::parse(&response).unwrap().id, id);
        drop(stream);
    }
}

#[test]
fn tcp_tolerates_a_connection_that_sends_nothing() {
    let server = start_tcp();
    let idle = connect(server);
    // Opening and closing without sending a byte is a clean EOF, not an error,
    // and must not disturb the next client.
    drop(idle);
    let mut stream = connect(server);
    let response = ask_tcp(&mut stream, &query(99, MATCHED, TYPE_A));
    assert_eq!(response, expected_matched_response(99));
}

// --- the binary itself ---------------------------------------------------
//
// `tools/jail-setup.sh` will launch the process, not the library, so the
// argument handling and the two listeners it starts are worth exercising for
// real. The binary is asked to bind port 0 and reports back what the kernel
// gave it, which keeps this free of hardcoded ports.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

/// Kills the child on the way out, including when a test panics.
struct Server {
    child: Child,
    udp: SocketAddr,
    tcp: Option<SocketAddr>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Launch the binary with `args` and wait for it to report its ports.
fn spawn_binary(args: &[&str]) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_eve-dns"))
        .args(["--bind", "127.0.0.1:0"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");

    // Drain the child's stderr for as long as it lives, forwarding the banner
    // once. Letting the pipe fill, or closing it early, would interfere with
    // the process under test.
    let stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(rest) = line.strip_prefix("eve-dns: listening ") {
                let _ = tx.send(rest.to_string());
            }
        }
    });
    let banner = rx
        .recv_timeout(CLIENT_TIMEOUT)
        .expect("the binary should report its ports");

    // "udp 127.0.0.1:NNNN tcp 127.0.0.1:NNNN" or "... tcp none"
    let mut fields = banner.split_whitespace();
    assert_eq!(fields.next(), Some("udp"), "banner was {banner:?}");
    let udp = fields.next().unwrap().parse().unwrap();
    assert_eq!(fields.next(), Some("tcp"), "banner was {banner:?}");
    let tcp = match fields.next().unwrap() {
        "none" => None,
        addr => Some(addr.parse().unwrap()),
    };
    Server { child, udp, tcp }
}

#[test]
fn the_binary_serves_its_defaults_on_both_transports() {
    let server = spawn_binary(&["--target", "10.137.0.1"]);
    let response = ask_udp(server.udp, &query(0xABCD, MATCHED, TYPE_A));
    assert_eq!(
        response,
        expected_matched_response(0xABCD),
        "the default suffix is servers.eveonline.com and the default TTL is 60"
    );

    let mut stream = connect(server.tcp.expect("tcp is on by default"));
    let over_tcp = ask_tcp(&mut stream, &query(0xABCD, MATCHED, TYPE_A));
    assert_eq!(over_tcp, response);

    let denied = ask_udp(server.udp, &query(2, "sentry.io", TYPE_A));
    assert_eq!(
        Header::parse(&denied).unwrap().rcode(),
        RCODE_NAME_ERROR,
        "anything outside the suffix list is a name error"
    );
}

#[test]
fn the_binary_honours_suffix_target_ttl_and_no_tcp() {
    let server = spawn_binary(&[
        "--target",
        "127.0.0.9",
        "--suffix",
        "example.test",
        "--suffix",
        "other.test",
        "--ttl",
        "7",
        "--no-tcp",
    ]);
    assert_eq!(server.tcp, None, "--no-tcp must leave TCP unbound");

    for name in ["a.example.test", "other.test"] {
        let response = ask_udp(server.udp, &query(1, name, TYPE_A));
        let header = Header::parse(&response).unwrap();
        assert_eq!(header.rcode(), RCODE_NO_ERROR, "{name}");
        assert_eq!(header.ancount, 1, "{name}");
        assert_eq!(&response[response.len() - 4..], &[127, 0, 0, 9], "{name}");
        let ttl_at = response.len() - 10;
        assert_eq!(
            u32::from_be_bytes(response[ttl_at..ttl_at + 4].try_into().unwrap()),
            7,
            "{name}"
        );
    }

    // The first --suffix replaces the default, so the EVE name is gone.
    let response = ask_udp(server.udp, &query(2, MATCHED, TYPE_A));
    assert_eq!(
        Header::parse(&response).unwrap().rcode(),
        RCODE_NAME_ERROR,
        "--suffix replaces the default list rather than adding to it"
    );
}

#[test]
fn the_binary_rejects_bad_arguments() {
    for args in [
        vec!["--bind", "not-an-address"],
        vec!["--target", "::1"],
        vec!["--ttl", "soon"],
        vec!["--suffix", "a..b"],
        vec!["--nonsense"],
        vec!["--bind"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_eve-dns"))
            .args(&args)
            .output()
            .expect("the binary should start");
        assert!(
            !output.status.success(),
            "{args:?} should have been refused"
        );
    }

    let help = Command::new(env!("CARGO_BIN_EXE_eve-dns"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("NXDOMAIN"));
}
