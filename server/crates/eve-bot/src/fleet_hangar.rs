//! **FLEET-HANGAR (#189): a miner fills its fleet boss's fleet hangar.**
//!
//! When the laser stops on a full hold, a miner in a fleet whose boss's ship
//! is on the grid does not go home: it compresses what it can (PROG-MINE,
//! [`crate::ore::on_hold_full`]), flies to the boss's ship, compresses again
//! there (the boss's Orca, Rorqual or Porpoise is the compressor most likely
//! to be in range), and moves every ore stack into the boss's fleet hangar.
//! Refused, out of range for too long, or the boss gone: it goes home and
//! unloads as before, and does not try the drop again for [`DROP_RETRY`].
//!
//! What the client does **[V]**, and the server already serves (CAP-H,
//! `eve_server::ship_config`): the drop on another ship's fleet hangar is
//! `Add(itemID, sourceID, qty=n, flag=155)` on that ship's inventory
//! (`invControllers.py:__AddItem`), allowed when the owner's
//! `FleetHangar_AllowFleetAccess` (or corp) switch is on and the two ships
//! are within `maxCargoContainerTransferDistance` 2,500 m surface to surface
//! (`invControllers.ShipFleetHangar.GetOperationalDistance`,
//! `appConst.py:1460`). The refusals are the client's own keys:
//! `FleetHangarAccessDenied`, `CannotAccessShipHangar`, and the ordinary
//! capacity refusals for a full bay.
//!
//! **[A]** the fleet boss is the ship to fill: the bot's fleet boss is its
//! inviter ([`crate::brain::FleetView::boss`]), which in a human's fleet is
//! the human. A bot foreman's Orca is the boss only in a bot fleet, and its
//! owner's switch is off by default, so the first try is refused and the
//! miner goes home. Falsified by a human fleet whose boss is a bot.

use crate::brain::{self, Act, Ball, Homing, Memory, Observation, ParkView};
use crate::ore::OreAct;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// `flagFleetHangar` **[V]** (`invControllers.ShipFleetHangar.locationFlag`).
pub const FLAG_FLEET_HANGAR: i64 = eve_server::ship_config::FLAG_FLEET_HANGAR;
/// `maxCargoContainerTransferDistance` **[V]**, surface to surface, metres.
pub const TRANSFER_RANGE: f64 = eve_server::ship_config::MAX_TRANSFER_DISTANCE;
/// Inside this gap the miner drops **[A]**: a fifth of the range spare for
/// the boss's drift between the bot's read and the server's. Falsified by
/// `TooFar` refusals at the drop (too thin) or long approaches (too thick).
pub const DROP_MARGIN: f64 = 500.0;
/// The `Follow` range towards the boss's ship **[A]**: well inside the drop
/// gap, so an approach that ends there is in range. Falsified by bumping
/// (too close) or a miner that stops outside [`TRANSFER_RANGE`] -
/// [`DROP_MARGIN`].
pub const FOLLOW_RANGE: f64 = 1_000.0;
/// How long the approach may take before the miner gives up and goes home
/// **[A]**: a slow barge crosses a belt grid in a couple of minutes.
/// Falsified by miners that give up beside a boss still moving away, or
/// chase him for ever.
pub const DROP_TIMEOUT: Duration = Duration::from_secs(4 * 60);
/// After a refused or abandoned drop, no new try before this **[A]**: the
/// owner's switch or the bay's room does not change every trip. Falsified by
/// a miner that goes home with a fleet hangar open and empty beside it.
pub const DROP_RETRY: Duration = Duration::from_secs(20 * 60);

/// What the miner remembers of its fleet-hangar drops.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drop {
    /// A drop is under way: since when (the hold-full stop).
    pub since: Option<Instant>,
    /// The last refused or abandoned drop.
    pub refused_at: Option<Instant>,
    /// Stacks delivered, this run.
    pub stacks: u32,
}

/// The boss's ship, when the bot is in a fleet with a boss and his ship is
/// in the park.
fn boss_ship<'a>(o: &Observation, park: &'a ParkView) -> Option<&'a Ball> {
    o.fleet.and_then(|f| f.boss)?;
    park.boss.as_ref()
}

/// **Hold full: start a drop?** (pure). True, and the drop is under way,
/// when the boss's ship is in the park, the holds carry ore and no drop was
/// refused inside [`DROP_RETRY`].
pub fn begin(m: &mut Memory, o: &Observation) -> bool {
    let Some(park) = &o.park else {
        return false;
    };
    if boss_ship(o, park).is_none() || !has_ore(o) {
        return false;
    }
    if m.hangar_drop
        .refused_at
        .is_some_and(|at| o.now.saturating_duration_since(at) < DROP_RETRY)
    {
        return false;
    }
    m.hangar_drop.since = Some(o.now);
    true
}

