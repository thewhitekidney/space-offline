//! **PERF-STAGES' bench**: 500 pilots in one hot system, per-stage cost of
//! the world clock's stages that scale with the pilots there (`mining`,
//! `npc`, `missile`, `wormhole`, `dungeon_tracking`, `site_spawn`, `grid`).
//!
//! The owner's dev world (2026-10-28) had ~260 bot miners in one hot system,
//! a thousand offline characters, a handful of NPCs and some site pockets;
//! this world is that shape with invented ids and the fixture content.
//! `#[ignore]`d (a timing, not a gate). Run it in release:
//!
//! ```text
//! tools/dev.sh cargo test --release --locked -p eve-server stage_bench -- --ignored --nocapture
//! ```
//!
//! `STAGE_BENCH_PILOTS` (default 500), `STAGE_BENCH_TICKS` (default 160).

use crate::asteroid::{Field, Law, Rock};
use crate::content::Content;
use crate::service::Registry;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SYSTEM: i64 = 30_000_001;
const STATION: i64 = 60_012_526;
const FIRST_CHARACTER: i64 = 91_000_000;
const FIRST_SHIP: i64 = 191_000_000;
const FIRST_LASER: i64 = 192_000_000;
const FIRST_POD: i64 = 193_000_000;
const FIRST_CLIENT: i64 = 500_000_000;
const MERLIN: i64 = 603;
const MINER_I: i64 = 483;
const VELDSPAR: i64 = 1230;
/// Characters in the world that are not in space (the dev world had ~1000
/// accounts, most docked or logged off).
const OFFLINE: usize = 800;
const SITES: usize = 8;
const HI_SLOT_0: i64 = 27;
const LAW: Law = Law {
    growth: 0.5,
    unit_size: 90.0,
    max_radius: 16_255.0,
};

fn character(i: usize) -> i64 {
    FIRST_CHARACTER + i as i64
}
fn ship(i: usize) -> i64 {
    FIRST_SHIP + i as i64
}
fn rock_id(i: usize) -> i64 {
    crate::asteroid::MIN_ROCK_ID + i as i64
}

