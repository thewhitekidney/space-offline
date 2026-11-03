//! **G-FIGHT's measurement** (lane perf-fight): N ships on one grid, every
//! one locking and firing, driven through the world clock's own two halves
//! ([`super::clock_turn`] under the lock, [`super::encode_frames`] after
//! it), with the hold and the encode timed per tick.
//!
//! `#[ignore]`d: it runs in real time (the clock's queues are keyed on
//! `Instant::now()`), so it is a bench, not a gate. Run it in release:
//!
//! ```text
//! tools/dev.sh cargo test --release --locked -p eve-server fight_bench -- --ignored --nocapture
//! ```
//!
//! Every id here is invented; the hull and gun types are the ones the
//! combat tests already fly (`combat::tests::locked_on`).
//!
//! **`FIGHT_BENCH_CONTENT=DIR`** runs it on a content bundle instead of the
//! fixture: the real tables, the real celestial catalogue and belts, the
//! fight at the first station the catalogue lists and the background at
//! the second. Nothing from the bundle is written anywhere but stdout.

use crate::service::Registry;
use std::time::{Duration, Instant};

const SYSTEM_FIRST_CHARACTER: i64 = 91_000_000;
const FIRST_SHIP: i64 = 191_000_000;
const FIRST_GUN: i64 = 192_000_000;
const FIRST_POD: i64 = 193_000_000;
const FIRST_CLIENT: i64 = 500_000_000;
/// The Rifter and the civilian blaster `combat::tests` fly.
const RIFTER: i64 = 587;
const BLASTER: i64 = 3640;
/// Pilots who undock onto the fight one at a time, to time a bind.
const LATE: usize = 5;
/// Idle pilots in space in another system: the rest of the universe's
/// roster, which every by-ship lookup walks.
const BACKGROUND: usize = 150;
/// Spread over this many systems on a bundle (the fixture has one more).
const BACKGROUND_SYSTEMS: usize = 10;
const OTHER_SYSTEM: i64 = 30_000_002;
const OTHER_STATION: i64 = 60_000_002;
/// How many targets each ship holds: its gun's target and two more, the
/// way a bot in a fight keeps a short list.
const LOCKS_EACH: usize = 3;

fn character(i: usize) -> i64 {
    SYSTEM_FIRST_CHARACTER + i as i64
}
fn ship(i: usize) -> i64 {
    FIRST_SHIP + i as i64
}

/// A content bundle to fight in (`FIGHT_BENCH_CONTENT`).
pub(crate) struct Real {
    content: std::sync::Arc<crate::content::Content>,
    celestials: crate::celestial::Catalogue,
    rocks: crate::asteroid::Field,
    /// `(system, station)` of the fight, and of each background system.
    here: (i64, i64),
    there: Vec<(i64, i64)>,
}

impl Real {
    pub(crate) fn load(dir: &std::path::Path) -> Real {
        let loaded = crate::content::load(Some(dir), &crate::authored::Dir::default())
            .expect("the bundle loads");
        let bundle = loaded.bundle.as_ref().expect("a bundle");
        let celestials = crate::load_celestials(
            &bundle
                .path(crate::content::CELESTIALS)
                .expect("the bundle has celestials"),
        )
        .expect("celestials load");
        let rocks = bundle
            .path(crate::content::ROCKS)
            .map(|p| {
                crate::asteroid::index(&p, &celestials, &crate::asteroid::Quantities::default())
                    .expect("rocks index")
            })
            .unwrap_or_default();
        let (here, there) = {
            let mut stations = celestials
                .served()
                .filter(|c| c.kind == crate::celestial::Kind::Station)
                .filter_map(|c| Some((celestials.system_of(c.id)?, c.id)));
            // Null-sec: the attack must not meet CW-1 in a high-sec station's system.
            let uni = loaded.content.get::<crate::celestial::Universe>();
            let mut skipped = Vec::new();
            let here = loop {
                let s = stations.next().expect("a null-sec station");
                if uni.security_class(s.0) == Some(crate::celestial::SecurityClass::Zero) {
                    break s;
                }
                skipped.push(s);
            };
            let stations = skipped.into_iter().chain(stations);
            let mut there: Vec<(i64, i64)> = Vec::new();
            for (sys, station) in stations {
                if sys != here.0 && there.iter().all(|t| t.0 != sys) {
                    there.push((sys, station));
                }
                if there.len() == BACKGROUND_SYSTEMS {
                    break;
                }
            }
            (here, there)
        };
        Real {
            content: std::sync::Arc::new(loaded.content),
            celestials,
            rocks,
            here,
            there,
        }
    }
}

