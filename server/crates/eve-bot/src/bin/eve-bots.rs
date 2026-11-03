//! N player-like bots in one process, one thread each, each playing a role
//! from `roles::ROLES` -- mining (P8b), escort (P8c), courier (CP34),
//! builder (CP47, driven since FX6), hostile (CP56), sites (CP55) --
//! (`docs/lanes/bots.md` §4.4). [`usage`] lists the rows.
//!
//! ```text
//! eve-bots --connect 10.137.0.1:26000 --name Miner1 --name Miner2 ...
//!          [--prefix Miner --count 3]   [--loop ROLE]
//!          [--pace human|fast]          [--gm-setup on|off]
//!          [--accept-fleet any|from:NAME|allied|none] [--chat on|off]
//!          [--run SECONDS]              # 0 = until killed
//!          [--tokens FILE]              # NAME TOKEN per line, outside the repo
//!          [--agent ID]                 # the courier's agent
//!          [--hostile]                  # allow --loop hostile (CP56)
//!          [--friends FILE]             # humans the bots count as friends (REL-FIX)
//!          [--society-pvp]              # bots hunt pilots they are not allied with (SOC-C5)
//!          [--society-wars]             # bots declare wars and mutual-war invites (R-REL-5)
//!          [--closed [--bootstrap]]     # the closed economy: acquire, not GM (E8)
//!          [--home SYSTEM [--bot-systems ID,ID..] [--celestials FILE | --content DIR]]
//!          [--security-floor F]         # the least security a route passes (travel)
//!          [--society [--state DIR] [--society-seed N]
//!                     [--sessions daily|always] [--day-secs N]]   # SOC-A2
//!
//! eve-bots --connect 10.137.0.1:26000 \
//!          --loop mine --prefix Miner --count 2 --loop escort --prefix Escort --count 1
//! ```
//!
//! **`--loop` is sticky**: it sets the role of every bot named after it, by
//! `--name` or by a `--prefix`/`--count` pair, so one process runs a mix.
//!
//! Each bot plays the character of its name, which it picks off the select
//! screen, so the server's roster must have a character of that name
//! (`eve-server --character Miner1 ...`; `tools/bots.sh` does both halves).
//!
//! **Its name is never its token** (ruling 2: nothing is derived from a
//! visible name). The token is the bot's line in `--tokens FILE`, else the
//! literal [`driver::DEV_TOKEN`] [A]. Without `eve-server --accounts` every
//! token is the catch-all, so no file is needed; with it, each bot needs an
//! account whose token is its line here (`eve-server --new-token NAME ROLE
//! NAME` prints the line), and a bot without one is refused at login.
//!
//! The file is one bot per line, `NAME<TAB>TOKEN` (as `--new-token` prints
//! it) or `NAME TOKEN`: the token is the last whitespace-separated field, so
//! a name may hold spaces. `#` starts a comment line. It is refused inside a
//! git work tree and when its group or others may read it (`chmod 600`),
//! since it holds the tokens themselves, not hashes (`account-spec.md` §7
//! step 3). Nothing prints a token: errors name the line.
//!
//! **`--connect` is an IP literal and nothing else.** It is parsed with
//! `SocketAddr::from_str`, which cannot resolve a name, and each bot opens
//! exactly one TCP socket, to it (`docs/guides/jail.md`, `docs/lanes/bots.md` §6).
//! No config file: flags are greppable in `ps`.
//!
//! **Homes** (CP39, C7 §6.3): `--home` is the humans' home system. Each bot
//! draws its home from `--bot-systems`, by default every system within
//! `driver::HOME_RANGE_JUMPS` of `--home` by the gate graph of
//! `--celestials` (or `--content`'s); a listed system farther out is
//! refused. No act routes through gates yet, so a bot away from its home
//! says `MISSING` once a connection and plays where it is.
//!
//! **NPC profiles** (CP46): `--content DIR` also hands every bot the
//! client's own data about each NPC type (`eve_bot::profile`), with or
//! without `--home`; the escort reads it to pick a target and to retreat.
//!
//! **The market** (FX6): a miner refines and sells its ore when it docks;
//! a `--loop builder` bot lists its goods and buys minerals on its cadence.
//! The minerals it wants are the published ones in `--content`'s type
//! table; without `--content` it only lists. C5 §7.2's demand budget is
//! the server's since E11 (bot fills on human asks), so there is no
//! `--humans` any more.
//!
//! **Society mode** (SOC-A2, R-SOCIETY §2): `--society` replaces `--loop`
//! mixes. Every bot gets a mind ([`eve_bot::mind`]) seeded from its name
//! and `--society-seed`, or read back from `--state DIR` (one file a bot,
//! [`eve_bot::state`]); its career picks its role, and it changes career
//! only docked ([`driver::decision_point`]). It plays in sessions
//! ([`driver::Timetable`]): `--sessions daily` (the default) gives each bot
//! a weekly timetable from its `diligence`; `--day-secs N` compresses a
//! day into N seconds for a soak; `--sessions always` keeps every bot on.
//! Every [`driver::SOC_SUMMARY_EVERY`] the process prints the population's
//! `[soc]` line (career mix, sessions online, switches per bot-day, and
//! SOC-C4's corps, alliances, blue and red pairs). The bots found, join and
//! leave corps and alliances and set blue and red contacts through one
//! [`eve_bot::corpmind::Hall`], kept in `--state DIR` as `society.tsv`.
//!
//! **The closed economy** (E8, R-ECON §2.4): `--closed` turns GM setup
//! off and has a bot that lacks its ship, modules, rounds or skill get them
//! the player's way ([`eve_bot::acquire`]): the rookie ship from
//! `CreateNewbieShip`, skillbooks from the school station's asks, the rest
//! from others' asks; whatever is not there is a `MISSING` line and the bot
//! keeps its rookie ship. `--bootstrap` (only with `--closed`) allows GM
//! setup once a connection first, for seeding an empty world; `--gm-setup`
//! is refused beside `--closed`. A miner sells at most once per
//! `brain::SELL_EVERY` at a person's pace.
//!
//! **The hostile bot** (CP56, plan §4.7) is opt-in: `--loop hostile` is
//! refused without `--hostile` ([`eve_bot::hostile::check_opt_in`]), and
//! only `--hostile` loads its rules ([`eve_bot::hostile::Rules::open`]):
//! `content/bots/hostile.tsv`, and with `--content` each system's security,
//! each hull's class and, with `--home`, the systems it may hunt in.
//!
//! **The hunter** (SOC-C5, [`eve_bot::hunt`]) is the society's: `--society-pvp`
//! turns on the `hunter` role (`--loop hunter` is refused without it) and,
//! with `--society`, the `pirate` career that plays it. Hunters fight any
//! pilot they are not allied with ([`eve_bot::corpmind::Hall::hostile_to`]),
//! humans included, in low sec, null sec and wormhole space, and never in
//! high sec. Their hulls and the systems' security come from `--content`;
//! without it they know no hunting ground and hunt nobody.

use eve_bot::brain::{AcceptFleet, RoleRef};
use eve_bot::driver::{self, BotConfig, Pace};
use eve_bot::roles;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

