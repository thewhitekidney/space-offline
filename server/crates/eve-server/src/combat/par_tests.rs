//! PA-8c: `on_cycles` plans its volley units in parallel and gets the old
//! loop's result bit for bit, through kills inside the batch, a launcher, a
//! bank and rolled misses; and the cached audience equals the per-volley
//! scan after an arrival and a kill. Frozen clock, invented ids.

use super::*;
use crate::damage::{Damage, Profile, ShipDamage};
use crate::space_state::{Motion, Presence};
use std::sync::Arc;
use std::time::Instant;

const SYSTEM: i64 = 30_000_001;
const PILOTS: usize = 60;
const GUNS: usize = 9;
const TICKS: usize = 6;
const GUN_TYPE: i64 = 3640;
const LAUNCHER_TYPE: i64 = 990_470_001;
const MISSILE_TYPE: i64 = 990_470_002;
/// Invented: a gun that rolls to hit (54 optimal, 158 falloff, 160 tracking,
/// 620 resolution) and a launcher with its missile.
const ROWS: &str = "\
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t
effect\t9\t2\t1\t0\t0\t0\t0\t0\tmissileLaunching\teffects.FixtureMissileLaunch
type\t3640\t55\t7\t1\t1\t5.0\t0.6\t0.0\t1.0\t0\t12,16\t54=9000.0;158=3000.0;160=0.02;620=400.0\tTest Blaster
type\t990470001\t99461\t7\t1\t1\t5.0\t0.6\t0.0\t1.0\t101\t12,16,40,101\t51=4000.0;56=1.0;604=99462.0;1795=8000.0\tTest Launcher
type\t990470002\t99462\t8\t1\t100\t0.015\t0.0\t700.0\t300.0\t9\t9\t37=3000.0;70=0.0001;114=10.0;117=20.0;281=5000.0;653=100.0;654=40.0\tTest Missile
";

fn pilot(i: usize) -> i64 {
    90_070_001 + i as i64
}

fn ship(i: usize) -> i64 {
    100_070_001 + i as i64
}

fn frail(at: BlueTime, hp: f64) -> ShipDamage {
    let one = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };
    ShipDamage::full(
        Profile {
            shield_capacity: hp,
            shield_recharge_rate: 1.5e6,
            armor_hp: hp,
            hp,
            shield_resonance: one,
            armor_resonance: one,
            hull_resonance: one,
        },
        at,
    )
}

/// `PILOTS` pilots in space (the last `late` of them not yet bound), spread
/// over some 40 km and drifting, a third of them one volley from death.
fn arena(late: usize) -> Registry {
    crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
    let chars = (0..PILOTS)
        .map(|i| crate::model::Character {
            character_id: pilot(i),
            name: format!("Pilot {i}"),
            ship_id: ship(i),
            ship_type_id: 587,
            ..crate::model::Character::default()
        })
        .collect();
    let mut content = crate::content::Content::fixture();
    let table = crate::typedata::Table::parse(ROWS).expect("the invented rows parse");
    content.insert(
        crate::typedata::TABLE.name,
        crate::typedata::Installed::build(table),
    );
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: chars,
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(content));
    for i in 0..PILOTS - late {
        bind(&mut reg, i);
    }
    let now = crate::pyutil::filetime_now();
    for i in (0..PILOTS).step_by(3) {
        reg.combat_mut().set_ship(ship(i), frail(now, 1.0));
    }
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
    reg
}

fn bind(reg: &mut Registry, i: usize) {
    crate::destiny::on_ballpark_bind(reg, Some(pilot(i)));
    let at = [(i % 10) as f64 * 4_000.0, 0.0, (i / 10) as f64 * 4_000.0];
    let v = [(i % 7) as f64 * 40.0, 0.0, (i % 5) as f64 * 30.0];
    let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
    p.motion = Motion::arrived(at, v);
}

/// `GUNS` guns on every pilot (a 540-gun burst) and a launcher on every
/// seventh, all at pilots a few places on.
fn cycles(at: i64, round: usize) -> Vec<Cycle> {
    let mut out = Vec::new();
    for i in 0..PILOTS {
        let target = ship((i * 7 + 3 + round) % PILOTS);
        if target == ship(i) {
            continue;
        }
        for g in 0..GUNS {
            out.push(Cycle {
                character_id: pilot(i),
                ship_id: ship(i),
                module_id: 100_071_000 + (i * GUNS + g) as i64,
                type_id: GUN_TYPE,
                effect_id: 10,
                target_id: Some(target),
                at: at + g as i64,
                index: 0,
                charge_type_id: None,
            });
        }
        if i % 7 == 0 {
            out.push(Cycle {
                module_id: 100_072_000 + i as i64,
                type_id: LAUNCHER_TYPE,
                effect_id: 101,
                charge_type_id: Some(MISSILE_TYPE),
                ..out[out.len() - 1]
            });
        }
    }
    out
}

