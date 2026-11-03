//! Asteroid belts: rats on the world clock.
//!
//! Moved verbatim out of `crate::dungeon` by W10.5 seam S3
//! (`docs/content/plan.md` §6.1); **belts v2** is W14 packet CP30 (lane note
//! `docs/lanes/cp30.md`, spec C3 §8 and P-C3e, C6 R4). The state lives in
//! `npc::Npcs` (`Npcs::belts`), and `npc::tick` runs [`tick_belts`] first. A
//! spawn is announced through the pockets' own `dungeon::announce`.
//!
//! What CP30 changed:
//!
//! - **Whose rats.** A spawn is `npc_gen::belt_squad_in` (CP22, C6 R4): the
//!   system's loaded pirate (`npc::pirate_of`, CP20 R3), its band's budget
//!   and class window, drawn from the world seed. With no bundle no real
//!   system has a loaded pirate and the squad is CP22's hand squad, the
//!   Angels this file spawned before -- 2-3 frigates in a 1.0 system.
//! - **Where it runs.** Only in the systems on the liveness hot list
//!   (`liveness::Liveness::hot`, CP6), which this tick's `liveness` stage
//!   wrote: a pilot in space, or rats still in space, makes a system hot.
//! - **When it decides.** A spawn decision is not re-taken inside
//!   [`DECISION_CADENCE`].
//! - **What persists.** A cleared belt's respawn wait is a `beltwait` delta
//!   ([`KIND`]), so a restart does not refill a belt its pilot just cleared.
//!   The rats themselves are **not persisted**, like every NPC.

use crate::damage::BlueTime;
use crate::dungeon::{Turn, announce, audience};
use crate::npc::{self, NpcType};
use crate::service::Registry;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Asteroid belts: rats on the world clock
// ---------------------------------------------------------------------------
//
// **A belt is a pocket that fills itself.** No mission, no GM command: a
// pilot in a belt gets a squad of the region's pirates round the belt
// centre; a belt nobody is in empties; a belt whose rats were all killed
// stays empty for [`RESPAWN_AFTER`], then fills again for the next pilot.
// The timing rules here are the server's -- the client ships no spawn
// tables -- so each is `[A]`; which ships a squad holds is CP22's policy over
// the client's rows, and what the rats *are* (hull, gun, bounty, wreck) is
// the client's, through `npc::find` and `npc::derive_loaded`.

/// The delta kind this module owns (`delta_kinds::KINDS`, plan §4.2):
/// `belt_item_id → respawn_at` (blue FILETIME). A row exists while a cleared
/// belt waits; it goes when the belt fills again, or at the rollover after
/// its time has passed ([`on_rollover`]).
pub const KIND: &str = "beltwait";

/// `[A]` **How close to a belt's centre a pilot is "in the belt".** A warp to
/// a belt lands `space_state::WARP_STANDOFF` (1 km) from its centre, and the
/// rocks lie in a field centred `asteroid::FIELD_OFFSET_FRACTION` of its outer
/// radius off it, out to `asteroid::outer_radius` -- about 68 km for the
/// largest belt in this build, so up to 95 km from the belt point and more
/// after a crowded belt grows -- so 120 km covers a pilot mining anywhere in
/// it. Falsified by a rock more than 120 km from its belt point. Belts in one
/// system are AU apart, so no pilot is in two.
pub const BELT_RANGE: f64 = 120_000.0;

/// `[A]` How far from the belt centre a spawn's rats start: inside the
/// rocks' inner edge (`asteroid::INNER_RADIUS`, 10 km), so no rat starts
/// inside a rock, and within `npc::AGGRESSION_RANGE` of a pilot who has just
/// landed at the standoff -- the rats notice whoever brought them.
pub const BELT_SPAWN_RING: f64 = 8_000.0;

// The ring is clear of the rocks, clear of the landing point, and close
// enough to it that the spawn wakes up. Checked when the crate compiles.
const _: () = assert!(
    BELT_SPAWN_RING < crate::asteroid::INNER_RADIUS
        && BELT_SPAWN_RING > crate::space_state::WARP_STANDOFF
        && BELT_SPAWN_RING + crate::space_state::WARP_STANDOFF < npc::AGGRESSION_RANGE
        && BELT_SPAWN_RING < BELT_RANGE
);

/// Blue time is 10,000 ticks to the millisecond.
const BLUE_PER_SECOND: BlueTime = 10_000_000;

