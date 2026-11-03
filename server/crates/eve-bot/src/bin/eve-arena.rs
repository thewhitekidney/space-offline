//! **The arena** (DOC-9, G8): the matrix's predicted counters, fought out in
//! the real combat engine. `tools/arena.sh` is the front door.
//!
//! ```text
//! eve-arena --content DIR [--pairs N | --a KEY --b KEY] [--seeds N] [--cap N]
//!           [--jobs N] [--range on|off] [--sep-m M] [--limit-s S] [--list]
//! ```
//!
//! The catalog is built from `DIR`'s type data exactly as `eve-bots` does
//! (`DoctrineWorld::from_content`); each skirmish runs on a private
//! in-process server whose tables are the sandbox fixture plus `DIR`'s type
//! data (so the hulls, modules and rounds are the real ones, in the
//! sandbox's one system). For each chosen doctrine `A` the arena fields the
//! counter `B` the matrix names (`B` has the highest score against `A`) at
//! equal spend, `--seeds` times, and prints `[arena]` lines
//! (`eve_bot::arena`); `tools/doctrine-report.py` turns them into gate G8.

use eve_bot::arena::{self, Fleet, Outcome, Rig, Side, Tally};
use eve_bot::doctrine_exchange::Features;
use eve_bot::doctrine_hall::DoctrineWorld;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const USAGE: &str = "usage: eve-arena --content DIR [--pairs N | --a KEY --b KEY] [--seeds N] \
[--cap N] [--jobs N] [--range on|off] [--sep-m M] [--limit-s S] [--list] [--trace]";

struct Args {
    content: String,
    pairs: usize,
    a: Option<String>,
    b: Option<String>,
    seeds: u64,
    cap: usize,
    jobs: usize,
    range: bool,
    sep_m: f64,
    limit_s: u64,
    list: bool,
    trace: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        content: String::new(),
        pairs: 3,
        a: None,
        b: None,
        seeds: 10,
        cap: arena::ARENA_CAP,
        jobs: 2,
        range: true,
        sep_m: arena::START_SEP_M,
        limit_s: arena::LIMIT_S,
        list: false,
        trace: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        let num = |s: String, name: &str| {
            s.parse::<f64>()
                .map_err(|_| format!("{name} needs a number, got {s:?}"))
        };
        match flag.as_str() {
            "--content" => a.content = val("--content")?,
            "--pairs" => a.pairs = num(val("--pairs")?, "--pairs")? as usize,
            "--a" => a.a = Some(val("--a")?),
            "--b" => a.b = Some(val("--b")?),
            "--seeds" => a.seeds = num(val("--seeds")?, "--seeds")? as u64,
            "--cap" => a.cap = num(val("--cap")?, "--cap")? as usize,
            "--jobs" => a.jobs = num(val("--jobs")?, "--jobs")? as usize,
            "--sep-m" => a.sep_m = num(val("--sep-m")?, "--sep-m")?,
            "--limit-s" => a.limit_s = num(val("--limit-s")?, "--limit-s")? as u64,
            "--range" => {
                a.range = match val("--range")?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("--range wants on or off, got {other:?}")),
                }
            }
            "--list" => a.list = true,
            "--trace" => a.trace = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    if a.content.is_empty() {
        return Err(format!("--content DIR is required\n{USAGE}"));
    }
    if a.a.is_some() != a.b.is_some() {
        return Err("--a and --b go together".to_owned());
    }
    Ok(a)
}

