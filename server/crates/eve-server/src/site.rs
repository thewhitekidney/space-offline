//! **Site instantiation: a live site's entry room becomes balls** when a
//! pilot reaches it.
//!
//! W14 packet CP28a (`docs/content/plan.md` §3.3, §3.4, §6.2; spec
//! `docs/content/C3-population.md` §4.5 and P-C3d, `docs/content/
//! C2-live-surface.md` §4; erratum 12 in `docs/content/rulings.md`). Lane
//! note: `docs/lanes/cp28a.md`.
//!
//! A `population` site (CP21) is only a record until a character comes
//! within [`ARRIVAL_RANGE`] of its anchor. Then [`tick`] calls
//! [`instantiate`], which places the dungeon's **entry room** ([`entry_room`],
//! the "room 0" of C3 §4.5) from the catalogue's own objects (CP16):
//!
//! - one ball per object, at `anchor + object.pos` (room-local metres, whose
//!   origin is the warp-in point, `docs/re/dungeon-model.md` §2-3) **[V]**;
//! - an object whose type is one of the room's `room-rocks` ore types is a
//!   rock, put in `asteroid::Field` with the authored value as its
//!   **quantity** (client `dungeonHelper.py:85-165`) and its radius from
//!   that quantity by the ore's own [`Law`] **[V]**, so lock, mine, approach and the bootstrap
//!   treat it like a belt rock;
//! - a **gas cloud** (SOC-B2) is a rock too: a `room-rocks` type that the
//!   dungeon's `resources.tsv` gas row names, or whose item type is a
//!   harvestable cloud (`mining::is_gas`), gets the linear [`Law::cloud`] -- the client
//!   ships no radius law for a cloud -- so a gas harvester cuts it as a
//!   laser cuts ore, and CP28b's G2 completes a gas site when its clouds
//!   are gone;
//! - every ball goes out in **one** `AddBalls2`, on the clock, as
//!   `dungeon::announce` does;
//! - every site slim carries `dunObjectID` and `entityGroupID` (C2 §4 [V
//!   names]) through seam S9 ([`slim_extras`]).
//!
//! **Erratum 12: `objectiveTargetGroup` is dropped.** It is set on 0 of the
//! catalogue's objects and 0 of its entities (`schema.md` §dungeons), and
//! the objective tables that could derive it (`dungeonObjectives`) are not
//! in the bundle. No slim carries the key; a test pins that.
//!
//! **Not here** (CP28b, `site_spawn.rs`): the squads (`npc_gen::
//! dungeon_squads`), triggers, in-pocket respawn and completion. CP28b
//! stamps its NPCs' slims through [`tag`], and removes an instance with
//! [`vacate`]. The instance ids are the plan's band (§3.4): [`object_id`].
//!
//! **Not persisted.** An instance is a function of its site and the
//! catalogue; a restart re-instantiates on the next arrival, with fresh ids
//! (plan §3.4: no id here is ever persisted). Site rocks write no `rock`
//! delta (`asteroid::Field::mine` skips [`is_instance_id`]).
//!
//! **A placed object is a loot source only as a hacked can** (a hackable
//! type, `hacking_loot::loot_source`); no placed wreck is one [I: no wreck
//! type in the content carries the hackable attribute], so nothing fills or
//! empties a placed wreck.

use crate::asteroid::{Law, Rock};
use crate::content::Content;
use crate::damage::BlueTime;
use crate::destiny::{Ball, Slim, SlimExtras};
use crate::dungeon_data::{self, Dungeon};
use crate::model::Target;
use crate::population::{self, Site};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Builds empty and reads no deltas (nothing here is persisted).
pub const SLOT: SlotSpec = SlotSpec { name: "site", make };

fn make(_: &Content) -> Boxed {
    Box::new(Sites::default())
}

/// The instance-object band, plan §3.4:
/// `8_700_000_000 + (seq % 100,000)·1,000 + n`. Listed in
/// `id_bands_do_not_overlap` (`site.rs` tests).
pub const INSTANCE_BASE: i64 = 8_700_000_000;
/// One past the band (war HQ items start here, `war::HQ_ITEM_BASE`; the sov
/// stand-in hubs sit above them). Those two persist, so this band shrinks to
/// stay clear of them. Instances are not persisted.
pub const INSTANCE_END: i64 = 8_800_000_000;
/// Instance sequence numbers before the band wraps. [`Sites::alloc`] skips
/// a sequence number that is still live, so a wrap never reuses a live id;
/// 100,000 live instances at once is far above any real count.
pub const SEQS: u64 = 100_000;
/// Ids per instance: the largest dungeon has 813 objects [V, plan §3.4].
pub const IDS_PER_INSTANCE: usize = 1_000;

// The band is exactly SEQS instances of IDS_PER_INSTANCE ids, and it starts
// where the site band (CP21) ends.
const _: () = assert!(INSTANCE_BASE + SEQS as i64 * IDS_PER_INSTANCE as i64 == INSTANCE_END);
const _: () = assert!(population::SITE_END == INSTANCE_BASE);

/// How near a pilot must be to a site's anchor for it to instantiate:
/// `BELT_RANGE`, as C3 §4.5 says. **[A]** (C3's choice, not the client's);
/// falsified by a pilot who lands on an empty grid and sees the room appear
/// late, or a room that appears for a pilot still in warp past it.
pub const ARRIVAL_RANGE: f64 = crate::belts::BELT_RANGE;

/// How often [`tick`] re-reads a hot system's sites for arrivals, in blue
/// time (1 s). **[A]**; falsified by a pilot who waits on an empty grid.
pub const CHECK_EVERY: BlueTime = 10_000_000;

/// A ball's radius when neither the object nor its type has one. **[A]**;
/// falsified by a structure the client draws far larger than its bracket.
pub const DEFAULT_RADIUS: f64 = 1_000.0;

/// `radius` (162) and the three asteroid-law attributes (1980, 1981, 2727)
/// `asteroid::Law` names **[V]**.
pub const ATTR_RADIUS: i64 = 162;
pub const ATTR_GROWTH: i64 = 1980;
pub const ATTR_UNIT_SIZE: i64 = 1981;
pub const ATTR_MAX_RADIUS: i64 = 2727;

/// A cloud type's radius when the type table has none: every cloud type's
/// own `radius` is 100 m [V-data] (`Law::cloud` takes a tenth of it per
/// unit). **[A]**; falsified by a cloud type with another radius that
/// the table fails to give.
pub const CLOUD_TYPE_RADIUS: f64 = 100.0;

/// The gas types a dungeon names: its `resources.tsv` gas rows' types.
pub fn gas_types(d: &Dungeon) -> BTreeSet<i64> {
    d.resources
        .iter()
        .filter(|r| r.kind == dungeon_data::ResourceKind::Gas)
        .flat_map(|r| r.type_ids.iter().copied())
        .collect()
}

/// **A `room-rocks` type's law** in a dungeon: an ore's own (`ore`, from
/// the type table or a loaded rock), else the client's linear cloud law
/// ([`Law::cloud`], `quantity x radius / 10` **[V]**) for a gas type --
/// one the dungeon's gas row names (`dungeon_gas`) or one the item table
/// files as a cloud (`known_gas`); `type_radius` is the type's own radius.
/// `None`: not a rock.
pub fn law_in(
    ore: Option<Law>,
    dungeon_gas: bool,
    known_gas: bool,
    type_radius: Option<f64>,
) -> Option<Law> {
    ore.or((dungeon_gas || known_gas).then(|| Law::cloud(type_radius.unwrap_or(CLOUD_TYPE_RADIUS))))
}

/// A site object's `ownerID`: the EVE system owner, as for a planet or a
/// rock (`destiny::celestial_ball`). **[A]**: the dungeon's faction is
/// [I] the real owner (`dungeon-model.md` §2), and an owner with standings
/// could colour a bracket. Falsified by a beacon or structure the client
/// names or colours wrongly.
pub const OWNER: i64 = 1;

/// The slim keys this module adds (C2 §4, [V] names:
/// `objective_task/target:89,411`, `destroy_counter:248`).
pub const KEY_DUN_OBJECT: &str = "dunObjectID";
pub const KEY_ENTITY_GROUP: &str = "entityGroupID";
pub use crate::combat_site::KEY_GATE_RANGE;

/// One placed object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The ball's item id, inside the instance band.
    pub item_id: i64,
    /// The catalogue's `objectID`: the slim's `dunObjectID`.
    pub dun_object_id: i64,
    pub type_id: i64,
    /// `anchor + object.pos`.
    pub position: [f64; 3],
    pub radius: f64,
    /// The slim's `entityGroupID`. **`None` for every room object [A]**: the
    /// catalogue gives a room object no entity group (an entity is a spawn
    /// group *at* an object, `dungeon-model.md` §5), and stamping the spawn's
    /// group on the object it stands at could let a destroy counter count a
    /// structure. Falsified by a client objective that waits on an object's
    /// group. NPCs get theirs through [`tag`].
    pub entity_group: Option<i64>,
    /// `Some` for an ore rock: the rock as it went into `asteroid::Field`.
    pub rock: Option<Rock>,
    /// The slim's `ownerID`: [`OWNER`] when placed, the hacker once a
    /// hackable can is hacked (HK2, [`set_owner`]) [I].
    pub owner_id: i64,
}

/// One instantiated site.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub site_id: i64,
    pub system: i64,
    pub dungeon_id: i64,
    /// `None` when the catalogue has no such dungeon or it has no room.
    pub room_id: Option<i64>,
    pub anchor: [f64; 3],
    pub seq: u64,
    pub objects: Vec<Placed>,
}

/// The slim extras one item carries: an NPC's (from CP28b), by [`tag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag {
    pub site_id: i64,
    pub dun_object_id: i64,
    pub entity_group: Option<i64>,
}

/// The world slot.
#[derive(Debug, Default)]
pub struct Sites {
    live: BTreeMap<i64, Instance>,
    /// Placed item id to its site.
    by_item: BTreeMap<i64, i64>,
    /// NPC item id to its tag (CP28b).
    tags: BTreeMap<i64, Tag>,
    next_seq: u64,
    /// Per hot system, when [`tick`] next reads it.
    due: BTreeMap<i64, BlueTime>,
    /// Placed objects destroyed while their instance lives (HK2: a can
    /// failed twice). Out of space, but still counted by [`hackables_left`].
    destroyed: BTreeSet<i64>,
    /// Placed objects held back from every park ([`cloak`]): live, but no
    /// pilot has their ball until [`uncloak`].
    cloaked: BTreeSet<i64>,
    /// Placed objects and rocks no lock may take ([`set_targetable`]).
    untargetable: BTreeSet<i64>,
}

