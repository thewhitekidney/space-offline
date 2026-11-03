//! Places in space with hostile NPCs in them: a mission pocket, and an
//! asteroid belt's rats.
//!
//! Owned by the P4.2 lane (`docs/lanes/agents-missions.md`, "P4.2"). The
//! belt half is `crate::belts` now (W10.5 seam S3, `docs/content/plan.md`
//! §6.1); it shares [`announce`] and [`audience`] with the pockets.
//!
//! **What this is, and what it is not.** A real dungeon is a room graph out
//! of `dungeons.fsdbinary` -- objects, gates, spawn rules (`docs/re/
//! dungeon-model.md` §2). This is the smallest thing a combat mission needs
//! instead: **a point in the agent's solar system and a handful of NPCs
//! round it**, spawned through `crate::npc`'s own public API so that every
//! subsystem that already handles an `/entity deploy` NPC -- the bootstrap,
//! locks, combat, the wreck -- handles these too with no new code.
//!
//! The mission still names a **real** `dungeonID` (`crate::mission`), because
//! the client reads `GetDungeon(dungeonID).connections` itself
//! (`agentDialogueUtil.py:248-249`) **[V]**. The *layout* below is ours, and
//! every number in it is `[A]`: the dungeon table says where CCP's objects
//! sit but never which ships a spawn contains (`npcGroupingID` points outside
//! the corpus, `docs/re/dungeon-model.md` §5.5), so there is no composition
//! to copy.
//!
//! **Not persisted**, like the NPCs it holds: a restart empties space, and
//! the mission that pointed here is a conversation the snapshot also drops.

use crate::model::Target;
use crate::npc::{self, NpcType};
use crate::service::Registry;
use eve_marshal::PyValue;

/// `invConst.typeSolarSystem`, **[V]** `inventorycommon/const.py:1835`. A
/// pocket has no item of its own, so its bookmark is a solar-system bookmark
/// with coordinates -- the one shape `bookmarkChecker.getDistanceToActiveShip`
/// measures from `x, y, z` without calling back to the agent
/// (`bookmarkChecker.py:129-141`) **[V]**.
pub const TYPE_SOLAR_SYSTEM: i64 = 5;

/// Where the pocket is, relative to the system's origin, in metres.
///
/// `[A]` **about 2.2 AU out, off every axis**: far enough from the station
/// (which this server places at the origin, `Registry::ball_target`) that
/// reaching it is a real warp, and a point no celestial is likely to sit on.
/// Falsified by a warp that lands inside a planet or a sun, or a flight so
/// long that it reads as a hang.
pub const POCKET_OFFSET: [f64; 3] = [2.9e11, 1.0e9, 1.6e11];

/// How far from the pocket's centre each NPC starts.
///
/// `[A]` 10 km: inside `npc::AGGRESSION_RANGE` (20 km) of a ship that lands
/// at the centre, so the spawn wakes up as the player arrives, and outside
/// the 1 km a warp to the centre stops short of it.
pub const SPAWN_RING: f64 = 10_000.0;

// The ring sits inside the NPCs' own aggression range of a ship at the
// centre -- otherwise a player who warps in finds a pocket that ignores them
// -- and outside the standoff a warp to the centre stops short by. Checked
// when the crate compiles, so a change to either constant breaks the build.
const _: () =
    assert!(SPAWN_RING < npc::AGGRESSION_RANGE && SPAWN_RING > crate::space_state::WARP_STANDOFF);

/// One live pocket: where it is and who was put in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pocket {
    pub solar_system_id: i64,
    pub position: [f64; 3],
    /// Every NPC spawned here, alive or not. Cleared is "none of these is in
    /// space any more", which is the test [`cleared`] runs.
    pub npc_ids: Vec<i64>,
}

impl Pocket {
    /// Aim for a warp to the pocket: its centre, with no body to clear.
    ///
    /// `radius` 0 and no warp-in point, so the warp code's own floor --
    /// `space_state::WARP_STANDOFF` past the radius -- puts the ship 1 km
    /// short of the centre, which is inside the ring of NPCs.
    pub fn target(&self) -> Target {
        Target {
            position: self.position,
            radius: 0.0,
            warp_in: None,
        }
    }
}