fn run() -> Result<(), String> {
    let args = parse()?;
    let dir = Path::new(&args.content);
    let feat = Features { range: args.range };
    arena::set_trace(args.trace);
    // The catalog, from the whole bundle.
    let full = eve_bot::hulls::open(dir)?;
    let world = DoctrineWorld::from_content(&full, feat).ok_or("no type data in --content")?;
    println!(
        "[arena] catalog doctrines={} range={}",
        world.catalog.len(),
        if args.range { "on" } else { "off" }
    );
    if args.list {
        for (i, d) in world.catalog.iter().enumerate() {
            println!(
                "[arena] doctrine {i} {} cost={:.0}",
                d.key,
                d.cost_per_ship()
            );
        }
        return Ok(());
    }
    // The server's tables: the sandbox fixture, with the bundle's type data.
    let table = eve_server::typedata::Table::load(&dir.join(eve_server::content::TYPEDATA))?;
    let mut content = eve_server::content::Content::fixture();
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    // The bundle's skills too (DAMAGE-GAP): the fixture holds a handful, so
    // `/giveskill` answered "no such skill" for the missile, gunnery and
    // support skills the doctrine lists and every arena pilot flew without
    // them (flight time, explosion radius and velocity, damage).
    arena::install_bundle_skills(&mut content, dir)?;
    let mut rig = Rig::new(Arc::new(content));
    rig.sep_m = args.sep_m;
    rig.features = Features { range: args.range };
    rig.limit = Duration::from_secs(args.limit_s);

    // The matchups: (a, b) with b the counter the matrix names for a.
    let mut matchups: Vec<(usize, usize, f64)> = Vec::new();
    if let (Some(ka), Some(kb)) = (&args.a, &args.b) {
        let find = |k: &String| world.find(k).ok_or_else(|| format!("no doctrine {k:?}"));
        let (i, j) = (find(ka)?, find(kb)?);
        matchups.push((i, j, world.matrix.er[j][i]));
    } else {
        let mut all: Vec<(usize, usize, f64)> = (0..world.catalog.len())
            .filter(|&i| world.catalog[i].line().is_some())
            .filter_map(|i| arena::counter_of(&world.matrix, i).map(|(j, er)| (i, j, er)))
            .filter(|&(_, j, er)| er > 0.0 && world.catalog[j].line().is_some())
            .collect();
        // The most decisive predictions first: they are the ones that must
        // come true.
        all.sort_by(|x, y| y.2.total_cmp(&x.2).then(x.0.cmp(&y.0)));
        let mut seen = std::collections::BTreeSet::new();
        for m in all {
            if seen.insert((m.0.min(m.1), m.0.max(m.1))) {
                matchups.push(m);
            }
            if matchups.len() >= args.pairs {
                break;
            }
        }
    }
    if matchups.is_empty() {
        return Err("the matrix names no counter with a positive score".to_owned());
    }

    // Every (matchup, seed) is one task.
    let tasks: Vec<(usize, u64)> = (0..matchups.len())
        .flat_map(|m| (1..=args.seeds).map(move |s| (m, s)))
        .collect();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Vec<Outcome>>> = Mutex::new(vec![Vec::new(); matchups.len()]);
    let fail = Mutex::new(None::<String>);
    std::thread::scope(|s| {
        for _ in 0..args.jobs.max(1) {
            s.spawn(|| {
                loop {
                    let t = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&(m, seed)) = tasks.get(t) else {
                        return;
                    };
                    let (i, j, _) = matchups[m];
                    let (a, b) = (&world.catalog[i], &world.catalog[j]);
                    let (na, nb) = arena::fielded(a, b, args.cap);
                    match arena::skirmish(
                        &rig,
                        Fleet { doctrine: a, n: na },
                        Fleet { doctrine: b, n: nb },
                        seed,
                    ) {
                        Ok(o) => {
                            println!("{}", arena::skirmish_line(&a.key, &b.key, seed, &o));
                            results.lock().unwrap_or_else(|p| p.into_inner())[m].push(o);
                        }
                        Err(e) => {
                            println!("[arena] error a={} b={} seed={seed}: {e}", a.key, b.key);
                            *fail.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                        }
                    }
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_else(|p| p.into_inner());
    let mut tallies = Vec::new();
    for (m, outcomes) in results.iter().enumerate() {
        let (i, j, er) = matchups[m];
        // `b` is the predicted winner.
        let t = Tally::of(outcomes, Side::B);
        println!(
            "{}",
            arena::pair_line(
                &world.catalog[i].key,
                &world.catalog[j].key,
                er,
                args.range,
                &t
            )
        );
        tallies.push(t);
    }
    println!("{}", arena::summary_line(&tallies, feat));
    match fail.into_inner().unwrap_or_else(|p| p.into_inner()) {
        Some(e) => Err(format!("some skirmishes failed (last: {e})")),
        None => Ok(()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eve-arena: {e}");
            ExitCode::FAILURE
        }
    }
}