impl Sites {
    pub fn get(&self, site_id: i64) -> Option<&Instance> {
        self.live.get(&site_id)
    }

    /// The instance a placed item belongs to.
    pub fn of_item(&self, item_id: i64) -> Option<&Instance> {
        self.live.get(self.by_item.get(&item_id)?)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Instance> {
        self.live.values()
    }

    /// A placed object still in space.
    fn placed(&self, item_id: i64) -> Option<&Placed> {
        if self.destroyed.contains(&item_id) {
            return None;
        }
        self.of_item(item_id)?
            .objects
            .iter()
            .find(|p| p.item_id == item_id)
    }

    /// The next free sequence number, wrapping at [`SEQS`] and skipping
    /// any still live.
    fn alloc(&mut self) -> Option<u64> {
        let used: BTreeSet<u64> = self.live.values().map(|i| i.seq).collect();
        (0..SEQS)
            .map(|k| (self.next_seq + k) % SEQS)
            .find(|s| !used.contains(s))
            .inspect(|s| self.next_seq = (s + 1) % SEQS)
    }
}

/// Object `n` of instance `seq`: `INSTANCE_BASE + (seq % SEQS)·1000 + n`;
/// `None` for `n` past [`IDS_PER_INSTANCE`].
pub fn object_id(seq: u64, n: usize) -> Option<i64> {
    (n < IDS_PER_INSTANCE)
        .then(|| INSTANCE_BASE + (seq % SEQS) as i64 * IDS_PER_INSTANCE as i64 + n as i64)
}

/// The first object number past every object `inst` holds. The ids are
/// `object_id(seq, n)` by the dungeon's own order, and a filtered layout
/// (an abyssal room drops its PvP gate and nodes) leaves holes, so
/// `objects.len()` can name an id a kept object already has (#196).
fn next_index(inst: &Instance) -> usize {
    let base = INSTANCE_BASE + (inst.seq % SEQS) as i64 * IDS_PER_INSTANCE as i64;
    inst.objects
        .iter()
        .filter_map(|p| usize::try_from(p.item_id - base).ok())
        .filter(|n| *n < IDS_PER_INSTANCE)
        .map(|n| n + 1)
        .max()
        .unwrap_or(0)
}

/// Whether an id is in the instance band.
pub fn is_instance_id(id: i64) -> bool {
    (INSTANCE_BASE..INSTANCE_END).contains(&id)
}

/// The room a pilot warps into: the `entryObjectID` object's room, else the
/// lowest room id. **[I]** (`dungeon-model.md` §2: R0 holds the entry
/// object in the worked example, and room ids run in chain order).
/// Falsified by a dungeon whose entry object is not in its first room.
pub fn entry_room(d: &Dungeon) -> Option<i64> {
    d.entry_object
        .and_then(|e| d.objects.iter().find(|o| o.id == e))
        .map(|o| o.room)
        .or_else(|| {
            d.rooms
                .iter()
                .map(|r| r.id)
                .chain(d.objects.iter().map(|o| o.room))
                .min()
        })
}

/// **The pure layout** of a dungeon's entry room at `anchor`, as instance
/// `seq` of `site_id` in `system`. `radius_of` is a type's own radius and
/// `law_of` an ore type's radius law; both may answer `None`.
///
/// Objects go in catalogue (object id) order. An object of a `room-rocks`
/// type with a usable law and a positive authored value becomes a rock
/// whose quantity is that value rounded (the client reads it as a quantity,
/// `dungeonHelper.py:85-165` [V]) and whose radius is the law of that
/// quantity (what the pilot sees and what mining takes agree); a gas cloud
/// does the same with the linear [`Law::cloud`]. Any other object keeps its radius, else its type's,
/// else [`DEFAULT_RADIUS`]. Objects past [`IDS_PER_INSTANCE`] are dropped
/// (none in this build [V]).
pub fn layout(
    d: &Dungeon,
    system: i64,
    site_id: i64,
    anchor: [f64; 3],
    seq: u64,
    radius_of: &dyn Fn(i64) -> Option<f64>,
    law_of: &dyn Fn(i64) -> Option<Law>,
) -> (Option<i64>, Vec<Placed>) {
    let Some(room) = entry_room(d) else {
        return (None, Vec::new());
    };
    let at = RoomAt {
        room,
        anchor,
        first: 0,
    };
    (
        Some(room),
        layout_room(d, system, site_id, seq, at, radius_of, law_of),
    )
}

/// Where a room goes: which room, its anchor, and the number of the first
/// id it takes inside the instance's band (rooms after the entry room
/// continue the numbering, so every id of one instance is distinct;
/// `combat_site`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomAt {
    pub room: i64,
    pub anchor: [f64; 3],
    pub first: usize,
}

/// **The pure layout of one room** at `at` (see [`layout`]).
pub fn layout_room(
    d: &Dungeon,
    system: i64,
    site_id: i64,
    seq: u64,
    at: RoomAt,
    radius_of: &dyn Fn(i64) -> Option<f64>,
    law_of: &dyn Fn(i64) -> Option<Law>,
) -> Vec<Placed> {
    let RoomAt {
        room,
        anchor,
        first,
    } = at;
    let ores: BTreeSet<i64> = d
        .room_rocks
        .iter()
        .filter(|r| r.room == room)
        .map(|r| r.type_id)
        .collect();
    let mut out = Vec::new();
    for (n, o) in d.objects.iter().filter(|o| o.room == room).enumerate() {
        let Some(item_id) = object_id(seq, first + n) else {
            crate::outln!(
                "[site] dungeon {} room {room}: more than {IDS_PER_INSTANCE} objects; the rest are dropped",
                d.id
            );
            break;
        };
        let position = [
            anchor[0] + o.pos[0],
            anchor[1] + o.pos[1],
            anchor[2] + o.pos[2],
        ];
        let own = o.radius.filter(|r| r.is_finite() && *r > 0.0);
        let law = ores
            .contains(&o.type_id)
            .then(|| law_of(o.type_id))
            .flatten()
            .filter(usable);
        let rock = match (law, own) {
            (Some(law), Some(r)) => {
                // An ore's authored value is its QUANTITY (client
                // dungeonHelper.py:85-165) [V], not a radius; the radius
                // follows from the quantity by the type's law (a cloud's
                // is linear).
                let (quantity, radius) = {
                    let q = r.round().max(1.0) as i64;
                    (q, law.radius(q))
                };
                Some(Rock {
                    id: item_id,
                    type_id: o.type_id,
                    belt_id: site_id,
                    solar_system_id: system,
                    quantity,
                    radius,
                    position,
                    law,
                })
            }
            _ => None,
        };
        let radius = rock.map(|r| r.radius).unwrap_or_else(|| {
            own.or_else(|| radius_of(o.type_id).filter(|r| r.is_finite() && *r > 0.0))
                .unwrap_or(DEFAULT_RADIUS)
        });
        out.push(Placed {
            item_id,
            dun_object_id: o.id,
            type_id: o.type_id,
            position,
            radius,
            entity_group: None,
            rock,
            owner_id: OWNER,
        });
    }
    out
}

fn usable(l: &Law) -> bool {
    l.is_usable()
}

/// A type's radius from the world's typedata (`reg.content()`).
fn type_radius(w: &Registry, type_id: i64) -> Option<f64> {
    w.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .get(type_id)?
        .attribute(ATTR_RADIUS)
}

/// An ore's law: the world's typedata's three attributes, else the law of
/// any loaded rock of that type.
fn law_for(w: &Registry, type_id: i64) -> Option<Law> {
    let from_types = w
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|i| i.table().get(type_id))
        .and_then(|t| {
            Some(Law {
                growth: t.attribute(ATTR_GROWTH)?,
                unit_size: t.attribute(ATTR_UNIT_SIZE)?,
                max_radius: t.attribute(ATTR_MAX_RADIUS)?,
            })
        })
        .filter(usable);
    from_types.or_else(|| {
        w.rocks()
            .iter()
            .find(|r| r.type_id == type_id)
            .map(|r| r.law)
    })
}

/// A static object's ball: a celestial's shape (`Mode::Stop`, not free,
/// mass 0), as `destiny::rock_ball` is. **[A]** the same precedent;
/// falsified by a site structure that the client moves or bumps wrongly.
pub fn ball(p: &Placed) -> Ball {
    use crate::destiny::{FLAGS_STATIC_BALL, Mode, NO_FORMATION};
    match &p.rock {
        Some(r) => crate::destiny::rock_ball(r),
        None => Ball {
            ball_id: p.item_id,
            mode: Mode::Stop,
            radius: p.radius as f32,
            position: p.position,
            flags: FLAGS_STATIC_BALL,
            mass: 0.0,
            cloak: 0,
            harmonic: -1,
            corporation_id: -1,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        },
    }
}

/// A placed object's slim: its type, [`OWNER`], no pilot.
pub fn slim(p: &Placed) -> Slim {
    match &p.rock {
        Some(r) => crate::destiny::rock_slim(r),
        None => Slim {
            item_id: p.item_id,
            type_id: p.type_id,
            owner_id: p.owner_id,
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        },
    }
}

