//! **The command line and the run loop** of every `eve-server` binary:
//! `eve_server::cli::run(&[modules])` is the whole of `main` (PR-SEAM-1). The
//! core binary passes no modules; a distribution with modules passes its own,
//! and their [`crate::module::Flag`]s are accepted after the core flags.
//!
//! Run the listener.
//!
//!   eve-server [--bind LISTENER]... [--capture DIR] [--mode gps|capture]
//!              [--user-id N | --accounts FILE] [--proxy-node N] [--once]
//!              [--push-probe N] [--client-build N[,M]]
//!   eve-server [--accounts FILE] --new-token NAME ROLE [START_CHARACTER]
//!   eve-server --world FILE --grant USERNAME ROLE
//!   eve-server --world FILE --mark-bot USERNAME [off]
//!
//! Defaults bind `127.0.0.1:26000`, which is deliberate: the client jail
//! (`docs/guides/jail.md`) puts the client in its own network namespace and points it
//! here, and a server that defaults to a public interface in a project whose
//! requirement is "no phoning home whatsoever" would be the wrong default to
//! have shipped even once.
//!
//! The real machoNet port is 26000 for a direct connection; the jail's stub
//! resolver answers `*.servers.eveonline.com` with our address.

use crate::authored;
use crate::content;
use crate::handshake;
use crate::module::{self, Assembled, Module};
use crate::server::{Config, Listener, Mode, Server};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: eve-server [options]

  --bind LISTENER    a machoNet listener. Repeatable; the first replaces the
                     default 127.0.0.1:26000. One thread and one live cap per
                     listener. LISTENER is
                       ADDR[,ceiling=gm|player][,cap=N][,account=UID]
                           [,chat=ADDR][,advertise=IP][,signup=on|off]
                           [,capture=on|off][,botnames=on|off]
                     cap: live connections allowed at once; past it a new one
                       is closed at once (default 128 on loopback or the
                       jail host, 16 elsewhere).
                     chat: where chat listens beside this listener (turns
                       chat on there; --chat puts it beside every one);
                       advertise: the address its clients are told to dial
                       for chat.
                     Without --accounts, refused at startup: any address
                     that is not loopback or the jail host 10.137.0.1,
                     ceiling=player, and an account= other than --user-id.
                     With --accounts: account= must name a line of FILE.
                     signup=on (default off; --world-accounts only): a
                       login with an unknown user name signs up, as a
                       player with one character of that name.
                     capture=off: with --capture, this listener's
                       connections are not recorded (default on).
                     botnames=on (default off; signup=on only): a sign-up
                       gets a character name from content/bots/names.tsv
                       (picked from the account name, e.g. hulka3), not the
                       account's own. tools/bot-accounts.sh turns it on.
  --client-build N[,M]
                     the client builds this server answers (default: every
                     build eve_marshal::build::BUILDS / tools/builds.tsv
                     marks pinned or equivalent). Each must be registered
                     and all on one release line; H1 advertises the lowest.
                     Accounts modes refuse any other build; the catch-all
                     lets a newer one in with a warning. The content bundle
                     must come from one of them (or carry a data_hash the
                     registry proved for one).
  --accounts FILE    accounts mode. FILE (outside every repo, not writable
                     by group or others) holds one account per line:
                       sha256(token) user_id role name [start_character]
                     tab-separated, role player or gm. A login whose token
                     is in no line is refused; a session's role is the
                     line's, capped by its listener's ceiling; every client
                     build outside --client-build's set is refused;
                     --user-id is refused.
  --world-accounts   accounts mode, the accounts in the --world file (owner
                     decision O9): the client logs in with
                     /ssoToken=USERNAME:PASSWORD, a known name needs its
                     password, an unknown one signs up on a signup=on
                     listener. Every new account is a player; a session's
                     role is the account's, capped by the listener's
                     ceiling; only our client build may log in. Needs
                     --world; refuses --accounts, --user-id, --character.
  --signup           signup=on for every --bind listener, the default one
                     included (tools/up.sh binds its own; pass this after
                     its --). --world-accounts only.
  --grant USERNAME ROLE
                     set an account's role (player or gm) in the --world
                     file and exit. Refused while a server runs with that
                     world (it holds FILE.lock). Must come last. In game, a
                     GM can do the same with /grant USERNAME ROLE.
  --mark-bot USERNAME [off]
                     mark an account in the --world file as a bot (or
                     clear the mark with off) and exit: every character it
                     owns counts as a bot for liveness and the economy. It
                     never changes the role (use --grant). Offline only, no
                     in-game path; refused while a server runs with that
                     world. Must come last.
  --new-token NAME ROLE [START_CHARACTER]
                     print a new account and exit: the accounts line, the
                     token (for /ssoToken=) and a NAME<TAB>TOKEN line for a
                     bots' tokens file. Writes no file. Must come last; with
                     --accounts FILE, the next free user_id.
  --capture DIR      write one .evecap per connection into DIR
  --world FILE       keep the world in FILE between runs. Off by default, so
                     a run without it is a fresh universe every time. Plain
                     text on purpose: 'grep item FILE' answers where a ship
                     went, with no tool of ours.
  --accept-downgrade save over a --world file that holds records this build
                     does not know (it is backed up first). Without it such a
                     file is never written; the session goes to FILE.downgrade.
  --writers N        socket-writer threads for pushed frames (default: a
                     quarter of the cores, 1 to 8). Chat writes use them too.
  --coalesce-destiny merge a client's same-stamp DoDestinyUpdate pushes into one
                     frame in its outbox (PA-10). On by default.
  --no-coalesce-destiny
                     one frame per DoDestinyUpdate push, as before PA-10.
  --conn-stack-mib N stack of each machoNet connection thread in MiB (1 to
                     1024; default 16, which the client's decode depth
                     needs in a debug build). Virtual memory: only touched
                     pages are resident, so this matters for ulimit -v and
                     for a lane that wants to measure RSS.
  --stop-file PATH   stop cleanly when PATH appears (polled by the clock):
                     no new login, every session hung up, the world
                     written, exit 0. Default with --world: FILE.stop.
                     A stale one is removed at startup. In game, a GM
                     types /shutdown [seconds] [message] (or cancel).
  --status FILE      rewrite FILE every 10 s with one TSV row per warm or hot
                     system ('watch column -t FILE'). Default with --world:
                     out/live/status.tsv. A failed write is logged once. Disposable;
                     never fsynced.
  --character NAME[,bot][,school=N]
                     add a character to the select screen. Repeatable. With
                     none, the roster is the one default character. Each one
                     gets its own characterID and shipID. The first time the
                     world sees it, it starts at its school's first station
                     with the starting ISK (',school=N' picks the school;
                     default: the lowest school of its race, so they all
                     start in one station, which is what lets two clients
                     see each other through the lobby's guest list; with no
                     content bundle, the default station). ',bot' marks
                     it a bot: under load its systems tick less often than a
                     human's (never the reverse). The mark is not part of
                     the name and no client is ever told it. Refused with
                     --accounts: the roster is then the world's plus each
                     account's start_character.
  --content DIR      the extracted content bundle (out/content/, written by
                     tools/content.py; tools/up.sh makes and passes it).
                     Refused when its format, client build or any file's row
                     count is wrong. Without it the server runs on invented
                     fixture rows and says so loudly. The five flags below
                     each override one file of it.
  --belt-rocks FILE  rocks for the asteroid belts, from tools/asteroids.py.
                     Needs --celestials for the same system, which says
                     where each belt is.
  --planets FILE     planetary interaction's static table (pin types,
                     schematics, resources, commodity types), from
                     tools/pi_statics.py. Replaces the bundle's.
  --skills FILE      every skill (rank, attributes, prerequisites, skillbook
                     row), from tools/skills.py. Replaces the bundle's.
  --typedata FILE    every ship, module, charge and drone type, with its
                     dogma attributes and effects, from tools/typedata.py.
                     Replaces the bundle's.
  --authored DIR     our own authored tables (content/*.tsv). Default:
                     content, relative to the working directory. A file DIR
                     has wins over the copy compiled into the binary; a file
                     it lacks uses the compiled-in copy. The startup lines
                     name the copy used for each file.
  --mode MODE        gps (run the handshake) or capture (answer nothing)
  --user-id N        the *account* id: session_init userid, and the id
                     GetCharacterSelectionData reports. Not a character id
                     and not a connection id -- the connection id is minted
                     per connection and deliberately has no flag.
  --proxy-node N     proxy_nodeid handed to the client
  --once             serve a single connection, then exit
  --probe ADDR       health check, not a server: connect to ADDR (HOST:PORT), read one
                     frame within 2 s, exit 0 if it is our VersionExchange greeting,
                     else 1. Must be the first argument. For a systemd watchdog or a
                     Docker HEALTHCHECK.
  --gateway          answer the client's public-gateway (gRPC) requests at once,
                     beside every local listener, on port 8443 (--gateway-port N), the
                     one EVE_PUBLIC_GATEWAY_ADDRESS names. Without it each
                     such request waits out the client's own 3-10 s deadline:
                     the slow fitting window and right-click menu
  --gateway-addr IP  run the gateway on IP instead of beside the local
                     listeners (implies --gateway): a container publishing
                     its ports, whose listener is 0.0.0.0, not local
  --chat             run in-game chat (XMPP) beside every --bind listener: at
                     its chat=, else on port 5222 of its address. Without
                     it, chat runs only beside listeners naming chat=, and
                     with none the client is told no chat address and never
                     dials. Each client needs
                     chatserver_disable_tls=1 in its prefs.ini, which
                     tools/run-client.sh writes (docs/lanes/chat.md)
  --chat-advertise IP  the address every client is told to dial for chat,
                     over any advertise=. Implies --chat. Default: the
                     listener's advertise=, else the address each client's
                     own connection arrived on
  --scenario NAME    dev only: run the client-CI scenario NAME inside the
                     client, after character selection. The server emits a
                     GetSettingsInfo blob that execfiles
                     c:\\eve-logs\\client-ci\\NAME.py; tools/client-ci.sh
                     deploys the file and collects its verdict. Off by
                     default, and a name this server will not accept is a
                     startup error rather than a quiet normal session.
  --time-scale N     run the slow game timers N times faster (1..=60,
                     default 1; else the EVE_TIME_SCALE variable): industry
                     jobs, PI cycles, skill training, the daily rollover
                     (11:00 UTC at 1, every 24/N hours above it), site
                     respawns, market and contract expiry. Physics, module
                     cycles, warp and the network stay real time. Every
                     stored deadline is a real time and survives a restart
                     at another scale (docs/lanes/clock.md).
  --dev              a debugging world (DEV-MODE, docs/guides/dev-mode.md): a dev
                     seller in every station (10,000,000 of every market
                     type at 100 ISK, restocked), 1,000,000,000 ISK for every
                     character, /devskills /devcorp /devhome. Keeps its world
                     in out/dev/world.dat unless --world names a file, and
                     refuses a --world that is not marked as a dev world
                     (FILE.dev); a normal run refuses a marked one.
  --training-scale N skill training only, N times faster still (1..=10000,
                     default 1; else EVE_TRAINING_SCALE), on top of
                     --time-scale: training runs time-scale x N times the
                     wall clock, and the attributes the client is told are
                     scaled the same, so its queue view agrees. For dev runs.
  --bot-control DIR  where the eve-bots control link writes bot-control.addr
                     and bot-control.token (loopback only; default: the
                     world file's directory; none without --world). /bots
                     also needs a dev server.
  --admin ADDR       the admin web page and its JSON API on ADDR (e.g.
                     0.0.0.0:8090; off by default). The token is in
                     admin.token in the --bot-control directory, sent as
                     the X-Admin-Token header. Read-only.
  --concord          CONCORD answers a high-sec crime: a squad, a warp
  --no-concord       refusal, a dock and gate refusal (the default), or
                     none of it (--no-concord, a CONCORD-free world).
  --no-undock-invuln a ship out of a station or through a gate takes no
                     damage for 30 s unless it acts first (the default);
                     this turns that off.
  --tick-threads N   threads the tick's parallel stages use (default: half the
                     hardware threads; 1 = serial, the oracle)
  --turn-sched       sample /proc/thread-self/schedstat around every payload turn
                     (cpu, run-queue wait, blocked) into the [turn-slow] lines.
  --log-sync         write log lines on the calling thread, as before the
                     log channel. Default: a writer thread behind a bounded
                     queue that never blocks a caller; a full queue drops
                     lines (marked `[log] dropped N`) and SIGKILL loses the
                     queue (milliseconds of lines).
  --push-probe N     dev only: post N inert probe notifications, 1/s, to each
                     connection. Off by default. Nothing is visible on screen
                     by design -- only a capture can tell you it worked.
  -h, --help         this text
";

/// [`USAGE`], then the flags the modules added.
fn usage(plan: &Assembled) -> String {
    let mut out = USAGE.to_owned();
    if !plan.flags.is_empty() {
        out.push_str("\nmodule options:\n");
        for f in &plan.flags {
            let value = if f.takes_value { " VALUE" } else { "" };
            out.push_str(&format!("  {}{value}    {}\n", f.name, f.usage));
        }
    }
    out
}

/// glibc's per-thread malloc pools, capped (tracker §78).
///
/// **Every connection is a thread, and glibc gives busy threads their own
/// arena** -- up to eight per core -- which keeps its high-water mark after
/// the memory is freed. A large transient answer (a region's NPC order book
/// for a mineral, `marketProxy.GetOrders`) built once on each of fifty
/// connection threads left the W17 soak 325 MB above its post-login level,
/// flat after the bots stopped: fragmentation, not a leak. The same run with
/// `MALLOC_ARENA_MAX=2` grew 21 MB. **[V]** by A/B soak.
///
/// The workspace forbids `unsafe`, so `mallopt` is out of reach; glibc reads
/// this variable when a process image starts, so the server re-executes
/// itself once with it set. An operator's own value, any value, wins.
pub const MALLOC_ARENA_MAX: &str = "2";

/// The environment to re-execute with, or `None` to run as we are.
fn arena_cap(current: Option<std::ffi::OsString>) -> Option<(&'static str, &'static str)> {
    current
        .is_none()
        .then_some(("MALLOC_ARENA_MAX", MALLOC_ARENA_MAX))
}

/// Re-execute this binary with [`arena_cap`]'s variable, unless it is set.
/// `exec` replaces the process image (same pid, stdio and signals); if it
/// cannot, the server runs uncapped and says so rather than not starting.
#[cfg(target_os = "linux")]
fn cap_malloc_arenas() {
    use std::os::unix::process::CommandExt;
    let Some((key, value)) = arena_cap(std::env::var_os("MALLOC_ARENA_MAX")) else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("eve-server: cannot find own binary; malloc arenas uncapped");
        return;
    };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(key, value)
        .exec();
    eprintln!("eve-server: re-exec with {key}={value} failed ({err}); malloc arenas uncapped");
}

#[cfg(not(target_os = "linux"))]
fn cap_malloc_arenas() {}

/// `--probe ADDR`: a liveness check for a systemd watchdog or a Docker
/// `HEALTHCHECK`. Connect, read one frame within 2 s, and require it to be our
/// VersionExchange greeting (the unprompted 7-tuple: ints, a real at 3, bytes
/// at 5, `None` at 6). Nothing is sent, so a probe leaves no session behind.
fn probe(addr: &str) -> bool {
    use eve_marshal::PyValue;
    use std::io::Read;
    use std::net::ToSocketAddrs;
    use std::time::{Duration, Instant};
    let Some(sa) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return false;
    };
    let limit = Duration::from_secs(2);
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&sa, limit) else {
        return false;
    };
    let start = Instant::now();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match eve_proto::try_split_frame(&buf) {
            Err(_) => return false,
            Ok(Some((frame, _))) => {
                return matches!(
                    crate::loads(frame.payload),
                    Ok(PyValue::Tuple(t)) if t.len() == 7
                        && matches!(t[3], PyValue::Real(_))
                        && matches!(t[5], PyValue::Bytes(_))
                        && matches!(t[6], PyValue::None)
                );
            }
            Ok(None) => {}
        }
        let Some(left) = limit.checked_sub(start.elapsed()).filter(|d| !d.is_zero()) else {
            return false;
        };
        if s.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Every flag the core accepts, without its value. A module flag may not
/// repeat one (`module::build` refuses it); `core_flags_match_the_parser`
/// keeps this list and the parser's arms the same set.
pub const CORE_FLAGS: &[&str] = &[
    "-h",
    "--help",
    "--bind",
    "--capture",
    "--world",
    "--status",
    "--stop-file",
    "--content",
    "--celestials",
    "--belt-rocks",
    "--planets",
    "--typedata",
    "--skills",
    "--authored",
    "--mode",
    "--accounts",
    "--client-build",
    "--world-accounts",
    "--signup",
    "--grant",
    "--mark-bot",
    "--new-token",
    "--user-id",
    "--proxy-node",
    "--push-probe",
    "--tick-threads",
    "--turn-sched",
    "--log-sync",
    "--scenario",
    "--time-scale",
    "--dev",
    "--training-scale",
    "--bot-control",
    "--admin",
    "--concord",
    "--no-concord",
    "--no-undock-invuln",
    "--character",
    "--once",
    "--accept-downgrade",
    "--writers",
    "--coalesce-destiny",
    "--no-coalesce-destiny",
    "--conn-stack-mib",
    "--gateway",
    "--gateway-port",
    "--gateway-addr",
    "--chat",
    "--chat-advertise",
    "--probe",
];

/// The whole of `main`: fix the module plan to `mods`, parse the command
/// line, run the server. `run(&[])` is the core server.
pub fn run(mods: &[&Module]) -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--probe") {
        return match args.next() {
            Some(a) if probe(&a) => ExitCode::SUCCESS,
            Some(_) => ExitCode::FAILURE,
            None => {
                eprintln!("eve-server: --probe needs an address (HOST:PORT)");
                ExitCode::FAILURE
            }
        };
    }
    cap_malloc_arenas();
    let plan = match module::install(mods) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("eve-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `--log-sync` is read here, before `run()`, so the channel exists for the
    // whole run; `run()` accepts it as a no-op.
    let sync = std::env::args().any(|a| a == "--log-sync");
    if let Err(e) = crate::logline::init(sync) {
        eprintln!("eve-server: log channel not started ({e}); logging directly");
    }
    let code = match serve(plan) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            crate::errln!("eve-server: {e}");
            ExitCode::FAILURE
        }
    };
    crate::logline::shutdown(std::time::Duration::from_secs(2));
    code
}

fn serve(plan: &Assembled) -> Result<(), String> {
    let mut module_flags: Vec<(&module::Flag, Option<String>)> = Vec::new();
    let mut listeners: Vec<Listener> = Vec::new();
    let mut capture_dir: Option<PathBuf> = None;
    let mut world_file: Option<PathBuf> = None;
    let mut status_file: Option<PathBuf> = None;
    let mut stop_file: Option<PathBuf> = None;
    let mut celestials: Option<PathBuf> = None;
    let mut belt_rocks: Option<PathBuf> = None;
    let mut planets: Option<PathBuf> = None;
    let mut typedata: Option<PathBuf> = None;
    let mut skills: Option<PathBuf> = None;
    let mut authored_dir: Option<PathBuf> = None;
    let mut content_dir: Option<PathBuf> = None;
    let mut mode = Mode::Gps;
    let mut hs = handshake::Config::default();
    let mut once = false;
    let mut accept_downgrade = false;
    let mut writers: Option<usize> = None;
    let mut coalesce_destiny = true;
    let mut conn_stack_mib: Option<usize> = None;
    let mut push_probe: u32 = 0;
    let mut scenario: Option<String> = None;
    let mut character_names: Vec<String> = Vec::new();
    let mut chat: Option<crate::chat::ChatConfig> = None;
    let mut gateway: Option<u16> = None;
    let mut gateway_addr: Option<std::net::IpAddr> = None;
    let mut accounts_file: Option<PathBuf> = None;
    let mut user_id_given = false;
    let mut new_token: Option<Vec<String>> = None;
    let mut world_accounts = false;
    let mut signup_everywhere = false;
    let mut grant: Option<Vec<String>> = None;
    let mut mark_bot: Option<Vec<String>> = None;
    let mut time_scale: Option<String> = None;
    let mut training_scale: Option<String> = None;
    let mut bot_control: Option<PathBuf> = None;
    let mut admin: Option<std::net::SocketAddr> = None;
    let mut concord = true;
    let mut undock_invuln = true;
    let mut dev = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", usage(plan));
                return Ok(());
            }
            "--bind" => {
                let v = value("--bind")?;
                listeners.push(Listener::parse(&v).map_err(|e| format!("--bind {v}: {e}"))?);
            }
            "--capture" => capture_dir = Some(PathBuf::from(value("--capture")?)),
            "--world" => world_file = Some(PathBuf::from(value("--world")?)),
            "--status" => status_file = Some(PathBuf::from(value("--status")?)),
            "--stop-file" => stop_file = Some(PathBuf::from(value("--stop-file")?)),
            "--content" => content_dir = Some(PathBuf::from(value("--content")?)),
            "--celestials" => celestials = Some(PathBuf::from(value("--celestials")?)),
            "--belt-rocks" => belt_rocks = Some(PathBuf::from(value("--belt-rocks")?)),
            "--planets" => planets = Some(PathBuf::from(value("--planets")?)),
            "--typedata" => typedata = Some(PathBuf::from(value("--typedata")?)),
            "--skills" => skills = Some(PathBuf::from(value("--skills")?)),
            "--authored" => authored_dir = Some(PathBuf::from(value("--authored")?)),
            "--mode" => {
                let v = value("--mode")?;
                mode = match v.as_str() {
                    "gps" => Mode::Gps,
                    "capture" => Mode::Capture,
                    other => return Err(format!("--mode {other}: expected gps or capture")),
                };
            }
            "--accounts" => accounts_file = Some(PathBuf::from(value("--accounts")?)),
            "--client-build" => hs.builds = handshake::Builds::parse(&value("--client-build")?)?,
            "--world-accounts" => world_accounts = true,
            "--signup" => signup_everywhere = true,
            "--grant" => {
                grant = Some(args.by_ref().collect());
                break;
            }
            "--mark-bot" => {
                mark_bot = Some(args.by_ref().collect());
                break;
            }
            "--new-token" => {
                new_token = Some(args.by_ref().collect());
                break;
            }
            "--user-id" => {
                user_id_given = true;
                let v = value("--user-id")?;
                hs.user_id = v.parse().map_err(|e| format!("--user-id {v}: {e}"))?;
            }
            "--proxy-node" => {
                let v = value("--proxy-node")?;
                hs.proxy_node_id = v.parse().map_err(|e| format!("--proxy-node {v}: {e}"))?;
            }
            "--push-probe" => {
                let v = value("--push-probe")?;
                push_probe = v.parse().map_err(|e| format!("--push-probe {v}: {e}"))?;
            }
            "--tick-threads" => {
                let v = value("--tick-threads")?;
                let n: usize = v.parse().map_err(|e| format!("--tick-threads {v}: {e}"))?;
                crate::par::set_threads(n);
            }
            "--turn-sched" => crate::sched_stat::set_enabled(true),
            "--log-sync" => {}
            "--scenario" => scenario = Some(value("--scenario")?),
            "--time-scale" => time_scale = Some(value("--time-scale")?),
            "--dev" => dev = true,
            "--training-scale" => training_scale = Some(value("--training-scale")?),
            "--bot-control" => bot_control = Some(PathBuf::from(value("--bot-control")?)),
            "--admin" => {
                let v = value("--admin")?;
                admin = Some(
                    v.parse()
                        .map_err(|e| format!("--admin {v}: not an address ({e})"))?,
                );
            }
            "--concord" => concord = true,
            "--no-concord" => concord = false,
            "--no-undock-invuln" => undock_invuln = false,
            "--character" => character_names.push(value("--character")?),
            "--once" => once = true,
            "--accept-downgrade" => accept_downgrade = true,
            "--writers" => {
                let v = value("--writers")?;
                writers = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=64).contains(n))
                        .ok_or_else(|| format!("--writers: want 1 to 64, got {v:?}"))?,
                );
            }
            "--coalesce-destiny" => coalesce_destiny = true,
            "--no-coalesce-destiny" => coalesce_destiny = false,
            "--conn-stack-mib" => {
                let v = value("--conn-stack-mib")?;
                conn_stack_mib = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=1024).contains(n))
                        .ok_or_else(|| format!("--conn-stack-mib: want 1 to 1024, got {v:?}"))?,
                );
            }
            "--gateway" => gateway = Some(crate::gateway::PORT),
            "--gateway-port" => {
                let v = value("--gateway-port")?;
                gateway = Some(v.parse().map_err(|e| format!("--gateway-port {v}: {e}"))?);
            }
            "--gateway-addr" => {
                let v = value("--gateway-addr")?;
                gateway_addr = Some(v.parse().map_err(|e| format!("--gateway-addr {v}: {e}"))?);
                gateway.get_or_insert(crate::gateway::PORT);
            }
            "--chat" => {
                chat.get_or_insert_with(crate::chat::ChatConfig::default);
            }
            "--chat-advertise" => {
                let v = value("--chat-advertise")?;
                let ip = v
                    .parse()
                    .map_err(|e| format!("--chat-advertise {v}: {e}"))?;
                chat.get_or_insert_with(crate::chat::ChatConfig::default)
                    .advertise = Some(ip);
            }
            other => match plan.flag(other) {
                Some(flag) => {
                    let v = flag.takes_value.then(|| value(other)).transpose()?;
                    module_flags.push((flag, v));
                }
                None => return Err(format!("unknown argument {other}\n\n{}", usage(plan))),
            },
        }
    }

    // **The rules, settled and checked before any path acts on them** (the
    // content load, `--new-token`, `--grant`, `--mark-bot`): the modules'
    // `rules` functions in list order, then the flags, which win. A bad
    // module flag is refused here, whichever path the command line takes.
    let mut rules = crate::rules::Rules::default();
    plan.apply_rules(&mut rules);
    for (flag, v) in &module_flags {
        (flag.apply)(&mut rules, v.as_deref()).map_err(|e| format!("{}: {e}", flag.name))?;
    }
    rules.validate()?;
    if !plan.modules.is_empty() {
        let list: Vec<String> = plan
            .modules
            .iter()
            .map(|(n, v)| format!("{n} v{v}"))
            .collect();
        println!("[modules] {}", list.join(", "));
    }

    // DEV-MODE: a dev run keeps its world apart from the normal save.
    if dev && world_file.is_none() && grant.is_none() && mark_bot.is_none() && new_token.is_none() {
        world_file = Some(PathBuf::from("out/dev/world.dat"));
    }
    if let Some(rest) = new_token {
        let existing = accounts_file
            .as_deref()
            .map(crate::accounts::Accounts::load)
            .transpose()?;
        let (name, role, start) = match rest.as_slice() {
            [n, r] => (n, r, None),
            [n, r, s] => (n, r, Some(s.as_str())),
            _ => return Err("usage: --new-token NAME ROLE [START_CHARACTER], last".to_owned()),
        };
        let random = crate::accounts::urandom32().map_err(|e| format!("/dev/urandom: {e}"))?;
        let t = crate::accounts::new_token(&random, name, role, start, existing.as_ref())?;
        println!("{}\n{}\n{}", t.accounts_line, t.token, t.bots_line);
        return Ok(());
    }
    if let Some(rest) = grant {
        let [name, role] = rest.as_slice() else {
            return Err("usage: --world FILE --grant USERNAME ROLE, last".to_owned());
        };
        let path = world_file.ok_or("--grant needs --world FILE")?;
        println!("{}", crate::account_book::grant_offline(&path, name, role)?);
        return Ok(());
    }
    if let Some(rest) = mark_bot {
        let (name, bot) = match rest.as_slice() {
            [n] => (n, true),
            [n, off] if off == "off" => (n, false),
            _ => return Err("usage: --world FILE --mark-bot USERNAME [off], last".to_owned()),
        };
        let path = world_file.ok_or("--mark-bot needs --world FILE")?;
        println!(
            "{}",
            crate::account_book::mark_bot_offline(&path, name, bot)?
        );
        return Ok(());
    }
    // World accounts (ACC2): the world's rows, never beside the file or a
    // catch-all.
    if world_accounts {
        let clash = [
            (accounts_file.is_some(), "--accounts"),
            (user_id_given, "--user-id"),
            (!character_names.is_empty(), "--character"),
            (world_file.is_none(), "a missing --world"),
        ];
        if let Some((_, what)) = clash.iter().find(|(yes, _)| *yes) {
            return Err(format!("--world-accounts is refused with {what}"));
        }
        println!(
            "accounts: in the world file; only client builds {} may log in",
            builds_list(&hs.builds)
        );
        hs.accounts = Some(std::sync::Arc::new(crate::accounts::Accounts::in_world(
            crate::account_book::Book::default(),
        )));
    }
    // Accounts mode: the file, never beside a catch-all (account-spec §4.1).
    if let Some(path) = &accounts_file {
        if user_id_given {
            return Err(
                "--user-id is the catch-all account's id; with --accounts every \
                        user id comes from the file"
                    .to_owned(),
            );
        }
        // [A] In accounts mode characters come from the world and from
        // `start_character` only (account-spec §3.4): flag order would give
        // ids that a saved world may already hold for someone else.
        if !character_names.is_empty() {
            return Err(
                "--character is refused with --accounts: give the account a \
                 start_character instead"
                    .to_owned(),
            );
        }
        let accounts = crate::accounts::Accounts::load(path)?;
        println!(
            "accounts: {} from {}; only client builds {} may log in",
            accounts.list().len(),
            path.display(),
            builds_list(&hs.builds)
        );
        hs.accounts = Some(std::sync::Arc::new(accounts));
    }
    // The handshake goes in through the constructor, not onto the field: the
    // node id and the user id are derived from it, and assigning it
    // afterwards left `--proxy-node` and `--user-id` silently broken. See
    // `Config::with_handshake`.
    let mut config =
        Config::with_handshake("127.0.0.1:26000".parse().expect("literal address"), hs);
    if !listeners.is_empty() {
        config.listeners = listeners;
    }
    if signup_everywhere {
        for l in &mut config.listeners {
            l.signup = true;
        }
    }
    // **Fail closed, first**: before the content loads, so a refused
    // listener costs no wait. `Server::bind` checks again.
    for l in &config.listeners {
        crate::server::fail_closed(l, &config.handshake)
            .map_err(|e| format!("--bind {}: {e}", l.addr))?;
    }
    // A directory the player typed and that is not there is an error they
    // can read; the default one missing only means every authored table uses
    // its compiled-in copy (`crate::authored`, plan §1.4). `content::load`
    // hands `authored` to every table's `load` (IP1, CA1), and each table
    // prints one line naming its source (CA4).
    let authored = match &authored_dir {
        Some(dir) if !dir.is_dir() => {
            return Err(format!("--authored {}: not a directory", dir.display()));
        }
        // Absolute, so every `[authored]` and `[content] table` line names
        // the directory actually read, whatever the cwd (V7 F2).
        Some(dir) => authored::Dir::new(dir.canonicalize().unwrap_or_else(|_| dir.clone())),
        None => match std::path::Path::new(authored::DEFAULT_DIR).canonicalize() {
            Ok(abs) => authored::Dir::new(abs),
            Err(_) => authored::Dir::default(),
        },
    };
    if !character_names.is_empty() {
        (config.sandbox.characters, config.sandbox.fresh_roster) = roster(&character_names)?;
    }
    // Accounts mode has no default character: the roster is the world's
    // and the accounts' `start_character`s (IP27-b). `Server::bind` sets
    // the mode itself, from the handshake.
    if config.handshake.accounts.is_some() {
        config.sandbox.characters = Vec::new();
        config.sandbox.fresh_roster = Vec::new();
    }
    config.capture_dir = capture_dir;
    // C7 §7.2: a kept world gets a status file unless one is named.
    config.status_file = resolve_status_file(status_file, world_file.as_deref());
    // The clean stop's file: `<world>.stop` unless one is named.
    config.stop_file = stop_file.or_else(|| {
        world_file
            .as_deref()
            .map(crate::shutdown::default_stop_file)
    });
    if let Some(w) = &world_file {
        crate::dev_mode::check_marker(w, dev)?;
    }
    if dev {
        config.sandbox.dev = true;
        config.dev = true;
        println!("################################################################");
        println!("# DEV MODE: every station sells every item at 100 ISK, new and");
        println!("# existing characters get 1,000,000,000 ISK, and /devskills,");
        println!("# /devcorp and /devhome are open. This is a debugging world, not a");
        println!(
            "# real one. World: {}",
            world_file
                .as_deref()
                .map_or_else(|| "memory".to_owned(), |w| w.display().to_string())
        );
        println!("################################################################");
    }
    config.world_file = world_file;
    config.accept_downgrade = accept_downgrade;
    config.writers = writers;
    config.coalesce_destiny = coalesce_destiny;
    config.conn_stack_mib = conn_stack_mib;
    // **The bundle first**, because it is the default source of the files
    // below; an explicit flag still overrides its file (plan §1.3).
    // Refused here, before anything reads it: a bundle for another build or
    // with a truncated file would load as a quietly wrong universe.
    println!("[build] {}", config.handshake.builds.describe());
    let loaded = content::load_for(
        content_dir.as_deref(),
        &authored,
        &config.handshake.builds.accepted,
    )?;
    for line in &loaded.lines {
        println!("{line}");
    }
    let bundle = loaded.bundle.as_ref();
    let celestials = content::pick(celestials.as_deref(), bundle, content::CELESTIALS);
    let belt_rocks = content::pick(belt_rocks.as_deref(), bundle, content::ROCKS);
    // `--planets`, `--typedata` and `--skills` replace the bundle's table
    // (CP60, CP59b): the bundle's own file is the table's `load`, so only
    // the explicit flag is read here. Fatal when unreadable or empty, for
    // the reason the world file is: a table that failed to load is a ship
    // or a skill the player cannot have, with nothing saying why.
    let mut tables = loaded.content;
    if let Some(path) = &planets {
        println!("{}", crate::planet::override_in(&mut tables, path)?);
    }
    if let Some(path) = &typedata {
        println!("{}", crate::typedata::override_in(&mut tables, path)?);
    }
    if let Some(path) = &skills {
        println!("{}", crate::skill::override_in(&mut tables, path)?);
    }
    // Handed over, never kept: the registry owns the tables (§67).
    config.content = Some(std::sync::Arc::new(tables));

    // **Loaded here and not in `Server::bind`**, because a path the player
    // typed deserves an error they can read. A missing or unreadable extract
    // is fatal for the same reason the world file is: continuing would give a
    // solar system with no planets in it and no explanation, and the player
    // would find out by right-clicking empty space. `tools/celestials.py`
    // generates the file; `crate::celestial` has why it is a file.
    if let Some(path) = &celestials {
        config.sandbox.celestials = crate::load_celestials(path)?;
    }
    // After the celestials, which say where each belt is. Fatal when it
    // indexes no belt, for the same reason: `crate::asteroid::index`. Only
    // the offset index is built here; a system's rocks are laid out while it
    // is warm or hot (CP14', `asteroid::sync`).
    if let Some(path) = &belt_rocks {
        config.sandbox.rocks = crate::asteroid::index(
            path,
            &config.sandbox.celestials,
            &config
                .content
                .as_deref()
                .map(crate::asteroid::Quantities::from_content)
                .unwrap_or_default(),
        )?;
    }
    // The typedata reaches the world through `Content` alone (X2b: the
    // process-wide `OnceLock` and `typedata::install` are gone): the
    // bundle's table, or `--typedata`'s through `typedata::override_in`
    // above, refused there when it holds no ship or no module.
    config.sandbox.rules = rules;
    config.mode = mode;
    config.push_probe = push_probe;
    config.chat = chat;
    config.gateway = gateway;
    config.gateway_addr = gateway_addr;
    config.concord = concord;
    config.undock_invuln = undock_invuln;
    config.time_scale = crate::game_clock::GameClock::from_flag_or_env(
        time_scale.as_deref(),
        std::env::var(crate::game_clock::ENV).ok().as_deref(),
    )?;
    // BOT-CTL: skill training faster still, on top of --time-scale.
    config.time_scale = config.time_scale.with_training(
        crate::game_clock::GameClock::training_from_flag_or_env(
            training_scale.as_deref(),
            std::env::var(crate::game_clock::TRAINING_ENV)
                .ok()
                .as_deref(),
        )?,
    )?;
    config.bot_control = crate::bot_control::dir_for(bot_control, config.world_file.as_deref());
    config.admin = admin;
    // **Validated here, and a bad name is fatal.**
    //
    // The temptation is to fall back to the ordinary blob on a name we
    // cannot use. That would be the worst outcome available: a mistyped
    // `--scenario` would start a perfectly normal session, the harness would
    // wait out its timeout finding no result file, and the diagnosis would be
    // "the client never ran the scenario" with no clue as to why. **A test
    // that passes by doing nothing is worse than a test that fails.** So the
    // blob is built once, now, and a name that will not build one stops the
    // server before it binds.
    if let Some(name) = &scenario {
        let path =
            crate::pycode::scenario_path(name).map_err(|e| format!("--scenario {name}: {e}"))?;
        crate::pycode::settings_info_scenario_func(name)
            .map_err(|e| format!("--scenario {name}: {e}"))?;
        println!("client-CI scenario: the client will run {path}");
        config.sandbox.scenario = Some(name.clone());
    }

    // Counted before the universe moves into the server: the binary keeps
    // no second copy of it (`docs/port-tracker.md` §67).
    let n_celestials = config.sandbox.celestials.len();
    let n_served = config.sandbox.celestials.served().count();
    let n_rocks = config.sandbox.rocks.static_len();
    let celestials_data = std::mem::take(&mut config.sandbox.celestials);
    let rocks_data = std::mem::take(&mut config.sandbox.rocks);
    let content_data = config.content.take();
    let mut server_config = config.clone();
    server_config.sandbox.celestials = celestials_data;
    server_config.sandbox.rocks = rocks_data;
    server_config.content = content_data;
    // Refuses, before anything binds, every listener a friend could reach
    // (`server::fail_closed`).
    let server = Server::bind(server_config).map_err(|e| format!("listen: {e}"))?;
    let addrs = server.local_addrs().map_err(|e| e.to_string())?;
    for (addr, l) in addrs.iter().zip(&config.listeners) {
        println!(
            "eve-server listening on {addr} in {:?} mode (ceiling {:?}, cap {}{})",
            config.mode,
            l.ceiling,
            l.cap(),
            l.account
                .map_or_else(String::new, |uid| format!(", account {uid}"))
        );
    }
    {
        let cap = config.listeners.iter().map(|l| l.cap()).max().unwrap_or(0);
        let limit = crate::server::fd_limit();
        println!(
            "[limits] ulimit -n {}, connection cap {cap} per listener",
            limit.map_or_else(|| "unknown".to_owned(), |n| n.to_string())
        );
        if let Some(w) = crate::server::fd_warning(limit, cap) {
            eprintln!("{w}");
            println!("{w}");
        }
    }
    match &config.capture_dir {
        Some(dir) => println!("capturing frames to {}", dir.display()),
        None => println!("capture disabled (pass --capture DIR to record)"),
    }
    match authored.root() {
        Some(d) if d.is_dir() => println!(
            "[authored] tables from {}; a file missing there uses its compiled-in copy",
            d.display()
        ),
        _ => println!(
            "[authored] no {} directory: every authored table uses its compiled-in copy \
             (pass --authored DIR)",
            authored::DEFAULT_DIR
        ),
    }
    match &config.world_file {
        Some(p) => println!("world kept in {}", p.display()),
        None => println!("world is in memory only (pass --world FILE to keep it)"),
    }
    match &celestials {
        Some(p) => println!(
            "{} celestials from {} ({} of them in the ballpark)",
            n_celestials,
            p.display(),
            n_served
        ),
        None => println!("no celestials (pass --content DIR; tools/up.sh makes one)"),
    }
    match &belt_rocks {
        Some(p) => println!(
            "{n_rocks} rocks indexed from {} (laid out per system while it is live)",
            p.display()
        ),
        None => println!("no rocks (pass --content DIR; tools/up.sh makes one)"),
    }
    match (&config.chat, server.chat_addrs().is_empty()) {
        (_, true) => println!("chat off (pass --chat to run it)"),
        (c, false) => println!(
            "chat on {:?} (advertising {})",
            server.chat_addrs(),
            c.and_then(|c| c.advertise).map_or_else(
                || "each listener's advertise=, else the connection's own address".to_owned(),
                |ip| ip.to_string()
            )
        ),
    }
    if config.push_probe > 0 {
        // Loud, because it is a flag that makes the server send the client
        // something it never asked for. It is provably inert on the client
        // (`server::PUSH_PROBE_EVENT`), and it is still unsolicited traffic.
        println!(
            "PUSH PROBE ON: posting {} inert '{}' notifications per connection, one per second",
            config.push_probe,
            crate::server::PUSH_PROBE_EVENT
        );
    }

    if once {
        let end = server.run_once().map_err(|e| e.to_string())?;
        println!("connection ended: {end}");
        return Ok(());
    }
    server.run().map_err(|e| e.to_string())?;
    // **The clean stop** (`crate::shutdown`): `/shutdown` or the stop
    // file ended every accept loop. Hang up, let the sessions end, then drop
    // the server: its `Drop` writes the last world through the saver and
    // releases the `--world` lock. Returning exits with status 0.
    println!(
        "[shutdown] stopping: {}",
        server
            .stop_reason()
            .unwrap_or_else(|| "the listeners ended".to_owned())
    );
    server.hang_up(crate::shutdown::SESSION_DRAIN);
    let stop = server.stop();
    drop(server);
    stop.clear_file();
    match &config.world_file {
        Some(p) => println!("[shutdown] world written to {}; exit 0", p.display()),
        None => println!("[shutdown] no --world to write; exit 0"),
    }
    Ok(())
}

/// One `--character` spec: `NAME` or `NAME,bot`. **The bot marker is the
/// spec's flag, never the name** (ruling 2): `Bot Alice` is a human, and a
/// bot's name carries no trace of the flag. A comma never occurs in an EVE
/// character name, so it cannot be part of one.
/// The status file: the one `--status` names, else `out/live/status.tsv`
/// for a kept world, else none. A deployment whose working directory is
/// read-only (deploy/entrypoint.sh) names a path under its data volume.
fn resolve_status_file(named: Option<PathBuf>, world: Option<&Path>) -> Option<PathBuf> {
    named.or_else(|| world.map(|_| PathBuf::from("out/live/status.tsv")))
}

fn character_spec(spec: &str) -> Result<CharacterSpec<'_>, String> {
    let mut parts = spec.split(',');
    let name = parts.next().unwrap_or_default();
    let mut out = CharacterSpec {
        name,
        is_bot: false,
        school: None,
    };
    for option in parts {
        if option == "bot" {
            out.is_bot = true;
        } else if let Some(n) = option.strip_prefix("school=") {
            let n = n.parse().map_err(|_| {
                format!("--character {spec}: school={n} is not a school id (a number)")
            })?;
            out.school = Some(n);
        } else {
            return Err(format!(
                "--character {spec}: unknown option '{option}' \
                 (expected NAME[,bot][,school=N])"
            ));
        }
    }
    Ok(out)
}

/// One `--character` spec, parsed.
struct CharacterSpec<'a> {
    name: &'a str,
    is_bot: bool,
    /// `,school=N`: the school it starts at (P-START-2); `None` is its
    /// race's default school (`start::default_school_for_race`).
    school: Option<i64>,
}

/// Build a roster from `--character` specs (`NAME` or `NAME,bot`).
///
/// **Every id that is a key somewhere must differ.** `Registry::new` asserts
/// distinct `character_id` and `ship_id` -- the store's `insert` is a `push`
/// with no uniqueness check, so two characters sharing a `ship_id` would put
/// two items with one id in it -- so this derives both from the index rather
/// than letting a caller choose and get it wrong.
///
/// Everything else is deliberately shared, the station above all: two
/// characters in one station is what makes them visible to each other through
/// `station.GetGuests`, which is the cheapest thing two players can see each
/// other through.
/// Returns the characters and, for `SandboxConfig::fresh_roster`, each
/// one's id with its `,school=N`: `Server::bind` places and funds the ones
/// the world file does not know yet (P-START-2).
type Roster = (Vec<crate::model::Character>, Vec<(i64, Option<i64>)>);

/// `3619879,3628513`: the accepted builds, for the accounts-mode banner.
fn builds_list(b: &handshake::Builds) -> String {
    b.accepted
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn roster(names: &[String]) -> Result<Roster, String> {
    let base = crate::model::Character::default();
    let mut out = Vec::with_capacity(names.len());
    let mut fresh = Vec::with_capacity(names.len());
    let specs = names
        .iter()
        .map(|s| character_spec(s))
        .collect::<Result<Vec<_>, _>>()?;
    for (i, spec) in specs.iter().enumerate() {
        let (name, is_bot) = (spec.name, spec.is_bot);
        if name.trim().is_empty() {
            return Err("--character needs a name".to_owned());
        }
        if specs[..i].iter().any(|s| s.name == name) {
            // Two characters with one name is legal in EVE and confusing
            // here: the select screen would show two identical rows and the
            // only way to tell which is which is the id nobody can see.
            return Err(format!("--character {name}: given twice"));
        }
        out.push(crate::model::Character {
            character_id: base.character_id + i as i64,
            ship_id: base.ship_id + i as i64,
            name: name.to_owned(),
            is_bot,
            ..base.clone()
        });
        fresh.push((base.character_id + i as i64, spec.school));
    }
    Ok((out, fresh))
}

#[cfg(test)]
mod tests {
    /// `CORE_FLAGS` is the set of flags the parser matches: a flag added to
    /// one and not the other fails here, so a module cannot shadow a core
    /// flag unseen.
    #[test]
    fn core_flags_match_the_parser() {
        let src = include_str!("cli.rs");
        let body = &src[src.find("match arg.as_str()").unwrap()..];
        let body = &body[..body.find("other => match plan.flag").unwrap()];
        let mut found: Vec<&str> = Vec::new();
        for line in body.lines() {
            let t = line.trim_start();
            if !t.starts_with('"') {
                continue;
            }
            let Some((pat, _)) = t
                .split_once("=>")
                .or_else(|| t.strip_suffix('{').map(|p| (p, "")))
            else {
                continue;
            };
            for alt in pat.split('|') {
                let alt = alt.trim().trim_end_matches('{').trim().trim_matches('"');
                if alt.starts_with('-') {
                    found.push(alt);
                }
            }
        }
        // `--probe` is read before the parser.
        let mut want: Vec<&str> = super::CORE_FLAGS
            .iter()
            .copied()
            .filter(|f| *f != "--probe")
            .collect();
        found.sort_unstable();
        want.sort_unstable();
        assert_eq!(found, want);
    }

    /// Module flags are listed after the core usage, and only then.
    #[test]
    fn usage_lists_module_flags() {
        fn nop(_: &mut crate::rules::Rules, _: Option<&str>) -> Result<(), String> {
            Ok(())
        }
        const FLAGS: &[module::Flag] = &[module::Flag {
            name: "--ext-flag",
            takes_value: true,
            usage: "an invented flag",
            apply: nop,
        }];
        let none = module::build(&[]).unwrap();
        assert_eq!(super::usage(&none), super::USAGE);
        let with = module::build(&[&Module {
            name: "ext",
            cli: FLAGS,
            ..Module::NONE
        }])
        .unwrap();
        let text = super::usage(&with);
        assert!(text.starts_with(super::USAGE));
        assert!(text.contains("--ext-flag VALUE") && text.contains("an invented flag"));
    }

    /// An unset variable is capped; any operator value, even empty, is kept.
    #[test]
    fn the_arena_cap_applies_only_when_the_operator_set_none() {
        assert_eq!(
            super::arena_cap(None),
            Some(("MALLOC_ARENA_MAX", super::MALLOC_ARENA_MAX))
        );
        assert_eq!(super::arena_cap(Some("8".into())), None);
        assert_eq!(super::arena_cap(Some("".into())), None);
    }

    use super::*;

    /// `--status PATH` wins over the default, which stays out/live/status.tsv
    /// for a kept world and is none without one.
    #[test]
    fn the_status_flag_overrides_the_default_path() {
        let w = Path::new("w.dat");
        let named = PathBuf::from("/data/live/status.tsv");
        assert_eq!(
            resolve_status_file(Some(named.clone()), Some(w)),
            Some(named.clone())
        );
        assert_eq!(resolve_status_file(Some(named.clone()), None), Some(named));
        assert_eq!(
            resolve_status_file(None, Some(w)),
            Some(PathBuf::from("out/live/status.tsv"))
        );
        assert_eq!(resolve_status_file(None, None), None);
    }

    /// The probe is happy with the real server and unhappy with nothing, or
    /// with something that is not our greeting.
    #[test]
    fn the_probe_passes_a_live_server_and_fails_a_closed_port() {
        let server =
            Server::bind(crate::server::Config::new("127.0.0.1:0".parse().unwrap())).unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let _ = server.run_once();
        });
        assert!(probe(&addr.to_string()));
        let _ = handle.join();
        // That port is closed now.
        assert!(!probe(&addr.to_string()));
        // A listener that says something else, and one that says nothing.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            use std::io::Write;
            let (mut c, _) = l.accept().unwrap();
            c.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        });
        assert!(!probe(&other.to_string()));
        let _ = t.join();
    }

    fn specs(names: &[&str]) -> Result<Vec<crate::model::Character>, String> {
        roster(&names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>()).map(|(c, _)| c)
    }

    /// **`,school=N` picks the start** (P-START-2) and is not part of the
    /// name; without it the school is `None` (the race's default). A school
    /// that is not a number is an error naming the spec. Every roster
    /// character is listed fresh, with its own id.
    #[test]
    fn the_character_school_option_is_parsed_and_a_bad_one_names_the_spec() {
        let names: Vec<String> = ["Alice,school=99", "Bob,bot,school=98", "Carol"]
            .iter()
            .map(|n| (*n).to_owned())
            .collect();
        let (chars, fresh) = roster(&names).expect("a roster");
        let got: Vec<(&str, bool)> = chars.iter().map(|c| (c.name.as_str(), c.is_bot)).collect();
        assert_eq!(got, vec![("Alice", false), ("Bob", true), ("Carol", false)]);
        let ids: Vec<i64> = chars.iter().map(|c| c.character_id).collect();
        assert_eq!(
            fresh,
            vec![(ids[0], Some(99)), (ids[1], Some(98)), (ids[2], None)]
        );
        let e = specs(&["Dave,school=x"]).unwrap_err();
        assert!(e.contains("Dave,school=x") && e.contains("school"), "{e}");
    }

    /// **The `--character` bot spec sets `is_bot`**, and only the spec:
    /// `,bot` marks a bot and is not part of its name; a name that says
    /// "bot" without the spec is a human (ruling 2); a name given once
    /// plainly and once as a bot is still given twice; an unknown option is
    /// refused rather than read as part of a name.
    #[test]
    fn the_character_bot_spec_sets_is_bot_and_never_the_name() {
        let r = specs(&["Alice", "Bob,bot", "Bot Carol"]).expect("a roster");
        let got: Vec<(&str, bool)> = r.iter().map(|c| (c.name.as_str(), c.is_bot)).collect();
        assert_eq!(
            got,
            vec![("Alice", false), ("Bob", true), ("Bot Carol", false)]
        );
        assert!(
            specs(&["Bob", "Bob,bot"])
                .unwrap_err()
                .contains("given twice")
        );
        assert!(
            specs(&["Bob,human"])
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(specs(&[",bot"]).unwrap_err().contains("needs a name"));
        assert!(!crate::model::Character::default().is_bot);
    }
}