/// `n` miners in Merlins, a rock each, a tenth of them orbiting another
/// pilot (the escorts and followers of the bot groups), `SITES` site
/// pockets, nine NPCs, and `OFFLINE` more characters on the books.
pub(crate) fn world(n: usize) -> Registry {
    // `STAGE_BENCH_CONTENT=DIR` runs on a content bundle (the real item,
    // dungeon and NPC tables) instead of the fixture; nothing from it is
    // written anywhere but stdout.
    let real: Option<Arc<Content>> = std::env::var_os("STAGE_BENCH_CONTENT").map(|d| {
        let loaded = crate::content::load(
            Some(std::path::Path::new(&d)),
            &crate::authored::Dir::default(),
        )
        .expect("the bundle loads");
        Arc::new(loaded.content)
    });
    let all = n + OFFLINE;
    let characters = (0..all)
        .map(|i| {
            let mut c = crate::model::Character {
                character_id: character(i),
                name: format!("Bench{i}"),
                ship_id: ship(i),
                ship_type_id: MERLIN,
                ..crate::model::Character::default()
            };
            c.solar_system_id = Some(SYSTEM);
            c.station_id = Some(STATION);
            c
        })
        .collect();
    let rocks = (0..n).map(|i| {
        let quantity = 400_000;
        let radius = LAW.radius(quantity);
        let (gx, gz) = ((i % 25) as f64, (i / 25) as f64);
        Rock {
            id: rock_id(i),
            type_id: VELDSPAR,
            belt_id: 40_000_003,
            solar_system_id: SYSTEM,
            quantity,
            radius,
            position: [gx * 40_000.0, radius + 2_000.0, gz * 40_000.0],
            law: LAW,
        }
    });
    let content = real.clone().unwrap_or_else(|| {
        let mut c = Content::fixture();
        c.insert("dungeon_data", crate::dungeon_data::Table::fixture());
        // PERF-BURST: a price table of the real size (~12k types, invented
        // ids), so `config.GetAverageMarketPrices` costs what it costs live.
        // `STAGE_BENCH_PRICES=0` keeps the fixture's few rows.
        let rows: i64 = std::env::var("STAGE_BENCH_PRICES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(12_000);
        if rows > 0 {
            let mut table = crate::econ_prices::Table::fixture();
            let proto = table.prices.values().next().expect("a fixture row").clone();
            for i in 0..rows {
                let mut p = proto.clone();
                p.ref_price = 100.0 + i as f64;
                table.prices.insert(900_000 + i, p);
            }
            c.insert("econ_prices", table);
        }
        Arc::new(c)
    });
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters,
        rocks: Field::from_rocks(rocks),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content);
    if real.is_some() {
        reg.deltas_mut()
            .set_world(crate::delta::World::mint(crate::pyutil::filetime_now()));
    }
    let merlin = *crate::item::find_in(reg.content(), MERLIN).expect("Merlin");
    let laser = *crate::item::find_in(reg.content(), MINER_I).expect("Miner I");
    let mut items = reg.store().items().to_vec();
    for it in items
        .iter_mut()
        .filter(|it| (FIRST_SHIP..FIRST_SHIP + all as i64).contains(&it.item_id))
    {
        it.type_id = merlin.type_id;
        it.group_id = merlin.group_id;
    }
    for i in 0..all {
        items.push(crate::item::Item {
            item_id: FIRST_LASER + i as i64,
            type_id: MINER_I,
            owner_id: character(i),
            location_id: ship(i),
            flag_id: HI_SLOT_0,
            quantity: 1,
            group_id: laser.group_id,
            category_id: laser.category_id,
            singleton: true,
        });
        items.push(crate::item::Item {
            item_id: FIRST_POD + i as i64,
            type_id: crate::damage::CAPSULE_TYPE_ID,
            owner_id: character(i),
            location_id: ship(i),
            flag_id: crate::fitting::FLAG_CAPSULE,
            quantity: 1,
            group_id: crate::fitting::GROUP_CAPSULE,
            category_id: crate::model::CATEGORY_SHIP,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: FIRST_POD + all as i64 + 1,
        ..crate::store::Snapshot::default()
    });
    for i in 0..n {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: FIRST_CLIENT + i as i64,
            session_id: FIRST_CLIENT + i as i64,
            user_id: character(i),
            character_id: Some(character(i)),
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(character(i)));
    }
    let now_s = crate::space_state::now_seconds();
    for i in 0..n {
        let r = reg.rocks().get(rock_id(i)).copied().expect("rock");
        let p = reg.roster_mut().get_mut(character(i)).expect("in space");
        p.motion =
            crate::space_state::Motion::arrived([r.position[0], 0.0, r.position[2]], [0.0; 3]);
        p.since = now_s;
        reg.locks_mut().add(ship(i), rock_id(i));
        reg.modules_mut().set_online(FIRST_LASER + i as i64, true);
    }
    // A tenth follow another pilot: a tethered pair in the system, which is
    // what routes every `advance_character` to the whole system.
    for i in (0..n).step_by(10) {
        let target = ship((i + 1) % n);
        let _ = reg.dispatch(&crate::service::Request {
            service: "beyonce",
            method: "CmdOrbit",
            args: &[
                eve_marshal::PyValue::Int(target),
                eve_marshal::PyValue::Real(2_000.0),
            ],
            kwargs: &[],
            bind_params: None,
            actor: Some(character(i)),
        });
    }
    // Site pockets (instanced through the fixture dungeon).
    let table = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .expect("a dungeon table");
    let dungeons: Vec<i64> = table
        .dungeons
        .values()
        .filter(|d| !d.objects.is_empty() && d.entry_object.is_some())
        .map(|d| d.id)
        .collect();
    assert!(!dungeons.is_empty(), "a dungeon with objects");
    for k in 0..SITES {
        let dungeon = dungeons[k % dungeons.len()];
        let site = crate::population::Site {
            system: SYSTEM,
            row: 0,
            k: k as u32,
            slot: 0,
            generation: 0,
            site_id: crate::population::SITE_BASE + 4000 + k as i64,
            dungeon_id: dungeon,
            archetype: 97,
            entry: crate::dungeon_data::Entry::Anomaly,
            anchor: [-4.0e9 - k as f64 * 2.0e8, 0.0, 3.0e9],
            target_id: format!("ABC-{k:03}"),
            state: crate::population::State::Live,
            at: 0,
        };
        assert!(crate::site::instantiate(&mut reg, &site));
    }
    let rats: Vec<i64> = (0..9)
        .filter_map(|k| {
            crate::npc::spawn(
                &mut reg,
                &crate::npc::FIXTURE_RAT,
                SYSTEM,
                [k as f64 * 3_000.0, 0.0, -50_000.0],
            )
        })
        .collect();
    // Twenty slow missiles with a long fuse: in the air for the whole run,
    // so the `missile` stage flies them every tick.
    let charge = crate::missile::Charge {
        type_id: 990_460_002,
        damage: crate::damage::Damage {
            em: 3.0,
            explosive: 5.0,
            kinetic: 7.0,
            thermal: 11.0,
        },
        speed: 20.0,
        fuse_s: 3_000.0,
        aoe: crate::missile::Aoe {
            cloud: 0.0,
            velocity: 0.0,
            drf: 0.0,
        },
        radius: 10.0,
        mass: 500.0,
        agility: 0.001,
    };
    for k in 0..20 {
        let rat = rats[k % rats.len()];
        let launched = crate::missile::launch(
            &mut reg,
            crate::missile::Launch {
                shooter: rat,
                launcher_ship: rat,
                launcher_item_id: rat,
                launch_modules: vec![crate::destiny::NPC_LAUNCH_MODULE],
                charge,
                volley: charge.damage,
                target: ship((k * 7) % n),
                fx: None,
            },
            crate::pyutil::filetime_now(),
        );
        assert!(launched.is_ok(), "the missile flies");
    }
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
    reg
}