/// The help text. The roles come from `roles::ROLES`, so a new row is
/// listed with no edit here.
fn usage() -> String {
    let names: Vec<&str> = roles::ROLES.iter().map(|r| r.name()).collect();
    let rows: String = roles::ROLES
        .iter()
        .map(|r| {
            let kind = if r.civilian { "civilian" } else { "fights" };
            format!(
                "                          {:<9} {kind}: \"{}\"\n",
                r.name(),
                r.greeting
            )
        })
        .collect();
    format!(
        "\
usage: eve-bots --connect IP:PORT (--name NAME ... | --prefix P --count N ...)
                [--loop {loops}] [--pace human|fast] [--gm-setup on|off]
                [--accept-fleet any|from:NAME|allied|none] [--chat on|off]
                [--run SECONDS] [--tokens FILE] [--agent ID] [--hostile]
                [--society-pvp] [--society-wars] [--friends FILE]
                [--roster FILE [--roster-home SYSTEM]]
                [--difficulty easy|normal|hard|brutal|0..4]
                [--closed [--bootstrap]]
                [--home SYSTEM [--bot-systems ID,..] [--celestials FILE | --content DIR]]
                [--security-floor F]
                [--society [--state DIR] [--society-seed N] [--sessions daily|always]
                           [--day-secs N]]

  --roster FILE         DEV-MODE, against `eve-server --dev` (docs/guides/dev-mode.md):
                        the bots are the roster's TSV rows (group side role
                        count skills ship [system]; content/bots/dev/). Closed
                        economy; skills are granted by /devskills, kits bought
                        from the dev market. Needs --content for ship names
  --roster-home SYSTEM  the system a roster row without one is seated in
  --quiet               print no act line and no per-bot summary (a production host:
                        the act log is still kept in memory, bounded); startup,
                        `[soc]`, `[doc]` and `[stats]` lines still print
  --stats SECS          every SECS print a `[stats]` line: process CPU, RSS, threads,
                        frame bytes in, and the time all bots spent decoding frames,
                        applying destiny, refreshing, deciding, absorbing, logging
  --ramp N0,K,T[,GAP_MS]  BENCH (tools/bench-ramp.sh): replace the human login stagger.
                        The first N0 bots log in GAP_MS apart (default 50) from the
                        start; then K more every T seconds, GAP_MS apart (GAP_MS 0:
                        the whole batch at once, a login burst). Roster order
  --bubble              BUBBLE: keep at least 3 bots in each human's system (sent by
                        the gates after 45 s) and release bots while the server's
                        tick p95 is over 60 ms (needs --control DIR, which holds
                        the server's bubble.tsv, and the gate graph). Off by default.
  --brawl               BRAWL (a fleet-fight bench): hunters take any odds, never
                        warp off, and close on the nearest foe on the grid
                        instead of patrolling. Roster and /bots spawns alike.
  --control DIR         BOT-CTL: dial the server's control link (the
                        bot-control.addr and .token files in DIR, written by
                        `eve-server --bot-control DIR`), so a GM's `/bots`
                        commands spawn, list, bring and despawn bots here.
                        Zero start-up bots are fine with it; the process
                        runs until killed (or --run SECONDS)
  --difficulty LEVEL    how sharp the bots are (DOC-8): easy, normal (default),
                        hard, brutal or 0..4 (0: leaders never switch doctrine).
                        Scales bot behaviour only; table: content/bots/difficulty.tsv
  --connect IP:PORT     the server, as an IP literal; names are not resolved
  --name NAME           a bot, by its character's name. Repeatable
  --prefix P --count N  N bots named P1..PN. Repeatable, one pair at a time
  --loop ROLE           the role of the bots named after it (sticky; default
                        {default}). The roles:
{rows}                        A civilian keeps working when a human comes near
                        (its courtesy rule, which would send it home within
                        {courtesy} km of one, is off: Policy::courtesy)
  --pace human|fast     human (default): reaction delays, dwells, staggered
                        logins, at most {rate} payloads a second over {window} s.
                        fast: none of them, for tests
  --gm-setup on|off     on (default): a bot missing its ship, modules or
                        rounds gives itself them with GM slash commands,
                        once, docked. off: it logs MISSING and idles instead
  --accept-fleet WHO    any (default), from:NAME, allied (own corp or
                        alliance), or none
  --chat on|off         on (default): say a line in fleet chat or Local now
                        and then, if the server's chat is on. off: never try
  --chat-host IP:PORT   dial this address for chat instead of the one the
                        server advertises (default: the advertised one on
                        5222). For bots in the server's own network
                        namespace, e.g. 127.0.0.1:5222
  --run SECONDS         stop after this long; 0 (default) runs until killed
  --tokens FILE         each bot's SSO token, `NAME TOKEN` per line, outside
                        the repo, chmod 600. A bot with no line (or no file)
                        sends `dev`, which any --accounts server refuses.
                        A name is never sent as a token
  --agent ID            the agent a courier takes missions from; without
                        it a courier logs MISSING
  --hostile             allow --loop hostile: a bot that attacks humans
                        outside its fleet, only where the policy table
                        (content/bots/hostile.tsv) allows and never where
                        it would be a crime (needs --content for security)
  --friends FILE        humans the bots count as friends: CHARACTER[<TAB>STANDING]
                        per line (id or exact name; default +3); every bot sets
                        a personal contact and never hunts them (REL-FIX)
  --society-wars        with --society: bot corps and alliances declare war
                        (or a mutual-war invite between two bot groups) on a
                        group their relation marks an enemy, and their
                        members hunt its pilots in high sec while it lasts;
                        a group of humans only with --hostile too. Needs the
                        corp wallet to hold the fee (500 M) twice over
  --society-pvp         allow --loop hunter, and with --society the pirate
                        career: bots hunt any pilot they are not allied
                        with, humans included, in low sec, null sec and
                        wormhole space (never high sec). Needs --content
                        for the systems' security and the hulls' classes;
                        the killmails of every fight go into the society's
                        matrix (SOC-C5)
  --closed              the closed economy: no GM setup; a bot lacking its
                        ship, modules, rounds or skill takes the rookie ship
                        (CreateNewbieShip), buys skillbooks at its school and
                        the rest from asks at its station, every {acquire} min
                        at most, and logs MISSING for what is not there
  --bootstrap           with --closed only: GM setup is allowed first, once
                        a connection (seeding an empty world)
  --home SYSTEM         the humans' home system: each bot's home is drawn
                        from --bot-systems, by default every system within
                        {jumps} jumps of it (needs the gate graph, from
                        --celestials FILE or --content DIR; without one, the
                        home alone)
  --content DIR         the content bundle (tools/up.sh): the gate graph for
                        --home, the NPC profiles an escort picks its
                        target and retreats by (without it, by distance
                        alone), and the minerals a builder buys
  --real-fits FILE      the fits players fly (tools/killfits.py's
                        out/bots/fits.tsv), seeding the doctrines and each
                        bot's fit. Default with --content: the bundle's
                        sibling bots/fits.tsv when it exists
  --celestials FILE     the celestial catalogue alone (gates, stations)
                        With either, bots travel: a builder or a --closed
                        bot whose station sells no blueprint, book, hull or
                        module it needs fetches it from the nearest station
                        within {trip} jumps that does, over the gates
  --security-floor F    the least security a route passes through (default
                        {floor}); security comes from --content only, and a
                        system with none passes only -inf
  --society             every bot gets a mind: its career picks its role
                        (no --loop), weighed docked every {decide} min, and
                        it plays in sessions; bots found and join corps and
                        alliances and set blues and reds (SOC-C4). A [soc]
                        line says each choice
  --state DIR           with --society: one state file a bot, read at start
                        and written at each decision and log-off
  --society-seed N      with --society: the seed traits are drawn under
                        (default {seed}); a stored mind keeps its own
  --sessions WHEN       with --society: daily (default), a weekly timetable
                        of {lo}-{hi} h a day by diligence; or always
  --day-secs N          with --sessions daily: a day lasts N seconds
                        (default {day}), to see sessions in a short soak
",
        loops = names.join("|"),
        default = roles::default().name(),
        courtesy = eve_bot::brain::COURTESY_RANGE / 1_000.0,
        rate = driver::BOT_PAYLOADS_PER_S,
        window = driver::PACING_WINDOW.as_secs(),
        jumps = driver::HOME_RANGE_JUMPS,
        acquire = eve_bot::acquire::ACQUIRE_EVERY.as_secs() / 60,
        trip = eve_bot::travel::TRIP_MAX_JUMPS,
        floor = eve_bot::travel::SECURITY_FLOOR,
        decide = driver::DECIDE_EVERY.as_secs() / 60,
        seed = driver::SOCIETY_SEED,
        lo = driver::SESSION_HOURS.0,
        hi = driver::SESSION_HOURS.1,
        day = driver::DAY_SECS,
    )
}