/// `[A]` How long a belt with rats in it may stand empty of pilots before
/// they go: long enough that warping off to a station and back, or a
/// disconnect, does not reset the fight; short enough that an abandoned belt
/// is not a world full of idle balls. Plan §5 keeps it at 60 s (the review's
/// re-check, `plan-review.md` §5); C3 §8.3's 900 s is not taken.
pub const VACANT_DESPAWN: BlueTime = 60 * BLUE_PER_SECOND;

/// `[A]` How long a belt whose rats were all killed stays empty. Then the
/// next pilot in it gets a fresh group.
pub const RESPAWN_AFTER: BlueTime = 5 * 60 * BLUE_PER_SECOND;

/// `[I]` **A belt takes a spawn decision at most once in this long** (C3
/// §8.3, P-C3e): `reactiveSpawnComponent.sensorUpdateTimeSeconds` is 60 on
/// 776 of 882 spawners **[V-data]**. Kill and vacancy bookkeeping stay per
/// tick. A decision is a fill: the squad drawn and spawned. A belt that
/// waits on [`RESPAWN_AFTER`] has not decided, so it fills on the first tick
/// its wait has passed. Falsified by a belt that refills visibly late, or a
/// spawn storm when a fill yields no ship (no `--typedata` for a loaded
/// type).
pub const DECISION_CADENCE: BlueTime = 60 * BLUE_PER_SECOND;

// The cadence never delays a fill the other rules allow: a cleared belt waits
// longer than it, and abandoned rats go no sooner than it. Checked when the
// crate compiles.
const _: () = assert!(DECISION_CADENCE <= RESPAWN_AFTER && DECISION_CADENCE <= VACANT_DESPAWN);

/// One belt's rats.
#[derive(Debug, Clone, PartialEq)]
pub struct Belt {
    pub belt_id: i64,
    pub solar_system_id: i64,
    pub centre: [f64; 3],
    /// The NPCs of the current spawn that are still in space. Empty: no rats.
    pub npc_ids: Vec<i64>,
    /// How many times this belt has spawned: the generation in the squad's
    /// seed parts `(system, belt, generation)`. Not persisted `[A]`: after a
    /// restart a belt's first squad is its first again.
    pub spawns: u64,
    /// When the last pilot left, while rats are still here.
    pub vacant_since: Option<BlueTime>,
    /// When this belt last took a spawn decision ([`DECISION_CADENCE`]).
    pub decided_at: Option<BlueTime>,
}

impl Belt {
    fn new(belt_id: i64, solar_system_id: i64, centre: [f64; 3]) -> Belt {
        Belt {
            belt_id,
            solar_system_id,
            centre,
            npc_ids: Vec::new(),
            spawns: 0,
            vacant_since: None,
            decided_at: None,
        }
    }
}

/// Every belt that has ever had a pilot in it, by belt id. Lives in
/// `npc::Npcs` (not persisted, like the NPCs it names).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Belts {
    belts: BTreeMap<i64, Belt>,
    /// Each system's belts, `(id, centre)`, read from the celestial catalogue
    /// the first time a pilot is in that system. The catalogue does not
    /// change after startup, and a whole-universe one is too big to walk
    /// four times a second.
    by_system: BTreeMap<i64, Vec<(i64, [f64; 3])>>,
    /// The loaded types a squad named that `npc::find` does not know, derived
    /// once each (`npc::derive_loaded`) and kept for the process: an `Npc`
    /// holds `&'static NpcType`.
    kinds: BTreeMap<i64, &'static NpcType>,
}