/// Start every laser, its cycle phase spread over `period` so completions
/// arrive evenly. Returns the cycle length in blue ticks.
fn start_lasers(reg: &mut Registry, n: usize, ft0: i64) -> i64 {
    let period = crate::modules::find_in(reg.content(), MINER_I)
        .expect("Miner I")
        .duration_ms()
        * crate::modules::MSEC;
    for i in 0..n {
        let at = ft0 - period * i as i64 / n as i64;
        crate::modules::activate(
            reg,
            Some(character(i)),
            FIRST_LASER + i as i64,
            "miningLaser",
            Some(rock_id(i)),
            1_000_000,
            at,
        )
        .expect("the laser starts");
    }
    period
}

type Stage = (&'static str, fn(&mut Registry, Instant, i64));

/// The stages in the order the clock runs them, those this lane owns and
/// the ones they depend on.
fn stages() -> Vec<Stage> {
    vec![
        ("liveness", crate::liveness::refresh),
        ("modules", |w, _, ft| {
            crate::modules::tick(w, ft);
        }),
        ("combat", |w, _, ft| crate::combat::tick(w, ft)),
        ("mining", |w, _, ft| crate::mining::tick(w, ft)),
        ("npc", |w, _, ft| crate::npc::tick(w, ft)),
        ("missile", crate::missile::tick),
        ("wormhole", crate::wormhole::tick),
        ("dungeon_tracking", crate::dungeon_tracking::tick),
        ("site_spawn", crate::site_spawn::tick),
        ("grid", crate::grid::tick),
    ]
}

/// Mean and max per stage, ms, over `ticks` clock turns of `step` blue
/// ticks each; the caller drains the pushes like the ticker does.
pub(crate) fn measure(
    reg: &mut Registry,
    ft0: i64,
    step: i64,
    ticks: usize,
) -> BTreeMap<&'static str, (f64, f64)> {
    let table = stages();
    let mut sum: BTreeMap<&'static str, (Duration, Duration)> = BTreeMap::new();
    let mut ft = ft0;
    for n in 0..ticks + 10 {
        ft += step;
        let now = Instant::now();
        for (name, run) in &table {
            let t = Instant::now();
            run(reg, now, ft);
            let took = t.elapsed();
            if n >= 10 {
                let e = sum.entry(name).or_default();
                e.0 += took;
                e.1 = e.1.max(took);
            }
        }
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
    }
    sum.into_iter()
        .map(|(k, (s, m))| {
            (
                k,
                (
                    s.as_secs_f64() * 1000.0 / ticks as f64,
                    m.as_secs_f64() * 1000.0,
                ),
            )
        })
        .collect()
}