/// N pilots in Rifters, each with a civilian blaster online, all undocked
/// at the same station (one grid), each with a session, each locking the
/// next [`LOCKS_EACH`] ships and firing at the first of them.
pub(crate) fn fight(n: usize, real: Option<&Real>) -> Registry {
    let (here, there) = real.map_or(
        (
            (30_000_001, 60_012_526),
            vec![(OTHER_SYSTEM, OTHER_STATION)],
        ),
        |r| (r.here, r.there.clone()),
    );
    let all = n + LATE + BACKGROUND;
    let characters = (0..all)
        .map(|i| {
            let mut c = crate::model::Character {
                character_id: character(i),
                name: format!("Bench{i}"),
                ship_id: ship(i),
                ship_type_id: RIFTER,
                ..crate::model::Character::default()
            };
            let (system, station) = if i >= n + LATE {
                there[i % there.len()]
            } else {
                here
            };
            c.solar_system_id = Some(system);
            c.station_id = Some(station);
            c
        })
        .collect();
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters,
        celestials: real.map(|r| r.celestials.clone()).unwrap_or_default(),
        rocks: real.map(|r| r.rocks.clone()).unwrap_or_default(),
        ..crate::model::SandboxConfig::default()
    });
    if let Some(r) = real {
        reg = reg.with_content(std::sync::Arc::clone(&r.content));
        reg.deltas_mut()
            .set_world(crate::delta::World::mint(crate::pyutil::filetime_now()));
    }
    let rifter = *crate::item::find_in(reg.content(), RIFTER).expect("Rifter is spawnable");
    let mut items = reg.store().items().to_vec();
    for it in items
        .iter_mut()
        .filter(|it| (FIRST_SHIP..FIRST_SHIP + all as i64).contains(&it.item_id))
    {
        it.type_id = rifter.type_id;
        it.group_id = rifter.group_id;
    }
    for i in 0..all {
        items.push(crate::item::Item {
            item_id: FIRST_GUN + i as i64,
            type_id: BLASTER,
            owner_id: character(i),
            location_id: ship(i),
            flag_id: 27,
            quantity: 1,
            group_id: 53,
            category_id: 7,
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
    for i in 0..all {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: FIRST_CLIENT + i as i64,
            session_id: FIRST_CLIENT + i as i64,
            user_id: character(i),
            character_id: Some(character(i)),
        });
    }
    for i in (0..n).chain(n + LATE..all) {
        crate::destiny::on_ballpark_bind(&mut reg, Some(character(i)));
    }
    let now = crate::pyutil::filetime_now();
    for i in 0..n {
        for k in 1..=LOCKS_EACH.min(n - 1) {
            reg.locks_mut().add(ship(i), ship((i + k) % n));
        }
        reg.modules_mut().set_online(FIRST_GUN + i as i64, true);
        crate::modules::activate(
            &mut reg,
            Some(character(i)),
            FIRST_GUN + i as i64,
            "targetAttack",
            Some(ship((i + 1) % n)),
            1000,
            now,
        )
        .expect("the gun activates");
    }
    let _ = reg.take_pending();
    reg
}

/// The payload turn's drain as `dispatch::pending_pushes` does it: every
/// push pickled once, then addressed to every audience member's every
/// session. Returns `(frames, bytes)`.
pub(crate) fn drain_like_dispatch(reg: &mut Registry) -> (usize, usize) {
    let mut frames = 0;
    let mut bytes = 0;
    for (_, n) in reg.take_pending() {
        let (event, args) = n.to_push();
        let audience = n.audience().unwrap_or_default();
        let body = eve_proto::MachoPacket::notification_body(args);
        for character_id in audience {
            for e in reg.sessions().of_character(character_id) {
                let payload = eve_proto::MachoPacket::notification_pickled(
                    eve_proto::MachoAddress::node(1),
                    event,
                    "*clientID",
                    vec![eve_marshal::PyValue::Int(e.client_id)],
                    body.clone(),
                )
                .to_payload()
                .expect("encodes");
                frames += 1;
                bytes += payload.len();
            }
        }
    }
    (frames, bytes)
}