impl Belts {
    pub fn get(&self, belt_id: i64) -> Option<&Belt> {
        self.belts.get(&belt_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Belt> {
        self.belts.values()
    }
}

/// When a cleared belt may fill again: its `beltwait` row, `None` when it
/// has none (never cleared, or filled since).
pub fn respawn_at(w: &Registry, belt_id: i64) -> Option<BlueTime> {
    w.deltas()
        .of(KIND)
        .get(&key(belt_id))?
        .first()?
        .parse()
        .ok()
}

fn key(belt_id: i64) -> Vec<String> {
    vec![belt_id.to_string()]
}

/// **The rollover step** (plan §4.4, step 6): drop every `beltwait` row whose
/// time has passed at `now`. Its count is the rows dropped. A row that has
/// not passed stays, so a wait spanning downtime still holds.
pub fn on_rollover(w: &mut Registry, now: BlueTime) -> usize {
    let passed: Vec<Vec<String>> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter(|(_, v)| {
            v.first()
                .and_then(|t| t.parse::<BlueTime>().ok())
                .is_none_or(|t| t <= now)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in &passed {
        w.deltas_mut().del(KIND, k);
    }
    passed.len()
}

/// A system's belts, from the cache or (once) from the catalogue.
fn belts_of(w: &mut Registry, system: i64) -> Vec<(i64, [f64; 3])> {
    if let Some(b) = w.npcs().belts().by_system.get(&system) {
        return b.clone();
    }
    let found: Vec<(i64, [f64; 3])> = w
        .celestials()
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Belt)
        .map(|c| (c.id, c.position))
        .collect();
    w.npcs_mut()
        .belts_mut()
        .by_system
        .insert(system, found.clone());
    found
}

/// **The world clock's belt pass**, run first by `npc::tick`.
///
/// For every belt in a **hot** system (this tick's liveness list): bring the
/// system to now, say whether a pilot is within [`BELT_RANGE`], and step the
/// belt ([`step`]). A belt with rats keeps its system hot (its rats are
/// balls), so its vacancy clock runs with nobody there. Belts come from the
/// celestial catalogue (`--celestials`); without one there are none and this
/// does nothing.
pub fn tick_belts(w: &mut Registry, now: BlueTime) {
    let systems: Vec<i64> = w.ext::<crate::liveness::Liveness>().hot().collect();
    for system in systems {
        // TICK-1: positions only matter to a pilot. A system with none is
        // not carried to now (the closed form composes, so the next reader
        // finds the same positions); one with a pilot is, as before.
        let mut pilots = pilots_in(w, system);
        if !pilots.is_empty() {
            w.advance_system(system);
            pilots = pilots_in(w, system);
        }
        let mut belts = belts_of(w, system);
        for b in w.npcs().belts().iter() {
            if b.solar_system_id == system && !belts.iter().any(|(id, _)| *id == b.belt_id) {
                belts.push((b.belt_id, b.centre));
            }
        }
        for (belt_id, centre) in belts {
            let occupied = pilots
                .iter()
                .any(|p| crate::space_state::distance(*p, centre) <= BELT_RANGE);
            let known = w.npcs().belts().get(belt_id).is_some();
            if !occupied && !known {
                // Never visited, nobody here: nothing to keep.
                continue;
            }
            step(w, belt_id, system, centre, occupied, now);
        }
    }
}

/// Where every **character** in a system is: the roster less its NPCs and
/// drones.
fn pilots_in(w: &Registry, solar_system_id: i64) -> Vec<[f64; 3]> {
    w.roster()
        .in_system(solar_system_id)
        .into_iter()
        .filter(|p| w.character(p.character_id).is_some())
        .map(|p| p.motion.position)
        .collect()
}

/// One belt, one tick:
///
/// 1. forget the rats that have left space -- **all of them gone by the
///    roster, not by us, is a cleared belt**, which writes its `beltwait`
///    row, [`RESPAWN_AFTER`] ahead;
/// 2. a pilot here, no rats, the wait passed and no decision inside
///    [`DECISION_CADENCE`]: spawn ([`fill`]) and drop the row;
/// 3. rats here and no pilot for [`VACANT_DESPAWN`]: they go ([`empty`]),
///    with no respawn wait -- nobody cleared them.
fn step(
    w: &mut Registry,
    belt_id: i64,
    system: i64,
    centre: [f64; 3],
    occupied: bool,
    now: BlueTime,
) {
    let mut b = w
        .npcs()
        .belts()
        .get(belt_id)
        .cloned()
        .unwrap_or_else(|| Belt::new(belt_id, system, centre));
    let had_rats = !b.npc_ids.is_empty();
    b.npc_ids.retain(|id| w.roster().get(*id).is_some());
    if had_rats && b.npc_ids.is_empty() {
        let at = now.saturating_add(RESPAWN_AFTER);
        w.deltas_mut().put(KIND, key(belt_id), vec![at.to_string()]);
        b.vacant_since = None;
    }
    if occupied {
        b.vacant_since = None;
        let waited = respawn_at(w, belt_id).is_none_or(|t| now >= t);
        let due = b
            .decided_at
            .is_none_or(|t| now.saturating_sub(t) >= DECISION_CADENCE);
        if b.npc_ids.is_empty() && waited && due {
            b.npc_ids = fill(w, &b);
            b.spawns += 1;
            b.decided_at = Some(now);
            w.deltas_mut().del(KIND, &key(belt_id));
        }
    } else if !b.npc_ids.is_empty() {
        let since = *b.vacant_since.get_or_insert(now);
        if now.saturating_sub(since) >= VACANT_DESPAWN {
            empty(w, system, &b.npc_ids);
            b.npc_ids.clear();
            b.vacant_since = None;
        }
    }
    w.npcs_mut().belts_mut().belts.insert(belt_id, b);
}

/// The ship a squad member names: the hand row or the Angel table
/// (`npc::find`), else the loaded rows over the content's typedata
/// (`typedata::installed_in`, `npc::derive_loaded`), derived once and cached
/// in [`Belts`]. `None` for a type neither knows.
fn kind_of(w: &mut Registry, type_id: i64) -> Option<&'static NpcType> {
    if let Some(k) = npc::find(type_id) {
        return Some(k);
    }
    if let Some(k) = w.npcs().belts().kinds.get(&type_id) {
        return Some(*k);
    }
    let td = crate::typedata::installed_in(w.content()).table();
    let made = npc::derive_loaded(
        td.get(type_id)?,
        td,
        w.content().get::<crate::npc_data::Table>(),
    )?;
    let k: &'static NpcType = Box::leak(Box::new(made));
    w.npcs_mut().belts_mut().kinds.insert(type_id, k);
    Some(k)
}

/// **Spawn one squad round the belt centre** and tell the system, in one
/// `AddBalls2`, on the clock. The squad is `npc_gen::belt_squad_in` for this
/// system, belt and generation: the system's pirate, or the hand squad with
/// no loaded one. Spaced evenly on a horizontal ring of [`BELT_SPAWN_RING`],
/// turned a little per spawn. A member whose ship is unknown
/// ([`kind_of`]) is skipped.
fn fill(w: &mut Registry, b: &Belt) -> Vec<i64> {
    let squad = crate::npc_gen::belt_squad_in(w, b.solar_system_id, &[b.belt_id as u64, b.spawns]);
    let phase = std::f64::consts::TAU * npc::roll(b.belt_id, b.spawns.wrapping_mul(16) + 1);
    let n = squad.members.len().max(1);
    let mut ids = Vec::new();
    for (i, type_id) in squad.members.iter().enumerate() {
        let Some(kind) = kind_of(w, *type_id) else {
            continue;
        };
        let angle = phase + std::f64::consts::TAU * i as f64 / n as f64;
        let at = [
            b.centre[0] + BELT_SPAWN_RING * angle.cos(),
            b.centre[1],
            b.centre[2] + BELT_SPAWN_RING * angle.sin(),
        ];
        // The squad warps in from outside the ring, along its own bearing.
        let out = [angle.cos(), 0.0, angle.sin()];
        match npc::spawn_warping_in(w, kind, b.solar_system_id, at, out) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    if !ids.is_empty() {
        announce(w, b.solar_system_id, &ids, Turn::Clock);
    }
    ids
}

/// **One extra squad at a belt, on demand** (lane l6b-mining-events): the
/// system's pirate squad, drawn for `(belt, salt)` instead of the belt's own
/// generation, spawned on the same ring and announced on the clock. The
/// belt's own rats and its respawn wait are not touched; the caller owns the
/// ids it gets back and removes them with [`clear_wave`]. Empty when no
/// member's ship is known.
pub fn wave(w: &mut Registry, system: i64, belt_id: i64, centre: [f64; 3], salt: u64) -> Vec<i64> {
    let mut b = Belt::new(belt_id, system, centre);
    // Past every generation a belt reaches on its own, so the draw is its own.
    b.spawns = salt | (1 << 40);
    fill(w, &b)
}

/// Take a [`wave`]'s rats back out of space, the way a vacated belt's go.
pub fn clear_wave(w: &mut Registry, system: i64, ids: &[i64]) {
    let live: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| w.roster().get(*id).is_some())
        .collect();
    if !live.is_empty() {
        empty(w, system, &live);
    }
}

/// A system's belts and their centres, from the catalogue.
pub fn belts_in(w: &mut Registry, system: i64) -> Vec<(i64, [f64; 3])> {
    belts_of(w, system)
}

/// **Rats leave a belt nobody is in**: out of every lock graph (anyone who
/// still had one locked is told, `'Unlocked'`), out of the roster and the
/// NPC table, and one `RemoveBalls` to the system's characters -- the plural
/// form, a ball that left rather than one that exploded (`destiny::BallsRemoved`).
fn empty(w: &mut Registry, system: i64, ids: &[i64]) {
    for id in ids {
        crate::targeting::left_space(
            w,
            *id,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::OnTheClock,
        );
        w.roster_mut().leave(*id);
        w.npcs_mut().remove(*id);
    }
    let to = audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, ids.to_vec()) {
        w.schedule(std::time::Duration::ZERO, gone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pilot anywhere in the largest field, offset included, is in range.
    #[test]
    fn belt_range_covers_the_largest_field() {
        let outer = crate::asteroid::outer_radius(crate::asteroid::IDS_PER_BELT as usize);
        assert!(outer * (1.0 + crate::asteroid::FIELD_OFFSET_FRACTION) < BELT_RANGE);
    }
    use eve_marshal::PyValue;

    // ---- Belt rats ---------------------------------------------------------

    use crate::space_state::Motion;
    use std::time::{Duration, Instant};

    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const SYSTEM: i64 = 30_000_001;
    const BELT: i64 = 40_000_003;
    const OTHER_BELT: i64 = 40_000_006;
    const BELT_AT: [f64; 3] = [1.0e11, 0.0, 0.0];
    const OTHER_AT: [f64; 3] = [-2.0e11, 0.0, 0.0];

    /// Alice in a Rifter, in space in a system with two belts (positions
    /// invented, not the client's), every push drained.
    fn belt_world() -> Registry {
        world_with("")
    }

    /// [`belt_world`], with `extra` catalogue lines before the belts.
    fn world_with(extra: &str) -> Registry {
        let celestials = crate::celestial::Catalogue::parse(&format!(
            "{extra}belt\t{BELT}\t15\t0.0\t{}\t{}\t{}\t{SYSTEM}\n\
             belt\t{OTHER_BELT}\t15\t0.0\t{}\t{}\t{}\t{SYSTEM}\n",
            BELT_AT[0], BELT_AT[1], BELT_AT[2], OTHER_AT[0], OTHER_AT[1], OTHER_AT[2],
        ));
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ship_type_id: 587,
                ..crate::model::Character::default()
            }],
            celestials,
            ..crate::model::SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
        reg
    }

    /// One world-clock tick of what this module reads: the `liveness`
    /// stage, which runs first, then the belt pass.
    fn run(reg: &mut Registry, now: BlueTime) {
        crate::liveness::refresh(reg, Instant::now(), now);
        tick_belts(reg, now);
    }

    /// Put Alice at rest at `at`.
    fn put_alice(reg: &mut Registry, at: [f64; 3]) {
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion = Motion::arrived(at, [0.0; 3]);
    }

    /// Where a warp to the belt lands: the standoff short of its centre.
    fn landed() -> [f64; 3] {
        [
            BELT_AT[0] - crate::space_state::WARP_STANDOFF,
            BELT_AT[1],
            BELT_AT[2],
        ]
    }

    fn rats(reg: &Registry) -> Vec<i64> {
        reg.npcs()
            .belts()
            .get(BELT)
            .map(|b| b.npc_ids.clone())
            .unwrap_or_default()
    }

    /// Everything on the clock, however far ahead, as `(audience, name)`.
    fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// One volley nothing survives, from Alice.
    fn kill(reg: &mut Registry, id: i64, now: BlueTime) {
        let victim = reg.roster().get(id).copied().expect("in space");
        let shot = crate::damage::Shot {
            at: now,
            attacker_ship_id: ALICE_SHIP,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: id,
            damage: crate::damage::Damage {
                em: 1.0e7,
                explosive: 1.0e7,
                kinetic: 1.0e7,
                thermal: 1.0e7,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, ALICE, victim, now);
        assert!(reg.roster().get(id).is_none(), "{id} died");
    }

    /// **A pilot in a belt gets rats; the belt they are not in gets none.**
    /// Every rat is the system faction's, of a size and type its lists name,
    /// on the spawn ring round the centre; one `AddBalls2` names them all,
    /// on the clock, to the pilot -- nothing waits on `pending`.
    #[test]
    fn a_pilot_in_a_belt_gets_rats_and_the_other_belt_none() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let now = crate::pyutil::filetime_now();
        run(&mut reg, now);
        let ids = rats(&reg);
        assert!(!ids.is_empty(), "rats");
        assert!(ids.len() <= crate::npc_gen::BELT_SQUAD_SIZE.1 as usize);
        assert!(reg.npcs().belts().get(OTHER_BELT).is_none());
        let faction = npc::faction_for(SYSTEM);
        let allowed: Vec<i64> = [faction.frigates, faction.destroyers, faction.cruisers].concat();
        for id in &ids {
            let n = reg.npcs().get(*id).expect("an NPC");
            assert!(
                allowed.contains(&n.kind.type_id),
                "{} is listed",
                n.kind.type_id
            );
            assert_eq!(n.kind.owner_corp_id, faction.owner_corp_id);
            assert_eq!(n.solar_system_id, SYSTEM);
            // It flies in: the ring is where the warp ends.
            let m = reg.roster().get(*id).expect("in space").motion;
            let crate::destiny::Mode::Warp {
                goto,
                total_distance,
                effect_stamp,
                ..
            } = m.mode
            else {
                panic!("{id} is not warping in: {:?}", m.mode);
            };
            assert!(effect_stamp >= 0, "the warp is active, not aligning");
            let d = crate::space_state::distance(goto, BELT_AT);
            assert!((d - BELT_SPAWN_RING).abs() < 1.0, "on the ring: {d}");
            let from = crate::space_state::distance(m.position, goto);
            assert!(
                (from - npc::WARP_IN_DISTANCE).abs() < 1.0,
                "starts out: {from}"
            );
            assert!((total_distance - npc::WARP_IN_DISTANCE).abs() < 1.0);
        }
        assert!(reg.take_pending().is_empty(), "the clock is not a turn");
        let out = due(&mut reg);
        let added: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .collect();
        assert_eq!(added.len(), 1, "one batch");
        assert_eq!(added[0].0, vec![ALICE]);
        // A second tick with the rats alive spawns nothing more.
        run(&mut reg, now);
        assert_eq!(rats(&reg), ids);
        assert_eq!(reg.npcs().len(), ids.len());
    }

    /// **A rat flying in has no brain until it lands**, and lands parked on
    /// its ring point: the warp-in mode goes to the client in the
    /// `AddBalls2` (the client's `effects.WarpIn` trigger), the NPC plans
    /// nothing meanwhile, and a later tick finds it in `Stop` at the ring.
    #[test]
    fn a_rat_warps_in_idle_and_parks_on_the_ring() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let now = crate::pyutil::filetime_now();
        run(&mut reg, now);
        let ids = rats(&reg);
        assert!(!ids.is_empty());
        let _ = due(&mut reg);
        for id in &ids {
            assert!(reg.npcs().get(*id).expect("an NPC").target.is_none());
        }
        // Far into the future the flight is over: parked, on the ring.
        crate::test_clock::freeze(std::time::Duration::from_secs(
            crate::space_state::now_seconds() as u64 + 600,
        ));
        reg.advance_system(SYSTEM);
        for id in &ids {
            let m = reg.roster().get(*id).expect("in space").motion;
            assert!(matches!(m.mode, crate::destiny::Mode::Stop), "{:?}", m.mode);
            let d = crate::space_state::distance(m.position, BELT_AT);
            assert!((d - BELT_SPAWN_RING).abs() < 1.0, "on the ring: {d}");
        }
        crate::test_clock::thaw();
    }

