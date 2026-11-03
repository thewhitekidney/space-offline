//! **Dungeon tracking: the client is told when a pilot is inside a site.**
//!
//! W15 packet CP36 (`docs/content/plan.md` §6.2; spec
//! `docs/content/C2-live-surface.md` §4 and its packet K5). Lane note:
//! `docs/lanes/cp36.md`.
//!
//! The objective chains run **in the client, from static data** (C2 §4 [V]:
//! `evedungeons/client/data:52-57` reads `dungeons.objectivesID`; the
//! `objectives` package makes no call). They advance from ballpark events
//! that match the slims' `dunObjectID` and `entityGroupID`, which CP28a
//! already stamps (`crate::site::slim_extras`). What the server owes is
//! where the pilot is, as three pushes to `parklife/dungeonTracking` [V]:
//!
//! - [`ENTERING`] `OnEnteringDungeonRoom(dungeon_id, room_id, room_position,
//!   instance_id, dungeon_values)`: **5 positional arguments** [V]. Sent as
//!   `(dungeonID, roomID, None, instanceID, {})`, the C2 §4 spec; `None` and
//!   an empty dict are what the handler accepts [V].
//! - [`COMPLETED`] `OnDungeonCompleted(dungeon_id)`: once per instance
//!   ([`complete`]), and to a pilot who enters an instance already complete.
//! - [`EXITING`] `OnExitingDungeon(dungeon_id)`: on warp-out.
//!
//! **Inside** ([`is_inside`]) means: in the instance's system, within
//! `site::ARRIVAL_RANGE` of its anchor (the warp-in point), and not in an
//! *activated* warp. A ship that is aligning is still inside (the client's
//! own discriminator, `effect_stamp < 0` is `IsPreparingWarp` [V], see
//! `destiny::Mode::Warp`), so a cancelled warp neither exits nor re-enters.
//! The stage ([`tick`], after `site`) compares that with what each pilot
//! was last told:
//!
//! - became inside: `OnEnteringDungeonRoom` (plus `OnDungeonCompleted` if
//!   the instance is done);
//! - stopped being inside while still in the system (warp-out, or the
//!   instance was vacated): `OnExitingDungeon`;
//! - left the system (jump, dock, logoff): dropped **silently**, because a
//!   `locationid` change already clears the client's tracker (C2 §4 [V]).
//!
//! **Only the entry room** exists (CP28a places room 0 only), so `room_id`
//! is the instance's `room_id`. An instance with no room (an unknown or empty
//! dungeon) is never entered: there is no objective chain to start.
//!
//! **Completion is CP28b's to decide** ("when the last spawn group dies",
//! C2 §4). It calls [`complete`] *before* `site::vacate`; a second call for
//! the same instance does nothing, so completion fires once.
//!
//! **Not persisted** (like `site`): the instance ids are not stable across
//! a restart, and after a relog the client's tracker is gone anyway
//! (`keeper.GetCurrentDungeonForCharacter`, `:92-96` [V]; [`current`] is
//! that answer, unrouted).

use crate::content::Content;
use crate::damage::BlueTime;
use crate::service::Registry;
use crate::site::{ARRIVAL_RANGE, Sites};
use crate::space_state::Motion;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Builds empty and reads no deltas (nothing here is persisted).
pub const SLOT: SlotSpec = SlotSpec {
    name: "dungeon_tracking",
    make,
};

fn make(_: &Content) -> Boxed {
    Box::new(Tracking::default())
}

/// `dungeonTracking.OnEnteringDungeonRoom` [V name, C2 §4].
pub const ENTERING: &str = "OnEnteringDungeonRoom";
/// `dungeonTracking.OnDungeonCompleted` [V name, C2 §4].
pub const COMPLETED: &str = "OnDungeonCompleted";
/// `dungeonTracking.OnExitingDungeon` [V name, C2 §4].
pub const EXITING: &str = "OnExitingDungeon";

