//! The stub DNS resolver the jailed client resolves through.
//!
//! Why this exists. The modified client runs in a network namespace with no
//! default route and a default-deny egress policy (`docs/guides/jail.md`), because
//! the requirement is that it cannot phone home at all — not that it is
//! configured not to. But the client still resolves its server hostname at
//! startup: `serverInfo._GetServerIP` builds
//! `"{}.servers.eveonline.com".format(serverName)` and looks it up. Rather
//! than let that resolution escape, the jail's `resolv.conf` points at this
//! resolver, which answers the EVE hostnames with our own server's address
//! and returns `NXDOMAIN` for everything else. No client file is modified —
//! `manifest.dat` is signed and covers `start.ini`, so editing the server
//! address there would trip the integrity check.
//!
//! That makes DNS the jail's canary as well as its redirection mechanism:
//! `tools/jail-verify.sh` asserts that `sentry.io` and
//! `clientresources.eveonline.com` do **not** resolve, so a policy that
//! answered them would silently disarm the leak test.
//!
//! Source of truth for the wire format is RFC 1035 (§4.1.1 header and RCODEs,
//! §4.1.2 question, §4.1.3 resource record, §4.1.4 compression, §2.3.4 size
//! limits, §4.2.2 TCP framing), with RFC 3596 §2.1 for `AAAA`, RFC 2181 §8 for
//! the TTL range, and RFC 2308 §2 for the NXDOMAIN/NODATA distinction that
//! [`policy::Decision`] turns on. Sections are cited at each constant.
//!
//! Shape of the crate:
//!
//! - [`wire`] parses a query and builds a response. It assumes every byte is
//!   hostile and returns errors instead of panicking.
//! - [`policy`] decides which names resolve, to what, and what to say about
//!   the ones that do not.
//! - [`server`] is [`handle_message`], the entire server as a pure
//!   bytes-in/bytes-out function, plus the UDP and TCP loops that feed it.
//!
//! ```no_run
//! use std::net::Ipv4Addr;
//! use eve_dns::{Policy, bind_udp, serve_udp};
//!
//! let policy = Policy::for_jail(Ipv4Addr::new(10, 137, 0, 1));
//! let socket = bind_udp("10.137.0.1:53".parse().unwrap())?;
//! serve_udp(&socket, &policy)?;
//! # Ok::<(), std::io::Error>(())
//! ```

#![forbid(unsafe_code)]

/// Write one diagnostic line to stderr, ignoring a failure to write it.
///
/// `eprintln!` panics if stderr cannot be written, and the most plausible way
/// for that to happen is whatever launched us going away and closing the pipe.
/// A resolver the jailed client depends on must not die because nobody is
/// reading its log; it must keep resolving.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = writeln!(::std::io::stderr(), $($arg)*);
    }};
}

pub mod policy;
pub mod server;
pub mod wire;

pub use policy::{DEFAULT_TTL, Decision, EVE_SERVERS_SUFFIX, Policy};
pub use server::{
    MAX_TCP_CONNECTIONS, TCP_IDLE_TIMEOUT, UDP_RECV_BUF, bind_tcp, bind_udp, handle_message,
    serve_tcp, serve_udp,
};
pub use wire::{
    Answer, DnsError, HEADER_LEN, Header, MAX_LABEL_LEN, MAX_NAME_LEN, MAX_UDP_PAYLOAD, Name,
    Query, Question, RCODE_FORMAT_ERROR, RCODE_NAME_ERROR, RCODE_NO_ERROR, RCODE_NOT_IMPLEMENTED,
    TCP_LENGTH_PREFIX_LEN, TYPE_A, TYPE_AAAA, build_response, parse_query,
};