/// Put `count` NPCs of `kind` round a new pocket in `solar_system_id`, tell
/// everyone already in that system, and answer the pocket.
///
/// Spaced evenly on a horizontal ring of [`SPAWN_RING`], the same geometry
/// `npc::entity_slash` uses round a player. `None` when not one NPC could be
/// spawned (the NPC id range is exhausted), so the caller never holds a
/// pocket that is cleared before it was fought.
///
/// **One `AddBalls2` for the lot**, through `npc::ball`/`npc::slim`, to the
/// characters in the system; a player who is docked when they accept gets the
/// NPCs from the ordinary bootstrap when they undock, because they are in the
/// roster like any other ball.
pub fn spawn(
    w: &mut Registry,
    kind: &'static NpcType,
    solar_system_id: i64,
    count: usize,
) -> Option<Pocket> {
    let centre = POCKET_OFFSET;
    let mut ids = Vec::new();
    for i in 0..count {
        let angle = std::f64::consts::TAU * i as f64 / count.max(1) as f64;
        let at = [
            centre[0] + SPAWN_RING * angle.cos(),
            centre[1],
            centre[2] + SPAWN_RING * angle.sin(),
        ];
        match npc::spawn(w, kind, solar_system_id, at) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    if ids.is_empty() {
        return None;
    }
    announce(w, solar_system_id, &ids, Turn::Payload);
    Some(Pocket {
        solar_system_id,
        position: centre,
        npc_ids: ids,
    })
}

/// Whose turn a batch leaves in: a payload's ([`Registry::push`], the
/// mission accept that spawns a pocket) or the world clock's
/// ([`Registry::schedule`], a belt spawning round a pilot who called
/// nothing). `npc.rs`'s header has why the two must not be mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    Payload,
    Clock,
}

/// Tell every character in the system about these NPCs, in one batch.
pub(crate) fn announce(w: &mut Registry, solar_system_id: i64, ids: &[i64], turn: Turn) {
    let now = crate::pyutil::filetime_now();
    let mut balls = Vec::new();
    let mut extra = Vec::new();
    for id in ids {
        let (Some(n), Some(p)) = (w.npcs().get(*id), w.roster().get(*id)) else {
            continue;
        };
        balls.push(npc::ball(w.content(), n, &p.motion));
        extra.push((
            npc::slim(n),
            Some(crate::combat::damage_state(w, *id, now).to_pyvalue()),
        ));
    }
    let to = audience(w, solar_system_id);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    if let Some(added) = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls,
        },
        extra,
    ) {
        match turn {
            Turn::Payload => w.push(added),
            Turn::Clock => w.schedule(std::time::Duration::ZERO, added),
        }
    }
}

/// The characters in a system: the roster less its NPCs.
pub(crate) fn audience(w: &Registry, solar_system_id: i64) -> Vec<i64> {
    w.roster()
        .in_system(solar_system_id)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// Whether every NPC put in this pocket is gone from space.
///
/// **The roster, not the NPC table**, because `combat::kill` takes a dead
/// ball out of the roster at once while `npc::tick` reaps the NPC record a
/// tick later -- and the player who just shot the last one should be able to
/// turn the mission in without waiting for the clock.
pub fn cleared(w: &Registry, pocket: &Pocket) -> bool {
    pocket
        .npc_ids
        .iter()
        .all(|id| w.roster().get(*id).is_none())
}

/// Whether any NPC of this pocket is still in space -- so a re-accepted
/// mission sends the player back to the fight they left rather than
/// spawning a second one beside it.
pub fn alive(w: &Registry, pocket: &Pocket) -> bool {
    !cleared(w, pocket)
}

/// The pocket as a `LocationWrapper` dict -- `solarsystemID`, `typeID`,
/// `locationID`, all three subscripted (`agentDialogueUtil.py:275`, `:305`)
/// **[V]**. The location is the **solar system**, which the client can always
/// name; the pocket itself has no name in `cfg.evelocations`, and an unknown
/// `locationID` raises there (`docs/re/agents-missions.md` §8.3).
pub fn location_wrap(solar_system_id: i64) -> PyValue {
    crate::pyutil::pydict(vec![
        ("solarsystemID", PyValue::Int(solar_system_id)),
        ("typeID", PyValue::Int(TYPE_SOLAR_SYSTEM)),
        ("locationID", PyValue::Int(solar_system_id)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pocket's wrapper carries the three keys `LocationWrapper`
    /// subscripts, all naming the solar system.
    #[test]
    fn the_location_wrapper_carries_its_three_subscripted_keys() {
        let v = location_wrap(30_000_001);
        for key in ["solarsystemID", "typeID", "locationID"] {
            assert!(
                crate::pyutil::dict_get(&v, key.as_bytes()).is_some(),
                "missing {key}"
            );
        }
        assert_eq!(
            crate::pyutil::dict_get(&v, b"typeID"),
            Some(&PyValue::Int(TYPE_SOLAR_SYSTEM))
        );
    }

    /// The warp aim is the centre, with no body to clear.
    #[test]
    fn a_pocket_is_aimed_at_its_centre() {
        let p = Pocket {
            solar_system_id: 30_000_001,
            position: POCKET_OFFSET,
            npc_ids: vec![],
        };
        let t = p.target();
        assert_eq!(t.position, POCKET_OFFSET);
        assert_eq!(t.warp_in, None);
    }
}