/// One payload turn's hold, in microseconds: the arm and the drain.
fn turn(reg: &mut Registry, arm: impl FnOnce(&mut Registry)) -> (u128, usize) {
    let start = Instant::now();
    arm(reg);
    let (frames, _) = drain_like_dispatch(reg);
    (start.elapsed().as_micros(), frames)
}

/// The payload turns a fight is made of, each timed on the `n`-ship grid:
/// an orbit, a lock and an unlock, a gun stopped and started, and a
/// latecomer's bind. `(name, max µs, frames)`.
pub(crate) fn turns(reg: &mut Registry, n: usize) -> Vec<(&'static str, u128, usize)> {
    use eve_marshal::PyValue;
    let mut out: Vec<(&'static str, u128, usize)> = Vec::new();
    let mut note = |name, (us, frames): (u128, usize)| match out.iter_mut().find(|r| r.0 == name) {
        Some(r) => {
            r.1 = r.1.max(us);
            r.2 = r.2.max(frames);
        }
        None => out.push((name, us, frames)),
    };
    for i in 0..n.min(10) {
        let target = ship((i + n / 2) % n);
        note(
            "CmdOrbit",
            turn(reg, |reg| {
                let _ = reg.dispatch(&crate::service::Request {
                    service: "beyonce",
                    method: "CmdOrbit",
                    args: &[PyValue::Int(target), PyValue::Real(2_000.0)],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(character(i)),
                });
            }),
        );
        note(
            "AddTarget",
            turn(reg, |reg| {
                let _ = crate::targeting::lock(reg, character(i), target);
            }),
        );
        note(
            "RemoveTarget",
            turn(reg, |reg| {
                crate::targeting::unlock(
                    reg,
                    character(i),
                    target,
                    crate::targeting::Reason::Unlocked,
                );
            }),
        );
        let gun = FIRST_GUN + i as i64;
        let victim = ship((i + 1) % n);
        note(
            "Deactivate",
            turn(reg, |reg| {
                let now = crate::pyutil::filetime_now();
                let _ =
                    crate::modules::deactivate(reg, Some(character(i)), gun, "targetAttack", now);
            }),
        );
        note(
            "Activate",
            turn(reg, |reg| {
                let now = crate::pyutil::filetime_now();
                let _ = crate::modules::activate(
                    reg,
                    Some(character(i)),
                    gun,
                    "targetAttack",
                    Some(victim),
                    1000,
                    now,
                );
            }),
        );
    }
    for late in n..n + LATE {
        note(
            "MachoBindObject",
            turn(reg, |reg| {
                crate::destiny::on_ballpark_bind(reg, Some(character(late)))
            }),
        );
    }
    out
}

/// The stats of one run: per-tick hold and encode, in microseconds.
#[derive(Debug, Default)]
pub(crate) struct Run {
    pub holds: Vec<u128>,
    pub encodes: Vec<u128>,
    pub frames: usize,
    pub bytes: usize,
    pub alive_end: usize,
    /// Each clock stage's worst tick, µs.
    pub stages: Vec<(&'static str, u128)>,
    /// Every tick's stages over 0.5 ms, by tick.
    pub detail: Vec<Vec<(&'static str, u128)>>,
}

fn pct(v: &[u128], p: f64) -> u128 {
    let mut s = v.to_vec();
    s.sort_unstable();
    if s.is_empty() {
        return 0;
    }
    s[((s.len() - 1) as f64 * p).round() as usize]
}

/// Run the clock for `ticks` real ticks against `reg`.
pub(crate) fn run(reg: &mut Registry, n: usize, ticks: u32) -> Run {
    let mut out = Run::default();
    let mut due = Instant::now() + super::TICK;
    for _ in 0..ticks {
        if let Some(nap) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(nap);
        }
        due += super::TICK;
        let now = Instant::now();
        let ft = crate::pyutil::filetime_now();
        let acquired = Instant::now();
        let turn = super::clock_turn(reg, now, ft);
        let held = acquired.elapsed().as_micros();
        out.detail.push(
            turn.timings
                .iter()
                .map(|(n, d)| (*n, d.as_micros()))
                .filter(|(_, us)| *us > 500)
                .collect(),
        );
        out.holds.push(held);
        for (name, took) in &turn.timings {
            let us = took.as_micros();
            match out.stages.iter_mut().find(|s| s.0 == *name) {
                Some(s) => s.1 = s.1.max(us),
                None => out.stages.push((name, us)),
            }
        }
        let start = Instant::now();
        let payloads = super::encode_frames(1, turn.frames);
        out.encodes.push(start.elapsed().as_micros());
        out.frames += payloads.len();
        out.bytes += payloads.iter().map(|(_, p)| p.cap_len()).sum::<usize>();
    }
    out.alive_end = (0..n)
        .filter(|&i| reg.roster().by_ship(ship(i)).is_some())
        .count();
    out
}

#[test]
#[ignore = "a real-time bench: run it in release with --ignored --nocapture"]
fn fight_bench() {
    let secs: u32 = std::env::var("FIGHT_BENCH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);
    let ticks = (Duration::from_secs(u64::from(secs)).as_millis() / super::TICK.as_millis()) as u32;
    crate::outln!(
        "| N | hold p50 | hold p99 | hold max | ticks > 25 ms | encode p50 | encode max | frames/s | KB/s | alive |"
    );
    crate::outln!("|---|---|---|---|---|---|---|---|---|---|");
    let real =
        std::env::var_os("FIGHT_BENCH_CONTENT").map(|d| Real::load(std::path::Path::new(&d)));
    if let Some(r) = &real {
        crate::outln!("bundle: fight at {:?}, background at {:?}", r.here, r.there);
    }
    for n in [10usize, 20, 30, 50] {
        let built = Instant::now();
        let mut reg = fight(n, real.as_ref());
        let setup = built.elapsed();
        let r = run(&mut reg, n, ticks);
        let ms = |us: u128| us as f64 / 1000.0;
        crate::outln!(
            "| {n} | {:.2} | {:.2} | {:.2} | {} | {:.2} | {:.2} | {:.0} | {:.0} | {} |  (setup {:.0} ms)",
            ms(pct(&r.holds, 0.5)),
            ms(pct(&r.holds, 0.99)),
            ms(*r.holds.iter().max().unwrap_or(&0)),
            r.holds.iter().filter(|&&h| h > 25_000).count(),
            ms(pct(&r.encodes, 0.5)),
            ms(*r.encodes.iter().max().unwrap_or(&0)),
            r.frames as f64 / f64::from(secs),
            r.bytes as f64 / 1024.0 / f64::from(secs),
            r.alive_end,
            setup.as_secs_f64() * 1000.0,
        );
        let mut worst = r.stages.clone();
        worst.sort_by_key(|s| std::cmp::Reverse(s.1));
        let worst: Vec<String> = worst
            .iter()
            .take(4)
            .map(|(name, us)| format!("{name} {:.2}", *us as f64 / 1000.0))
            .collect();
        crate::outln!("|   | worst stages (ms): {} |", worst.join(", "));
        let mut order: Vec<usize> = (0..r.holds.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(r.holds[i]));
        for &i in order.iter().take(3) {
            crate::outln!(
                "|   | tick #{i}: {:.2} ms {:?} |",
                ms(r.holds[i]),
                r.detail[i]
            );
        }
        for (name, us, frames) in turns(&mut reg, n) {
            crate::outln!(
                "|   | turn {name}: max {:.2} ms, {frames} frames |",
                us as f64 / 1000.0
            );
        }
    }
}

fn plain_payloads(v: Vec<(i64, super::Frame)>) -> Vec<(i64, Vec<u8>)> {
    v.into_iter()
        .map(|(c, f)| (c, f.payload().to_vec()))
        .collect()
}

/// **The golden test of the lock slices** (PA-3): the same seeded world,
/// the same ticks at the same instants and blue times, once through
/// [`super::clock_turn`] (one `&mut`, the oracle) and once through
/// [`super::clock_slices`] with the lock taken per slice. The encoded
/// frames of every tick, the session changes, the orphans, the drop counts
/// and the saved snapshot at the end must be identical. The only thing
/// slicing may change is who can get in between.
#[test]
fn sliced_clock_equals_unsliced_tick_for_tick() {
    use std::sync::RwLock;
    const N: usize = 8;
    const TICKS: u32 = 48;
    // A frozen wall clock (PA-8-0's test clock), stepped per tick and set to
    // the same instant for both runs: a run that straddled a real second
    // boundary used to see a different Destiny tick (the gate once saw 0
    // frames against 96 at one tick). `Instant`s still run, so the skew
    // below stays for them.
    let t0 = std::time::Duration::from_secs(1_800_000_000);
    crate::test_clock::freeze(t0);
    let mut plain = fight(N, None);
    let built_plain = (Instant::now(), crate::pyutil::filetime_now());
    let sliced = RwLock::new(fight(N, None));
    // The second world is built later, so its module cycles and queued
    // pushes start later on the real clocks; its ticks are shifted by that
    // much so both worlds see the same instants relative to their start.
    let built_sliced = (Instant::now(), crate::pyutil::filetime_now());
    let (skew_now, skew_ft) = (
        built_sliced.0 - built_plain.0,
        built_sliced.1 - built_plain.1,
    );
    // Both worlds are built before the clock starts, and the synthetic
    // instants sit half a tick past the edges the worlds' own `Instant::now()`
    // schedules fall near, so the few milliseconds between the two runs
    // cannot move a push across a tick.
    let base = Instant::now() + super::TICK / 2;
    let ft0 = crate::pyutil::filetime_now();
    let step_ft = (super::TICK.as_millis() as i64) * 10_000;
    let mut a_all: Vec<Vec<(i64, Vec<u8>)>> = Vec::new();
    let mut b_all: Vec<Vec<(i64, Vec<u8>)>> = Vec::new();
    let (mut a_chg, mut b_chg) = (Vec::new(), Vec::new());
    let (mut a_orph, mut b_orph) = (0usize, 0usize);
    let (mut a_drop, mut b_drop) = (0usize, 0usize);
    let mut slices_seen = 0;
    for k in 0..TICKS {
        let now = base + super::TICK * (k + 1);
        let ft = ft0 + i64::from(k + 1) * step_ft;
        let wall = t0 + super::TICK * (k + 1) + super::TICK / 2;
        crate::test_clock::freeze(wall);
        let t = super::clock_turn(&mut plain, now, ft);
        a_drop += t.dropped;
        a_orph += t.orphans.len();
        a_chg.push(t.changes);
        a_all.push(plain_payloads(super::encode_frames(1, t.frames)));

        crate::test_clock::freeze(wall);
        let (t, (), stats) = super::clock_slices(
            &sliced,
            now + skew_now,
            Some(ft + skew_ft),
            true,
            |_, _, _| (),
        );
        slices_seen = slices_seen.max(usize::from(stats.max <= stats.held));
        b_drop += t.dropped;
        b_orph += t.orphans.len();
        b_chg.push(t.changes);
        b_all.push(plain_payloads(super::encode_frames(1, t.frames)));
    }
    assert!(a_all.iter().any(|f| !f.is_empty()), "the fight sent frames");
    assert_eq!(slices_seen, 1);
    // Not byte-exact, and not because of slicing: a few pushes carry the
    // wall clock (`filetime_now()` inside `combat`, the Destiny stamp is
    // `SystemTime`), read microseconds apart by the two runs. So the golden
    // is: same ticks, same frames in the same order for the same clients,
    // same lengths, and at most `JITTER` bytes apart per frame (the low
    // bytes of a 64-bit FILETIME). A reordered, missing or extra push, or a
    // changed field, moves far more than that.
    println!(
        "plain {:?}\nsliced {:?}",
        a_all.iter().map(Vec::len).collect::<Vec<_>>(),
        b_all.iter().map(Vec::len).collect::<Vec<_>>()
    );
    const JITTER: usize = 8;
    let mut jittered = 0;
    for (k, (a, b)) in a_all.iter().zip(&b_all).enumerate() {
        assert_eq!(a.len(), b.len(), "tick {k}: frame count");
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!((x.0, x.1.len()), (y.0, y.1.len()), "tick {k} frame {i}");
            let apart = x.1.iter().zip(&y.1).filter(|(p, q)| p != q).count();
            assert!(apart <= JITTER, "tick {k} frame {i}: {apart} bytes apart");
            jittered += usize::from(apart > 0);
        }
    }
    let total: usize = a_all.iter().map(Vec::len).sum();
    assert!(total > 50, "enough frames to mean something: {total}");
    println!("golden: {total} frames, {jittered} with wall-clock jitter");
    assert_eq!(a_chg, b_chg, "session changes");
    assert_eq!((a_drop, a_orph), (b_drop, b_orph));
    let snap = |r: &Registry| format!("{:?}", r.to_snapshot());
    let (sa, sb) = (snap(&plain), snap(&sliced.read().unwrap()));
    assert_eq!(sa.len(), sb.len(), "world state");
    crate::test_clock::thaw();
}

/// **PA-3's wait bench**: a payload-turn stand-in takes the world write lock
/// every ~7 ms and records how long it waited, while the clock runs on the
/// 250 ms deadline, once unsliced and once sliced, on the same fight world.
/// Run it in release:
///
/// ```text
/// tools/dev.sh cargo test --release --locked -p eve-server slice_wait_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a real-time bench: run it in release with --ignored --nocapture"]
fn slice_wait_bench() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, RwLock};
    let secs: u64 = std::env::var("FIGHT_BENCH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);
    println!(
        "| N | mode | wait p50 us | wait p95 us | wait max us | clock held mean ms | slice max ms |"
    );
    println!("|---|---|---|---|---|---|---|");
    for n in [30usize, 100, 262] {
        for sliced in [false, true] {
            let world = Arc::new(RwLock::new(fight(n, None)));
            let stop = Arc::new(AtomicBool::new(false));
            let payload = {
                let (world, stop) = (Arc::clone(&world), Arc::clone(&stop));
                std::thread::spawn(move || {
                    let mut waits: Vec<u128> = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(7));
                        let asked = Instant::now();
                        let w = world.write().unwrap();
                        waits.push(asked.elapsed().as_micros());
                        // A turn's own work: a short hold.
                        std::hint::black_box(w.roster().is_in_space(character(0)));
                    }
                    waits
                })
            };
            let mut due = Instant::now() + super::TICK;
            let (mut held, mut slice_max, mut ticks) = (Duration::ZERO, Duration::ZERO, 0u32);
            let end = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < end {
                if let Some(nap) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(nap);
                }
                due += super::TICK;
                let (turn, (), stats) =
                    super::clock_slices(&world, Instant::now(), None, sliced, |_, _, _| ());
                let _ = super::encode_frames(1, turn.frames);
                held += stats.held;
                slice_max = slice_max.max(stats.max);
                ticks += 1;
            }
            stop.store(true, Ordering::Relaxed);
            let waits = payload.join().unwrap();
            println!(
                "| {n} | {} | {} | {} | {} | {:.2} | {:.2} |",
                if sliced { "sliced" } else { "one lock" },
                pct(&waits, 0.5),
                pct(&waits, 0.95),
                pct(&waits, 1.0),
                held.as_secs_f64() * 1000.0 / f64::from(ticks.max(1)),
                slice_max.as_secs_f64() * 1000.0,
            );
        }
    }
}