/// One instance, as its pilots know it. The `seq` tells a re-instantiation
/// of the same site apart from the old instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Visit {
    pub site_id: i64,
    pub seq: u64,
    pub system: i64,
    pub dungeon_id: i64,
    pub room_id: i64,
}

impl Visit {
    fn key(&self) -> (i64, u64) {
        (self.site_id, self.seq)
    }
}

/// The world slot.
#[derive(Debug, Default)]
pub struct Tracking {
    /// Character to the instance it was last told it entered.
    inside: BTreeMap<i64, Visit>,
    /// Instances `(site_id, seq)` that are complete.
    done: BTreeSet<(i64, u64)>,
}

impl Tracking {
    /// The instance `character` is inside, as last pushed.
    #[cfg(test)]
    pub fn visit(&self, character: i64) -> Option<&Visit> {
        self.inside.get(&character)
    }
}

/// One push, to a list of characters.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonPush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for DungeonPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The arguments of [`ENTERING`]: `(dungeonID, roomID, None, instanceID,
/// {})`, five positional [V] (C2 §4). The instance id is the site id, as in
/// the anomaly's `instanceID` (C2 §2.2 [V]).
pub fn entering_args(v: &Visit) -> Vec<PyValue> {
    vec![
        PyValue::Int(v.dungeon_id),
        PyValue::Int(v.room_id),
        PyValue::None,
        PyValue::Int(v.site_id),
        PyValue::Dict(Vec::new()),
    ]
}

/// A ship in an **activated** warp: the stamp is the park tick of
/// activation, it has come due (`Motion::warp_active`). A ship still
/// aligning is in ordinary flight.
pub fn in_warp(m: &Motion) -> bool {
    m.warp_active()
}

/// Inside an instance anchored at `anchor`: within [`ARRIVAL_RANGE`] and
/// not in an activated warp (the caller has checked the system).
pub fn is_inside(m: &Motion, anchor: [f64; 3]) -> bool {
    !in_warp(m) && crate::space_state::distance(m.position, anchor) <= ARRIVAL_RANGE
}

/// **Completion** (for CP28b, before `site::vacate`): mark the live
/// instance of `site_id` complete and tell every pilot inside it. `false`
/// when the site has no instance or it is already complete, so it fires
/// once.
pub fn complete(w: &mut Registry, site_id: i64) -> bool {
    let Some((key, dungeon_id)) = w
        .ext::<Sites>()
        .get(site_id)
        .map(|i| ((i.site_id, i.seq), i.dungeon_id))
    else {
        return false;
    };
    if !w.ext_mut::<Tracking>().done.insert(key) {
        return false;
    }
    let to: Vec<i64> = w
        .ext::<Tracking>()
        .inside
        .iter()
        .filter(|(_, v)| v.key() == key)
        .map(|(c, _)| *c)
        .collect();
    send(w, COMPLETED, vec![PyValue::Int(dungeon_id)], to);
    true
}

/// `keeper.GetCurrentDungeonForCharacter()` (`dungeonTracking:92-96` [V]):
/// `(dungeon_id, room_id, instance_id, dungeon_values)` while inside, else
/// `None`. Not routed: the client's tracker is lost after a relog, and
/// `None` is safe (C2 §4).
#[cfg(test)]
pub fn current(w: &Registry, character: i64) -> PyValue {
    match w.ext::<Tracking>().visit(character) {
        Some(v) => PyValue::Tuple(vec![
            PyValue::Int(v.dungeon_id),
            PyValue::Int(v.room_id),
            PyValue::Int(v.site_id),
            PyValue::Dict(Vec::new()),
        ]),
        None => PyValue::None,
    }
}

fn send(w: &mut Registry, event: &'static str, args: Vec<PyValue>, to: Vec<i64>) {
    if !to.is_empty() {
        w.schedule(Duration::ZERO, DungeonPush { event, args, to });
    }
}