/// **Instantiate a live site's entry room**: lay it out, put its rocks in
/// the field, and send every ball in one `AddBalls2` on the clock to the
/// pilots in the system. `false` when the site is already instantiated.
///
/// A site whose dungeon the catalogue lacks, or whose entry room is empty,
/// is still recorded (with no objects), so the stage does not retry it.
pub fn instantiate(w: &mut Registry, site: &Site) -> bool {
    if w.ext::<Sites>().live.contains_key(&site.site_id) {
        return false;
    }
    let Some(seq) = w.ext_mut::<Sites>().alloc() else {
        crate::outln!(
            "[site] the instance band is full; site {} waits",
            site.site_id
        );
        return false;
    };
    let dungeon = w
        .content()
        .try_get::<dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&site.dungeon_id))
        .cloned();
    let (room_id, objects) = match &dungeon {
        Some(d) => {
            let reg: &Registry = w;
            let gas = gas_types(d);
            layout(
                d,
                site.system,
                site.site_id,
                site.anchor,
                seq,
                &|t| type_radius(reg, t),
                &|t| {
                    law_in(
                        law_for(reg, t),
                        gas.contains(&t),
                        crate::mining::is_gas(reg.content(), t),
                        type_radius(reg, t),
                    )
                },
            )
        }
        None => (None, Vec::new()),
    };
    // An abyssal PvE room does not place the PvP gate or its nodes.
    let objects: Vec<Placed> = match &dungeon {
        Some(d) if d.archetype == crate::abyss::ARCHETYPE => objects
            .into_iter()
            .filter(|p| !crate::abyss::is_stray(w.content(), p.type_id))
            .collect(),
        _ => objects,
    };
    for r in objects.iter().filter_map(|p| p.rock) {
        w.rocks_mut().add(r);
    }
    let s = w.ext_mut::<Sites>();
    for p in &objects {
        s.by_item.insert(p.item_id, site.site_id);
    }
    s.live.insert(
        site.site_id,
        Instance {
            site_id: site.site_id,
            system: site.system,
            dungeon_id: site.dungeon_id,
            room_id,
            anchor: site.anchor,
            seq,
            objects: objects.clone(),
        },
    );
    announce(w, site.system, &objects);
    true
}

/// **Place one more room of a live site** (`combat_site`: the room behind an
/// acceleration gate): lay `room` out at `anchor`, number its objects on
/// past the instance's highest object id, put its rocks in the field, add it to the
/// instance (so `vacate`, the bootstrap, `object` and the slim extras all
/// see it) and send it in one `AddBalls2`. `None` when the site is not live,
/// its dungeon is unknown or the room is already placed.
pub fn open_room(
    w: &mut Registry,
    site_id: i64,
    room: i64,
    anchor: [f64; 3],
) -> Option<Vec<Placed>> {
    let (system, dungeon_id, seq, first) = {
        let i = w.ext::<Sites>().live.get(&site_id)?;
        (i.system, i.dungeon_id, i.seq, next_index(i))
    };
    let d = w
        .content()
        .try_get::<dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&dungeon_id))
        .cloned()?;
    let objects = {
        let reg: &Registry = w;
        let gas = gas_types(&d);
        layout_room(
            &d,
            system,
            site_id,
            seq,
            RoomAt {
                room,
                anchor,
                first,
            },
            &|t| type_radius(reg, t),
            &|t| {
                law_in(
                    law_for(reg, t),
                    gas.contains(&t),
                    crate::mining::is_gas(reg.content(), t),
                    type_radius(reg, t),
                )
            },
        )
    };
    for r in objects.iter().filter_map(|p| p.rock) {
        w.rocks_mut().add(r);
    }
    let s = w.ext_mut::<Sites>();
    for p in &objects {
        s.by_item.insert(p.item_id, site_id);
    }
    s.live
        .get_mut(&site_id)?
        .objects
        .extend(objects.iter().copied());
    announce(w, system, &objects);
    Some(objects)
}

/// **Put one object the dungeon does not list into a live instance** (the
/// Abyss probe's exit gate, `crate::abyss`): numbered on past the
/// highest object id in it, part of the instance (so `vacate`, the bootstrap and `object` see
/// it) and sent in one `AddBalls2`. `dun_object_id` is 0: no catalogue object
/// stands for it. `None` when the site is not live or its id band is full.
pub fn add_object(
    w: &mut Registry,
    site_id: i64,
    type_id: i64,
    position: [f64; 3],
) -> Option<Placed> {
    let (system, seq, n) = {
        let i = w.ext::<Sites>().live.get(&site_id)?;
        (i.system, i.seq, next_index(i))
    };
    let radius = type_radius(w, type_id)
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(DEFAULT_RADIUS);
    let placed = Placed {
        item_id: object_id(seq, n)?,
        dun_object_id: 0,
        type_id,
        position,
        radius,
        entity_group: None,
        rock: None,
        owner_id: OWNER,
    };
    let s = w.ext_mut::<Sites>();
    s.by_item.insert(placed.item_id, site_id);
    s.live.get_mut(&site_id)?.objects.push(placed);
    announce(w, system, &[placed]);
    Some(placed)
}

/// One `AddBalls2` for the lot, on the clock, with every source's slim
/// extras (S9).
fn announce(w: &mut Registry, system: i64, objects: &[Placed]) {
    if objects.is_empty() {
        return;
    }
    let to = crate::dungeon::audience(w, system);
    if to.is_empty() {
        return;
    }
    let ids: Vec<i64> = objects.iter().map(|p| p.item_id).collect();
    let extras = crate::slim_extra::for_slims(w, &ids);
    let stamp = w.next_destiny_stamp();
    let park = crate::destiny::Park {
        current_time: stamp,
        balls: objects.iter().map(ball).collect(),
    };
    let extra = objects.iter().map(|p| (slim(p), None)).collect();
    if let Some(added) = crate::destiny::BallsAdded::new(to, stamp, park, extra) {
        w.schedule(std::time::Duration::ZERO, added.with_extras(extras));
    }
}

/// **Take an instance out of space**: its rocks leave the field, its tags
/// go, and every ball still there leaves in one `RemoveBalls` on the clock.
/// `false` for a site that is not instantiated. For CP28b's `SITE_VACANT`,
/// and for [`tick`] when the site stops being live.
pub fn vacate(w: &mut Registry, site_id: i64) -> bool {
    let s = w.ext_mut::<Sites>();
    let Some(inst) = s.live.remove(&site_id) else {
        return false;
    };
    for p in &inst.objects {
        s.by_item.remove(&p.item_id);
        s.cloaked.remove(&p.item_id);
        s.untargetable.remove(&p.item_id);
    }
    s.tags.retain(|_, t| t.site_id != site_id);
    let destroyed: BTreeSet<i64> = inst
        .objects
        .iter()
        .map(|p| p.item_id)
        .filter(|id| s.destroyed.remove(id))
        .collect();
    let mut gone = Vec::new();
    for p in inst
        .objects
        .iter()
        .filter(|p| !destroyed.contains(&p.item_id))
    {
        match p.rock {
            Some(_) => {
                if w.rocks_mut().remove(p.item_id).is_some() {
                    gone.push(p.item_id);
                }
            }
            None => gone.push(p.item_id),
        }
    }
    let to = crate::dungeon::audience(w, inst.system);
    if !to.is_empty() {
        let stamp = w.next_destiny_stamp();
        if let Some(rm) = crate::destiny::BallsRemoved::new(to, stamp, gone) {
            w.schedule(std::time::Duration::ZERO, rm);
        }
    }
    true
}

/// Stamp `item_id`'s slim (an NPC CP28b spawned for this site) with its
/// `dunObjectID` and `entityGroupID`.
pub fn tag(w: &mut Registry, item_id: i64, tag: Tag) {
    w.ext_mut::<Sites>().tags.insert(item_id, tag);
}

/// The NPCs tagged to a site (`tag`): what the Abyss reads to know a room is
/// clear. They are only ever listed here; whether they live is the NPC table's.
pub fn tagged(w: &Registry, site_id: i64) -> Vec<i64> {
    w.ext::<Sites>()
        .tags
        .iter()
        .filter(|(_, t)| t.site_id == site_id)
        .map(|(id, _)| *id)
        .collect()
}

/// The site objects of a system that are not rocks, as `(ball, slim)`: what
/// the bootstrap adds (the rocks come with the field's).
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    let s = w.ext::<Sites>();
    s.live
        .values()
        .filter(|i| i.system == system)
        .flat_map(|i| i.objects.iter())
        .filter(|p| {
            p.rock.is_none() && !s.destroyed.contains(&p.item_id) && !s.cloaked.contains(&p.item_id)
        })
        .map(|p| (ball(p), slim(p)))
        .collect()
}

/// **The S9 source** (`slim_extra::SOURCES`): `dunObjectID` and
/// `entityGroupID` on every placed object and every tagged NPC among `ids`.
/// `entityGroupID` is `None` where the data has none. `objectiveTargetGroup`
/// is never sent (erratum 12).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> SlimExtras {
    let s = w.ext::<Sites>();
    ids.iter()
        .filter_map(|id| {
            let (dun, group) = match s.placed(*id) {
                Some(p) => (p.dun_object_id, p.entity_group),
                None => {
                    let t = s.tags.get(id)?;
                    (t.dun_object_id, t.entity_group)
                }
            };
            let mut pairs = vec![
                (KEY_DUN_OBJECT, PyValue::Int(dun)),
                (KEY_ENTITY_GROUP, group.map_or(PyValue::None, PyValue::Int)),
            ];
            if s.placed(*id).is_some() {
                pairs.extend(dun_pairs(w, *id, dun));
            }
            // Y-COMBAT: a gate says so.
            if is_gate(w, *id) {
                pairs.push((
                    KEY_GATE_RANGE,
                    PyValue::Real(crate::jump::MAX_JUMP_DISTANCE),
                ));
            }
            // HK2: a hackable can's state rides on this source's entry,
            // because a slim takes only the **first** extras entry with its
            // id (`destiny::encode::slim_pairs`), so a second source for the
            // same balls would be dropped.
            if let Some(state) = crate::hacking::security_state(w, *id) {
                pairs.push((crate::hacking::KEY_SECURITY_STATE, PyValue::Int(state)));
            }
            // ABYSS-1: an entrance gate carries its key, mode and openness.
            pairs.extend(crate::abyss::gate_pairs(w, *id));
            // A placed wreck says whether it holds anything, as a combat
            // wreck's slim does (`destiny::WreckSlim`).
            if is_wreck(w, *id) {
                pairs.push((
                    KEY_IS_EMPTY,
                    PyValue::Bool(w.store().contents_of(*id).next().is_none()),
                ));
            }
            Some((*id, pairs))
        })
        .collect()
}

/// The slim keys a live server puts on every dungeon object **[V-TQ]** (the
/// AIR void pocket's `SetState`): the dungeon, the object's catalogue
/// position (room-relative) and, when the catalogue has one, its rotation.
/// The client draws scenery from them: `BackgroundObject.LoadModel` sets the
/// model's translation from `dunPosition`, `SpaceObject.SetStaticRotation`
/// reads `dunRotation`, and `LoadDungeonAssetManipulations` looks the object
/// up by `dunID` and `dunObjectID` **[V]**.
pub const KEY_DUN_ID: &str = "dunID";
pub const KEY_DUN_POSITION: &str = "dunPosition";
pub const KEY_DUN_ROTATION: &str = "dunRotation";