/// **Fewer malloc arenas** (BOT-OPT). glibc gives each thread its own arena
/// (up to 8 x cores) and a bot is a thread: 40 bots held 554 MB RSS, and 288 MB
/// with `MALLOC_ARENA_MAX=2`, the same work. The variable is read when the
/// process starts, so the process starts itself again once with it set
/// (`exec`: same pid, same stdout; a failed exec carries on as it is). Set
/// `MALLOC_ARENA_MAX` yourself to choose another number, or to switch this off.
#[cfg(target_os = "linux")]
fn fewer_arenas() {
    use std::os::unix::process::CommandExt;
    if std::env::var_os("MALLOC_ARENA_MAX").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("MALLOC_ARENA_MAX", "2")
        .exec();
    eprintln!("eve-bots: could not restart with MALLOC_ARENA_MAX=2 ({err}); going on");
}

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    fewer_arenas();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("eve-bots: {why}\n\n{}", usage());
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut connect: Option<String> = None;
    let mut names: Vec<(String, RoleRef)> = Vec::new();
    let mut prefix: Option<String> = None;
    let mut count: Option<usize> = None;
    let mut role = roles::default();
    let mut chat = true;
    let mut chat_host: Option<SocketAddr> = None;
    let mut pace = Pace::Human;
    let mut gm_setup = true;
    let mut gm_flag = false;
    let mut closed = false;
    let mut bootstrap = false;
    let mut accept = AcceptFleet::Any;
    let mut run_for: Option<Duration> = None;
    let mut tokens: Option<String> = None;
    let mut friends_file: Option<String> = None;
    let mut agent: Option<i64> = None;
    let mut home: Option<i64> = None;
    let mut security_floor = eve_bot::travel::SECURITY_FLOOR;
    let mut bot_systems: Vec<i64> = Vec::new();
    let mut celestials: Option<String> = None;
    let mut content: Option<String> = None;
    let mut real_fits_file: Option<String> = None;
    let mut hostile = false;
    let mut pvp = false;
    let mut society_wars = false;
    let mut society = false;
    let mut looped = false;
    let mut state: Option<String> = None;
    let mut society_seed: Option<u64> = None;
    let mut sessions: Option<bool> = None;
    let mut day_secs: Option<u64> = None;
    let mut difficulty_level = eve_bot::difficulty::NORMAL_LEVEL;
    let mut roster: Option<String> = None;
    let mut roster_home: Option<String> = None;
    let mut control: Option<String> = None;
    let mut bubble = false;
    let mut brawl = false;
    let mut ramp: Option<(usize, usize, u64, u64)> = None;
    let mut quiet = false;
    let mut stats_every: Option<Duration> = None;
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", usage());
                return Ok(());
            }
            "--connect" => connect = Some(value("--connect")?),
            "--name" => names.push((value("--name")?, role)),
            "--prefix" => {
                if prefix.is_some() {
                    return Err("--prefix twice before its --count".to_owned());
                }
                prefix = Some(value("--prefix")?);
            }
            "--count" => {
                if count.is_some() {
                    return Err("--count twice before its --prefix".to_owned());
                }
                let v = value("--count")?;
                count = Some(v.parse().map_err(|e| format!("--count {v}: {e}"))?);
            }
            "--loop" => {
                looped = true;
                let v = value("--loop")?;
                role = roles::parse(&v).ok_or_else(|| format!("--loop {v}: {}", roles::names()))?;
            }
            "--chat" => {
                chat = match value("--chat")?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("--chat {other}: on or off")),
                }
            }
            "--chat-host" => chat_host = Some(parse_chat_host(&value("--chat-host")?)?),
            "--pace" => {
                pace = match value("--pace")?.as_str() {
                    "human" => Pace::Human,
                    "fast" => Pace::Fast,
                    other => return Err(format!("--pace {other}: human or fast")),
                }
            }
            "--gm-setup" => {
                gm_flag = true;
                gm_setup = match value("--gm-setup")?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("--gm-setup {other}: on or off")),
                }
            }
            "--accept-fleet" => {
                let v = value("--accept-fleet")?;
                accept = match v.as_str() {
                    "any" => AcceptFleet::Any,
                    "none" => AcceptFleet::Nobody,
                    "allied" => AcceptFleet::Allied,
                    _ => match v.strip_prefix("from:") {
                        Some(name) if !name.is_empty() => AcceptFleet::From(name.to_owned()),
                        _ => return Err(format!("--accept-fleet {v}: any, from:NAME or none")),
                    },
                }
            }
            "--run" => {
                let v = value("--run")?;
                let secs: u64 = v.parse().map_err(|e| format!("--run {v}: {e}"))?;
                run_for = (secs > 0).then(|| Duration::from_secs(secs));
            }
            "--tokens" => tokens = Some(value("--tokens")?),
            "--friends" => friends_file = Some(value("--friends")?),
            "--agent" => agent = Some(id("--agent", &value("--agent")?)?),
            "--home" => home = Some(id("--home", &value("--home")?)?),
            "--bot-systems" => {
                for v in value("--bot-systems")?.split(',') {
                    bot_systems.push(id("--bot-systems", v.trim())?);
                }
            }
            "--celestials" => celestials = Some(value("--celestials")?),
            "--content" => content = Some(value("--content")?),
            "--real-fits" => real_fits_file = Some(value("--real-fits")?),
            "--security-floor" => {
                let v = value("--security-floor")?;
                security_floor = v
                    .parse::<f64>()
                    .ok()
                    .filter(|f| !f.is_nan())
                    .ok_or_else(|| format!("--security-floor {v}: want a number"))?;
            }
            "--difficulty" => {
                difficulty_level =
                    eve_bot::difficulty::Difficulty::parse_level(&value("--difficulty")?)?;
            }
            "--roster" => roster = Some(value("--roster")?),
            "--roster-home" => roster_home = Some(value("--roster-home")?),
            "--control" => control = Some(value("--control")?),
            "--bubble" => bubble = true,
            "--brawl" => brawl = true,
            "--quiet" => quiet = true,
            "--stats" => {
                let v = value("--stats")?;
                let secs: u64 = v
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("--stats {v}: want seconds above 0"))?;
                stats_every = Some(Duration::from_secs(secs));
            }
            "--ramp" => {
                let v = value("--ramp")?;
                let n: Vec<u64> = v
                    .split(',')
                    .map(|x| x.trim().parse().ok())
                    .collect::<Option<_>>()
                    .ok_or_else(|| format!("--ramp {v}: want N0,K,T[,GAP_MS] (numbers)"))?;
                if !(3..=4).contains(&n.len()) || n[1] == 0 {
                    return Err(format!("--ramp {v}: want N0,K,T[,GAP_MS] with K above 0"));
                }
                ramp = Some((
                    n[0] as usize,
                    n[1] as usize,
                    n[2],
                    n.get(3).copied().unwrap_or(50),
                ));
            }
            "--hostile" => hostile = true,
            "--society-pvp" => pvp = true,
            "--society-wars" => society_wars = true,
            "--closed" => closed = true,
            "--bootstrap" => bootstrap = true,
            "--society" => society = true,
            "--state" => state = Some(value("--state")?),
            "--society-seed" => {
                let v = value("--society-seed")?;
                society_seed = Some(v.parse().map_err(|e| format!("--society-seed {v}: {e}"))?);
            }
            "--sessions" => {
                sessions = match value("--sessions")?.as_str() {
                    "daily" => Some(true),
                    "always" => Some(false),
                    other => return Err(format!("--sessions {other}: daily or always")),
                }
            }
            "--day-secs" => {
                let v = value("--day-secs")?;
                let n: u64 = v.parse().map_err(|e| format!("--day-secs {v}: {e}"))?;
                if n == 0 {
                    return Err("--day-secs 0: want a positive number".to_owned());
                }
                day_secs = Some(n);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
        // A pair is complete: its bots take the role in force now.
        if let (Some(p), Some(n)) = (&prefix, count) {
            names.extend((1..=n).map(|i| (format!("{p}{i}"), role)));
            prefix = None;
            count = None;
        }
    }

    // **An IP literal, never a name** (module header).
    let connect = connect.ok_or("--connect is required")?;
    let addr: SocketAddr = connect.parse().map_err(|_| {
        format!("--connect wants an IP:PORT literal, got {connect:?} (names are not resolved)")
    })?;
    if prefix.is_some() || count.is_some() {
        return Err("--prefix and --count go together".to_owned());
    }
    // **DEV-MODE roster** (`eve_bot::dev`): the roster's bots are the bots,
    // each with its dev settings; the economy is closed, so every kit is
    // bought from the dev market.
    let mut dev_map: BTreeMap<String, std::sync::Arc<eve_bot::dev::DevBot>> = BTreeMap::new();
    let mut gang_map: BTreeMap<String, eve_bot::roam_gang::Seat> = BTreeMap::new();
    let mut wing_map: BTreeMap<String, eve_bot::wing::Seat> = BTreeMap::new();
    if let Some(path) = &roster {
        if !names.is_empty() {
            return Err("--roster replaces --name and --prefix/--count".to_owned());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("--roster {path}: {e}"))?;
        let mut rows =
            eve_bot::dev::parse_roster(&text).map_err(|e| format!("--roster {path}: {e}"))?;
        for r in &mut rows {
            if r.system.is_none() {
                r.system.clone_from(&roster_home);
            }
        }
        let c = match content.as_deref() {
            Some(dir) => Some(eve_bot::hulls::open(Path::new(dir))?),
            None => {
                println!(
                    "eve-bots: --roster without --content: ship names cannot be resolved; role skills become all5"
                );
                None
            }
        };
        for e in eve_bot::dev::expand(&rows, c.as_ref())? {
            names.push((e.name.clone(), e.role));
            if let Some(seat) = e.gang {
                gang_map.insert(e.name.clone(), seat);
            }
            if let Some(seat) = e.wing {
                wing_map.insert(e.name.clone(), seat);
            }
            dev_map.insert(e.name, e.dev);
        }
        closed = true;
        println!(
            "eve-bots: --roster {path}: {} bots in {} groups, economy closed",
            names.len(),
            rows.len()
        );
    } else if roster_home.is_some() {
        return Err("--roster-home needs --roster".to_owned());
    }
    if names.is_empty() && control.is_none() {
        return Err("no bots: give --name or --prefix/--count (or --control DIR)".to_owned());
    }
    if bubble && control.is_none() {
        return Err(
            "--bubble reads the server's bubble.tsv from the --control directory".to_owned(),
        );
    }
    if control.is_some() && society {
        return Err("--control is not for --society runs (the minds pick the roles)".to_owned());
    }
    for (i, (n, _)) in names.iter().enumerate() {
        if names[..i].iter().any(|(m, _)| m == n) {
            return Err(format!("--name {n}: given twice"));
        }
    }

    // **Society mode** (SOC-A2): the mind picks the role, so no --loop;
    // its flags only beside it.
    let society = society_options(
        society,
        looped,
        state.is_some(),
        society_seed.is_some(),
        sessions.is_some(),
        day_secs.is_some(),
    )?;
    if sessions == Some(false) && day_secs.is_some() {
        return Err(
            "--day-secs with --sessions always: a day only times daily sessions".to_owned(),
        );
    }

    // **The closed economy** (E8): GM setup only with `--bootstrap`.
    let gm_setup = closed_gm_setup(closed, bootstrap, gm_flag, gm_setup)?;

    // **Opt-in** (CP56): no hostile bot without `--hostile`.
    eve_bot::hostile::check_opt_in(names.iter().map(|(_, r)| *r), hostile)?;
    // **Opt-in** (SOC-C5): no hunter without `--society-pvp`.
    eve_bot::hunt::check_opt_in(names.iter().map(|(_, r)| *r), pvp)?;
    eve_bot::roam_gang::check_opt_in(names.iter().map(|(_, r)| *r), pvp)?;

    let friends = match &friends_file {
        Some(path) => Some(std::sync::Arc::new(eve_bot::friends::Friends::read(
            Path::new(path),
        )?)),
        None => None,
    };
    let tokens = match &tokens {
        Some(path) => load_tokens(Path::new(path))?,
        None => BTreeMap::new(),
    };
    let untokened: Vec<&str> = names
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| !tokens.contains_key(*n))
        .collect();
    if !untokened.is_empty() {
        println!(
            "eve-bots: no token for {} bot(s), they send {:?}: {}",
            untokened.len(),
            driver::DEV_TOKEN,
            untokened.join(" ")
        );
    }

    let homes = homes(
        home,
        &bot_systems,
        celestials.as_deref(),
        content.as_deref(),
    )?;
    // The client's static data, read once (FX10, `eve_bot::hulls`): hull
    // and module (FX11, `eve_bot::modules`) numbers where the server's own
    // row is not read, and the NPC data for
    // the escort's target choice and retreat (CP46, `eve_bot::profile`).
    // Without `--content`, the invented fixture and no profiles.
    let tables = match content.as_deref() {
        Some(dir) => {
            let mut c = eve_bot::hulls::open(Path::new(dir))?;
            println!("eve-bots: --content {dir}: hull and module numbers from the bundle");
            // The minerals each system's rocks yield, so a builder never
            // works a blueprint its miners cannot feed (GB-FIX).
            if let Some(s) = eve_bot::supply::Supply::open(Path::new(dir), &c)? {
                println!("eve-bots: --content: ore supply of {} systems", s.systems());
                c.insert("mineral_supply", s);
            }
            // PROG-NULL: who mines the deep belts, so builders count their
            // minerals as obtainable.
            c.insert("rare_board", eve_bot::rare_ore::RareBoard::new());
            // FE-BUILD / DOC-7: the parts the leaders' doctrines want, which
            // builders read to pick what to make and what rung to climb.
            c.insert("parts_board", eve_bot::doctrine_parts::PartsBoard::new());
            Some(std::sync::Arc::new(c))
        }
        None => {
            println!("{}", eve_bot::hulls::NO_CONTENT);
            println!("{}", eve_bot::modules::NO_CONTENT);
            None
        }
    };
    let profiles = tables.as_deref().map(|c| {
        let p = eve_bot::profile::Profiles::from_content(c);
        println!("eve-bots: --content: {} NPC profiles", p.len());
        std::sync::Arc::new(p)
    });
    // The hostile bot's rules, only when opted in (CP56).
    let rules = if hostile {
        let r = eve_bot::hostile::Rules::open(content.as_deref().map(Path::new), home)?;
        println!("eve-bots: --hostile: {r:?}");
        Some(std::sync::Arc::new(r))
    } else {
        None
    };
    // The hunters' doctrine, only when opted in (SOC-C5).
    let doctrine = if pvp {
        let d = eve_bot::hunt::Ground::open(content.as_deref().map(Path::new))?;
        println!("eve-bots: --society-pvp: {d:?}");
        Some(std::sync::Arc::new(d))
    } else {
        None
    };
    // The geode crate types and SKIN licences (L7 BOT-ECON): every bot may
    // open a geode or wear a SKIN, so this is not opt-in.
    let econ = std::sync::Arc::new(eve_bot::geode_haul::Table::open(
        content.as_deref().map(Path::new),
    )?);
    // The minerals a builder buys (FX6), only when one runs (in society
    // mode any bot may become one).
    let builders = names
        .iter()
        .filter(|(_, r)| r.name() == eve_bot::builder::ROLE.name())
        .count();
    let wants = match content.as_deref() {
        Some(dir) if builders > 0 || society => {
            let w = eve_bot::builder::wants_in(Path::new(dir))?;
            println!(
                "eve-bots: {builders} builder(s), {} mineral types wanted",
                w.len()
            );
            w
        }
        _ => Vec::new(),
    };
    let wants = std::sync::Arc::new(wants);
    // DOC-8: the one difficulty setting, the table under it from
    // content/bots/difficulty.tsv or the compiled-in copy.
    let difficulty = eve_bot::difficulty::Difficulty::open(
        difficulty_level,
        &eve_server::authored::Dir::default(),
    )?;
    let knobs = difficulty.knobs();
    println!(
        "eve-bots: difficulty {} ({})",
        difficulty.level(),
        difficulty.name()
    );
    // The map bots route over (the travel lane), from the same catalogue.
    let map = eve_bot::travel::Map::open(
        celestials.as_deref().map(Path::new),
        content.as_deref().map(Path::new),
    )?
    .map(std::sync::Arc::new);
    match &map {
        Some(_) => println!("eve-bots: travel on, security floor {security_floor}"),
        None => println!("eve-bots: no --celestials or --content: bots do not travel"),
    }
    // **The minds** (society mode): read back or seeded, one per bot.
    let online = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut minds: BTreeMap<String, driver::Society> = BTreeMap::new();
    // SOC-C4: the corp minds, one hall for the process.
    let mut hall: Option<std::sync::Arc<eve_bot::corpmind::Hall>> = None;
    // DOC-4: the board the members post what they see on; the battle-tick
    // thread reads it. Set with the hall.
    let mut board: Option<std::sync::Arc<eve_bot::doctrine_sense::Board>> = None;
    if society {
        let seed = society_seed.unwrap_or(driver::SOCIETY_SEED);
        let signals = signals(pvp);
        let now = driver::unix_now();
        let dir = state.as_deref().map(Path::new);
        let day = day_secs.unwrap_or(driver::DAY_SECS);
        let h = match dir {
            Some(d) => {
                let (h, said) = eve_bot::corpmind::Hall::open(d, day);
                if let Err(why) = said {
                    println!("eve-bots: a torn society file, started afresh: {why}");
                }
                h
            }
            None => eve_bot::corpmind::Hall::new(day),
        };
        // R-REL-2: the hall classes hulls and security bands for the facts it
        // observes (pods, ganks, where); the hunters' ground when opted in,
        // else the catalogue's, else none (every hull unclassed).
        let ground = match &doctrine {
            Some(d) => Some(std::sync::Arc::clone(d)),
            None => eve_bot::hunt::Ground::open(content.as_deref().map(Path::new))
                .ok()
                .map(std::sync::Arc::new),
        };
        let h = match ground {
            Some(g) => h.with_ground(g),
            None => h,
        };
        // DOC-4/5: the doctrine layer awake at the run's difficulty: the catalog over what the world can make, built
        // once; without type data in `--content` the layer sleeps.
        let real_fits = {
            // `<repo>/out/content` -> `<repo>/out/bots/fits.tsv`; a missing
            // file is silent, an unreadable one an error.
            let path = real_fits_file
                .as_deref()
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    content
                        .as_deref()
                        .map(|d| Path::new(d).join("..").join("bots").join("fits.tsv"))
                });
            match path {
                Some(p) => eve_bot::real_fits::load(&p)?,
                None => None,
            }
        };
        if let Some(r) = &real_fits {
            let rows: usize = r.by_hull.values().map(Vec::len).sum();
            let makeable = tables
                .as_deref()
                .map(eve_bot::doctrine_hall::DoctrineWorld::makeable_of);
            let ok = |t: i64| makeable.as_ref().is_none_or(|m| m.has(t));
            let usable: usize = r.by_hull.keys().map(|h| r.usable(*h, &ok).count()).sum();
            println!(
                "eve-bots: real fits: {} hulls, {rows} rows, {usable} usable",
                r.by_hull.len()
            );
        }
        let h = match tables.as_deref().and_then(|c| {
            eve_bot::doctrine_hall::DoctrineWorld::from_content_with(
                c,
                Default::default(),
                real_fits.as_ref(),
            )
        }) {
            Some(world) => {
                println!(
                    "eve-bots: doctrine layer on: {} doctrines, level {}",
                    world.catalog.len(),
                    difficulty.level()
                );
                h.with_doctrine(std::sync::Arc::new(world), difficulty.level(), seed)
            }
            None => {
                println!("eve-bots: doctrine layer off: no type data (--content)");
                h
            }
        };
        let h = h
            .with_difficulty(&difficulty)
            .with_wars(society_wars, hostile);
        hall = Some(std::sync::Arc::new(h));
        // DOC-3: one board for the process, where members post what they see.
        let board_arc = std::sync::Arc::new(eve_bot::doctrine_sense::Board::default());
        board = Some(board_arc.clone());
        let board = board_arc;
        let (mut fresh, mut restored, mut torn) = (0, 0, 0);
        for (n, _) in &names {
            let (mut soc, source) =
                driver::Society::open(n, seed, eve_bot::career::CAREERS, signals.clone(), dir, now);
            match source {
                eve_bot::state::Source::Fresh => fresh += 1,
                eve_bot::state::Source::Restored => restored += 1,
                eve_bot::state::Source::Torn(why) => {
                    torn += 1;
                    println!("eve-bots: {n}: a torn state file, seeded again: {why}");
                }
            }
            let (diligence, mind_seed) = {
                let m = soc.mind.lock().map_err(|_| "a mind's lock")?;
                (m.traits.get(eve_bot::mind::Trait::Diligence), m.seed)
            };
            if sessions != Some(false) {
                soc.timetable = driver::Timetable::weekly(
                    n,
                    mind_seed,
                    diligence,
                    day_secs.unwrap_or(driver::DAY_SECS),
                );
            }
            soc.online = std::sync::Arc::clone(&online);
            soc.hall = hall.clone();
            soc.board = Some(board.clone());
            minds.insert(n.clone(), soc);
        }
        println!(
            "eve-bots: --society seed {seed}, minds {fresh} fresh, {restored} restored, \
             {torn} torn; state {}; sessions {}",
            state.as_deref().unwrap_or("none (this run only)"),
            match (sessions, day_secs) {
                (Some(false), _) => "always".to_owned(),
                (_, Some(d)) => format!("daily, a day of {d} s"),
                _ => "daily".to_owned(),
            }
        );
    }
    let roster: Vec<String> = names
        .iter()
        .map(|(n, r)| {
            let role = match minds.get(n) {
                Some(soc) => format!("{}={}", soc.career(), soc.role().name()),
                None => r.name().to_owned(),
            };
            match driver::pick_home(n, &homes) {
                Some(h) => format!("{n}:{role}@{h}"),
                None => format!("{n}:{role}"),
            }
        })
        .collect();
    // BENCH: with --ramp, the roster's groups are interleaved in proportion
    // to their size (a group's k-th of n goes at (k+0.5)/n), so every step
    // adds the same mix; each group's boss is still its first.
    if ramp.is_some() && !dev_map.is_empty() {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut total: BTreeMap<String, usize> = BTreeMap::new();
        for (n, _) in &names {
            if let Some(d) = dev_map.get(n) {
                *total.entry(d.group.clone()).or_default() += 1;
            }
        }
        let mut keyed: Vec<(f64, (String, _))> = Vec::new();
        for item in std::mem::take(&mut names) {
            let key = match dev_map.get(&item.0) {
                Some(d) => {
                    let k = seen.entry(d.group.clone()).or_default();
                    let key = (*k as f64 + 0.5) / total[&d.group] as f64;
                    *k += 1;
                    key
                }
                None => 0.0,
            };
            keyed.push((key, item));
        }
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        names = keyed.into_iter().map(|(_, i)| i).collect();
    }
    let marker = driver::BotMarker::default();
    println!(
        "eve-bots: {} bot(s) -> {addr}, pace {pace:?}, gm-setup {}, economy {}, \
         accept-fleet {accept:?}, chat {}: {}",
        names.len(),
        if gm_setup { "on" } else { "off" },
        if closed { "closed" } else { "open" },
        if chat { "on" } else { "off" },
        roster.join(" ")
    );
    // The Nth bot logs in N staggers after the first, each stagger its own
    // draw: from the names, so a rerun staggers the same way. [A]
    let joined: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
    let mut seed = eve_bot::brain::name_hash(&joined.join(",")) | 1;
    let mut delay = Duration::ZERO;
    let mut threads = Vec::new();
    // BOT-ADMIN: the launch bots' handles, for `/bots pause|inspect|stop-all`
    // and the KILL file.
    let mut launched: Vec<(String, driver::Handle)> = Vec::new();
    // BUBBLE: the same handles, for the planner.
    let mut handles = Vec::new();
    // SOC-D5: one sov hall for the process, shared by every `sov` bot.
    let sov_hall = std::sync::Arc::new(eve_bot::sov_bot::SovHall::new());
    // FC-1: where each wing's FC is, shared by every wing bot of the process.
    let wing_hall = std::sync::Arc::new(eve_bot::wing_hall::WingHall::new());
    // PERF-BURST: one price index for every bot of the process.
    let price_cache = std::sync::Arc::new(eve_bot::pricing::PriceIndexCache::default());
    for (idx, (name, role)) in names.iter().enumerate() {
        if let Some((n0, k, t, gap)) = ramp {
            // BENCH: batch 0 is the first n0 bots, batch b the b-th k after.
            let (batch, at) = if idx < n0 {
                (0, idx)
            } else {
                ((idx - n0) / k + 1, (idx - n0) % k)
            };
            delay = Duration::from_secs(t * batch as u64) + Duration::from_millis(gap * at as u64);
        }
        let mut config = BotConfig::new(name, addr, pace).with_role(role);
        if let Some(token) = tokens.get(name) {
            config = config.with_token(token);
        }
        config.chat = chat;
        apply_chat_host(&mut config, chat_host);
        config.friends = friends.clone();
        config.price_cache = Some(price_cache.clone());
        config.policy.gm_setup = gm_setup;
        config.policy.closed = closed;
        config.policy.accept_fleet = accept.clone();
        config.policy.agent = agent;
        config.policy.profiles = profiles.clone();
        config.policy.hostile = rules.clone();
        config.policy.trader = knobs.trader;
        config.policy.counter = knobs.counter;
        config.policy.pvp = doctrine.clone();
        config.policy.brawl = brawl;
        config.policy.econ = econ.clone();
        config.policy.sov = Some(std::sync::Arc::clone(&sov_hall));
        config.policy.wants = std::sync::Arc::clone(&wants);
        config.policy.travel = map.clone();
        config.policy.dev = dev_map.get(name).cloned();
        config.policy.gang = gang_map.get(name).cloned().map(|mut seat| {
            seat.knobs = knobs.gang;
            seat
        });
        config.policy.wing = wing_map.get(name).cloned().map(|mut seat| {
            seat.hall = std::sync::Arc::clone(&wing_hall);
            seat
        });
        config.policy.security_floor = security_floor;
        config.marker = marker.clone();
        config.home_system = driver::pick_home(name, &homes);
        config.content = tables.clone();
        config.start_delay = delay;
        config.run_for = run_for.map(|d| d + delay);
        config.echo = !quiet;
        if let Some(soc) = minds.get(name) {
            config = config.with_society(soc.clone());
        }
        let (handle, thread) = driver::spawn(config);
        handles.push(handle.clone());
        launched.push((name.to_owned(), handle));
        threads.push(thread);
        let (lo, hi) = pace.stagger();
        let draw = eve_bot::brain::unit(eve_bot::brain::xorshift(&mut seed));
        if ramp.is_none() {
            delay += lo + hi.saturating_sub(lo).mul_f64(draw);
        }
    }
    // BOT-CTL: the GM's `/bots` over the server's control link. The hook
    // builds a bot the way the loop above does, for a group's `<group>-<n>`
    // names; `skills` and `ship` are DEV-MODE's (the dev grant), not read here.
    let control_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let control_thread = control.as_deref().map(|dir| {
        let wing_hall = wing_hall.clone();
        let (tokens, friends, profiles, rules, doctrine, sov_hall, wants, map, marker, tables) = (
            tokens.clone(),
            friends.clone(),
            profiles.clone(),
            rules.clone(),
            doctrine.clone(),
            sov_hall.clone(),
            wants.clone(),
            map.clone(),
            marker.clone(),
            tables.clone(),
        );
        let (accept, trader, counter) = (accept.clone(), knobs.trader, knobs.counter);
        let dev_content = content.clone();
        let gang_knobs = knobs.gang;
        let hook: eve_bot::control::SpawnHook = Box::new(move |spec| {
            let role = roles::parse(&spec.role)
                .ok_or_else(|| format!("role {}: {}", spec.role, roles::names()))?;
            eve_bot::hostile::check_opt_in(std::iter::once(role), rules.is_some())?;
            eve_bot::hunt::check_opt_in(std::iter::once(role), doctrine.is_some())?;
            // The DEV-MODE seam: a spawn is one roster row, expanded the way
            // --roster expands one, so its bots get the dev skills, side and
            // hull from the dev market.
            let line = format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                spec.group, spec.side, spec.role, spec.count, spec.skills, spec.ship
            );
            let rows = eve_bot::dev::parse_roster(&line)?;
            let c = match dev_content.as_deref() {
                Some(dir) => Some(eve_bot::hulls::open(Path::new(dir))?),
                None => None,
            };
            let mut members = Vec::new();
            for e in eve_bot::dev::expand_with(&rows, c.as_ref(), gang_knobs)? {
                let name = e.name;
                let mut config = BotConfig::new(&name, addr, pace).with_role(e.role);
                config.policy.dev = Some(e.dev);
                // FAST-KIT: a spawned bot undocks as soon as it is armed.
                config.policy.quick = true;
                // ROAM-GANG: a roamer without its seat falls back to the
                // lone hunter's rules and never joins the gang's plans.
                // GANG-SUMMON: a gang spawned `hunt me` knows its quarry.
                if let (Some(seat), Some((who, system))) = (&e.gang, spec.quarry) {
                    eve_bot::roam_gang::summon(seat, system, who);
                }
                config.policy.gang = e.gang;
                // FC-1: a wing follows the FC the spawn names (its `fc=`),
                // through the process's hall, and takes only his invites.
                config.policy.wing = e.wing.map(|mut seat| {
                    seat.hall = std::sync::Arc::clone(&wing_hall);
                    seat.fc = spec.fc;
                    seat
                });
                // BOT-ACCOUNTS: the login the server just made wins over the
                // file's (a reused name got a new password).
                if let Some(token) = spec
                    .tokens
                    .get(&name)
                    .or_else(|| tokens.get(&name).map(String::as_str))
                {
                    config = config.with_token(token);
                }
                config.chat = chat;
                apply_chat_host(&mut config, chat_host);
                apply_chat_host(&mut config, chat_host);
                config.friends = friends.clone();
                config.price_cache = Some(price_cache.clone());
                config.policy.gm_setup = gm_setup;
                config.policy.closed = true;
                config.policy.accept_fleet = match spec.fc.filter(|_| config.policy.wing.is_some())
                {
                    Some(fc) => eve_bot::brain::AcceptFleet::Fc(fc),
                    None => accept.clone(),
                };
                config.policy.agent = agent;
                config.policy.profiles = profiles.clone();
                config.policy.hostile = rules.clone();
                config.policy.trader = trader;
                config.policy.counter = counter;
                config.policy.pvp = doctrine.clone();
                config.policy.brawl = brawl;
                config.policy.econ = econ.clone();
                config.policy.sov = Some(std::sync::Arc::clone(&sov_hall));
                config.policy.wants = std::sync::Arc::clone(&wants);
                config.policy.travel = map.clone();
                config.policy.security_floor = security_floor;
                config.marker = marker.clone();
                config.home_system = spec.system;
                config.content = tables.clone();
                config.run_for = run_for;
                config.echo = !quiet;
                let (handle, thread) = driver::spawn(config);
                members.push(eve_bot::control::Member {
                    name,
                    handle,
                    thread,
                });
            }
            Ok(members)
        });
        let mut ctl = eve_bot::control::Controller::new(hook);
        ctl.adopt_loose(launched.clone());
        let controller = std::sync::Arc::new(std::sync::Mutex::new(ctl));
        println!("eve-bots: --control {dir}: GM /bots commands are answered here");
        (
            eve_bot::control::run_link(
                std::path::PathBuf::from(dir),
                controller.clone(),
                control_stop.clone(),
            ),
            controller,
        )
    });
    // DOC-4: the battle tick, every `BATTLE_TICK_S` of wall time: the hall
    // reads the board, steps its theatres and queues the commanders'
    // actions (a commander's bot runs them on its own turn).
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let battle = match (&hall, &board) {
        (Some(h), Some(b)) => {
            let (h, b, stop) = (h.clone(), b.clone(), stop.clone());
            let parts_tables = tables.clone();
            Some(std::thread::spawn(move || {
                let every = Duration::from_secs(eve_bot::doctrine_sense::BATTLE_TICK_S);
                let mut next = std::time::Instant::now() + every;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(250));
                    if std::time::Instant::now() < next {
                        continue;
                    }
                    next += every;
                    for line in h.theatre_tick(driver::unix_now(), &b) {
                        println!("{line}");
                    }
                    // FE-BUILD / DOC-7: what the leaders' doctrines want,
                    // for the builders to read.
                    if let Some(parts) = parts_tables
                        .as_deref()
                        .and_then(|t| t.try_get::<eve_bot::doctrine_parts::PartsBoard>())
                    {
                        eve_bot::doctrine_parts::publish_from_hall(parts, &h.lock());
                    }
                }
            }))
        }
        _ => None,
    };
    // BUBBLE: pull bots to the humans and shed them under load (opt-in).
    // Only the bots started at launch are in it; a GM's `/bots spawn` ones
    // are not.
    let mut bubble_run = match (bubble, &map, &control) {
        (true, Some(m), Some(dir)) => {
            println!(
                "eve-bots: --bubble: {} bot(s) in the attention bubble",
                handles.len()
            );
            Some(eve_bot::bubble::Runner::new(
                Path::new(dir),
                m.clone(),
                security_floor,
                handles.clone(),
            ))
        }
        (true, None, _) => {
            println!("eve-bots: --bubble needs the gate graph (--content or --celestials): off");
            None
        }
        _ => None,
    };
    // The population's `[soc]` line now and then while the bots run.
    let mut last = std::time::Instant::now();
    let started = std::time::Instant::now();
    let mut reporter = eve_bot::stats::Reporter::default();
    let mut stats_last = std::time::Instant::now();
    // ADMIN-WEB: the groups' roster for the server's admin page, every 5 s.
    let mut groups_last: Option<std::time::Instant> = None;
    while threads.iter().any(|t| !t.is_finished())
        || (control_thread.is_some() && run_for.is_none_or(|d| started.elapsed() < d))
    {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(every) = stats_every
            && stats_last.elapsed() >= every
        {
            stats_last = std::time::Instant::now();
            let live = launched.iter().filter(|(_, h)| h.is_live()).count();
            println!("{}", reporter.line(live));
        }
        if let Some((_, c)) = &control_thread
            && let Some(dir) = control.as_deref()
            && groups_last.is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
        {
            groups_last = Some(std::time::Instant::now());
            if let Ok(c) = c.lock() {
                let _ = std::fs::write(
                    Path::new(dir).join(eve_bot::control::GROUPS_FILE),
                    c.groups_tsv(),
                );
            }
        }
        // BOT-ADMIN: a `KILL` file in the control directory releases every
        // bot (about a second to see it, then each stops at its next turn).
        // It is removed once seen, so the next start is not killed too.
        if let Some(dir) = control.as_deref() {
            let kill = Path::new(dir).join("KILL");
            if kill.exists() {
                println!("eve-bots: {} found: releasing every bot", kill.display());
                for (_, h) in &launched {
                    h.stop();
                }
                if let Some((_, c)) = &control_thread
                    && let Ok(mut c) = c.lock()
                {
                    println!("eve-bots: {}", c.handle("stop-all"));
                }
                let _ = std::fs::remove_file(&kill);
            }
        }
        if let Some(b) = bubble_run.as_mut() {
            for line in b.step(driver::unix_now()) {
                println!("{line}");
            }
        }
        if !minds.is_empty() && last.elapsed() >= driver::SOC_SUMMARY_EVERY {
            last = std::time::Instant::now();
            println!("{}", soc_summary(&minds, &online, hall.as_deref()));
            for line in doc_rollup(hall.as_deref()) {
                println!("{line}");
            }
        }
    }
    for t in threads {
        let _ = t.join();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    control_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some((link, controller)) = control_thread {
        let _ = link.join();
        // Whatever the GM spawned stops with the process.
        if let Ok(mut c) = controller.lock() {
            let _ = c.handle("despawn all");
        }
    }
    if let Some(t) = battle {
        let _ = t.join();
    }
    if !minds.is_empty() {
        println!("{}", soc_summary(&minds, &online, hall.as_deref()));
        for line in doc_rollup(hall.as_deref()) {
            println!("{line}");
        }
    }
    if let Some(h) = &hall
        && let Err(e) = h.save()
    {
        println!("eve-bots: the society file did not save: {e}");
    }
    Ok(())
}