fn has_ore(o: &Observation) -> bool {
    o.cargo
        .iter()
        .chain(o.ore_hold.iter())
        .any(|r| r.category_id == brain::CATEGORY_ASTEROID && !r.singleton)
}

/// Give the drop up: home, and no new try for [`DROP_RETRY`].
pub fn give_up(m: &mut Memory, now: Instant) {
    m.hangar_drop.since = None;
    m.hangar_drop.refused_at = Some(now);
    m.homing = Some(Homing::HoldFull);
}

/// **A drop under way: this turn's act** (pure). `None` when no drop is
/// under way, or when it was given up this turn ([`give_up`]: the caller
/// sees `m.homing` and goes home).
pub fn step(m: &mut Memory, o: &Observation, park: &ParkView) -> Option<Vec<Act>> {
    let since = m.hangar_drop.since?;
    let boss = match boss_ship(o, park) {
        Some(b) if has_ore(o) => *b,
        Some(_) => {
            // Nothing left to drop: done, back to the rock.
            m.hangar_drop.since = None;
            return None;
        }
        None => {
            give_up(m, o.now);
            return None;
        }
    };
    if o.now.saturating_duration_since(since) > DROP_TIMEOUT {
        give_up(m, o.now);
        return None;
    }
    if park.ego.surface_to(&boss) > TRANSFER_RANGE - DROP_MARGIN {
        if park.ego_tether == Some(boss.id) {
            return Some(vec![Act::Wait(brain::SHORT_WAIT)]);
        }
        return Some(vec![Act::Follow {
            id: boss.id,
            range: FOLLOW_RANGE,
        }]);
    }
    Some(vec![Act::Ore(OreAct::Drop(boss.id))])
}

// ---- the arm (I/O) ----------------------------------------------------------