fn dun_pairs(w: &Registry, item_id: i64, dun_object: i64) -> Vec<(&'static str, PyValue)> {
    let Some(dungeon_id) = w.ext::<Sites>().of_item(item_id).map(|i| i.dungeon_id) else {
        return Vec::new();
    };
    let mut pairs = vec![(KEY_DUN_ID, PyValue::Int(dungeon_id))];
    let object = w
        .content()
        .try_get::<dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&dungeon_id))
        .and_then(|d| d.objects.iter().find(|o| o.id == dun_object));
    if let Some(o) = object {
        let triple = |v: [f64; 3]| PyValue::Tuple(v.iter().map(|x| PyValue::Real(*x)).collect());
        pairs.push((KEY_DUN_POSITION, triple(o.pos)));
        if let (Some(y), Some(p), Some(r)) = (o.yaw, o.pitch, o.roll) {
            pairs.push((KEY_DUN_ROTATION, triple([y, p, r])));
        }
    }
    pairs
}

/// Whether a placed object is an acceleration gate: a catalogue object that
/// some connection of its dungeon names (`fromObjectID`) **[V-data]**.
pub fn is_gate(w: &Registry, item_id: i64) -> bool {
    let s = w.ext::<Sites>();
    let (Some(p), Some(i)) = (s.placed(item_id), s.of_item(item_id)) else {
        return false;
    };
    w.content()
        .try_get::<dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&i.dungeon_id))
        .is_some_and(|d| d.gates.iter().any(|g| g.from_object == p.dun_object_id))
}

/// A placed object still in space, copied out, and its solar system (HK2).
pub fn object(w: &Registry, item_id: i64) -> Option<(Placed, i64)> {
    let s = w.ext::<Sites>();
    let p = *s.placed(item_id)?;
    Some((p, s.of_item(item_id)?.system))
}

/// Give a placed object a new `ownerID` (HK2: a hacked can is the
/// hacker's). `false` for an object not in space.
pub fn set_owner(w: &mut Registry, item_id: i64, owner_id: i64) -> bool {
    let s = w.ext_mut::<Sites>();
    if s.destroyed.contains(&item_id) {
        return false;
    }
    let Some(site_id) = s.by_item.get(&item_id).copied() else {
        return false;
    };
    let Some(p) = s
        .live
        .get_mut(&site_id)
        .and_then(|i| i.objects.iter_mut().find(|p| p.item_id == item_id))
    else {
        return false;
    };
    p.owner_id = owner_id;
    true
}

/// **Take one placed object out of space** while its instance lives (HK2:
/// the failure that destroys a can). The caller sends the `RemoveBalls`.
/// `false` for an object not in space or a rock (a rock leaves through the
/// field).
pub fn destroy_object(w: &mut Registry, item_id: i64) -> bool {
    let s = w.ext_mut::<Sites>();
    if s.placed(item_id).is_none_or(|p| p.rock.is_some()) {
        return false;
    }
    s.destroyed.insert(item_id)
}

/// **Hold a placed object back from every park** (the node graphs' `Cloak`
/// atom): pilots who have its ball lose it, and the bootstrap and grid leave
/// it out until [`uncloak`]. A live server did not send the AIR tutorial's
/// cloaked wreck at all until it uncloaked **[V-TQ]**. `false` for an item
/// that is not a live placed object, or a rock.
pub fn cloak(w: &mut Registry, item_id: i64) -> bool {
    let Some((p, system)) = object(w, item_id) else {
        return false;
    };
    if p.rock.is_some() || !w.ext_mut::<Sites>().cloaked.insert(item_id) {
        return false;
    }
    let to = crate::dungeon::audience(w, system);
    if !to.is_empty() {
        let stamp = w.next_destiny_stamp();
        if let Some(rm) = crate::destiny::BallsRemoved::new(to, stamp, vec![item_id]) {
            w.schedule(std::time::Duration::ZERO, rm);
        }
    }
    true
}

/// The client's decloak flash (`OnSpecialFX`'s guid).
pub const FX_UNCLOAK: &str = "effects.Uncloak";

/// **Put a cloaked object back** (`Uncloak`): its ball to every pilot in the
/// system, then `OnSpecialFX(ball, None, None, None, None,
/// 'effects.Uncloak', 0, 1, 0)` -- the order and the arguments a live server
/// sent for the tutorial's wreck **[V-TQ]** (`AddBalls2`, then the effect
/// 0.1 s later). `false` for an object that was not cloaked.
pub fn uncloak(w: &mut Registry, item_id: i64) -> bool {
    if !w.ext_mut::<Sites>().cloaked.remove(&item_id) {
        return false;
    }
    let Some((p, system)) = object(w, item_id) else {
        return false;
    };
    announce(w, system, &[p]);
    let to = crate::dungeon::audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(fx) = crate::destiny::SpecialFx::new(to, stamp, item_id, FX_UNCLOAK, true) {
        w.schedule(std::time::Duration::ZERO, fx);
    }
    true
}

/// **Whether a lock may take it** (the node graphs' `Targetable` and
/// `Untargetable` atoms): a placed object or rock of a live site. Recorded
/// here and read by `targeting::lock`, which refuses an untargetable or a
/// cloaked one as it refuses a target that is not there. **[A]** the client
/// is not told: `attributeUntargetable` (1158) is named in `dogma/const` and
/// read nowhere in the client **[V]**; falsified by a live lock on an
/// untargetable rock that fails with a message.
pub fn set_targetable(w: &mut Registry, item_id: i64, targetable: bool) -> bool {
    let s = w.ext_mut::<Sites>();
    if !s.by_item.contains_key(&item_id) {
        return false;
    }
    if targetable {
        s.untargetable.remove(&item_id);
    } else {
        s.untargetable.insert(item_id);
    }
    true
}

/// Whether a lock on `item_id` is refused: it is a site object held back or
/// marked untargetable.
pub fn lock_refused(w: &Registry, item_id: i64) -> bool {
    let s = w.ext::<Sites>();
    s.cloaked.contains(&item_id) || s.untargetable.contains(&item_id)
}

/// A site's hackable cans: `(placed, left)`, where a can is done once it is
/// hacked or destroyed (HK2).
pub fn hackables_left(w: &Registry, site_id: i64) -> (usize, usize) {
    let s = w.ext::<Sites>();
    let Some(inst) = s.live.get(&site_id) else {
        return (0, 0);
    };
    let h = w.ext::<crate::hacking::Hacks>();
    let cans: Vec<i64> = inst
        .objects
        .iter()
        .filter(|p| p.rock.is_none() && crate::hacking::is_hackable_type(w, p.type_id))
        .map(|p| p.item_id)
        .collect();
    let left = cans
        .iter()
        .filter(|id| {
            !s.destroyed.contains(id) && h.can_state(**id) != crate::hacking::CanState::Hacked
        })
        .count();
    (cans.len(), left)
}

/// **A data or relic site's completion** (HK2, R-HACK §4): `base` is
/// `site_spawn::is_complete`'s answer, `other_goals` whether the site has
/// squads or rocks. A site with no can is `base`; one with cans completes
/// when every can is hacked or destroyed, and its other goals (if any) are
/// met. **[A]** a can counts whoever hacked it; falsified by a TQ site that
/// stays until the loot is taken.
pub fn complete_with_cans(base: bool, other_goals: bool, cans: usize, cans_left: usize) -> bool {
    if cans == 0 {
        return base;
    }
    cans_left == 0 && (!other_goals || base)
}

/// **The S8 resolver** (`warp_subject::RESOLVERS`, subject `item`): a
/// placed object that is not a rock (a rock is a ball the first row
/// resolves). Aimed like a rock: no warp-in point, the standoff clears it.
pub fn object_target(w: &Registry, _actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    let p = w.ext::<Sites>().placed(id)?;
    Some(Target {
        position: p.position,
        radius: p.radius,
        warp_in: None,
    })
}

/// The live sites a pilot has reached: live, not yet instantiated, and
/// with a pilot within [`ARRIVAL_RANGE`] of the anchor.
pub fn arrivals<'a>(sites: &'a [Site], have: &BTreeSet<i64>, pilots: &[[f64; 3]]) -> Vec<&'a Site> {
    sites
        .iter()
        .filter(|s| s.state == population::State::Live && !have.contains(&s.site_id))
        .filter(|s| {
            pilots
                .iter()
                .any(|p| crate::space_state::distance(*p, s.anchor) <= ARRIVAL_RANGE)
        })
        .collect()
}

/// **The stage** (after `population`): every [`CHECK_EVERY`], each hot
/// system's live sites are read; one a pilot has reached is instantiated,
/// and an instance whose site is no longer live is vacated. An instance in
/// a system gone cold is dropped (nobody is there to tell).
pub fn tick(w: &mut Registry, _now: Instant, ft: BlueTime) {
    use crate::liveness::{Liveness, Tier};
    let (cold, hot): (Vec<i64>, Vec<i64>) = {
        let live = w.ext::<Liveness>();
        let s = w.ext::<Sites>();
        let cold = s
            .live
            .values()
            .filter(|i| live.tier(i.system) == Tier::Cold)
            // A mission's pocket stands until the mission lets it go.
            .filter(|i| !crate::mission::is_mission_site(i.site_id))
            .map(|i| i.site_id)
            .collect();
        let hot = live
            .hot()
            .filter(|sys| s.due.get(sys).is_none_or(|t| ft >= *t))
            .collect();
        (cold, hot)
    };
    for id in cold {
        vacate(w, id);
    }
    {
        let live = w.ext::<Liveness>();
        let keep: BTreeSet<i64> = live.hot().collect();
        w.ext_mut::<Sites>().due.retain(|sys, _| keep.contains(sys));
    }
    for system in hot {
        w.ext_mut::<Sites>().due.insert(system, ft + CHECK_EVERY);
        if nothing_to_check(w, system) {
            continue;
        }
        check(w, system, ft);
    }
}

/// **TICK-1: whether an arrivals check in `system` can do nothing**: no
/// pilot is in space there (so nobody can reach a site) and no site
/// instance is open there (so none can be vacated). [`check`] and
/// `wormhole::arrivals` then change nothing, and the first tick that fails
/// this runs them over the same stored state.
pub(crate) fn nothing_to_check(w: &Registry, system: i64) -> bool {
    crate::tick_budget::class_of(w, system) != crate::tick_budget::Class::Hot
        && !w.ext::<Sites>().live.values().any(|i| i.system == system)
}