#[test]
#[ignore = "a timing: run it in release with --ignored --nocapture"]
fn stage_bench() {
    let n: usize = std::env::var("STAGE_BENCH_PILOTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let ticks: usize = std::env::var("STAGE_BENCH_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(160);
    let built = Instant::now();
    let mut reg = world(n);
    let ft0 = crate::pyutil::filetime_now();
    let period = start_lasers(&mut reg, n, ft0);
    println!(
        "stage_bench: {n} pilots, {OFFLINE} offline, {SITES} sites; setup {:.0} ms; Miner I cycle {:.1} s",
        built.elapsed().as_secs_f64() * 1000.0,
        period as f64 / 1.0e7
    );
    // A tick of 250 ms; the cycle completions per tick are n * 0.25 / period.
    let step = 2_500_000;
    let r = measure(&mut reg, ft0, step, ticks);
    println!("| stage | mean ms/tick | max ms |");
    println!("|---|---|---|");
    for (name, (mean, max)) in &r {
        println!("| {name} | {mean:.3} | {max:.3} |");
    }
    for (name, us) in probes(&mut reg, n) {
        println!("probe {name}: {us:.1} us");
    }
    println!(
        "(about {:.1} mining cycles end per tick)",
        n as f64 * 0.25 / (period as f64 / 1.0e7)
    );
}

/// Per-call cost of the primitives the stages are built on, µs.
pub(crate) fn probes(reg: &mut Registry, n: usize) -> Vec<(&'static str, f64)> {
    fn per<R>(reps: usize, mut f: impl FnMut() -> R) -> f64 {
        let t = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(f());
        }
        t.elapsed().as_secs_f64() * 1.0e6 / reps as f64
    }
    let mut out = Vec::new();
    out.push((
        "advance_system (all pilots)",
        per(50, || reg.advance_system(SYSTEM)),
    ));
    out.push((
        "roster.has_pair",
        per(200, || reg.roster().has_pair(SYSTEM)),
    ));
    out.push((
        "roster.in_system",
        per(200, || reg.roster().in_system(SYSTEM).len()),
    ));
    out.push((
        "character() x n",
        per(200, || {
            (0..n)
                .filter(|i| reg.character(character(*i)).is_some())
                .count()
        }),
    ));
    out.push((
        "advance_character (one, paired system)",
        per(200, || reg.advance_character(character(3))),
    ));
    out.push((
        "dogma::hull x n",
        per(50, || {
            (0..n)
                .filter(|i| crate::dogma::hull(reg, ship(*i)).is_some())
                .count()
        }),
    ));
    out
}