/// **Compress, then move every ore stack into `boss_ship`'s fleet hangar.**
/// A refusal is not an error: the drop is given up ([`give_up`]) and the
/// miner goes home. The stacks moved.
pub fn drop_into(c: &mut Conn, m: &mut Memory, boss_ship: i64) -> Result<u32> {
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("fleet hangar: no ship".to_owned()))?;
    // The boss's compressor is in range now, if he has one; a stack it
    // will not take goes raw [A].
    if let Ok(n) = crate::ore::compress(c) {
        m.compress.stacks += n;
    }
    let stacks: Vec<(i64, i64)> = c
        .ship_items(ship, None)?
        .iter()
        .filter(|r| {
            r.category_id == brain::CATEGORY_ASTEROID
                && !r.singleton
                && crate::ore::ORE_FLAGS.contains(&r.flag_id)
        })
        .map(|r| (r.item_id, r.quantity.max(1)))
        .collect();
    let inventory = c.open_ship(boss_ship)?;
    let mut moved = 0;
    for (item, qty) in stacks {
        let outcome = c.call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(item), PyValue::Int(ship)],
            vec![
                (PyValue::Int(qty), crate::bstr("qty")),
                (PyValue::Int(FLAG_FLEET_HANGAR), crate::bstr("flag")),
            ],
        )?;
        if outcome.refusal().is_some() {
            give_up(m, std::time::Instant::now());
            m.hangar_drop.stacks += moved;
            return Ok(moved);
        }
        moved += 1;
    }
    m.hangar_drop.since = None;
    m.hangar_drop.stacks += moved;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ItemRow;
    use crate::brain::{FleetView, Policy};

    const BOSS_CHAR: i64 = 7;
    const BOSS_SHIP: i64 = 70;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 0.0,
        }
    }

    fn ore_row() -> ItemRow {
        ItemRow {
            item_id: 11,
            type_id: 1,
            owner_id: 2,
            location_id: 1,
            flag_id: crate::FLAG_CARGO,
            quantity: 100,
            group_id: 0,
            category_id: brain::CATEGORY_ASTEROID,
            singleton: false,
        }
    }

    /// In a fleet, the boss's ship `gap` metres away, ore in the hold.
    fn obs(gap: f64) -> Observation {
        let now = Instant::now();
        let mut o = Observation::empty(now);
        o.fleet = Some(FleetView {
            fleet_id: 1,
            boss: Some(BOSS_CHAR),
        });
        o.cargo = vec![ore_row()];
        o.park = Some(ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: Some(ball(BOSS_SHIP, gap)),
            taken: Default::default(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        o
    }

    fn mem() -> Memory {
        Memory::new("m", Policy::fast())
    }

    fn turn(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
        let park = o.park.clone().unwrap();
        step(m, o, &park)
    }

    #[test]
    fn a_full_miner_flies_to_the_boss_then_drops_into_his_fleet_hangar() {
        let mut m = mem();
        let far = obs(TRANSFER_RANGE * 4.0);
        assert!(begin(&mut m, &far));
        assert_eq!(
            turn(&mut m, &far),
            Some(vec![Act::Follow {
                id: BOSS_SHIP,
                range: FOLLOW_RANGE
            }])
        );
        // Already following him: no second order.
        let mut tethered = far.clone();
        tethered.park.as_mut().unwrap().ego_tether = Some(BOSS_SHIP);
        assert_eq!(
            turn(&mut m, &tethered),
            Some(vec![Act::Wait(brain::SHORT_WAIT)])
        );
        let near = obs(0.0);
        assert_eq!(
            turn(&mut m, &near),
            Some(vec![Act::Ore(OreAct::Drop(BOSS_SHIP))])
        );
        assert!(m.homing.is_none());
    }

    /// The brain's own turn: a hold-full stop beside a boss compresses
    /// first, and the next turn flies to his ship instead of home.
    #[test]
    fn the_brain_compresses_then_flies_to_the_boss_instead_of_home() {
        let mut m = mem();
        let mut o = obs(TRANSFER_RANGE * 4.0);
        o.park.as_mut().unwrap().rocks = vec![ball(30, 5_000.0)];
        o.miner = Some(brain::ModuleInfo {
            item_id: 5,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            range: 10_000.0,
        });
        o.last_stop = Some(brain::Stop {
            seq: 1,
            reason: brain::StopReason::HoldFull,
        });
        assert_eq!(
            brain::in_space(&mut m, &o),
            vec![Act::Ore(OreAct::Compress)]
        );
        assert!(m.homing.is_none());
        assert_eq!(
            brain::in_space(&mut m, &o),
            vec![Act::Follow {
                id: BOSS_SHIP,
                range: FOLLOW_RANGE
            }]
        );
        // Without a boss's ship in the park the same stop goes home.
        let mut m = mem();
        o.park.as_mut().unwrap().boss = None;
        brain::in_space(&mut m, &o);
        assert_eq!(m.homing, Some(Homing::HoldFull));
    }

    #[test]
    fn just_outside_the_transfer_range_is_not_a_drop() {
        let mut m = mem();
        let o = obs(TRANSFER_RANGE + 1.0);
        assert!(begin(&mut m, &o));
        assert!(matches!(
            turn(&mut m, &o).as_deref(),
            Some([Act::Follow { .. }])
        ));
    }

    #[test]
    fn no_fleet_no_boss_ship_or_no_ore_is_no_drop() {
        let mut m = mem();
        let mut o = obs(0.0);
        o.fleet = None;
        assert!(!begin(&mut m, &o));
        let mut o = obs(0.0);
        o.park.as_mut().unwrap().boss = None;
        assert!(!begin(&mut m, &o));
        let mut o = obs(0.0);
        o.cargo.clear();
        assert!(!begin(&mut m, &o));
        assert_eq!(turn(&mut m, &obs(0.0)), None, "nothing under way");
    }

    #[test]
    fn a_boss_who_leaves_the_grid_sends_the_miner_home_and_blocks_a_retry() {
        let mut m = mem();
        let o = obs(0.0);
        assert!(begin(&mut m, &o));
        let mut gone = o.clone();
        gone.park.as_mut().unwrap().boss = None;
        assert_eq!(turn(&mut m, &gone), None);
        assert_eq!(m.homing, Some(Homing::HoldFull));
        assert!(m.hangar_drop.since.is_none());
        assert!(!begin(&mut m, &o), "refused a moment ago: no retry yet");
    }

    #[test]
    fn an_approach_that_takes_too_long_is_given_up() {
        let mut m = mem();
        let o = obs(TRANSFER_RANGE * 4.0);
        assert!(begin(&mut m, &o));
        let mut later = o.clone();
        later.now = o.now + DROP_TIMEOUT + Duration::from_secs(1);
        assert_eq!(turn(&mut m, &later), None);
        assert_eq!(m.homing, Some(Homing::HoldFull));
    }

    #[test]
    fn an_empty_hold_ends_the_drop_without_going_home() {
        let mut m = mem();
        let o = obs(0.0);
        assert!(begin(&mut m, &o));
        let mut empty = o.clone();
        empty.cargo.clear();
        assert_eq!(turn(&mut m, &empty), None);
        assert!(m.homing.is_none());
        assert!(m.hangar_drop.since.is_none());
    }
}
