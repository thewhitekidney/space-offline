//! PA-8b: `advance_system` builds its movers in parallel and gets the serial
//! result, bit for bit, on a frozen clock (invented ids throughout).

use super::*;
use crate::destiny::Mode;
use crate::space_state::{Motion, Presence};
use std::sync::Arc;
use std::time::Duration;

const SYSTEM: i64 = 30_000_001;
const PILOTS: usize = 640;
const STEPS: usize = 20;

fn pilot(i: usize) -> i64 {
    90_090_001 + i as i64
}

fn ship(i: usize) -> i64 {
    100_090_001 + i as i64
}

/// 640 pilots (a fifth orbit or follow a neighbour, a fifth fly to a point,
/// the rest sit still), 30 NPCs and 30 drone balls. Frozen clock.
fn world(pilots: usize) -> Registry {
    crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
    let chars = (0..pilots)
        .map(|i| crate::model::Character {
            character_id: pilot(i),
            name: format!("Pilot {i}"),
            ship_id: ship(i),
            ship_type_id: 587,
            ..crate::model::Character::default()
        })
        .collect();
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: chars,
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(crate::content::Content::fixture()));
    for i in 0..pilots {
        crate::destiny::on_ballpark_bind(&mut reg, Some(pilot(i)));
        let at = [(i % 40) as f64 * 900.0, 0.0, (i / 40) as f64 * 900.0];
        let other = ship((i + 1) % pilots);
        let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
        p.motion = Motion::arrived(at, [0.0; 3]);
        match i % 5 {
            0 => {
                p.motion.mode = Mode::Orbit {
                    ball_id: other,
                    range: 2_000.0,
                };
                p.motion.speed_fraction = 1.0;
            }
            1 => {
                p.motion.mode = Mode::Follow {
                    ball_id: other,
                    range: 700.0,
                };
                p.motion.speed_fraction = 0.8;
            }
            2 => {
                p.motion.mode = Mode::Goto([30_000.0, 1_000.0, 20_000.0]);
                p.motion.speed_fraction = 1.0;
            }
            _ => {}
        }
    }
    for k in 0..30 {
        let at = [-5_000.0 - 300.0 * k as f64, 0.0, 100.0 * k as f64];
        crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, SYSTEM, at).expect("an id");
        let id = 195_090_001 + k;
        reg.roster_mut().arrive(Presence::arriving(
            id,
            id,
            SYSTEM,
            Motion::arrived([100.0 * k as f64, 50.0, 0.0], [10.0, 0.0, 0.0]),
        ));
    }
    reg
}

fn print(reg: &Registry) -> String {
    format!("{:?}", reg.roster().in_system(SYSTEM))
}

#[test]
fn parallel_movers_advance_like_the_serial_ones() {
    let mut worlds = [world(PILOTS), world(PILOTS), world(PILOTS)];
    assert!(worlds[0].roster().in_system(SYSTEM).len() >= MOVERS_PAR_MIN);
    assert!(
        worlds[0].roster().has_pair(SYSTEM),
        "the on-ticks path runs"
    );
    let spawned0 = crate::par::spawned_here();
    let start = print(&worlds[0]);
    for step in 0..STEPS {
        crate::test_clock::step(Duration::from_millis(250));
        worlds[0].advance_system_legacy(SYSTEM);
        let before = crate::par::spawned_here();
        worlds[1].advance_system_with(SYSTEM, 1);
        assert_eq!(
            before,
            crate::par::spawned_here(),
            "one thread spawns nothing"
        );
        worlds[2].advance_system_with(SYSTEM, 4);
        let gold = print(&worlds[0]);
        assert_eq!(print(&worlds[1]), gold, "threads 1, step {step}");
        assert_eq!(print(&worlds[2]), gold, "threads 4, step {step}");
    }
    assert_ne!(print(&worlds[0]), start, "the ships moved");
    assert!(
        crate::par::spawned_here() > spawned0,
        "threads 4 went parallel"
    );
    crate::test_clock::thaw();
}

#[test]
fn below_the_gate_nothing_is_spawned() {
    let mut reg = world(100);
    assert!(reg.roster().in_system(SYSTEM).len() < MOVERS_PAR_MIN);
    let before = crate::par::spawned_here();
    crate::test_clock::step(Duration::from_millis(250));
    reg.advance_system_with(SYSTEM, 8);
    assert_eq!(crate::par::spawned_here(), before);
    crate::test_clock::thaw();
}