/// **PERF-HOLDS' request bench**: the world-lock cost of the two requests
/// the profile named, on the 500-pilot world with a crowded station hangar
/// (every character holds `HANGAR_STACKS` stacks at the one station, one of
/// them 20 times that). Microseconds per call.
///
/// `GetInventoryFromId` on a bound `invbroker` only mints a handle; the
/// listing of the hangar it names is the call that reads the items, so both
/// are timed. `GetAverageMarketPrices` is timed warm (the kept answer) and
/// after a write to an unrelated delta kind (must stay warm) and to `mktidx`
/// (must recompute).
#[test]
#[ignore = "a timing: run it in release with --ignored --nocapture"]
fn request_bench() {
    const HANGAR_STACKS: usize = 40;
    let n: usize = 500;
    let mut reg = world(n);
    let mut items = reg.store().items().to_vec();
    let mut next = reg.store().next_id() + 1_000_000;
    let all = n + OFFLINE;
    for c in 0..all {
        let stacks = if c == 0 {
            HANGAR_STACKS * 20
        } else {
            HANGAR_STACKS
        };
        for k in 0..stacks {
            items.push(crate::item::Item {
                item_id: next,
                type_id: MINER_I + (k as i64 % 7),
                owner_id: character(c),
                location_id: STATION,
                flag_id: crate::model::FLAG_HANGAR,
                quantity: 1 + k as i64,
                group_id: 54,
                category_id: 7,
                singleton: false,
            });
            next += 1;
        }
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: next + 1,
        ..crate::store::Snapshot::default()
    });
    fn per<R>(reps: usize, mut f: impl FnMut() -> R) -> f64 {
        let t = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(f());
        }
        t.elapsed().as_secs_f64() * 1.0e6 / reps as f64
    }
    let actor = Some(character(0));
    let bind = eve_marshal::PyValue::Tuple(vec![
        eve_marshal::PyValue::Int(10_004),
        eve_marshal::PyValue::None,
    ]);
    let mint = per(2000, || {
        reg.dispatch(&crate::service::Request {
            service: "invbroker",
            method: "GetInventoryFromId",
            args: &[
                eve_marshal::PyValue::Int(STATION),
                eve_marshal::PyValue::Int(0),
            ],
            kwargs: &[],
            bind_params: None,
            actor,
        })
    });
    let list = per(100, || {
        reg.dispatch(&crate::service::Request {
            service: "invbroker::inventory",
            method: "List",
            args: &[],
            kwargs: &[],
            bind_params: Some(&bind),
            actor,
        })
    });
    let list_small = per(100, || {
        reg.dispatch(&crate::service::Request {
            service: "invbroker::inventory",
            method: "List",
            args: &[],
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(character(9)),
        })
    });
    println!("invbroker.GetInventoryFromId (mint): {mint:.1} us");
    println!(
        "invbroker::inventory.List, {} stacks: {list:.1} us; {HANGAR_STACKS} stacks: {list_small:.1} us",
        HANGAR_STACKS * 20
    );
    let kinds: Vec<&str> = crate::delta_kinds::KINDS
        .iter()
        .map(|k| k.0)
        .filter(|k| *k != crate::econ_prices::KIND)
        .collect();
    let other = kinds[0];
    let cold = per(1, || crate::econ_prices::average_market_prices(&reg));
    let warm = per(200, || crate::econ_prices::average_market_prices(&reg));
    let mut tick = 0u32;
    let unrelated = per(200, || {
        tick += 1;
        reg.deltas_mut()
            .put_verbatim(other, vec![tick.to_string()], vec![]);
        crate::econ_prices::average_market_prices(&reg)
    });
    let (id, _) = {
        let t = reg.content().get::<crate::econ_prices::Table>();
        let (id, p) = t.prices.iter().next().expect("a priced type");
        (*id, p.ref_price)
    };
    let mut day = 1u32;
    let related = per(200, || {
        day += 1;
        crate::econ_prices::fold_day(&mut reg, id, 1.0e3, day);
        crate::econ_prices::average_market_prices(&reg)
    });
    println!(
        "config.GetAverageMarketPrices: first {cold:.1} us, warm {warm:.1} us, after an unrelated write {unrelated:.1} us, after a mktidx write {related:.1} us"
    );

    // PERF-BURST (b): `/bots spawn` of 300 over a book of ~1000 accounts, the
    // world lock held for the whole call. `old shape` is the per-bot loop at
    // BOT_ITERATIONS (what provision did); `provision` is the batch at one.
    let seed: Vec<(String, String, Vec<u8>)> = (0..1000u32)
        .map(|i| {
            (
                format!("Seed{i}"),
                format!("pw-{i}-invented"),
                vec![i as u8; 16],
            )
        })
        .collect();
    let seed_refs: Vec<(&str, &str, Vec<u8>)> = seed
        .iter()
        .map(|(n, p, s)| (n.as_str(), p.as_str(), s.clone()))
        .collect();
    crate::account_book::ensure_bot_accounts(reg.deltas_mut(), &seed_refs, 1).expect("seed");
    let t0 = Instant::now();
    for (i, (n, p, s)) in seed.iter().take(300).enumerate() {
        let name = format!("Old{i}x{n}");
        crate::account_book::ensure_bot_account_salted(
            reg.deltas_mut(),
            &name[..name.len().min(24)],
            p,
            s.clone(),
        )
        .expect("old shape");
    }
    let old_shape = t0.elapsed();
    let args = [
        "spawn",
        "benchg",
        "benchside",
        "mine",
        "300",
        "all5",
        "none",
    ];
    let bytes: Vec<u8> = (0..300 * 32u32).map(|i| (i % 251) as u8).collect();
    let t0 = Instant::now();
    crate::bot_control::provision_with(&mut reg, &args, &bytes).expect("provision");
    let took = t0.elapsed();
    println!(
        "provision 300 (1000 accounts in the book): {:.1} ms; accounts alone, old per-bot shape at 1000 iterations: {:.1} ms",
        took.as_secs_f64() * 1e3,
        old_shape.as_secs_f64() * 1e3
    );
}