/// One hot system's arrivals and departures.
fn check(w: &mut Registry, system: i64, ft: BlueTime) {
    w.advance_system(system);
    let pilots: Vec<[f64; 3]> = w
        .roster()
        .in_system(system)
        .into_iter()
        .filter(|p| w.character(p.character_id).is_some())
        .map(|p| p.motion.position)
        .collect();
    let sites: Vec<Site> = population::sites(w, system, ft)
        .sites
        .into_iter()
        .filter(|s| s.state == population::State::Live)
        .collect();
    let live_ids: BTreeSet<i64> = sites.iter().map(|s| s.site_id).collect();
    // A wormhole's instance is `crate::wormhole`'s to vacate (W16 CP54a).
    let have: BTreeSet<i64> = w
        .ext::<Sites>()
        .live
        .values()
        .filter(|i| {
            i.system == system
                && !crate::wormhole::is_wormhole_site(i.site_id)
                && !crate::escalation::is_escalation_site(i.site_id)
                && !crate::abyss::is_abyss_site(i.site_id)
                && !crate::mission::is_mission_site(i.site_id)
                && !crate::air_npe::is_npe_site(i.site_id)
        })
        .map(|i| i.site_id)
        .collect();
    for id in have.iter().filter(|id| !live_ids.contains(id)) {
        vacate(w, *id);
    }
    let reached: Vec<Site> = arrivals(&sites, &have, &pilots)
        .into_iter()
        .cloned()
        .collect();
    for site in &reached {
        instantiate(w, site);
    }
}

/// The slim key the client reads for whether a wreck holds anything.
/// **[V]** (decompiled 3707965) `parklife/bracketMgr.py` `GetBracketProps`
/// line 589 picks the bracket icon from `getattr(slimItem, 'isEmpty',
/// False)`, so a wreck without the key shows as holding loot;
/// `parklife/state.py` `CheckWreckEmpty` line 1233 reads `slimItem.isEmpty`
/// with no default for a wreck-group slim. `overview/spaceObjectIcon.py`
/// `UpdateSpaceObjectIcon` (291-295) reads it only for the tooltip.
pub const KEY_IS_EMPTY: &str = "isEmpty";