/// DOC-9: the `[doc]` rollup (doctrine per leader, switches, theatres,
/// commitment) printed after each `[soc]` line, for `tools/doctrine-report.py`.
fn doc_rollup(hall: Option<&eve_bot::corpmind::Hall>) -> Vec<String> {
    hall.map(|h| eve_bot::doctrine_report::rollup(&h.lock(), driver::unix_now()))
        .unwrap_or_default()
}

/// The `[soc]` summary over every bot's mind ([`eve_bot::mind::summary_line`]),
/// with the hall's corps, alliances and blue and red pairs (SOC-C4).
fn soc_summary(
    minds: &BTreeMap<String, driver::Society>,
    online: &std::sync::atomic::AtomicUsize,
    hall: Option<&eve_bot::corpmind::Hall>,
) -> String {
    let held: Vec<eve_bot::mind::Mind> = minds
        .values()
        .map(|s| {
            s.mind
                .lock()
                .map(|m| m.clone())
                .unwrap_or_else(|p| p.into_inner().clone())
        })
        .collect();
    let refs: Vec<&eve_bot::mind::Mind> = held.iter().collect();
    let line = format!(
        "{} {}",
        eve_bot::mind::summary_line(
            &refs,
            online.load(std::sync::atomic::Ordering::SeqCst),
            driver::unix_now(),
        ),
        // PROG-PVE2: the realised income by career, over time.
        eve_bot::earnings::income_fields(&refs),
    );
    match hall {
        Some(h) => format!("{line} {}", h.summary()),
        None => line,
    }
}