/// **PA-7's bench**: the grid stage (reconcile) and the drain (`take_due`
/// with the filter), steady and with a warp wave (every 10th pilot moved off
/// grid and back on alternate reconciles), at each pilot count and thread
/// count. `GRID_BENCH_PILOTS` (default `262,500`), `GRID_BENCH_THREADS`
/// (default `1,12`), `STAGE_BENCH_TICKS` (default 160).
///
/// ```text
/// tools/dev.sh cargo test --release --locked -p eve-server grid_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a timing: run it in release with --ignored --nocapture"]
fn grid_bench() {
    fn list(var: &str, default: &str) -> Vec<usize> {
        std::env::var(var)
            .unwrap_or_else(|_| default.to_string())
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    }
    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }
    fn mean(v: &[f64]) -> f64 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    }
    fn max(v: &[f64]) -> f64 {
        v.iter().cloned().fold(0.0, f64::max)
    }
    let ticks: usize = std::env::var("STAGE_BENCH_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(160);
    println!(
        "| pilots | threads | wave | grid mean | grid max | grid mean on scheduling ticks (n) | take_due mean | take_due max | take_pending mean |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for n in list("GRID_BENCH_PILOTS", "262,500") {
        for threads in list("GRID_BENCH_THREADS", "1,12") {
            for wave in [false, true] {
                crate::par::set_threads(threads);
                let mut reg = world(n);
                let ft0 = crate::pyutil::filetime_now();
                start_lasers(&mut reg, n, ft0);
                let home: Vec<[f64; 3]> = (0..n)
                    .map(|i| reg.roster().get(character(i)).unwrap().motion.position)
                    .collect();
                let table: Vec<Stage> = stages()
                    .into_iter()
                    .filter(|(name, _)| *name != "grid")
                    .collect();
                let (mut grid, mut sched, mut due, mut pending) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                let mut away = false;
                let mut ft = ft0;
                for t in 0..ticks + 10 {
                    ft += 2_500_000;
                    if wave && t % 8 == 1 {
                        away = !away;
                        let now_s = crate::space_state::now_seconds();
                        for i in (0..n).step_by(10) {
                            let mut at = home[i];
                            if away {
                                at[0] += 5.0e8;
                            }
                            if let Some(p) = reg.roster_mut().get_mut(character(i)) {
                                p.motion = crate::space_state::Motion::arrived(at, [0.0; 3]);
                                p.since = now_s;
                            }
                        }
                    }
                    let now = Instant::now();
                    for (_, run) in &table {
                        run(&mut reg, now, ft);
                    }
                    let before = reg.scheduled_count();
                    let s = Instant::now();
                    crate::grid::tick(&mut reg, now, ft);
                    let g = ms(s.elapsed());
                    let scheduled = reg.scheduled_count() > before;
                    let s = Instant::now();
                    let _ = reg.take_pending();
                    let p = ms(s.elapsed());
                    let s = Instant::now();
                    let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
                    let d = ms(s.elapsed());
                    if t >= 10 {
                        grid.push(g);
                        if scheduled {
                            sched.push(g);
                        }
                        due.push(d);
                        pending.push(p);
                    }
                }
                println!(
                    "| {n} | {threads} | {} | {:.2} | {:.2} | {:.2} ({}) | {:.2} | {:.2} | {:.2} |",
                    if wave { "wave" } else { "steady" },
                    mean(&grid),
                    max(&grid),
                    mean(&sched),
                    sched.len(),
                    mean(&due),
                    max(&due),
                    mean(&pending),
                );
            }
        }
    }
    crate::par::set_threads(0);
}
