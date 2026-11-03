//! PA-8: a brawl, per stage. `fight_bench`'s pilots, plus NPCs, missiles in
//! flight and drones, timed by the clock's own per-stage timings.
//!
//! ```text
//! tools/dev.sh env FIGHT_BENCH_CONTENT=/repo/out/content BRAWL_THREADS=12 \
//!   cargo test --release --locked -p eve-server --lib brawl_bench -- --ignored --nocapture
//! ```
//!
//! Knobs: `BRAWL_PILOTS` (500), `BRAWL_NPCS` (150), `BRAWL_MISSILES` (300),
//! `BRAWL_DRONE_PILOTS` (100), `BRAWL_ORBIT_EVERY` (10), `BRAWL_TICKS` (80),
//! `BRAWL_THREADS` (`--tick-threads`: default half the hardware, 1 = serial).
use crate::service::Registry;
use std::time::{Duration, Instant};

fn env(k: &str, d: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(d)
}

fn character(i: usize) -> i64 {
    91_000_000 + i as i64
}
fn ship(i: usize) -> i64 {
    191_000_000 + i as i64
}

fn build(n: usize, real: Option<&super::fight_bench::Real>) -> Registry {
    let npcs = env("BRAWL_NPCS", 150);
    let missiles = env("BRAWL_MISSILES", 300);
    let drone_pilots = env("BRAWL_DRONE_PILOTS", 100);
    let orbiters = env("BRAWL_ORBIT_EVERY", 10);
    let mut reg = super::fight_bench::fight(n, real);
    let system = reg
        .roster()
        .get(character(0))
        .expect("in space")
        .solar_system_id;
    let origin = reg
        .roster()
        .get(character(0))
        .expect("in space")
        .motion
        .position;
    // A tenth orbit another pilot: movement, a tethered pair.
    if orbiters > 0 {
        for i in (0..n).step_by(orbiters) {
            let _ = reg.dispatch(&crate::service::Request {
                service: "beyonce",
                method: "CmdOrbit",
                args: &[
                    eve_marshal::PyValue::Int(ship((i + 7) % n)),
                    eve_marshal::PyValue::Real(2_500.0),
                ],
                kwargs: &[],
                bind_params: None,
                actor: Some(character(i)),
            });
        }
    }
    // Tough rats, so the fight lasts the run.
    let mut tough = crate::npc::FIXTURE_RAT;
    tough.hull.hp = 1.0e9;
    tough.hull.armor_hp = 1.0e9;
    tough.hull.shield_capacity = 1.0e9;
    tough.hull.max_target_range = 150_000.0;
    let tough: &'static crate::npc::NpcType = Box::leak(Box::new(tough));
    let rats: Vec<i64> = (0..npcs)
        .filter_map(|k| {
            let a = k as f64 * 0.7;
            crate::npc::spawn(
                &mut reg,
                tough,
                system,
                [
                    origin[0] + 8_000.0 * a.cos(),
                    origin[1],
                    origin[2] + 8_000.0 * a.sin(),
                ],
            )
        })
        .collect();
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
    if !rats.is_empty() {
        for k in 0..missiles {
            let rat = rats[k % rats.len()];
            let _ = crate::missile::launch(
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
        }
    }
    // Five drones per drone pilot, engaging another pilot.
    let kind = crate::drones::find_in(reg.content(), 2454).unwrap_or(crate::drones::HOBGOBLIN_I);
    for i in 0..drone_pilots.min(n) {
        let at = reg
            .roster()
            .get(character(i))
            .expect("in space")
            .motion
            .position;
        for k in 0..5 {
            let id = 194_000_000 + (i * 5 + k) as i64;
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    id,
                    system,
                    crate::space_state::Motion::arrived(
                        [at[0] + 100.0 * k as f64, at[1], at[2] + 100.0],
                        [0.0; 3],
                    ),
                ));
            let mut d = crate::drones::Drone::launched(id, kind, character(i), ship(i), system);
            d.order = crate::drones::Order::Engage(ship((i + n / 2) % n));
            reg.modules_mut().drones_mut().insert(d);
        }
    }
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
    reg
}

#[test]
#[ignore = "a timing"]
fn brawl_bench() {
    let n = env("BRAWL_PILOTS", 500);
    let ticks = env("BRAWL_TICKS", 80);
    let real = std::env::var_os("FIGHT_BENCH_CONTENT")
        .map(|d| super::fight_bench::Real::load(std::path::Path::new(&d)));
    let built = Instant::now();
    let mut reg = build(n, real.as_ref());
    if let Ok(t) = std::env::var("BRAWL_THREADS") {
        crate::par::set_threads(t.parse().unwrap_or(1));
    }
    println!(
        "brawl: {n} pilots, {} tick threads, npcs {}, flights {}, drones {}; setup {:.0} ms",
        crate::par::threads(),
        reg.npcs().len(),
        reg.ext::<crate::missile::Flights>().len(),
        reg.modules().drones().len(),
        built.elapsed().as_secs_f64() * 1000.0
    );
    let mut sum: std::collections::BTreeMap<&'static str, (Duration, Duration)> =
        Default::default();
    let mut ft = crate::pyutil::filetime_now();
    let warm = 10;
    let mut totals = Vec::new();
    for t in 0..ticks + warm {
        ft += 2_500_000;
        let now = Instant::now();
        let s0 = Instant::now();
        let _ = crate::targeting::sweep_out_of_range(&mut reg);
        let mut ran = vec![("sweep", s0.elapsed())];
        ran.extend(crate::tick::run_all(&mut reg, now, ft));
        let d0 = Instant::now();
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
        ran.push(("drain", d0.elapsed()));
        if t >= warm {
            totals.push(ran.iter().map(|r| r.1).sum::<Duration>());
            for (name, d) in ran {
                let e = sum.entry(name).or_default();
                e.0 += d;
                e.1 = e.1.max(d);
            }
        }
    }
    let mean: Duration = totals.iter().sum::<Duration>() / ticks as u32;
    println!(
        "tick total mean {:.3} ms, max {:.3} ms",
        mean.as_secs_f64() * 1e3,
        totals.iter().max().unwrap().as_secs_f64() * 1e3
    );
    let mut rows: Vec<_> = sum.into_iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
    println!("| stage | mean ms | max ms |");
    for (name, (s, m)) in rows.iter().take(16) {
        println!(
            "| {name} | {:.3} | {:.3} |",
            s.as_secs_f64() * 1e3 / ticks as f64,
            m.as_secs_f64() * 1e3
        );
    }
    let alive = (0..n)
        .filter(|&i| reg.roster().by_ship(ship(i)).is_some())
        .count();
    println!(
        "alive {alive}/{n}, npcs {}, flights {}, drones {}",
        reg.npcs().len(),
        reg.ext::<crate::missile::Flights>().len(),
        reg.modules().drones().len()
    );
}