/// What the minds decide on, from this run's flags. The mission runner is
/// not blocked: it finds its agents at its station without `--agent`
/// (SOC-B5, `missioner::candidates`). The pirate is, without `--society-pvp`
/// (SOC-C5): nobody hunts unasked. Prices and corp need are read at each
/// docked decision (`signal`).
fn signals(pvp: bool) -> eve_bot::career::Signals {
    let mut s = eve_bot::career::Signals::default();
    if !pvp {
        s.blocked.insert("pirate".to_owned());
    }
    // CAP-L: nothing from nowhere -- capital careers open per bot, when it
    // owns the hull ([`eve_bot::capital::lift_blocks`]).
    eve_bot::career::block_capital(&mut s);
    s
}

/// `--society` and its flags (SOC-A2): `--loop` is refused beside it (the
/// mind picks the role), and `--state`, `--society-seed`, `--sessions`
/// and `--day-secs` are refused without it.
fn society_options(
    society: bool,
    looped: bool,
    state: bool,
    seed: bool,
    sessions: bool,
    day_secs: bool,
) -> Result<bool, String> {
    if society && looped {
        return Err("--loop with --society: the mind picks each bot's role".to_owned());
    }
    if !society && (state || seed || sessions || day_secs) {
        return Err("--state, --society-seed, --sessions and --day-secs need --society".to_owned());
    }
    Ok(society)
}