/// Every piece of state a volley writes, as text.
fn fingerprint(reg: &mut Registry) -> String {
    let balls = reg.roster().in_system(SYSTEM);
    let ships: Vec<_> = (0..PILOTS)
        .map(|i| (ship(i), reg.combat().ship(ship(i)).copied()))
        .collect();
    let wrecks: Vec<_> = reg.combat().wrecks().cloned().collect();
    let flights = reg.ext::<crate::missile::Flights>().len();
    let pending: Vec<_> = reg
        .take_pending()
        .into_iter()
        .map(|(after, p)| (after, p.audience(), p.to_push()))
        .collect();
    let due: Vec<_> = reg
        .take_due(Instant::now() + Duration::from_secs(3_600))
        .into_iter()
        .map(|p| (p.audience(), p.to_push()))
        .collect();
    format!("{balls:?}\n{ships:?}\n{wrecks:?}\n{flights}\n{pending:?}\n{due:?}")
}

fn same(got: &str, want: &str, what: &str) {
    if got == want {
        return;
    }
    let at = got
        .bytes()
        .zip(want.bytes())
        .position(|(g, w)| g != w)
        .unwrap_or(got.len().min(want.len()));
    let around = |s: &str| s[at.saturating_sub(300)..(at + 300).min(s.len())].to_string();
    panic!(
        "{what}: parted at byte {at}\n got: ...{}\nwant: ...{}",
        around(got),
        around(want)
    );
}

#[test]
fn on_cycles_par_equals_the_legacy_loop() {
    let mut worlds = [arena(0), arena(0), arena(0)];
    let spawned0 = crate::par::spawned_here();
    let dead = |reg: &Registry| {
        (0..PILOTS)
            .filter(|i| reg.roster().get(pilot(*i)).map(|p| p.ship_id) != Some(ship(*i)))
            .count()
    };
    for round in 0..TICKS {
        crate::test_clock::step(Duration::from_millis(250));
        let now = crate::pyutil::filetime_now();
        let dead0 = dead(&worlds[0]);
        on_cycles_legacy(&mut worlds[0], cycles(now, round), now);
        let gold = fingerprint(&mut worlds[0]);
        if round == 0 {
            assert!(dead(&worlds[0]) > dead0, "kills inside the batch");
        }
        let before = crate::par::spawned_here();
        on_cycles_with(&mut worlds[1], cycles(now, round), now, 1, 0);
        assert_eq!(
            before,
            crate::par::spawned_here(),
            "one thread spawns nothing"
        );
        same(
            &fingerprint(&mut worlds[1]),
            &gold,
            &format!("threads 1, round {round}"),
        );
        on_cycles_with(&mut worlds[2], cycles(now, round), now, 4, 0);
        same(
            &fingerprint(&mut worlds[2]),
            &gold,
            &format!("threads 4, round {round}"),
        );
    }
    assert!(
        crate::par::spawned_here() > spawned0,
        "threads 4 went parallel"
    );
    assert!(dead(&worlds[0]) >= 3, "{} dead", dead(&worlds[0]));
    assert!(
        !worlds[0].ext::<crate::missile::Flights>().is_empty(),
        "missiles flew"
    );
    assert!(
        (0..PILOTS)
            .filter(|i| i % 3 != 0)
            .any(|i| worlds[0].combat().ship(ship(i)).is_some()),
        "a volley landed on a ship nobody had hit"
    );
    crate::test_clock::thaw();
}

#[test]
fn the_default_gate_keeps_a_small_batch_serial() {
    let mut reg = arena(0);
    let now = crate::pyutil::filetime_now();
    let small: Vec<Cycle> = cycles(now, 0)
        .into_iter()
        .take(COMBAT_PAR_MIN - 1)
        .collect();
    let before = crate::par::spawned_here();
    on_cycles_with(&mut reg, small, now, 8, COMBAT_PAR_MIN);
    assert_eq!(crate::par::spawned_here(), before);
    crate::test_clock::thaw();
}

#[test]
fn audience_follows_arrivals_and_kills() {
    let mut reg = arena(2);
    let victim = |reg: &Registry, i: usize| *reg.roster().get(pilot(i)).expect("in space");
    let scan = |reg: &Registry, v: &Presence| {
        let mut park = vec![v.character_id];
        park.extend(
            reg.roster()
                .others_in_system(v.solar_system_id, v.character_id),
        );
        park.retain(|id| reg.character(*id).is_some());
        park
    };
    let v = victim(&reg, 4);
    assert_eq!(audience(&mut reg, &v), scan(&reg, &v));
    assert_eq!(audience(&mut reg, &v), scan(&reg, &v), "from the cache");
    // An arrival (the two late pilots), a second victim.
    bind(&mut reg, PILOTS - 1);
    assert_eq!(audience(&mut reg, &v), scan(&reg, &v), "after an arrival");
    let w = victim(&reg, PILOTS - 1);
    assert_eq!(audience(&mut reg, &w), scan(&reg, &w), "the newcomer");
    // A kill: a pilot leaves.
    let _ = reg.roster_mut().leave(pilot(9));
    assert_eq!(audience(&mut reg, &v), scan(&reg, &v), "after a kill");
    // An NPC victim is in nobody's park but sees everyone else's.
    let npc = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, SYSTEM, [0.0; 3]).expect("id");
    let n = *reg.roster().get(npc).expect("in space");
    assert_eq!(audience(&mut reg, &n), scan(&reg, &n), "an NPC victim");
    assert!(!audience(&mut reg, &n).contains(&npc));
    crate::test_clock::thaw();
}