/// Whether a placed object is a wreck (`combat::GROUP_WRECK`), by its type.
pub fn is_wreck(w: &Registry, id: i64) -> bool {
    let Some(p) = w.ext::<Sites>().placed(id).copied() else {
        return false;
    };
    w.content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(p.type_id))
        .is_some_and(|r| r.group_id == crate::combat::GROUP_WRECK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Entry, Object, Room, RoomRock};
    use crate::model::SandboxConfig;
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;
    use std::time::Duration;

    const SYSTEM: i64 = 30_990_001;
    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const SITE_ID: i64 = population::SITE_BASE + 4242;
    const ANCHOR: [f64; 3] = [1.0e11, 2.0e9, -3.0e10];
    /// Invented: an ore dungeon with one rock room.
    const ORE_DUNGEON: i64 = 999_201;
    const ORE_TYPE: i64 = 990_301;
    /// Invented law numbers (no client value).
    const LAW: Law = Law {
        growth: 0.5,
        unit_size: 100.0,
        max_radius: 20_000.0,
    };

    /// The catalogue fixture's combat dungeon: the one with objects.
    fn combat() -> Dungeon {
        dungeon_data::Table::fixture()
            .dungeons
            .values()
            .find(|d| !d.objects.is_empty() && d.entry_object.is_some())
            .cloned()
            .expect("the fixture has a combat dungeon with objects")
    }

    /// An invented ore dungeon: two rooms; room 5 holds a beacon at the
    /// origin, two rocks of `ORE_TYPE` and a structure with no radius.
    fn ore() -> Dungeon {
        let obj = |room, id, type_id, pos, radius| Object {
            room,
            id,
            type_id,
            pos,
            radius,
            ..Object::default()
        };
        Dungeon {
            id: ORE_DUNGEON,
            archetype: 97,
            entry_object: Some(7001),
            rooms: vec![
                Room {
                    id: 5,
                    ..Room::default()
                },
                Room {
                    id: 6,
                    ..Room::default()
                },
            ],
            objects: vec![
                obj(5, 7001, 990_400, [0.0; 3], None),
                obj(5, 7002, ORE_TYPE, [5_000.0, 0.0, 0.0], Some(3_000.0)),
                obj(5, 7003, ORE_TYPE, [-8_000.0, 1_000.0, 0.0], Some(9_000.0)),
                obj(5, 7004, 990_401, [0.0, 0.0, 12_000.0], Some(-4.0)),
                obj(6, 7005, ORE_TYPE, [0.0; 3], Some(3_000.0)),
            ],
            room_rocks: vec![
                RoomRock {
                    room: 5,
                    type_id: ORE_TYPE,
                    count: 2,
                },
                RoomRock {
                    room: 6,
                    type_id: ORE_TYPE,
                    count: 1,
                },
            ],
            ..Dungeon::default()
        }
    }

    /// Invented: a gas dungeon whose entry room holds a beacon and two
    /// clouds of `GAS_TYPE`, which its gas resource row names.
    const GAS_DUNGEON: i64 = 999_202;
    const GAS_TYPE: i64 = 990_302;

    fn gas() -> Dungeon {
        let obj = |id, type_id, pos, radius| Object {
            room: 5,
            id,
            type_id,
            pos,
            radius,
            ..Object::default()
        };
        Dungeon {
            id: GAS_DUNGEON,
            archetype: 98,
            entry_object: Some(8001),
            rooms: vec![Room {
                id: 5,
                ..Room::default()
            }],
            objects: vec![
                obj(8001, 990_400, [0.0; 3], None),
                obj(8002, GAS_TYPE, [4_000.0, 0.0, 0.0], Some(400.0)),
                obj(8003, GAS_TYPE, [-6_000.0, 0.0, 0.0], Some(2_000.0)),
            ],
            resources: vec![dungeon_data::Resource {
                kind: dungeon_data::ResourceKind::Gas,
                type_ids: vec![GAS_TYPE],
                ore_band: None,
            }],
            room_rocks: vec![RoomRock {
                room: 5,
                type_id: GAS_TYPE,
                count: 2,
            }],
            ..Dungeon::default()
        }
    }

    /// A world with the catalogue fixture plus [`ore`], an eager field
    /// holding one belt rock of `ORE_TYPE` (so its law is known), and Alice
    /// in space in `SYSTEM`, every push drained.
    fn world() -> Registry {
        let mut table = dungeon_data::Table::fixture();
        table.dungeons.insert(ORE_DUNGEON, ore());
        table.dungeons.insert(GAS_DUNGEON, gas());
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        let belt_rock = Rock {
            id: crate::asteroid::MIN_ROCK_ID,
            type_id: ORE_TYPE,
            belt_id: 40_000_003,
            solar_system_id: 30_990_002,
            quantity: 10,
            radius: LAW.radius(10),
            position: [0.0; 3],
            law: LAW,
        };
        let mut reg = Registry::new(SandboxConfig {
            rocks: crate::asteroid::Field::from_rocks([belt_rock]),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        reg
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

    fn drain(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        let _ = reg.take_pending();
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// The `(name, args)` of every entry of a `DoDestinyUpdate`.
    fn entries(args: &[PyValue]) -> Vec<(Vec<u8>, Vec<PyValue>)> {
        let PyValue::List(es) = &args[0] else {
            panic!("a batch is a list")
        };
        es.iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else { panic!() };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!()
                };
                let name = match &call[0] {
                    PyValue::Bytes(b) => b.to_vec(),
                    other => format!("{other:?}").into_bytes(),
                };
                let PyValue::Tuple(a) = &call[1] else {
                    panic!()
                };
                (name, a.clone())
            })
            .collect()
    }

    /// The slim dicts of one `AddBalls2` entry's arguments.
    fn slim_dicts(args: &[PyValue]) -> Vec<PyValue> {
        let PyValue::Tuple(pair) = &args[0] else {
            panic!("(state, extra)")
        };
        let PyValue::List(extra) = &pair[1] else {
            panic!("extra is a list")
        };
        extra
            .iter()
            .map(|e| match e {
                PyValue::Tuple(t) => t[0].clone(),
                d => d.clone(),
            })
            .collect()
    }

    fn item_id(dict: &PyValue) -> i64 {
        match crate::pyutil::dict_get(dict, b"itemID") {
            Some(PyValue::Int(i)) => *i,
            other => panic!("itemID: {other:?}"),
        }
    }

    /// An object added to a layout with a hole takes the number past the
    /// highest kept one, never `len()`, which a kept object may hold (#196).
    #[test]
    fn an_added_object_never_takes_an_id_a_filtered_layout_still_holds() {
        let mut w = Registry::new(SandboxConfig::default());
        let placed = |n: usize| Placed {
            item_id: object_id(7, n).unwrap(),
            dun_object_id: 9_100 + n as i64,
            type_id: 990_500,
            position: [n as f64, 0.0, 0.0],
            radius: DEFAULT_RADIUS,
            entity_group: None,
            rock: None,
            owner_id: OWNER,
        };
        // Index 1 was dropped by the layout (a stray): 0, 2, 3 are kept.
        let objects = vec![placed(0), placed(2), placed(3)];
        {
            let s = w.ext_mut::<Sites>();
            for p in &objects {
                s.by_item.insert(p.item_id, SITE_ID);
            }
            s.live.insert(
                SITE_ID,
                Instance {
                    site_id: SITE_ID,
                    system: SYSTEM,
                    dungeon_id: ORE_DUNGEON,
                    room_id: None,
                    anchor: ANCHOR,
                    seq: 7,
                    objects,
                },
            );
        }
        let g = add_object(&mut w, SITE_ID, 990_501, [9.0, 9.0, 9.0]).expect("placed");
        assert_eq!(
            g.item_id,
            object_id(7, 4).unwrap(),
            "past the highest, not len()"
        );
        let inst = w.ext::<Sites>().get(SITE_ID).unwrap();
        let ids: BTreeSet<i64> = inst.objects.iter().map(|p| p.item_id).collect();
        assert_eq!(ids.len(), inst.objects.len(), "every id once");
        assert_eq!(
            object(&w, object_id(7, 3).unwrap()).unwrap().0.type_id,
            990_500,
            "the old object keeps its id"
        );
    }

    /// Every object id is in the plan's band, an instance's ids are its own,
    /// and an id past the per-instance cap is refused.
    #[test]
    fn instance_ids_are_inside_the_band_and_never_collide() {
        for seq in [0, 1, SEQS - 1, SEQS, 3 * SEQS + 7] {
            for n in [0, 1, IDS_PER_INSTANCE - 1] {
                let id = object_id(seq, n).expect("in range");
                assert!(is_instance_id(id), "{seq} {n} -> {id}");
            }
            assert_eq!(object_id(seq, IDS_PER_INSTANCE), None);
        }
        let a: BTreeSet<i64> = (0..IDS_PER_INSTANCE)
            .filter_map(|n| object_id(4, n))
            .collect();
        assert!((0..IDS_PER_INSTANCE).all(|n| !a.contains(&object_id(5, n).unwrap())));
        assert_eq!(object_id(SEQS + 4, 0), object_id(4, 0), "the band wraps");
        assert!(!is_instance_id(INSTANCE_END) && !is_instance_id(population::SITE_BASE));
    }

    /// The entry room is the entry object's; with none, the lowest room.
    #[test]
    fn the_entry_room_is_the_entry_objects_room_else_the_lowest() {
        let d = ore();
        assert_eq!(entry_room(&d), Some(5));
        let mut moved = d.clone();
        moved.entry_object = Some(7005);
        assert_eq!(entry_room(&moved), Some(6));
        let mut none = d.clone();
        none.entry_object = None;
        none.rooms.reverse();
        assert_eq!(entry_room(&none), Some(5));
        assert_eq!(entry_room(&Dungeon::default()), None);
    }

    /// **Every entry-room object becomes a ball at `anchor + position`**,
    /// with its type, all in one `AddBalls2` on the clock to the pilots in
    /// the system, and no object of another room is placed.
    #[test]
    fn every_room0_object_becomes_a_ball_at_anchor_plus_position_in_one_addballs2() {
        let mut reg = world();
        let d = combat();
        let mut s = site(d.id);
        s.dungeon_id = d.id;
        assert!(instantiate(&mut reg, &s));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().expect("live");
        let room = entry_room(&d).unwrap();
        let want: Vec<_> = d.objects.iter().filter(|o| o.room == room).collect();
        assert!(!want.is_empty());
        assert_eq!(inst.objects.len(), want.len());
        for (p, o) in inst.objects.iter().zip(&want) {
            assert_eq!(p.dun_object_id, o.id);
            assert_eq!(p.type_id, o.type_id);
            let at = [
                ANCHOR[0] + o.pos[0],
                ANCHOR[1] + o.pos[1],
                ANCHOR[2] + o.pos[2],
            ];
            assert_eq!(p.position, at);
            assert_eq!(Some(p.radius), o.radius);
        }
        let pushes = drain(&mut reg);
        assert_eq!(pushes.len(), 1, "one batch: {pushes:?}");
        let (to, name, args) = &pushes[0];
        assert_eq!((to.as_slice(), *name), (&[ALICE][..], "DoDestinyUpdate"));
        let es = entries(args);
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].0, b"AddBalls2");
        let ids: Vec<i64> = slim_dicts(&es[0].1).iter().map(item_id).collect();
        let placed: Vec<i64> = inst.objects.iter().map(|p| p.item_id).collect();
        assert_eq!(ids, placed);
        assert!(
            !instantiate(&mut reg, &s),
            "a second arrival places nothing"
        );
        assert!(drain(&mut reg).is_empty());
    }

    /// The bootstrap of a pilot who binds at the site after the
    /// instantiation carries the site's balls, and every slim in it still
    /// names a ball. (At the site: a state is the pilot's grid, and a site
    /// is never on the station's -- `crate::grid`.)
    #[test]
    fn every_slim_has_a_ball() {
        let mut reg = world();
        reg.roster_mut().leave(ALICE);
        let d = ore();
        assert!(instantiate(&mut reg, &site(d.id)));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        let b = crate::destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
        assert!(b.every_slim_has_a_ball());
        let balls: BTreeSet<i64> = b.park.balls.iter().map(|x| x.ball_id).collect();
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        for p in &inst.objects {
            assert!(balls.contains(&p.item_id), "{} has no ball", p.item_id);
        }
        assert!(b.slims.iter().any(|s| s.item_id == inst.objects[0].item_id));
    }

    /// **The bind's state is the old path's** with a site on the grid and
    /// with the pilot far from it: sites are collected once, then filtered.
    #[test]
    fn a_bind_with_a_site_is_the_old_paths_state() {
        crate::test_clock::freeze(std::time::Duration::from_secs(1_800_000_000));
        for at in [ANCHOR, [0.0; 3]] {
            let mut reg = world();
            reg.roster_mut().leave(ALICE);
            let d = ore();
            assert!(instantiate(&mut reg, &site(d.id)));
            reg.roster_mut().arrive(Presence::arriving(
                ALICE,
                ALICE_SHIP,
                SYSTEM,
                Motion::arrived(at, [0.0; 3]),
            ));
            let b = crate::destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
            let old = crate::destiny::bootstrap_oracle(&reg, ALICE, b.stamp, None).expect("oracle");
            assert_eq!(format!("{:?}", b.to_push()), format!("{:?}", old.to_push()));
            let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
            let held: BTreeSet<i64> = b.park.balls.iter().map(|x| x.ball_id).collect();
            let first = inst
                .objects
                .iter()
                .find(|p| p.rock.is_none())
                .expect("an object");
            assert_eq!(held.contains(&first.item_id), at == ANCHOR);
        }
        crate::test_clock::thaw();
    }

    /// **Every site slim carries `dunObjectID` and `entityGroupID`**, in the
    /// `AddBalls2` and in the bootstrap; `dunObjectID` is the catalogue's
    /// object id; no slim carries `objectiveTargetGroup` (erratum 12: the
    /// lane drops it).
    #[test]
    fn every_site_slim_carries_dun_object_id_and_entity_group_id() {
        let mut reg = world();
        let d = ore();
        assert!(instantiate(&mut reg, &site(d.id)));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let pushes = drain(&mut reg);
        let es = entries(&pushes[0].2);
        for dict in slim_dicts(&es[0].1) {
            let id = item_id(&dict);
            let p = inst.objects.iter().find(|p| p.item_id == id).unwrap();
            assert_eq!(
                crate::pyutil::dict_get(&dict, KEY_DUN_OBJECT.as_bytes()),
                Some(&PyValue::Int(p.dun_object_id))
            );
            assert!(crate::pyutil::dict_get(&dict, KEY_ENTITY_GROUP.as_bytes()).is_some());
            assert!(crate::pyutil::dict_get(&dict, b"objectiveTargetGroup").is_none());
        }
        let ids: Vec<i64> = inst.objects.iter().map(|p| p.item_id).collect();
        let extras = crate::slim_extra::for_slims(&reg, &ids);
        assert_eq!(extras.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        for (_, kv) in &extras {
            let keys: Vec<&str> = kv.iter().map(|(k, _)| *k).collect();
            // dunRotation only where the catalogue has one.
            assert_eq!(
                keys[..4],
                [
                    KEY_DUN_OBJECT,
                    KEY_ENTITY_GROUP,
                    KEY_DUN_ID,
                    KEY_DUN_POSITION
                ]
            );
            assert!(
                keys.len() == 4 || keys[4..] == [KEY_DUN_ROTATION],
                "{keys:?}"
            );
            assert!(kv.contains(&(KEY_DUN_ID, PyValue::Int(inst.dungeon_id))));
        }
        assert!(slim_extras(&reg, &[ALICE_SHIP, 1]).is_empty());
    }

    /// A tagged NPC's slim carries its tag; vacating the site forgets it.
    #[test]
    fn a_tagged_npc_carries_its_object_and_group() {
        let mut reg = world();
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        let npc = 7_000_000_123;
        let t = Tag {
            site_id: SITE_ID,
            dun_object_id: 7001,
            entity_group: Some(70_001),
        };
        tag(&mut reg, npc, t);
        assert_eq!(
            slim_extras(&reg, &[npc]),
            vec![(
                npc,
                vec![
                    (KEY_DUN_OBJECT, PyValue::Int(7001)),
                    (KEY_ENTITY_GROUP, PyValue::Int(70_001)),
                ]
            )]
        );
        assert!(vacate(&mut reg, SITE_ID));
        assert!(slim_extras(&reg, &[npc]).is_empty());
    }

    /// A placed wreck's slim says whether it holds anything (the client's
    /// empty-wreck icon); any other placed object carries no such key.
    #[test]
    fn a_placed_wreck_says_whether_it_is_empty() {
        const WRECK_TYPE: i64 = 990_186_101;
        let mut dungeon = ore();
        dungeon.objects.push(Object {
            room: 5,
            id: 7009,
            type_id: WRECK_TYPE,
            pos: [500.0, 0.0, 0.0],
            radius: Some(50.0),
            ..Object::default()
        });
        let mut table = dungeon_data::Table::fixture();
        table.dungeons.insert(ORE_DUNGEON, dungeon);
        let rows = format!(
            "{}type\t{WRECK_TYPE}\t{}\t2\t0\t1\t0.0\t1.0\t1.0\t1.0\t0\t\t\tFixture Wreck\n",
            crate::typedata::FIXTURE,
            crate::combat::GROUP_WRECK
        );
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        c.insert(
            "typedata",
            crate::typedata::Installed::build(crate::typedata::Table::parse(&rows).unwrap()),
        );
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        drain(&mut reg);
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let wreck = inst
            .objects
            .iter()
            .find(|p| p.type_id == WRECK_TYPE)
            .unwrap()
            .item_id;
        let beacon = inst
            .objects
            .iter()
            .find(|p| p.dun_object_id == 7001)
            .unwrap()
            .item_id;
        let is_empty = |reg: &Registry, id| {
            slim_extras(reg, &[id])
                .into_iter()
                .next()
                .and_then(|(_, kv)| {
                    kv.into_iter()
                        .find(|(k, _)| *k == KEY_IS_EMPTY)
                        .map(|(_, v)| v)
                })
        };
        assert_eq!(is_empty(&reg, wreck), Some(PyValue::Bool(true)));
        assert_eq!(is_empty(&reg, beacon), None, "only a wreck says");
    }

    /// **Ore rocks go through `asteroid::Field`**: each entry-room rock is
    /// a field rock with its type, at its place, whose quantity is the law's
    /// the authored value (a quantity) and whose radius is the law of that
    /// quantity; a non-rock keeps its own or the default radius; the
    /// bootstrap's site balls (`balls_in`) leave the rocks to the field.
    /// A cloaked object leaves the pilots' parks and the bootstrap's list and
    /// cannot be locked; uncloaked, it comes back with `AddBalls2` and the
    /// `effects.Uncloak` flash. An untargetable rock cannot be locked until it
    /// is made targetable again.
    #[test]
    fn a_cloaked_object_is_held_back_and_an_untargetable_one_refuses_a_lock() {
        let mut reg = world();
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        drain(&mut reg);
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let can = inst
            .objects
            .iter()
            .find(|p| p.dun_object_id == 7004)
            .unwrap()
            .item_id;
        let rock = inst
            .objects
            .iter()
            .find(|p| p.dun_object_id == 7002)
            .unwrap()
            .item_id;

        assert!(cloak(&mut reg, can));
        assert!(!cloak(&mut reg, can), "once");
        let pushes = drain(&mut reg);
        assert!(
            pushes
                .iter()
                .any(|(to, n, _)| *n == "DoDestinyUpdate" && to == &vec![ALICE])
        );
        assert!(balls_in(&reg, SYSTEM).iter().all(|(b, _)| b.ball_id != can));
        assert!(crate::targeting::lock(&mut reg, ALICE, can).is_err());

        assert!(uncloak(&mut reg, can));
        let pushes = drain(&mut reg);
        assert_eq!(pushes.len(), 2, "the ball, then the flash: {pushes:?}");
        let flash = format!("{:?}", pushes[1].2);
        let guid = format!("{:?}", PyValue::Bytes(FX_UNCLOAK.as_bytes().to_vec()));
        assert!(flash.contains(&guid), "{flash}");
        assert!(balls_in(&reg, SYSTEM).iter().any(|(b, _)| b.ball_id == can));

        assert!(set_targetable(&mut reg, rock, false));
        assert!(crate::targeting::lock(&mut reg, ALICE, rock).is_err());
        assert!(set_targetable(&mut reg, rock, true));
        assert!(crate::targeting::lock(&mut reg, ALICE, rock).is_ok());
    }

    #[test]
    fn ore_rocks_go_through_the_field_and_round_trip_through_the_law() {
        let mut reg = world();
        let d = ore();
        assert!(instantiate(&mut reg, &site(d.id)));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let rocks: Vec<&Placed> = inst.objects.iter().filter(|p| p.rock.is_some()).collect();
        assert_eq!(rocks.len(), 2, "room 5's two ore objects, not room 6's");
        for p in &rocks {
            let o = d.objects.iter().find(|o| o.id == p.dun_object_id).unwrap();
            let r = *reg.rocks().get(p.item_id).expect("in the field");
            assert_eq!(
                (r.type_id, r.position, r.solar_system_id),
                (ORE_TYPE, p.position, SYSTEM)
            );
            assert_eq!(r.radius, LAW.radius(r.quantity));
            assert_eq!(r.quantity, o.radius.unwrap().round() as i64);
            assert_eq!(p.radius, r.radius);
        }
        let other = inst
            .objects
            .iter()
            .find(|p| p.dun_object_id == 7004)
            .unwrap();
        assert_eq!(
            other.radius, DEFAULT_RADIUS,
            "a negative radius is no radius"
        );
        let bs: Vec<i64> = balls_in(&reg, SYSTEM)
            .iter()
            .map(|(b, _)| b.ball_id)
            .collect();
        assert_eq!(bs.len(), inst.objects.len() - rocks.len());
        assert!(rocks.iter().all(|p| !bs.contains(&p.item_id)));
        assert!(balls_in(&reg, SYSTEM + 1).is_empty());
    }

    /// **Gas clouds are rocks with the cloud law** (SOC-B2): each cloud the
    /// dungeon's gas row names goes into the field with its authored value
    /// as the quantity and `quantity x type radius / 10` as the radius
    /// (`ballRadius.py:16-17` [V]), though no type table gives the
    /// type a law; the beacon stays a plain ball.
    #[test]
    fn gas_clouds_go_through_the_field_with_the_gas_law() {
        let mut reg = world();
        let d = gas();
        assert!(instantiate(&mut reg, &site(d.id)));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let clouds: Vec<&Placed> = inst.objects.iter().filter(|p| p.rock.is_some()).collect();
        assert_eq!(clouds.len(), 2);
        for p in &clouds {
            let o = d.objects.iter().find(|o| o.id == p.dun_object_id).unwrap();
            let r = *reg.rocks().get(p.item_id).expect("in the field");
            // Invented type radius 100 m (no type table in this world).
            assert_eq!((r.type_id, r.law), (GAS_TYPE, Law::cloud(100.0)));
            assert_eq!(r.quantity, o.radius.unwrap().round() as i64);
            assert_eq!(r.radius, r.quantity as f64 * 100.0 / 10.0);
            assert_eq!(r.law.quantity(r.radius).round() as i64, r.quantity);
        }
        assert!(
            inst.objects
                .iter()
                .any(|p| p.dun_object_id == 8001 && p.rock.is_none())
        );
    }

    /// The law rule: an ore's own law first, then the gas law for a type
    /// the dungeon or the item table calls gas, else no rock.
    #[test]
    fn law_in_prefers_the_ore_law_then_gas() {
        let cloud = Some(Law::cloud(250.0));
        assert_eq!(law_in(Some(LAW), true, true, Some(250.0)), Some(LAW));
        assert_eq!(law_in(None, true, false, Some(250.0)), cloud);
        assert_eq!(law_in(None, false, true, Some(250.0)), cloud);
        assert_eq!(law_in(None, false, false, Some(250.0)), None);
        assert_eq!(
            law_in(None, true, false, None),
            Some(Law::cloud(CLOUD_TYPE_RADIUS))
        );
        // The linear law: 40 units of a 250 m type is 40 x 25 = 1000 m;
        // an ore law is untouched by the cloud branch.
        let c = Law::cloud(250.0);
        assert_eq!(c.radius(40), 1_000.0);
        assert_eq!(c.radius(-3), 25.0, "negative reads as 1");
        assert_eq!(c.quantity(1_000.0), 40.0);
        assert_eq!(c.snap(1_000.0), (40, 1_000.0));
        assert!(!LAW.is_cloud());
        assert_eq!(LAW.radius(1), 100.0);
        assert_eq!(gas_types(&gas()), BTreeSet::from([GAS_TYPE]));
        assert!(gas_types(&ore()).is_empty());
    }

    /// **Vacating sends one `RemoveBalls`** naming every ball still there
    /// (a mined-out rock is not), and takes the rocks out of the field.
    #[test]
    fn vacating_sends_one_remove_balls_and_empties_the_field() {
        let mut reg = world();
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let rock = inst.objects.iter().find(|p| p.rock.is_some()).unwrap();
        let mined = reg.rocks_mut().mine(rock.item_id, i64::MAX).unwrap();
        assert!(mined.depleted());
        drain(&mut reg);
        assert!(vacate(&mut reg, SITE_ID));
        assert!(!vacate(&mut reg, SITE_ID));
        let pushes = drain(&mut reg);
        assert_eq!(pushes.len(), 1);
        let es = entries(&pushes[0].2);
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].0, b"RemoveBalls");
        let PyValue::List(ids) = &es[0].1[0] else {
            panic!("ids")
        };
        let ids: Vec<i64> = ids
            .iter()
            .map(|v| match v {
                PyValue::Int(i) => *i,
                _ => panic!(),
            })
            .collect();
        let want: Vec<i64> = inst
            .objects
            .iter()
            .map(|p| p.item_id)
            .filter(|id| *id != rock.item_id)
            .collect();
        assert_eq!(ids, want);
        assert!(
            inst.objects
                .iter()
                .all(|p| reg.rocks().get(p.item_id).is_none())
        );
        assert!(balls_in(&reg, SYSTEM).is_empty());
        assert!(reg.ext::<Sites>().of_item(want[0]).is_none());
    }

    /// The warp resolver answers a placed object and nothing else.
    #[test]
    fn the_warp_resolver_answers_only_placed_objects() {
        let mut reg = world();
        assert_eq!(object_target(&reg, ALICE, INSTANCE_BASE, None), None);
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        let inst = reg.ext::<Sites>().get(SITE_ID).cloned().unwrap();
        let p = inst.objects[0];
        let t = object_target(&reg, ALICE, p.item_id, None).expect("a target");
        assert_eq!(
            (t.position, t.radius, t.warp_in),
            (p.position, p.radius, None)
        );
        assert_eq!(object_target(&reg, ALICE, ALICE_SHIP, None), None);
        let via = crate::warp_subject::resolve(
            crate::warp_subject::RESOLVERS,
            &reg,
            crate::space_state::WARP_SUBJECT_ITEM,
            ALICE,
            p.item_id,
            None,
        );
        assert_eq!(via, Some(t));
    }

    /// Arrival: a live site with a pilot inside the range, not yet placed.
    #[test]
    fn arrival_needs_a_pilot_in_range_of_a_live_unplaced_site() {
        let a = site(1);
        let mut done = site(2);
        done.site_id += 1;
        done.state = population::State::Done;
        let mut placed = site(3);
        placed.site_id += 2;
        let sites = [a.clone(), done, placed.clone()];
        let have = BTreeSet::from([placed.site_id]);
        let inside = [ANCHOR[0] + ARRIVAL_RANGE, ANCHOR[1], ANCHOR[2]];
        let outside = [ANCHOR[0] + ARRIVAL_RANGE * 1.01, ANCHOR[1], ANCHOR[2]];
        let ids = |p: &[[f64; 3]]| -> Vec<i64> {
            arrivals(&sites, &have, p)
                .iter()
                .map(|s| s.site_id)
                .collect()
        };
        assert_eq!(ids(&[inside]), vec![a.site_id]);
        assert!(ids(&[outside]).is_empty());
        assert!(ids(&[]).is_empty());
    }

    /// A site whose dungeon the catalogue lacks is recorded with no
    /// objects and sends nothing, so the stage never retries it.
    #[test]
    fn an_unknown_dungeon_is_recorded_empty_and_sends_nothing() {
        let mut reg = world();
        assert!(instantiate(&mut reg, &site(-1)));
        assert!(reg.ext::<Sites>().get(SITE_ID).unwrap().objects.is_empty());
        assert!(drain(&mut reg).is_empty());
        assert!(!instantiate(&mut reg, &site(-1)));
    }

    /// **The stage, end to end**: a pilot at a live site's anchor gets its
    /// entry room on the next tick (one `AddBalls2`); once the site is
    /// completed the next check vacates it (one `RemoveBalls`).
    #[test]
    fn the_stage_places_a_reached_site_and_vacates_it_when_it_stops_being_live() {
        use crate::celestial::{Catalogue, Universe};
        let systems: Vec<i64> = Universe::fixture().systems().map(|s| s.id).collect();
        let mut table = dungeon_data::Table::fixture();
        // One more ore anomaly with no resource row: it fits any band's
        // `*` difficulty (CP16's Star rule), so a high-sec slot is filled.
        table.dungeons.insert(
            ORE_DUNGEON,
            Dungeon {
                entry_type: Some(dungeon_data::ENTRY_ANOMALY),
                room_rocks: Vec::new(),
                ..ore()
            },
        );
        for d in table.dungeons.values_mut() {
            d.entry_object = Some(1);
            d.objects = vec![Object {
                room: 1,
                id: 1,
                type_id: 990_400,
                ..Object::default()
            }];
        }
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeon_data", table);
        let mut text = String::new();
        for (n, s) in systems.iter().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\n",
                40_000_000 + n
            );
        }
        let mut reg = Registry::new(SandboxConfig {
            celestials: Catalogue::parse(&text),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        reg.deltas_mut().set_world(crate::delta::World {
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        let now = reg.world().birth;
        let Some(target) = systems
            .iter()
            .flat_map(|s| population::sites(&reg, *s, now).sites)
            .find(|s| s.state == population::State::Live)
        else {
            panic!("a fixture system has a live site")
        };
        let system = target.system;
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived(target.anchor, [0.0; 3]),
        ));
        drain(&mut reg);
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        tick(&mut reg, Instant::now(), now);
        let inst = reg
            .ext::<Sites>()
            .get(target.site_id)
            .cloned()
            .expect("placed");
        assert_eq!(inst.objects.len(), 1);
        let pushes = drain(&mut reg);
        assert_eq!(pushes.len(), 1);
        assert_eq!(entries(&pushes[0].2)[0].0, b"AddBalls2");
        assert!(population::complete(&mut reg, system, target.site_id, now));
        tick(&mut reg, Instant::now(), now);
        assert!(
            reg.ext::<Sites>().get(target.site_id).is_some(),
            "not due yet"
        );
        let later = now + CHECK_EVERY;
        crate::liveness::refresh(&mut reg, Instant::now(), later);
        tick(&mut reg, Instant::now(), later);
        assert!(reg.ext::<Sites>().get(target.site_id).is_none());
        let pushes = drain(&mut reg);
        let removes: Vec<_> = pushes
            .iter()
            .flat_map(|p| entries(&p.2))
            .filter(|e| e.0 == b"RemoveBalls")
            .collect();
        assert_eq!(removes.len(), 1);
    }

    /// Sequence numbers skip live instances and wrap.
    #[test]
    fn sequence_numbers_skip_live_instances() {
        let mut reg = world();
        reg.ext_mut::<Sites>().next_seq = SEQS - 1;
        assert!(instantiate(&mut reg, &site(ORE_DUNGEON)));
        let mut b = site(ORE_DUNGEON);
        b.site_id += 1;
        reg.ext_mut::<Sites>().next_seq = SEQS - 1;
        assert!(instantiate(&mut reg, &b));
        let s = reg.ext::<Sites>();
        assert_eq!(s.get(SITE_ID).unwrap().seq, SEQS - 1);
        assert_eq!(s.get(b.site_id).unwrap().seq, 0);
    }

    /// **TICK-1**: an arrivals check is skipped only where it can do
    /// nothing -- nobody flying there and no open site to vacate.
    #[test]
    fn an_arrivals_check_is_skipped_only_with_nobody_and_no_instance() {
        use crate::liveness::{Liveness, Reasons};
        let mut w = Registry::new(SandboxConfig::default());
        let at = |w: &mut Registry, reasons: Reasons| {
            let rows = [(SYSTEM, reasons)].into_iter().collect();
            w.ext_mut::<Liveness>().apply(&rows, 1);
        };
        at(&mut w, Reasons::PILOT);
        assert!(!nothing_to_check(&w, SYSTEM), "a pilot may reach a site");
        at(&mut w, Reasons::BALLS);
        assert!(nothing_to_check(&w, SYSTEM), "rats alone");
        w.ext_mut::<Sites>().live.insert(
            SITE_ID,
            Instance {
                site_id: SITE_ID,
                system: SYSTEM,
                dungeon_id: ORE_DUNGEON,
                room_id: None,
                anchor: ANCHOR,
                seq: 1,
                objects: Vec::new(),
            },
        );
        assert!(!nothing_to_check(&w, SYSTEM), "an open site may be vacated");
        assert!(nothing_to_check(&w, SYSTEM + 1), "another system is cold");
    }

    /// Every runtime id band this server mints in the item, ball, site and
    /// escrow-location space, as `(name, start, end)` with `end` one past
    /// the last id. **A new band goes in this list.** The list is hand-kept:
    /// a band nobody adds here is not checked. Where a module has no end
    /// const, the width is a design cap of this server's allocator (not a
    /// client fact); the system-id bound is `ess::SYSTEM_ID_END` **[A]**
    /// (falsified by a system id at or above it in the bundle). Left out as
    /// other namespaces: mail list ids and the market's order and
    /// transaction ids. Escrow locations are listed because a location id
    /// and an item id share one space.
    fn id_bands() -> Vec<(&'static str, i64, i64)> {
        use crate::{abyss, asteroid, bubble, command_nodes, contracts, cyno, econ_exchange};
        use crate::{ess, item, missile, moon_belt, moon_scan, npc, planet, population, scan};
        use crate::{item_trader, sov, trade, war};
        // Design caps of the allocators below (no end const of their own).
        const SPAN: i64 = 100_000_000_000;
        // `service::market::ESCROW_TICKET_BASE` (private there): market
        // escrow locations are `item::escrow_location(1e12 + order_id)`.
        let market_escrow = item::ESCROW_BASE + 1_000_000_000_000;
        let hypernet_escrow = item::ESCROW_BASE + crate::hypernet::ESCROW_RAFFLE_BASE;
        let sys = ess::SYSTEM_ID_END;
        vec![
            ("static and npc-owner ids", 0, asteroid::MIN_ROCK_ID),
            (
                "asteroid rocks",
                asteroid::MIN_ROCK_ID,
                asteroid::END_ROCK_ID,
            ),
            // The store counter is persisted and unbounded; this cap leaves
            // 6.9e9 items of headroom below the npc ships.
            ("player items", item::MIN_PLAYER_ITEM, npc::MIN_NPC_ID),
            ("npc ships", npc::MIN_NPC_ID, npc::END_NPC_ID),
            (
                "customs offices",
                planet::FIRST_CUSTOMS_OFFICE_ID,
                abyss::SITE_BASE,
            ),
            ("abyss sites", abyss::SITE_BASE, abyss::SITE_END),
            (
                "mission sites",
                crate::mission::SITE_BASE,
                crate::mission::SITE_END,
            ),
            (
                "population sites",
                population::SITE_BASE,
                population::SITE_END,
            ),
            ("site instances", INSTANCE_BASE, INSTANCE_END),
            ("war hq items", war::HQ_ITEM_BASE, sov::STAND_IN_HUB_BASE),
            (
                "air npe sites",
                crate::air_npe::NPE_SITE_BASE,
                crate::air_npe::NPE_SITE_END,
            ),
            (
                "sov stand-in hubs",
                sov::STAND_IN_HUB_BASE,
                sov::STAND_IN_HUB_BASE + sys,
            ),
            (
                "contract escrow",
                item::escrow_location(contracts::FIRST_CONTRACT_ID),
                planet::FIRST_PIN_ID,
            ),
            ("planet pins", planet::FIRST_PIN_ID, market_escrow),
            ("market escrow", market_escrow, trade::TICKET_BASE),
            ("trade tickets", trade::TICKET_BASE, ess::BEACON_ID_BASE),
            (
                "ess beacons",
                ess::BEACON_ID_BASE,
                ess::BEACON_ID_BASE + sys,
            ),
            // HN-1 (#203): raffle escrow locations, `item::escrow_location(
            // hypernet::ESCROW_RAFFLE_BASE + id)`. Width is a design cap of the
            // raffle id counter: it has about 9.1e10 of headroom before an escrow
            // location meets the ess band.
            (
                "hypernet escrow",
                hypernet_escrow,
                hypernet_escrow + SPAN / 2,
            ),
            ("ess", ess::ESS_ID_BASE, ess::ESS_ID_BASE + sys),
            (
                "item traders",
                item_trader::TRADER_ID_BASE,
                item_trader::TRADER_ID_END,
            ),
            (
                "scan probes",
                scan::PROBE_ID_BASE,
                scan::PROBE_ID_BASE + SPAN,
            ),
            ("missiles", missile::ID_BASE, missile::ID_END),
            (
                "command nodes",
                command_nodes::NODE_ID_BASE,
                command_nodes::NODE_ID_END,
            ),
            ("moon rocks", moon_belt::MOON_BASE, moon_belt::MOON_END),
            (
                "moon survey sites",
                moon_belt::MOON_END,
                moon_belt::MOON_END + moon_belt::SEQS as i64,
            ),
            ("survey probes", moon_scan::ID_BASE, moon_scan::ID_END),
            ("cyno fields", cyno::FIELD_ID_BASE, cyno::FIELD_ID_END),
            ("bubbles", bubble::ID_BASE, bubble::ID_END),
            (
                "npc orders",
                econ_exchange::NPC_ORDER_BASE,
                econ_exchange::NPC_ORDER_END,
            ),
        ]
    }

    /// No two of the listed id bands overlap, and none is empty.
    #[test]
    fn id_bands_do_not_overlap() {
        let bands = id_bands();
        for (name, lo, hi) in &bands {
            assert!(lo < hi, "{name} is empty or reversed");
        }
        for (i, (an, alo, ahi)) in bands.iter().enumerate() {
            for (bn, blo, bhi) in &bands[i + 1..] {
                assert!(
                    ahi <= blo || bhi <= alo,
                    "{an} {alo}..{ahi} overlaps {bn} {blo}..{bhi}"
                );
            }
        }
    }
}