/// GM setup under `--closed` (E8): off unless `--bootstrap`, and
/// `--gm-setup` is refused beside it; without `--closed`, `--gm-setup`'s
/// value and `--bootstrap` refused.
fn closed_gm_setup(
    closed: bool,
    bootstrap: bool,
    gm_flag: bool,
    gm_setup: bool,
) -> Result<bool, String> {
    match (closed, bootstrap, gm_flag) {
        (false, true, _) => Err("--bootstrap needs --closed".to_owned()),
        (true, _, true) => {
            Err("--gm-setup with --closed: GM setup there is --bootstrap".to_owned())
        }
        (true, b, false) => Ok(b),
        (false, false, _) => Ok(gm_setup),
    }
}

/// A positive id flag's value.
fn id(flag: &str, v: &str) -> Result<i64, String> {
    match v.parse::<i64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag} {v}: want a positive id")),
    }
}

/// The homes to draw from (module header): none without `--home` or
/// `--bot-systems`; checked against the gate graph when one is given.
fn homes(
    home: Option<i64>,
    listed: &[i64],
    celestials: Option<&str>,
    content: Option<&str>,
) -> Result<Vec<i64>, String> {
    let Some(home) = home else {
        return Ok(listed.to_vec());
    };
    let path = match (celestials, content) {
        (Some(p), _) => Some(std::path::PathBuf::from(p)),
        (None, Some(dir)) => {
            let (bundle, _) = eve_server::content::Bundle::open(Path::new(dir))?;
            Some(
                bundle
                    .path(eve_server::content::CELESTIALS)
                    .ok_or_else(|| {
                        format!("--content {dir}: no {}", eve_server::content::CELESTIALS)
                    })?,
            )
        }
        (None, None) => None,
    };
    let graph = match path {
        Some(p) => Some(driver::gate_graph(&eve_server::load_celestials(&p)?)),
        None => None,
    };
    driver::bot_homes(graph.as_ref(), home, listed)
}