/// **The stage** (after `site`): who is inside which instance now, against
/// what each pilot was told. See the module doc for the three edges.
pub fn tick(w: &mut Registry, now: Instant, _ft: BlueTime) {
    let live: BTreeSet<(i64, u64)> = w
        .ext::<Sites>()
        .iter()
        .map(|i| (i.site_id, i.seq))
        .collect();
    let sites: Vec<(i64, i64, u64, i64, i64, [f64; 3])> = w
        .ext::<Sites>()
        .iter()
        .filter_map(|i| {
            Some((
                i.site_id,
                i.system,
                i.seq,
                i.dungeon_id,
                i.room_id?,
                i.anchor,
            ))
        })
        .collect();
    // One entry per placed room (Y-COMBAT: a site behind gates has several),
    // each with its own `room_id` to tell the pilot.
    let mut instances: Vec<(Visit, [f64; 3])> = Vec::new();
    for (site_id, system, seq, dungeon_id, room_id, anchor) in sites {
        let rooms = crate::combat_site::open_rooms(w, site_id);
        let rooms = if rooms.is_empty() {
            vec![crate::combat_site::OpenRoom {
                room: room_id,
                anchor,
            }]
        } else {
            rooms
        };
        for r in rooms {
            let v = Visit {
                site_id,
                seq,
                system,
                dungeon_id,
                room_id: r.room,
            };
            instances.push((v, r.anchor));
        }
    }
    w.ext_mut::<Tracking>().done.retain(|k| live.contains(k));
    let systems: BTreeSet<i64> = instances.iter().map(|(v, _)| v.system).collect();
    let mut want: BTreeMap<i64, Visit> = BTreeMap::new();
    for system in systems {
        w.advance_system(system);
        for p in w.roster().in_system(system) {
            if w.character(p.character_id).is_none() {
                continue;
            }
            let hit = instances
                .iter()
                .find(|(v, anchor)| v.system == system && is_inside(&p.motion, *anchor));
            if let Some((v, _)) = hit {
                want.insert(p.character_id, *v);
            }
        }
    }
    let had = std::mem::take(&mut w.ext_mut::<Tracking>().inside);
    // Y-COMBAT: a pilot warping from one room to the next through a gate is
    // still inside; he lands and is told the new room.
    for (c, old) in &had {
        if !want.contains_key(c) && crate::combat_site::in_transit(w, *c, old.site_id, now) {
            want.insert(*c, *old);
        }
    }
    for c in want.keys() {
        if had.get(c).is_none_or(|old| old != &want[c]) {
            crate::combat_site::landed(w, *c);
        }
    }
    for (c, old) in &had {
        if want.get(c) == Some(old) {
            continue;
        }
        let here = w.roster().get(*c).map(|p| p.solar_system_id);
        // Moving to another room of the same instance is not leaving it.
        let same_site = want.get(c).is_some_and(|v| v.key() == old.key());
        if here == Some(old.system) && !same_site {
            send(w, EXITING, vec![PyValue::Int(old.dungeon_id)], vec![*c]);
        }
    }
    for (c, v) in &want {
        if had.get(c) == Some(v) {
            continue;
        }
        send(w, ENTERING, entering_args(v), vec![*c]);
        if w.ext::<Tracking>().done.contains(&v.key()) {
            send(w, COMPLETED, vec![PyValue::Int(v.dungeon_id)], vec![*c]);
        }
    }
    w.ext_mut::<Tracking>().inside = want;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destiny::Mode;
    use crate::dungeon_data::{self, Entry};
    use crate::model::SandboxConfig;
    use crate::population::{self, Site};
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const SYSTEM: i64 = 30_000_001;
    const OTHER_SYSTEM: i64 = 30_000_002;
    /// A character the default sandbox config knows.
    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const SITE_ID: i64 = population::SITE_BASE + 4242;
    const ANCHOR: [f64; 3] = [1.0e11, 2.0e9, -3.0e10];

    /// The catalogue fixture's dungeon with objects and an entry object.
    fn dungeon_id() -> i64 {
        dungeon_data::Table::fixture()
            .dungeons
            .values()
            .find(|d| !d.objects.is_empty() && d.entry_object.is_some())
            .map(|d| d.id)
            .expect("the fixture has a dungeon with a room")
    }

    fn site(dungeon_id: i64) -> Site {
        Site {
            system: SYSTEM,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: SITE_ID,
            dungeon_id,
            archetype: 97,
            entry: Entry::Anomaly,
            anchor: ANCHOR,
            target_id: "ABC-123".into(),
            state: population::State::Live,
            at: 0,
        }
    }

    /// The catalogue fixture, one instantiated site, and Alice in space in
    /// its system at `at`; every push drained.
    fn world(at: [f64; 3], dungeon_id: i64) -> Registry {
        let mut c = Content::fixture();
        c.insert("dungeon_data", dungeon_data::Table::fixture());
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(at, [0.0; 3]),
        ));
        assert!(crate::site::instantiate(&mut reg, &site(dungeon_id)));
        drain(&mut reg);
        reg
    }

    fn drain(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        let _ = reg.take_pending();
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .filter(|(_, name, _)| [ENTERING, COMPLETED, EXITING].contains(name))
            .collect()
    }

    fn step(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        tick(reg, Instant::now(), 0);
        drain(reg)
    }

    fn names(pushes: &[(Vec<i64>, &'static str, Vec<PyValue>)]) -> Vec<&'static str> {
        pushes.iter().map(|p| p.1).collect()
    }

    fn set_mode(reg: &mut Registry, mode: Mode) {
        reg.roster_mut().get_mut(ALICE).unwrap().motion.mode = mode;
    }

    fn warp(effect_stamp: i32) -> Mode {
        Mode::Warp {
            goto: [ANCHOR[0] + 1.0e12, ANCHOR[1], ANCHOR[2]],
            effect_stamp,
            total_distance: 1.0e12,
            stop_distance: 0.0,
            warp_factor: 3_000,
        }
    }

    /// **`OnEnteringDungeonRoom` has arity 5** [V] (C2 §4): `(dungeonID,
    /// roomID, None, instanceID, {})`, for Alice alone, and once.
    #[test]
    fn on_entering_dungeon_room_has_arity_5() {
        let mut reg = world(ANCHOR, dungeon_id());
        let room = reg.ext::<Sites>().get(SITE_ID).unwrap().room_id.unwrap();
        let first = step(&mut reg);
        assert_eq!(names(&first), vec![ENTERING]);
        let (to, _, args) = &first[0];
        assert_eq!(to, &vec![ALICE]);
        assert_eq!(args.len(), 5);
        assert_eq!(
            args,
            &vec![
                PyValue::Int(dungeon_id()),
                PyValue::Int(room),
                PyValue::None,
                PyValue::Int(SITE_ID),
                PyValue::Dict(Vec::new()),
            ]
        );
        assert!(step(&mut reg).is_empty(), "entered once");
        assert_eq!(
            current(&reg, ALICE),
            PyValue::Tuple(vec![
                PyValue::Int(dungeon_id()),
                PyValue::Int(room),
                PyValue::Int(SITE_ID),
                PyValue::Dict(Vec::new()),
            ]),
            "GetCurrentDungeonForCharacter is the 4-tuple [V]"
        );
    }

    /// Out of range, or in a site with no room, nothing is entered.
    #[test]
    fn nothing_is_entered_out_of_range_or_without_a_room() {
        let far = [ANCHOR[0] + ARRIVAL_RANGE * 1.01, ANCHOR[1], ANCHOR[2]];
        let mut reg = world(far, dungeon_id());
        assert!(step(&mut reg).is_empty());
        assert_eq!(current(&reg, ALICE), PyValue::None);
        let mut reg = world(ANCHOR, -1);
        assert!(step(&mut reg).is_empty(), "an unknown dungeon has no room");
    }

    /// **Completion fires once**: one `OnDungeonCompleted(dungeonID)` to the
    /// pilots inside; a second call does nothing; a pilot who enters later
    /// is told it is complete right after the room entry.
    #[test]
    fn completion_fires_once() {
        let mut reg = world(ANCHOR, dungeon_id());
        step(&mut reg);
        assert!(complete(&mut reg, SITE_ID));
        let done = drain(&mut reg);
        assert_eq!(names(&done), vec![COMPLETED]);
        assert_eq!(done[0].0, vec![ALICE]);
        assert_eq!(done[0].2, vec![PyValue::Int(dungeon_id())]);
        assert!(!complete(&mut reg, SITE_ID), "fires once");
        assert!(drain(&mut reg).is_empty());
        assert!(step(&mut reg).is_empty(), "the stage does not repeat it");
        assert!(
            !complete(&mut reg, SITE_ID + 1),
            "no instance, no completion"
        );

        // A late arrival: Alice warps out, then lands again.
        set_mode(&mut reg, warp(5));
        assert_eq!(names(&step(&mut reg)), vec![EXITING]);
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived(ANCHOR, [0.0; 3]);
        assert_eq!(names(&step(&mut reg)), vec![ENTERING, COMPLETED]);
    }

    /// **Exit on warp-out**: aligning is still inside; an activated warp
    /// pushes `OnExitingDungeon(dungeonID)` once.
    #[test]
    fn exit_on_warp_out() {
        let mut reg = world(ANCHOR, dungeon_id());
        step(&mut reg);
        set_mode(&mut reg, warp(-1));
        assert!(step(&mut reg).is_empty(), "aligning is not leaving");
        set_mode(&mut reg, warp(5));
        let out = step(&mut reg);
        assert_eq!(names(&out), vec![EXITING]);
        assert_eq!(out[0].0, vec![ALICE]);
        assert_eq!(out[0].2, vec![PyValue::Int(dungeon_id())]);
        assert!(step(&mut reg).is_empty(), "exited once");
        assert_eq!(current(&reg, ALICE), PyValue::None);
    }

    /// A vacated instance exits the pilots still in its system; a pilot who
    /// left the system is dropped with no push (the client clears on a
    /// location change).
    #[test]
    fn vacating_exits_and_leaving_the_system_is_silent() {
        let mut reg = world(ANCHOR, dungeon_id());
        step(&mut reg);
        assert!(crate::site::vacate(&mut reg, SITE_ID));
        drain(&mut reg);
        assert_eq!(names(&step(&mut reg)), vec![EXITING]);

        let mut reg = world(ANCHOR, dungeon_id());
        step(&mut reg);
        reg.roster_mut().leave(ALICE);
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            OTHER_SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        assert!(step(&mut reg).is_empty());
        assert!(reg.ext::<Tracking>().visit(ALICE).is_none());
    }

    /// The completion mark goes with its instance: a fresh instance of the
    /// same site starts incomplete.
    #[test]
    fn a_new_instance_starts_incomplete() {
        let mut reg = world(ANCHOR, dungeon_id());
        step(&mut reg);
        assert!(complete(&mut reg, SITE_ID));
        assert!(crate::site::vacate(&mut reg, SITE_ID));
        step(&mut reg);
        assert!(crate::site::instantiate(&mut reg, &site(dungeon_id())));
        drain(&mut reg);
        assert_eq!(names(&step(&mut reg)), vec![ENTERING]);
        assert!(complete(&mut reg, SITE_ID));
    }

    // Y-COMBAT: a site behind gates.

    const GATED: i64 = 999_801;
    const FIRST: i64 = 30;
    const SECOND: i64 = 10;
    /// A hull type the typedata does not know, so no ship list applies.
    const HULL: i64 = 990_801;

    /// Two rooms, the entry room (the higher id) holding a beacon and the
    /// gate to the second.
    fn gated() -> (Registry, i64) {
        use crate::dungeon_data::{Dungeon, Gate, Object, Room};
        let obj = |room, id, x| Object {
            room,
            id,
            type_id: 990_802,
            pos: [x, 0.0, 0.0],
            radius: Some(500.0),
            ..Object::default()
        };
        let mut table = dungeon_data::Table::fixture();
        table.dungeons.insert(
            GATED,
            Dungeon {
                id: GATED,
                entry_object: Some(1),
                rooms: [FIRST, SECOND]
                    .into_iter()
                    .map(|id| Room {
                        id,
                        ..Room::default()
                    })
                    .collect(),
                objects: vec![
                    obj(FIRST, 1, 0.0),
                    obj(FIRST, 2, 3_000.0),
                    obj(SECOND, 3, 0.0),
                ],
                gates: vec![Gate {
                    from_object: 2,
                    ..Gate::default()
                }],
                ..Dungeon::default()
            },
        );
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.character_mut(ALICE).expect("Alice exists").ship_type_id = HULL;
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        assert!(crate::site::instantiate(&mut reg, &site(GATED)));
        drain(&mut reg);
        let gate = reg
            .ext::<Sites>()
            .get(SITE_ID)
            .and_then(|i| i.objects.iter().find(|p| p.dun_object_id == 2))
            .map(|p| p.item_id)
            .expect("the gate is placed");
        (reg, gate)
    }

    /// **Through a gate the pilot is told the next room, never that he left
    /// the dungeon**: the warp between the rooms keeps him inside, and he
    /// lands in room two with `OnEnteringDungeonRoom` for it.
    #[test]
    fn going_through_a_gate_enters_the_next_room_without_leaving_the_dungeon() {
        let (mut reg, gate) = gated();
        let first = step(&mut reg);
        assert_eq!(names(&first), vec![ENTERING]);
        assert_eq!(first[0].2[1], PyValue::Int(FIRST));
        // Beside the gate (3 km from the anchor), then through it.
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived(
            [ANCHOR[0] + 3_000.0 + 1_500.0, ANCHOR[1], ANCHOR[2]],
            [0.0; 3],
        );
        let through = crate::combat_site::activate(&mut reg, ALICE, gate).expect("admitted");
        // In the warp, a grid from either room.
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived(
            [ANCHOR[0], ANCHOR[1] + through.aim[1] / 1.0e3, ANCHOR[2]],
            [0.0; 3],
        );
        set_mode(&mut reg, warp(10));
        assert!(
            step(&mut reg).is_empty(),
            "in transit: still inside, nothing said"
        );
        // Landed in the second room.
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived(through.aim, [0.0; 3]);
        let landed = step(&mut reg);
        assert_eq!(names(&landed), vec![ENTERING], "no exit between the rooms");
        assert_eq!(landed[0].2[1], PyValue::Int(SECOND));
        assert!(step(&mut reg).is_empty());
        // Warping away from the last room is leaving, as before.
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived([0.0; 3], [0.0; 3]);
        assert_eq!(names(&step(&mut reg)), vec![EXITING]);
    }

    /// A pilot who never lands (the transit's grace passes) is out.
    #[test]
    fn a_transit_that_never_lands_ends_with_its_grace() {
        let (mut reg, gate) = gated();
        step(&mut reg);
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived(
            [ANCHOR[0] + 3_000.0 + 1_500.0, ANCHOR[1], ANCHOR[2]],
            [0.0; 3],
        );
        crate::combat_site::activate(&mut reg, ALICE, gate).expect("admitted");
        reg.roster_mut().get_mut(ALICE).unwrap().motion = Motion::arrived([0.0; 3], [0.0; 3]);
        let later = Instant::now() + crate::combat_site::TRANSIT_GRACE + Duration::from_secs(1);
        tick(&mut reg, later, 0);
        assert_eq!(names(&drain(&mut reg)), vec![EXITING]);
    }
}
