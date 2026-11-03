//! `eve-dns`: the stub resolver process, as `tools/jail-setup.sh` will launch
//! it.
//!
//! Defaults match the jail described in `docs/guides/jail.md` — the host end of the
//! veth pair is `10.137.0.1`, which is both the only address the namespace can
//! reach and the address the client must be told its server lives at — so in
//! the normal case this takes no arguments at all. Everything is overridable
//! so that the whole thing is testable on a high port without root.

use std::env;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;

use eve_dns::server::{QueryLog, serve_tcp_logged, serve_udp_logged};
use eve_dns::{EVE_SERVERS_SUFFIX, Policy, bind_tcp, bind_udp};

/// The host side of the jail's veth pair (`EVE_JAIL_HOST_IP` in
/// `tools/jail-setup.sh`), which is where our server listens and the only
/// address the namespace can route to.
const DEFAULT_TARGET: Ipv4Addr = Ipv4Addr::new(10, 137, 0, 1);
/// Port 53 on that address, which the jail's nftables rules allow for both
/// UDP and TCP.
const DEFAULT_PORT: u16 = 53;

const USAGE: &str = "\
eve-dns -- stub DNS resolver for the EVE client jail

usage: eve-dns [options]

  -b, --bind ADDR     listen address and port  [10.137.0.1:53]
  -t, --target IP     address matched names resolve to  [10.137.0.1]
  -s, --suffix NAME   resolve NAME and anything beneath it; repeatable.
                      The first --suffix replaces the default list.
                      [servers.eveonline.com]
      --ttl SECONDS   TTL on answered records  [60]
      --no-tcp        serve UDP only
      --log FILE      append one line per query to FILE (created 0600,
                      never truncated) instead of stderr. Refused as root:
                      run it after the privilege drop, so the user, not
                      root, opens the path.
  -h, --help          this text

Each option also has an environment variable: EVE_DNS_BIND, EVE_DNS_TARGET,
EVE_DNS_SUFFIX (comma-separated), EVE_DNS_TTL, EVE_DNS_NO_TCP, EVE_DNS_LOG.
Command-line
options win.

Names matching a suffix resolve to the target address. Everything else gets
NXDOMAIN. A matched name queried for anything other than A gets NOERROR with
no answers, so that a dual-stack resolver does not cache the name as absent.

Each query is logged as one line, which tools/jail-drops.sh reads:
  query tranquility.servers.eveonline.com. A -> 10.137.0.1
  query sentry.io. A NXDOMAIN
If a line cannot be written to the --log file, eve-dns exits (status 3)
rather than answer queries it cannot record.

Binding port 53 needs root or CAP_NET_BIND_SERVICE; any port above 1023 does
not, which is how the tests exercise this.";

struct Config {
    bind: SocketAddr,
    target: Ipv4Addr,
    suffixes: Vec<String>,
    ttl: u32,
    tcp: bool,
    log: Option<PathBuf>,
}

/// A usage error, reported to the operator rather than propagated.
struct ArgError(String);

impl Config {
    /// Read the environment, then let the command line override it.
    fn parse() -> Result<Option<Self>, ArgError> {
        let mut config = Config {
            bind: SocketAddr::from((DEFAULT_TARGET, DEFAULT_PORT)),
            target: DEFAULT_TARGET,
            suffixes: vec![EVE_SERVERS_SUFFIX.to_string()],
            ttl: eve_dns::DEFAULT_TTL,
            tcp: true,
            log: None,
        };

        if let Ok(value) = env::var("EVE_DNS_BIND") {
            config.bind = parse_bind(&value)?;
        }
        if let Ok(value) = env::var("EVE_DNS_TARGET") {
            config.target = parse_target(&value)?;
        }
        if let Ok(value) = env::var("EVE_DNS_SUFFIX") {
            config.suffixes = value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Ok(value) = env::var("EVE_DNS_TTL") {
            config.ttl = parse_ttl(&value)?;
        }
        if env::var_os("EVE_DNS_NO_TCP").is_some() {
            config.tcp = false;
        }
        if let Some(value) = env::var_os("EVE_DNS_LOG") {
            config.log = Some(PathBuf::from(value));
        }

        let mut args = env::args().skip(1);
        // The first --suffix on the command line replaces the defaults rather
        // than adding to them; later ones accumulate. Otherwise there would be
        // no way to serve a different set of names.
        let mut suffix_seen = false;
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| ArgError(format!("{arg} needs a value")))
            };
            match arg.as_str() {
                "-h" | "--help" => return Ok(None),
                "-b" | "--bind" => config.bind = parse_bind(&value()?)?,
                "-t" | "--target" => config.target = parse_target(&value()?)?,
                "-s" | "--suffix" => {
                    let suffix = value()?;
                    if !suffix_seen {
                        config.suffixes.clear();
                        suffix_seen = true;
                    }
                    config.suffixes.push(suffix);
                }
                "--ttl" => config.ttl = parse_ttl(&value()?)?,
                "--no-tcp" => config.tcp = false,
                "--log" => config.log = Some(PathBuf::from(value()?)),
                other => return Err(ArgError(format!("unknown option {other}"))),
            }
        }
        Ok(Some(config))
    }
}

fn parse_bind(value: &str) -> Result<SocketAddr, ArgError> {
    value
        .parse()
        .map_err(|_| ArgError(format!("not an address:port: {value}")))
}