/// **PA-9's measurement**: the encode half of the clock at 262 and 500 pilots
/// on one grid, steady and with a 10% churn (every 10th ship warps off and
/// back on alternate reconciles, which makes the big `BallsAdded` frames),
/// for 1, 4, 8 and 12 threads. Each tick is drained once and encoded once per
/// thread count from the same pushes. `enc` is phase A + B (no deflate:
/// the 1-thread row is the old serial path, whose deflate ran on the writer
/// shards, so `enc+z` adds `compress_payload` over every frame to it);
/// the parallel rows deflate inside the encode.
#[test]
#[ignore = "a real-time bench: run it in release with --ignored --nocapture"]
fn encode_bench() {
    let secs: u64 = std::env::var("ENCODE_BENCH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let ticks = (secs * 1000 / super::TICK.as_millis() as u64) as u32;
    let sizes: Vec<usize> = std::env::var("ENCODE_BENCH_N")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![262, 500]);
    crate::outln!(
        "| N | world | threads | enc mean ms | enc max ms | enc+z mean ms | enc+z max ms | frames/tick | >=8KiB/tick |"
    );
    crate::outln!("|---|---|---|---|---|---|---|---|---|");
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    for n in sizes {
        for churn in [false, true] {
            let mut reg = fight(n, None);
            let home: Vec<[f64; 3]> = (0..n)
                .map(|i| {
                    reg.roster()
                        .get(character(i))
                        .expect("pilot")
                        .motion
                        .position
                })
                .collect();
            let threads = [1usize, 4, 8, 12];
            let mut enc: Vec<Vec<f64>> = vec![Vec::new(); threads.len()];
            let mut encz: Vec<Vec<f64>> = vec![Vec::new(); threads.len()];
            let (mut frames, mut big, mut measured) = (0usize, 0usize, 0usize);
            let mut due = Instant::now() + super::TICK;
            let mut away = false;
            for t in 0..ticks {
                if let Some(nap) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(nap);
                }
                due += super::TICK;
                if churn && t % 4 == 1 {
                    away = !away;
                    let now_s = crate::space_state::now_seconds();
                    for (k, home) in home.iter().enumerate().step_by(10) {
                        let mut at = *home;
                        if away {
                            at[0] += 5.0e8;
                        }
                        if let Some(p) = reg.roster_mut().get_mut(character(k)) {
                            p.motion = crate::space_state::Motion::arrived(at, [0.0; 3]);
                            p.since = now_s;
                        }
                    }
                }
                let turn =
                    super::clock_turn(&mut reg, Instant::now(), crate::pyutil::filetime_now());
                if t < 8 {
                    continue; // the lock-on and bind backlog
                }
                measured += 1;
                for (k, &th) in threads.iter().enumerate() {
                    let start = Instant::now();
                    let out = super::encode_frames_with(1, &turn.frames, th);
                    enc[k].push(ms(start.elapsed()));
                    let mut total = ms(start.elapsed());
                    if th == 1 {
                        // What the writer shards then do per frame.
                        let zs = Instant::now();
                        for (_, f) in &out {
                            std::hint::black_box(super::compress_payload(f.payload()));
                        }
                        total += ms(zs.elapsed());
                    }
                    encz[k].push(total);
                    if k == 0 {
                        frames += out.len();
                        big += out
                            .iter()
                            .filter(|(_, f)| f.cap_len() >= super::COMPRESS_MIN_BYTES)
                            .count();
                    }
                }
            }
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
            let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
            let m = measured.max(1);
            for (k, th) in threads.iter().enumerate() {
                crate::outln!(
                    "| {n} | {} | {th} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} |",
                    if churn { "churn" } else { "steady" },
                    mean(&enc[k]),
                    max(&enc[k]),
                    mean(&encz[k]),
                    max(&encz[k]),
                    frames / m,
                    big / m,
                );
            }
        }
    }
}