    /// Nobody in a belt, nothing in it; and no catalogue, no belts.
    #[test]
    fn a_belt_nobody_visits_stays_empty() {
        let mut reg = belt_world();
        // Alice stays by the station, far from both belts.
        run(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.npcs().is_empty());
        assert_eq!(reg.npcs().belts().iter().count(), 0);
        assert!(due(&mut reg).is_empty());
    }

    /// **A cleared belt has no rats** -- not on the next tick, not one tick
    /// before its respawn -- and fills again for a pilot once the respawn
    /// time has passed.
    #[test]
    fn a_cleared_belt_has_no_rats_until_it_respawns() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let t0 = crate::pyutil::filetime_now();
        run(&mut reg, t0);
        let first = rats(&reg);
        assert!(!first.is_empty());
        for id in &first {
            kill(&mut reg, *id, t0);
        }
        run(&mut reg, t0);
        assert!(rats(&reg).is_empty(), "cleared");
        assert!(first.iter().all(|id| reg.roster().get(*id).is_none()));
        assert!(reg.npcs().belts().get(BELT).is_some(), "known");
        assert_eq!(respawn_at(&reg, BELT), Some(t0 + RESPAWN_AFTER));
        run(&mut reg, t0 + RESPAWN_AFTER - 1);
        assert!(rats(&reg).is_empty(), "still cleared");
        assert!(
            reg.npcs().iter().all(|n| !first.contains(&n.id)),
            "the dead stay dead"
        );
        run(&mut reg, t0 + RESPAWN_AFTER);
        let second = rats(&reg);
        assert!(!second.is_empty(), "respawned");
        assert!(second.iter().all(|id| !first.contains(id)), "new ids");
    }

    /// **Rats leave a belt nobody is in** -- not at once, after
    /// [`VACANT_DESPAWN`] -- with one `RemoveBalls` to the system's pilots
    /// and nothing left in the roster or the NPC table. Nobody cleared
    /// them, so a returning pilot gets a group at once.
    #[test]
    fn an_abandoned_belt_loses_its_rats_and_refills_at_once() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let t0 = crate::pyutil::filetime_now();
        run(&mut reg, t0);
        let ids = rats(&reg);
        let _ = due(&mut reg);
        put_alice(&mut reg, [0.0; 3]);
        run(&mut reg, t0);
        run(&mut reg, t0 + VACANT_DESPAWN - 1);
        assert_eq!(rats(&reg), ids, "not yet");
        run(&mut reg, t0 + VACANT_DESPAWN);
        assert!(rats(&reg).is_empty());
        assert!(ids.iter().all(|id| reg.roster().get(*id).is_none()));
        assert!(ids.iter().all(|id| reg.npcs().get(*id).is_none()));
        let out = due(&mut reg);
        let gone = out
            .iter()
            .find(|(_, n, _)| *n == "DoDestinyUpdate")
            .expect("a removal batch");
        assert_eq!(gone.0, vec![ALICE]);
        assert_eq!(respawn_at(&reg, BELT), None);
        put_alice(&mut reg, landed());
        run(&mut reg, t0 + VACANT_DESPAWN);
        assert!(!rats(&reg).is_empty(), "back at once");
    }

    /// A pilot who leaves and comes back inside the grace keeps the same
    /// fight.
    #[test]
    fn a_short_absence_keeps_the_same_rats() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let t0 = crate::pyutil::filetime_now();
        run(&mut reg, t0);
        let ids = rats(&reg);
        put_alice(&mut reg, [0.0; 3]);
        run(&mut reg, t0);
        put_alice(&mut reg, landed());
        run(&mut reg, t0 + VACANT_DESPAWN - 1);
        put_alice(&mut reg, [0.0; 3]);
        run(&mut reg, t0 + VACANT_DESPAWN);
        assert_eq!(rats(&reg), ids, "the clock restarted when she came back");
    }

    // ---- CP30: belts v2 ---------------------------------------------------

    /// `npc_data::Table::fixture()`'s region with one pirate row (invented).
    const FIXTURE_REGION: i64 = 19_000_101;
    /// That pirate's base asteroid frigate in the fixture (invented).
    const FIXTURE_FRIGATE: i64 = 990_300_001;

    /// **A system's rats are of its pirate** (`npc::pirate_of`, CP20 R3): a
    /// system in a region the loaded rows give a pirate gets a squad whose
    /// every rat is that faction's type. The fixture type has no typedata
    /// here, so its ship is handed to the cache [`kind_of`] reads.
    #[test]
    fn a_systems_rats_are_of_its_pirate() {
        let mut reg = world_with(&format!("#system\t{SYSTEM}\t1\t{FIXTURE_REGION}\n"));
        let pirate = reg
            .content()
            .get::<crate::npc_data::Table>()
            .region_pirate(FIXTURE_REGION);
        assert!(pirate.is_some(), "the fixture region has a pirate");
        assert_eq!(npc::pirate_of(&reg, SYSTEM), pirate);
        let kind: &'static NpcType = Box::leak(Box::new(NpcType {
            type_id: FIXTURE_FRIGATE,
            ..npc::ANGEL_HIJACKER
        }));
        reg.npcs_mut()
            .belts_mut()
            .kinds
            .insert(FIXTURE_FRIGATE, kind);
        put_alice(&mut reg, landed());
        run(&mut reg, crate::pyutil::filetime_now());
        let ids = rats(&reg);
        assert!(!ids.is_empty(), "rats");
        let data = reg.content().get::<crate::npc_data::Table>();
        for id in &ids {
            let t = reg.npcs().get(*id).expect("an NPC").kind.type_id;
            assert_eq!(data.types.get(&t).map(|r| r.faction_id), pirate, "{t}");
        }
    }

    /// **The fixture content still spawns a squad**: with no bundle no real
    /// system has a loaded pirate, and the belt holds exactly the squad
    /// `npc_gen::belt_squad_in` draws for it -- the hand squad.
    #[test]
    fn the_fixture_content_still_spawns_a_squad() {
        let mut reg = belt_world();
        assert_eq!(npc::pirate_of(&reg, SYSTEM), None);
        let squad = crate::npc_gen::belt_squad_in(&reg, SYSTEM, &[BELT as u64, 0]);
        assert_eq!(squad.faction_id, None);
        put_alice(&mut reg, landed());
        run(&mut reg, crate::pyutil::filetime_now());
        let types: Vec<i64> = rats(&reg)
            .iter()
            .map(|id| reg.npcs().get(*id).expect("an NPC").kind.type_id)
            .collect();
        assert!(!types.is_empty());
        assert_eq!(types, squad.members);
    }

    /// **A spawn decision is not re-taken inside [`DECISION_CADENCE`]**: a
    /// belt that decided at `t0` and holds no rats stays empty, pilot or no,
    /// until the cadence has passed, then fills.
    #[test]
    fn a_spawn_decision_is_not_retaken_inside_the_cadence() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let t0 = crate::pyutil::filetime_now();
        let mut b = Belt::new(BELT, SYSTEM, BELT_AT);
        b.decided_at = Some(t0);
        reg.npcs_mut().belts_mut().belts.insert(BELT, b);
        run(&mut reg, t0 + DECISION_CADENCE - 1);
        assert!(rats(&reg).is_empty(), "inside the cadence");
        run(&mut reg, t0 + DECISION_CADENCE);
        assert!(!rats(&reg).is_empty(), "decided again");
        assert_eq!(
            reg.npcs().belts().get(BELT).and_then(|b| b.decided_at),
            Some(t0 + DECISION_CADENCE)
        );
    }

    /// **The pass walks the liveness hot list**: a pilot in a belt of a
    /// system the `liveness` stage has not marked hot gets nothing; once it
    /// has, she gets rats.
    #[test]
    fn a_system_off_the_hot_list_is_not_stepped() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let now = crate::pyutil::filetime_now();
        tick_belts(&mut reg, now);
        assert!(rats(&reg).is_empty());
        assert_eq!(reg.npcs().belts().iter().count(), 0);
        run(&mut reg, now);
        assert!(!rats(&reg).is_empty());
    }

    /// **A cleared belt's wait is a `beltwait` delta**, so a restart (the
    /// belt state forgotten, the deltas kept) does not refill it early; the
    /// row goes when the belt fills.
    #[test]
    fn a_cleared_belts_wait_survives_a_restart() {
        let mut reg = belt_world();
        put_alice(&mut reg, landed());
        let t0 = crate::pyutil::filetime_now();
        run(&mut reg, t0);
        for id in rats(&reg) {
            kill(&mut reg, id, t0);
        }
        run(&mut reg, t0);
        let row = reg.deltas().of(KIND).get(&key(BELT)).cloned();
        assert_eq!(row, Some(vec![(t0 + RESPAWN_AFTER).to_string()]));
        *reg.npcs_mut().belts_mut() = Belts::default();
        run(&mut reg, t0 + RESPAWN_AFTER - 1);
        assert!(rats(&reg).is_empty(), "the wait holds");
        run(&mut reg, t0 + RESPAWN_AFTER);
        assert!(!rats(&reg).is_empty(), "filled");
        assert!(reg.deltas().of(KIND).is_empty(), "the row went");
    }

    /// **The rollover drops the waits that have passed** and keeps the rest.
    #[test]
    fn the_rollover_drops_only_passed_waits() {
        let mut reg = belt_world();
        let now = crate::pyutil::filetime_now();
        reg.deltas_mut()
            .put(KIND, key(BELT), vec![(now - 1).to_string()]);
        reg.deltas_mut()
            .put(KIND, key(OTHER_BELT), vec![(now + 1).to_string()]);
        assert_eq!(on_rollover(&mut reg, now), 1);
        assert_eq!(respawn_at(&reg, BELT), None);
        assert_eq!(respawn_at(&reg, OTHER_BELT), Some(now + 1));
    }

    /// **The hand rows are gone from the belt code** (plan §6.2, CP30): no
    /// composition table, no hand faction, no hand rat outside the tests.
    #[test]
    fn belt_code_names_no_hand_rat_or_composition() {
        let src = include_str!("belts.rs");
        let code = src.split("#[cfg(test)]").next().expect("code first");
        for name in ["COMPOSITIONS", "ANGEL_", "faction_for", "Size::"] {
            assert!(!code.contains(name), "belts.rs still names `{name}`");
        }
    }

    // ---- W10.5 S3's pin ---------------------------------------------------

    /// **The belt half lives here and only here.** `dungeon.rs` neither
    /// defines a belt item nor re-exports one from this module, so the
    /// tuning consts have one home and one name (`docs/content/plan.md`
    /// §6.1, S3).
    #[test]
    fn consts_are_reexported_nowhere_else() {
        let dungeon = include_str!("dungeon.rs");
        for item in [
            "const BELT_RANGE",
            "const BELT_SPAWN_RING",
            "const BLUE_PER_SECOND",
            "const VACANT_DESPAWN",
            "const RESPAWN_AFTER",
            "const COMPOSITIONS",
            "struct Belt ",
            "struct Belts",
            "fn tick_belts",
        ] {
            assert!(!dungeon.contains(item), "dungeon.rs still has `{item}`");
        }
        let reexports = dungeon
            .lines()
            .map(str::trim_start)
            .filter(|l| {
                l.starts_with("use ")
                    || l.starts_with("pub use ")
                    || l.starts_with("pub(crate) use ")
            })
            .filter(|l| l.contains("belts"));
        assert_eq!(reexports.count(), 0, "dungeon.rs re-exports a belt item");
    }
}