fn parse_target(value: &str) -> Result<Ipv4Addr, ArgError> {
    value
        .parse()
        .map_err(|_| ArgError(format!("not an IPv4 address: {value}")))
}

fn parse_ttl(value: &str) -> Result<u32, ArgError> {
    value
        .parse()
        .map_err(|_| ArgError(format!("not a TTL in seconds: {value}")))
}

/// The effective uid from a `/proc/self/status` text: the second number on
/// its `Uid:` line (real, effective, saved, filesystem; proc(5)).
fn effective_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Open the `--log` file, refusing to do it as root.
///
/// The path is under the repo, which the user can rewrite -- a symlink there
/// would have root open whatever it points at (V6 F3). So the open happens
/// only after `setpriv` has dropped to the user, and this checks that it
/// did rather than trusting the launcher. If the uid cannot be read the
/// answer is no: an unaudited resolver is worse than none.
fn open_query_log(path: &Path) -> Result<QueryLog, String> {
    let status = fs::read_to_string("/proc/self/status")
        .map_err(|err| format!("cannot read /proc/self/status to check the uid: {err}"))?;
    match effective_uid(&status) {
        Some(0) => {
            return Err("refusing --log as root; drop privileges first (setpriv)".to_owned());
        }
        Some(_) => {}
        None => return Err("no effective uid in /proc/self/status".to_owned()),
    }
    QueryLog::open(path).map_err(|err| format!("cannot open --log {}: {err}", path.display()))
}

fn main() -> ExitCode {
    let config = match Config::parse() {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(ArgError(message)) => {
            eve_dns::log!("eve-dns: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let mut policy = Policy::new(config.target);
    policy.set_ttl(config.ttl);
    for suffix in &config.suffixes {
        if let Err(err) = policy.allow_suffix(suffix) {
            eve_dns::log!("eve-dns: bad suffix {suffix:?}: {err}");
            return ExitCode::from(2);
        }
    }
    if policy.suffixes().is_empty() {
        // Legal, and occasionally what you want for a leak test, but never an
        // accident worth staying quiet about.
        eve_dns::log!("eve-dns: warning: no suffixes configured; every name will be NXDOMAIN");
    }

    let udp = match bind_udp(config.bind) {
        Ok(socket) => socket,
        Err(err) => {
            eve_dns::log!("eve-dns: cannot bind udp {}: {err}", config.bind);
            if config.bind.port() < 1024 {
                eve_dns::log!("eve-dns: a port below 1024 needs root or CAP_NET_BIND_SERVICE");
            }
            return ExitCode::FAILURE;
        }
    };
    let tcp = if config.tcp {
        match bind_tcp(config.bind) {
            Ok(listener) => Some(listener),
            Err(err) => {
                eve_dns::log!("eve-dns: cannot bind tcp {}: {err}", config.bind);
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    // Opened after binding and before serving: no query goes unrecorded,
    // and a log that cannot be opened stops us before the client relies on
    // us.
    let query_log = match &config.log {
        None => QueryLog::stderr(),
        Some(path) => match open_query_log(path) {
            Ok(log) => log,
            Err(message) => {
                eve_dns::log!("eve-dns: {message}");
                return ExitCode::FAILURE;
            }
        },
    };

    // Report the addresses actually bound, not the ones asked for: with a port
    // of 0 the kernel picks, and the operator -- or a test driving this binary
    // -- needs to know what it picked.
    let udp_addr = udp.local_addr().unwrap_or(config.bind);
    let tcp_addr = tcp.as_ref().and_then(|l| l.local_addr().ok());
    let suffixes: Vec<String> = policy.suffixes().iter().map(ToString::to_string).collect();
    match tcp_addr {
        Some(addr) => eve_dns::log!("eve-dns: listening udp {udp_addr} tcp {addr}"),
        None => eve_dns::log!("eve-dns: listening udp {udp_addr} tcp none"),
    }
    eve_dns::log!(
        "eve-dns: {} -> {} ttl {}",
        suffixes.join(" "),
        policy.target(),
        policy.ttl(),
    );
    if let Some(path) = query_log.path() {
        eve_dns::log!("eve-dns: query log {}", path.display());
    }

    // Both loops run until their socket fails for good, which for a resolver
    // means something is badly wrong; report it and exit non-zero rather than
    // linger with one transport dead.
    let failed = thread::scope(|scope| {
        let udp_thread = scope.spawn(|| serve_udp_logged(&udp, &policy, &query_log));
        if let Some(listener) = tcp.as_ref() {
            scope.spawn(|| {
                if let Err(err) = serve_tcp_logged(listener, &policy, &query_log) {
                    eve_dns::log!("eve-dns: tcp listener stopped: {err}");
                }
            });
        }
        match udp_thread.join() {
            Ok(Ok(())) => false,
            Ok(Err(err)) => {
                eve_dns::log!("eve-dns: udp socket stopped: {err}");
                true
            }
            Err(_) => {
                eve_dns::log!("eve-dns: udp thread panicked");
                true
            }
        }
    });

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::effective_uid;

    #[test]
    fn the_effective_uid_is_the_second_field_of_uid() {
        let status = "Name:\teve-dns\nUid:\t1000\t0\t1000\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(
            effective_uid(status),
            Some(0),
            "real 1000, effective 0 is root"
        );
        assert_eq!(effective_uid("Uid:\t7\t7\t7\t7\n"), Some(7));
        assert_eq!(effective_uid("Name:\tx\n"), None);
        assert_eq!(effective_uid("Uid:\t7\n"), None);
    }
}