/// **PA-10's measurement**: the same drained tick put out both ways, flag
/// off (`encode_frames`, a frame per push per client) and flag on
/// (`stage_frames`, then each client's outbox tail merge, then the writer's
/// `encode_part`). In process, no sockets: one [`super::Outbox`] per client,
/// everything due at once, taken once per tick as a writer pass does.
/// Reports the Destiny frames the writers would send per second either way,
/// and the ticker's and the writers' encode time per tick. The payload-turn
/// path (`deliver_elsewhere`) is not driven here.
///
/// `COALESCE_BENCH_N=50,262,500`, `COALESCE_BENCH_SECS=10`.
#[test]
#[ignore = "a real-time bench: run it in release with --ignored --nocapture"]
fn coalesce_bench() {
    use std::collections::HashMap;
    let secs: u64 = std::env::var("COALESCE_BENCH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let ticks = (secs * 1000 / super::TICK.as_millis() as u64) as u32;
    let sizes: Vec<usize> = std::env::var("COALESCE_BENCH_N")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![50, 262, 500]);
    crate::outln!(
        "| N | flag | ticker ms mean | ticker ms max | writers ms mean | destiny frames/s | destiny KB/s | all frames/s | merged % |"
    );
    crate::outln!("|---|---|---|---|---|---|---|---|---|");
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    for n in sizes {
        let mut reg = fight(n, None);
        let mut due = Instant::now() + super::TICK;
        // [off, on]
        let mut ticker = [Vec::new(), Vec::new()];
        let mut writers_ms: Vec<f64> = Vec::new();
        let (mut dest, mut all, mut bytes) = ([0usize; 2], [0usize; 2], [0usize; 2]);
        let (mut parts, mut merged) = (0usize, 0usize);
        let mut measured = 0usize;
        for t in 0..ticks {
            if let Some(nap) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(nap);
            }
            due += super::TICK;
            let turn = super::clock_turn(&mut reg, Instant::now(), crate::pyutil::filetime_now());
            if t < 8 {
                continue; // the lock-on and bind backlog
            }
            measured += 1;
            let events: Vec<&'static str> =
                turn.frames.iter().map(|o| o.push.to_push().0).collect();
            let threads = crate::par::threads();

            // Off: a frame per push per client, as the clock posts today.
            let start = Instant::now();
            let off = super::encode_frames_with(1, &turn.frames, threads);
            ticker[0].push(ms(start.elapsed()));
            all[0] += off.len();
            let mut k = 0;
            for (o, ev) in turn.frames.iter().zip(&events) {
                for _ in &o.clients {
                    if *ev == "DoDestinyUpdate" {
                        dest[0] += 1;
                        bytes[0] += off[k].1.cap_len();
                    }
                    k += 1;
                }
            }

            // On: stage, post into each client's outbox, take, encode.
            let start = Instant::now();
            let staged = super::stage_frames_with(1, &turn.frames, threads);
            let mut boxes: HashMap<i64, super::Outbox> = HashMap::new();
            let now = Instant::now();
            for (client, frame) in staged {
                let o = boxes.entry(client).or_default();
                match frame {
                    super::Frame::Part(p) => {
                        parts += 1;
                        merged += usize::from(o.post_part(now, p).expect("posts"));
                    }
                    f => o.post(now, f).expect("posts"),
                }
            }
            ticker[1].push(ms(start.elapsed()));
            let start = Instant::now();
            for o in boxes.values_mut() {
                for f in o.take_due(now) {
                    all[1] += 1;
                    match f {
                        super::Frame::Part(p) => {
                            let payload = super::encode_part(&p).expect("encodes");
                            dest[1] += 1;
                            bytes[1] += payload.len();
                            std::hint::black_box(super::compress_payload(&payload));
                        }
                        // A non-part push (a SetState): counted as a frame
                        // only; its Destiny share is small and in `all`.
                        super::Frame::Raw(p) => {
                            std::hint::black_box(super::compress_payload(&p));
                        }
                        super::Frame::Ready { .. } => {}
                    }
                }
            }
            writers_ms.push(ms(start.elapsed()));
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
        let s = (measured.max(1) as f64) * super::TICK.as_secs_f64();
        for (i, flag) in ["off", "on"].iter().enumerate() {
            crate::outln!(
                "| {n} | {flag} | {:.2} | {:.2} | {:.2} | {:.0} | {:.0} | {:.0} | {:.1} |",
                mean(&ticker[i]),
                max(&ticker[i]),
                if i == 1 { mean(&writers_ms) } else { 0.0 },
                dest[i] as f64 / s,
                bytes[i] as f64 / 1024.0 / s,
                all[i] as f64 / s,
                if i == 1 && parts > 0 {
                    100.0 * merged as f64 / parts as f64
                } else {
                    0.0
                },
            );
        }
    }
}
