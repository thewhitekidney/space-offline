//! NPC-THREAT: the ledger's hooks and the brain's use of it, on a real
//! `Registry` with invented ids.

use super::tests::{ALICE, ALICE_SHIP, BOB, BOB_SHIP, hijacker_near, world};
use super::*;
use crate::damage::Shot;
use crate::npc_threat::{self, AGGRESSION_MEMORY, THREAT_WINDOW, Threat};
use crate::space_state::{Motion, Presence};

const CAROL_SHIP: i64 = 100_000_003;
const DRONE: i64 = 990_000_401;

fn volley(reg: &Registry, rat: i64, at: BlueTime) -> Shot {
    Shot {
        at,
        attacker_ship_id: 0,
        weapon_item_id: 1,
        weapon_type_id: 3640,
        target_ship_id: rat,
        damage: crate::combat::volley_in(reg.content(), 3640).expect("a gun"),
        hit_quality: crate::combat::HIT_QUALITY,
    }
}

/// `who` fires one real volley at `rat` through `combat::fire`.
fn shoot(reg: &mut Registry, who: i64, rat: i64, at: BlueTime) {
    let victim = reg.roster().get(rat).copied().expect("alive");
    let shot = volley(reg, rat, at);
    crate::combat::fire(reg, shot, who, victim, at);
}

fn standing(reg: &Registry, rat: i64, ship: i64, now: BlueTime) -> Option<f64> {
    reg.ext::<Threat>().standing(rat, ship, now)
}

/// A volley from a ship the rat has not noticed (40 km, past the notice
/// radius) puts it in the ledger, and the next decision acquires and
/// follows it.
#[test]
fn a_sniper_is_acquired_after_the_volley() {
    let mut reg = world(&[ALICE]);
    let rat = hijacker_near(&mut reg, ALICE, 40_000.0);
    let t0 = crate::pyutil::filetime_now();
    tick(&mut reg, t0);
    assert_eq!(reg.npcs().get(rat).and_then(|n| n.target), None);
    shoot(&mut reg, ALICE, rat, t0);
    let dealt = standing(&reg, rat, ALICE_SHIP, t0).expect("an aggressor");
    assert!(dealt > 0.0, "the volley counts: {dealt}");
    tick(&mut reg, t0 + 2_500_000);
    let n = reg.npcs().get(rat).copied().expect("alive");
    assert_eq!(n.target, Some(ALICE_SHIP));
    assert_eq!(n.phase, Phase::Approach);
}

/// A miss and an ewar cycle (0 damage) still make an aggressor.
#[test]
fn an_act_without_damage_is_still_aggression() {
    let mut reg = world(&[ALICE]);
    let rat = hijacker_near(&mut reg, ALICE, 5_000.0);
    let t0 = 1_000_000_000;
    npc_threat::on_act(&mut reg, ALICE, rat, 0.0, t0);
    assert_eq!(standing(&reg, rat, ALICE_SHIP, t0), Some(0.0));
    // Not a rat, not a character, or the rat itself: nothing.
    npc_threat::on_act(&mut reg, ALICE, ALICE, 5.0, t0);
    npc_threat::on_act(&mut reg, rat, rat, 5.0, t0);
    assert_eq!(standing(&reg, ALICE, ALICE_SHIP, t0), None);
    assert_eq!(standing(&reg, rat, ALICE, t0), None, "keyed by ship");
}

/// Threat is the last 10 s of damage; the aggressor is forgotten after the
/// memory.
#[test]
fn threat_decays_and_the_memory_lapses() {
    let mut reg = world(&[ALICE]);
    let rat = hijacker_near(&mut reg, ALICE, 5_000.0);
    let t0 = 1_000_000_000;
    npc_threat::on_act(&mut reg, ALICE, rat, 40.0, t0);
    npc_threat::on_act(&mut reg, ALICE, rat, 10.0, t0 + 1);
    assert_eq!(standing(&reg, rat, ALICE_SHIP, t0 + 2), Some(50.0));
    let later = t0 + 2 + THREAT_WINDOW;
    assert_eq!(standing(&reg, rat, ALICE_SHIP, later), Some(0.0));
    assert_eq!(
        standing(&reg, rat, ALICE_SHIP, t0 + 1 + AGGRESSION_MEMORY),
        Some(0.0)
    );
    assert_eq!(
        standing(&reg, rat, ALICE_SHIP, t0 + 2 + AGGRESSION_MEMORY),
        None
    );
}