/// `--tokens FILE`, after the file checks: refused inside a git work tree
/// (any ancestor of its canonical path holds `.git`, the accounts loader's
/// rule) [A: falsified by a checkout whose `.git` lives elsewhere], and when
/// its mode lets the group or others in at all (`mode & 0o077`) [A: the
/// file holds tokens, not hashes].
fn load_tokens(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let shown = path.display();
    let abs = path
        .canonicalize()
        .map_err(|e| format!("--tokens {shown}: {e}"))?;
    if let Some(repo) = abs.ancestors().find(|a| a.join(".git").exists()) {
        return Err(format!(
            "--tokens {shown}: inside the git work tree {}; keep it outside every repo \
             (~/.config/eve-offline/bots.tsv)",
            repo.display()
        ));
    }
    let meta = std::fs::metadata(&abs).map_err(|e| format!("--tokens {shown}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "--tokens {shown}: mode {:o} lets others read it; chmod 600 it",
                mode & 0o777
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let text = std::fs::read_to_string(&abs).map_err(|e| format!("--tokens {shown}: {e}"))?;
    parse_tokens(&text).map_err(|e| format!("--tokens {shown}: {e}"))
}

/// The file's text: `NAME TOKEN` per line, the token the last
/// whitespace-separated field and the name everything before it, trimmed.
/// Refused: a line with one field, a repeated name, and a token equal to
/// any name in the file (ruling 2). Errors name the line, never a token.
/// `--chat-host IP:PORT`: an address literal; names are not resolved.
fn parse_chat_host(v: &str) -> Result<SocketAddr, String> {
    v.parse()
        .map_err(|e| format!("--chat-host {v}: IP:PORT ({e})"))
}

fn apply_chat_host(config: &mut BotConfig, host: Option<SocketAddr>) {
    if let Some(sa) = host {
        config.chat_host = Some(sa.ip());
        config.chat_port = sa.port();
    }
}

fn parse_tokens(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, token)) = line.rsplit_once(char::is_whitespace) else {
            return Err(format!("line {n}: want NAME TOKEN"));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("line {n}: want NAME TOKEN"));
        }
        if map.insert(name.to_owned(), token.to_owned()).is_some() {
            return Err(format!("line {n}: {name} twice"));
        }
    }
    if let Some(name) = map.keys().find(|k| map.values().any(|t| t == *k)) {
        return Err(format!(
            "{name}: a token in this file is a bot's name; a name is never a token \
             (eve-server --new-token gives a random one)"
        ));
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_host_flag_parses_and_sets_the_config() {
        let sa = parse_chat_host("127.0.0.1:5222").unwrap();
        assert!(parse_chat_host("localhost:5222").is_err());
        assert!(parse_chat_host("127.0.0.1").is_err());
        let mut c = BotConfig::new("t", "127.0.0.1:1".parse().unwrap(), Pace::Fast);
        apply_chat_host(&mut c, None);
        assert_eq!(c.chat_host, None);
        apply_chat_host(&mut c, Some(sa));
        assert_eq!(c.chat_host, Some(sa.ip()));
        assert_eq!(c.chat_port, 5222);
    }

    /// DOC-8: the help names `--difficulty` and every value it takes, and
    /// the flag's values are the library's.
    #[test]
    fn the_help_names_the_difficulty_flag() {
        let u = usage();
        assert!(u.contains("--difficulty LEVEL"), "{u}");
        for w in ["easy", "normal", "hard", "brutal"] {
            assert!(eve_bot::difficulty::Difficulty::parse_level(w).is_ok());
        }
    }

    /// E8: `--closed` turns GM setup off, `--bootstrap` back on; neither
    /// `--bootstrap` alone nor `--gm-setup` beside `--closed` is taken.
    #[test]
    fn closed_means_no_gm_setup_unless_bootstrap() {
        assert_eq!(closed_gm_setup(true, false, false, true), Ok(false));
        assert_eq!(closed_gm_setup(true, true, false, true), Ok(true));
        assert_eq!(closed_gm_setup(false, false, true, false), Ok(false));
        assert_eq!(closed_gm_setup(false, false, false, true), Ok(true));
        assert!(closed_gm_setup(false, true, false, true).is_err());
        assert!(closed_gm_setup(true, false, true, true).is_err());
    }

    /// SOC-A2: `--society` refuses `--loop`, and its own flags are refused
    /// without it; plain runs are as before.
    #[test]
    fn society_replaces_loop_and_owns_its_flags() {
        assert_eq!(
            society_options(true, false, true, true, true, true),
            Ok(true)
        );
        assert_eq!(
            society_options(false, true, false, false, false, false),
            Ok(false)
        );
        assert!(society_options(true, true, false, false, false, false).is_err());
        for i in 0..4 {
            let f = |k| i == k;
            assert!(
                society_options(false, false, f(0), f(1), f(2), f(3)).is_err(),
                "{i}"
            );
        }
        // CAP-L: the capital careers are blocked for every bot until it owns
        // the hull.
        // PROG-MINE: and the foreman, until it holds a command hull.
        let mut capital: Vec<&str> = eve_bot::career::CAPITAL_CAREERS.to_vec();
        capital.push(eve_bot::foreman::CAREER);
        assert_eq!(
            signals(true)
                .blocked
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            capital
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        let mut want: Vec<&str> = capital.clone();
        want.push("pirate");
        want.sort_unstable();
        assert_eq!(
            signals(false)
                .blocked
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            want,
            "no hunting unless asked"
        );
    }

    /// Every row of `roles::ROLES` is in the help, by name, as a `--loop`
    /// choice and with its kind.
    #[test]
    fn the_usage_lists_every_role() {
        let text = usage();
        for r in roles::ROLES {
            assert!(
                text.contains(&format!("|{}", r.name()))
                    || text.contains(&format!("[--loop {}", r.name())),
                "{}",
                r.name()
            );
            let kind = if r.civilian { "civilian" } else { "fights" };
            assert!(
                text.contains(&format!("{:<9} {kind}", r.name())),
                "{}",
                r.name()
            );
        }
    }

    /// Invented tokens; never real ones.
    const A: &str = "ip41-invented-token-a";
    const B: &str = "ip41-invented-token-b";

    /// `--new-token`'s tab form and a hand-typed space form both load, a
    /// name may hold spaces, comments and blank lines are skipped, and a
    /// name the file does not list has no token (the bot then sends
    /// `driver::DEV_TOKEN`).
    #[test]
    fn a_tokens_file_maps_names_to_tokens() {
        let text = format!("# bots\n\nMiner1\t{A}\n  Ola Nordmann {B}  \n");
        let map = parse_tokens(&text).unwrap();
        assert_eq!(map.get("Miner1").map(String::as_str), Some(A));
        assert_eq!(map.get("Ola Nordmann").map(String::as_str), Some(B));
        assert_eq!(map.get("Miner2"), None);
        assert_eq!(map.len(), 2);
    }

    /// Ruling 2: no token may be a name; and a bad line is named without
    /// echoing what it holds.
    #[test]
    fn a_tokens_file_refuses_names_as_tokens_and_bad_lines() {
        let err = parse_tokens("Miner1 Miner1\n").unwrap_err();
        assert!(err.contains("never a token"), "{err}");
        let err = parse_tokens(&format!("Miner1 {A}\nMiner2 Miner1\n")).unwrap_err();
        assert!(err.contains("never a token"), "{err}");
        let err = parse_tokens(&format!("Miner1 {A}\nMiner1 {B}\n")).unwrap_err();
        assert!(err.contains("line 2") && !err.contains(B), "{err}");
        let err = parse_tokens(&format!("{A}\n")).unwrap_err();
        assert!(err.contains("line 1") && !err.contains(A), "{err}");
    }

    /// The file checks, on an invented repo in a temp dir: a file under a
    /// directory holding `.git` is refused, and so is one the group may
    /// read; `chmod 600` outside the repo loads.
    #[cfg(unix)]
    #[test]
    fn a_tokens_file_inside_a_repo_or_readable_by_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ip41-tokens-{}", std::process::id()));
        if dir.ancestors().any(|a| a.join(".git").exists()) {
            return; // a temp dir inside a repo: nothing outside one to test
        }
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let text = format!("Miner1\t{A}\n");
        let mode600 = || std::fs::Permissions::from_mode(0o600);
        let inside = repo.join("bots.tsv");
        std::fs::write(&inside, &text).unwrap();
        std::fs::set_permissions(&inside, mode600()).unwrap();
        let err = load_tokens(&inside).unwrap_err();
        assert!(err.contains("git work tree") && !err.contains(A), "{err}");

        let file = dir.join("bots.tsv");
        std::fs::write(&file, &text).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let err = load_tokens(&file).unwrap_err();
        assert!(err.contains("chmod 600"), "{err}");
        std::fs::set_permissions(&file, mode600()).unwrap();
        let map = load_tokens(&file).unwrap();
        assert_eq!(map.get("Miner1").map(String::as_str), Some(A));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