/// The logi: reping an aggressor books the heal to the repper and makes it
/// an aggressor; repping a bystander does nothing.
#[test]
fn a_repper_of_an_aggressor_becomes_one() {
    let mut reg = world(&[ALICE]);
    let rat = hijacker_near(&mut reg, ALICE, 5_000.0);
    let t0 = 1_000_000_000;
    npc_threat::assisted(&mut reg, CAROL_SHIP, BOB_SHIP, 300.0, t0);
    assert_eq!(
        standing(&reg, rat, CAROL_SHIP, t0),
        None,
        "Bob is no aggressor"
    );
    npc_threat::on_act(&mut reg, ALICE, rat, 100.0, t0);
    npc_threat::assisted(&mut reg, CAROL_SHIP, ALICE_SHIP, 300.0, t0 + 1);
    assert_eq!(standing(&reg, rat, CAROL_SHIP, t0 + 2), Some(300.0));
    assert_eq!(standing(&reg, rat, ALICE_SHIP, t0 + 2), Some(100.0));
    // A second repper behind Carol joins too (chains).
    npc_threat::assisted(&mut reg, BOB_SHIP, CAROL_SHIP, 50.0, t0 + 3);
    assert_eq!(standing(&reg, rat, BOB_SHIP, t0 + 4), Some(50.0));
}

/// A drone's volley is its owner's ship's; the drone itself is no aggressor.
#[test]
fn a_drone_counts_for_its_owner() {
    let mut reg = world(&[ALICE, BOB]);
    let rat = hijacker_near(&mut reg, ALICE, 5_000.0);
    let at = reg.roster().get(BOB).expect("in space").motion.position;
    reg.roster_mut().arrive(Presence::arriving(
        DRONE,
        DRONE,
        30_000_001,
        Motion::arrived(at, [0.0; 3]),
    ));
    let d = crate::drones::Drone::launched(
        DRONE,
        crate::drones::HOBGOBLIN_I,
        BOB,
        BOB_SHIP,
        30_000_001,
    );
    reg.modules_mut().drones_mut().insert(d);
    let t0 = crate::pyutil::filetime_now();
    shoot(&mut reg, DRONE, rat, t0);
    let dealt = standing(&reg, rat, BOB_SHIP, t0).expect("Bob's ship is the aggressor");
    assert!(dealt > 0.0);
    assert_eq!(standing(&reg, rat, DRONE, t0), None);
}

/// Sharing: a rat 3 km from the shot one takes the aggressor; one beyond the
/// grid radius, and a ship of the law, do not.
#[test]
fn the_group_shares_within_the_grid() {
    let mut reg = world(&[ALICE]);
    let a = hijacker_near(&mut reg, ALICE, 30_000.0);
    let b = hijacker_near(&mut reg, ALICE, 33_000.0);
    let far = hijacker_near(&mut reg, ALICE, 30_000.0 + 3.0 * crate::grid::GRID_ENTER);
    let t0 = crate::pyutil::filetime_now();
    npc_threat::on_act(&mut reg, ALICE, a, 80.0, t0);
    assert_eq!(standing(&reg, a, ALICE_SHIP, t0), Some(80.0));
    assert_eq!(standing(&reg, b, ALICE_SHIP, t0), Some(0.0));
    assert_eq!(standing(&reg, far, ALICE_SHIP, t0), None);
    // And B, with nothing locking it, goes for Alice at its next look.
    tick(&mut reg, t0);
    assert_eq!(reg.npcs().get(b).and_then(|n| n.target), Some(ALICE_SHIP));
}

/// The law is never given a ledger.
#[test]
fn a_ship_of_the_law_has_no_ledger() {
    let mut reg = world(&[ALICE]);
    let at = reg.roster().get(ALICE).expect("in space").motion.position;
    let law = spawn_bound(
        &mut reg,
        &FIXTURE_RAT,
        30_000_001,
        [at[0] + 3_000.0, at[1], at[2]],
        Law { offender: BOB },
    )
    .expect("an id");
    npc_threat::on_act(&mut reg, ALICE, law, 80.0, 1_000_000_000);
    assert_eq!(standing(&reg, law, ALICE_SHIP, 1_000_000_000), None);
}

/// The sweep forgets a dead rat's entry.
#[test]
fn the_sweep_drops_dead_rats() {
    let mut reg = world(&[ALICE]);
    let a = hijacker_near(&mut reg, ALICE, 5_000.0);
    let t0 = 1_000_000_000;
    npc_threat::on_act(&mut reg, ALICE, a, 1.0, t0);
    let rev = reg.ext::<Threat>().revision();
    assert!(rev > 0);
    let b = hijacker_near(&mut reg, ALICE, 6_000.0);
    reg.roster_mut().leave(a);
    reg.npcs_mut().remove(a);
    npc_threat::on_act(&mut reg, ALICE, b, 1.0, t0 + AGGRESSION_MEMORY);
    assert_eq!(standing(&reg, a, ALICE_SHIP, t0), None);
}
