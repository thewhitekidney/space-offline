//! **Combat sites with more than one room**: the acceleration gate, the last
//! room's loot cache and the escalation seam.
//!
//! Packet Y-COMBAT (`docs/archive/plan-deep-queue.md`, wave Y). Lane note:
//! `docs/lanes/y-combat.md`. `site.rs` places a site's entry room and
//! `site_spawn.rs` fills it with squads; this module is what lies *between*
//! the rooms of a dungeon that has several, and what a finished site leaves
//! behind.
//!
//! # What the dungeon data carries, and what it does not
//!
//! **[V-data]** (`out/content/dungeons`, 5,516 dungeons; 507 of archetype 24
//! "combat sites", 384 of 31 "escalating path"):
//!
//! - rooms have ids and `R0..Rn`, `Room1`, `Lobby`... short names; **only
//!   the room that holds the `entryObjectID` object is the way in**, and it
//!   is the *last* id in 5,000-odd dungeons (dungeon 43's `R0` is room
//!   14186, after `R1`..`R4`). A multi-room dungeon's entry room holds just
//!   a beacon and an acceleration gate;
//! - `connections[]` (`gates.tsv`) name a gate by its `fromObjectID` and
//!   carry the ship restriction; **no field names the room a gate leads
//!   to** (`docs/re/dungeon-model.md` §9.1);
//! - every one of those gates is an object of group 366 (`groupWarpGate`),
//!   which is also how the client tells a gate (`celestialCheckers.
//!   IsWarpGate`).
//!
//! **[I]** the edge: [`chain`] orders the rooms *entry first, then by
//! ascending room id* and sends the gate in the k-th room to the (k+1)-th.
//! It is accepted only when it explains the whole dungeon: the entry room
//! and every later room but the last hold exactly one gate, and the last
//! holds none. That is true of 1,551 of the 1,718 multi-room dungeons in
//! the bundle (the rest, including 110 with two gates in a room, keep their
//! entry room only, as before). Falsified by a gate that drops a pilot in a
//! room whose guards are not the ones its authoring shows.
//!
//! # The gate
//!
//! The client activates a gate with `PerformSessionChange(...,
//! keeper.ActivateAccelerationGate, gateID, violateSafetyTimer=1)`
//! (`movementFunctions.RealActivateAccelerationGate`) **[V]**, after
//! approaching to `gateActivationRange` or `maxStargateJumpingDistance`
//! (`menusvc.ActivateAccelerationGate`) **[V]**. [`activate`] is that call:
//!
//! 1. the gate must be a connection of a live site's dungeon, and the ship
//!    within [`jump::MAX_JUMP_DISTANCE`] of it (plus the loot range's slack);
//! 2. **the ship must be admitted**, by the client's own rule
//!    (`evedungeons/client/util.GetDungeonShipRestrictions`) **[V]**: a
//!    connection with `allowedShipsList` admits only the ships in that
//!    typelist; one with none admits every ship but the
//!    [`BLACKLISTED_GROUPS`] (`ACCELERATION_GATE_BLACKLISTED_GROUPS`).
//!    Refusals are the client's own keys, `DunShipCannotWarp` and
//!    `DunBlacklistCannotWarp`, whose catcher opens the ship-restrictions
//!    window **[V]**. The data's gates carry no race list or fitted-types
//!    list in either archetype (0 of 1,085 gates) **[V-data]**, so neither
//!    is enforced ([`Admit`]);
//! 3. the movement guards of a warp apply (a scrammed ship does not go);
//! 4. the next room is placed ([`site::open_room`]) at [`room_anchor`] and
//!    the ship **warps** to its warp-in point. The real gate plays the
//!    `WarpGateEffect` and moves the ship without a session change; we have
//!    no witness of its exact packets, so the ship takes an ordinary warp
//!    **[A]**, falsified by a client that shows the gate's own animation.
//!
//! # The cache
//!
//! When a site with squads completes, a cache lands in its last room: a
//! wreck holding stacks drawn from the bundle's `npc/loot.tsv` pools for the
//! dungeon's faction ([`cache_stacks`]). The dungeon data carries no loot of
//! its own (`additionalLootId` is set on 5 of 260,242 objects and points at
//! nothing in the corpus, `docs/re/dungeon-model.md` §6) **[V-data]**; the
//! rule below is ours **[A]**.
//!
//! # The escalation seam
//!
//! A completed site may *escalate* ([`escalation_for`]): an
//! archetype-31 dungeon of the same faction is picked and recorded as an
//! [`Escalation`] for Y-ESCAL to turn into a notification and a site
//! ([`take_escalations`]). Nothing in the data says which dungeon escalates
//! to which **[V-data]**; the pairing and the chance are **[A]**.
//!
//! **Not persisted**, like the sites.

use crate::damage::BlueTime;
use crate::dungeon_data::{self, Dungeon, TypeList};
use crate::movement_guard::Refusal as GuardRefusal;
use crate::npc_data::{Loot, Tier};
use crate::seed::{Kind, stream};
use crate::service::{Registry, ServiceError};
use crate::site::{self, Sites};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "combat_site",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(CombatSites::default())
}

/// `dunArchetypeCombatSites` **[V]** (`population::COMBAT_ARCHETYPE`).
pub const COMBAT_ARCHETYPE: i64 = crate::population::COMBAT_ARCHETYPE;
/// `dunArchetypeEscalatingPathDungeons` **[V]** (C3 §1, `appConst`).
pub const ESCALATION_ARCHETYPE: i64 = 31;

/// The slim key of an acceleration gate's activation range: the client
/// approaches that near before it calls the gate (`menusvc.
/// ActivateAccelerationGate` reads `getattr(slimItem, 'gateActivationRange',
/// maxStargateJumpingDistance)`) **[V]**. Y-COMBAT sends it on every gate
/// object (`site::is_gate`), valued as the default, so the real client
/// behaves as before and a bot can tell a gate from a structure.
pub const KEY_GATE_RANGE: &str = "gateActivationRange";

/// The distance between the anchors of a site's rooms, along +y, metres.
///
/// `[A]`: more than `grid::GRID_LEAVE` (4.3 Mm) plus the widest room's
/// extent (995 rooms of 100 to 1,000 km, `dungeon-model.md` §2 [V-data]), so
/// a pilot in one room never has another room's balls on his grid. +y keeps
/// the stack off the ecliptic, where the planets are. Falsified by a pilot
/// who sees the next room's structures from the last one, or a site whose
/// rooms land on a celestial.
pub const ROOM_SPACING: f64 = 6_000_000.0;

const _: () = assert!(ROOM_SPACING > crate::grid::GRID_LEAVE + 1_000_000.0);

/// How far past the jump range a gate call is still taken, metres. `[A]`:
/// the client measures on its own extrapolated park and we on ours (the loot
/// range's slack, `combat::LOOT_RANGE_SLACK`).
pub const GATE_SLACK: f64 = crate::combat::LOOT_RANGE_SLACK;

/// `ACCELERATION_GATE_BLACKLISTED_GROUPS` **[V]**
/// (`evedungeons/common/constants.py`, ids from `inventorycommon/const.py`):
/// capsule, dreadnought, supercarrier, titan, freighter, carrier, lancer
/// dreadnought, force auxiliary, jump freighter, capital industrial ship,
/// command carrier.
pub const BLACKLISTED_GROUPS: [i64; 11] = [29, 485, 659, 30, 513, 547, 4594, 1538, 902, 883, 5120];

/// The hull classes named in `DunBlacklistCannotWarp`'s list. The text is
/// ours **[A]** (the client formats `{list}` from what the server sends).
pub const BLACKLISTED_NAMES: &str = "Capsule, Dreadnought, Supercarrier, Titan, Freighter, \
Carrier, Lancer Dreadnought, Force Auxiliary, Jump Freighter, Capital Industrial Ship, \
Command Carrier";

/// `categoryShip` **[V]**.
pub const CATEGORY_SHIP: i64 = 6;

/// How many hull names `DunShipCannotWarp`'s whitelist text carries. `[A]`.
pub const WHITELIST_NAMES: usize = 16;

/// How long a pilot who went through a gate still counts as inside the site
/// while he warps to the next room. `[A]`: a ceiling over the warp across
/// [`ROOM_SPACING`] (a battleship's align and a 6 Mm warp take well under a
/// minute); without it the tracker would tell the client it left the
/// dungeon in mid-gate. Falsified by a client that resets its objectives
/// between rooms.
pub const TRANSIT_GRACE: std::time::Duration = std::time::Duration::from_secs(90);

/// Whether `character` is between two rooms of `site_id` at `now`
/// (`dungeon_tracking` keeps him inside through the warp).
pub fn in_transit(w: &Registry, character: i64, site_id: i64, now: std::time::Instant) -> bool {
    w.ext::<CombatSites>()
        .transit
        .get(&character)
        .is_some_and(|(s, until)| *s == site_id && now < *until)
}

/// A pilot's transit is over (he landed).
pub fn landed(w: &mut Registry, character: i64) {
    w.ext_mut::<CombatSites>().transit.remove(&character);
}

/// How many escalations wait for Y-ESCAL before the oldest is dropped.
/// `[A]`: a bound, so an undrained queue cannot grow without end.
pub const ESCALATION_QUEUE: usize = 256;

// ---------------------------------------------------------------------------
// The chain
// ---------------------------------------------------------------------------

/// One gate and the rooms either side of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hop {
    /// The gate's catalogue `objectID` (a connection's `fromObjectID`).
    pub gate_object: i64,
    pub from: i64,
    pub to: i64,
}

/// A dungeon's rooms in the order the gates lead through them: the entry
/// room, then the others by ascending id. `None` when the dungeon has one
/// room or none.
pub fn room_order(d: &Dungeon) -> Option<Vec<i64>> {
    let entry = site::entry_room(d)?;
    let rooms: BTreeSet<i64> = d
        .rooms
        .iter()
        .map(|r| r.id)
        .chain(d.objects.iter().map(|o| o.room))
        .collect();
    if rooms.len() < 2 || !rooms.contains(&entry) {
        return None;
    }
    Some(
        std::iter::once(entry)
            .chain(rooms.into_iter().filter(|r| *r != entry))
            .collect(),
    )
}

/// **The gate chain** (module header, **[I]**): `order[k]`'s gate leads to
/// `order[k + 1]`. `None` unless the dungeon has several rooms and the rule
/// explains all of them: every room but the last holds exactly one gate (an
/// object some connection names), the last none.
pub fn chain(d: &Dungeon) -> Option<Vec<Hop>> {
    let order = room_order(d)?;
    let gates_in = |room: i64| -> Vec<i64> {
        d.gates
            .iter()
            .filter(|g| {
                d.objects
                    .iter()
                    .any(|o| o.id == g.from_object && o.room == room)
            })
            .map(|g| g.from_object)
            .collect()
    };
    let mut hops = Vec::new();
    for (k, pair) in order.windows(2).enumerate() {
        let gates = gates_in(pair[0]);
        let [gate] = gates[..] else {
            return None;
        };
        hops.push(Hop {
            gate_object: gate,
            from: order[k],
            to: pair[1],
        });
    }
    if !gates_in(*order.last()?).is_empty() {
        return None;
    }
    Some(hops)
}

/// Where room number `index` of a site anchored at `anchor` goes: the entry
/// room (index 0) at the anchor itself, the rest [`ROOM_SPACING`] apart.
pub fn room_anchor(anchor: [f64; 3], index: usize) -> [f64; 3] {
    [
        anchor[0],
        anchor[1] + index as f64 * ROOM_SPACING,
        anchor[2],
    ]
}

// ---------------------------------------------------------------------------
// Who a gate admits
// ---------------------------------------------------------------------------

/// What a gate says to a ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Yes,
    /// The connection has a ship list and the ship is not in it:
    /// `DunShipCannotWarp`.
    NotListed,
    /// The connection restricts nothing and the ship's group is one of
    /// [`BLACKLISTED_GROUPS`]: `DunBlacklistCannotWarp`.
    Blacklisted,
}

/// **The client's rule** (`GetDungeonShipRestrictions(dungeonID, gateID)`
/// **[V]**) for one gate: with a ship list, the ship must be in it; with
/// none, it must not be in a blacklisted group. A list the table lacks
/// restricts nothing (the data closes 162 of 162 lists, so this is only a
/// bundle without `types/typelists.tsv`). The race list is not read: the
/// gates of both archetypes carry none **[V-data]**.
pub fn admits(
    lists: &BTreeMap<i64, TypeList>,
    gate: Option<&dungeon_data::Gate>,
    ship_type: i64,
    ship_group: i64,
    ship_category: i64,
) -> Admit {
    let list = gate
        .and_then(|g| g.allowed_ships_list)
        .and_then(|id| lists.get(&id));
    match list {
        Some(l) => {
            if l.contains(ship_type, ship_group, ship_category) {
                Admit::Yes
            } else {
                Admit::NotListed
            }
        }
        None if BLACKLISTED_GROUPS.contains(&ship_group) => Admit::Blacklisted,
        None => Admit::Yes,
    }
}

// ---------------------------------------------------------------------------
// The state
// ---------------------------------------------------------------------------

/// One placed room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenRoom {
    pub room: i64,
    pub anchor: [f64; 3],
}

#[derive(Debug, Clone, Default)]
struct State {
    seq: u64,
    /// The rooms placed so far, entry first.
    rooms: Vec<OpenRoom>,
    /// Placed since `site_spawn` last looked.
    fresh: Vec<OpenRoom>,
    /// The cache of this instance is dropped.
    cached: bool,
}

/// A completed site's escalation, for Y-ESCAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escalation {
    /// The site that completed, and the dungeon it ran.
    pub from_site: i64,
    pub from_dungeon: i64,
    pub system: i64,
    /// The archetype-31 dungeon picked.
    pub dungeon_id: i64,
    pub when: BlueTime,
}

/// The world slot.
#[derive(Debug, Default)]
pub struct CombatSites {
    sites: BTreeMap<i64, State>,
    escalations: Vec<Escalation>,
    /// Pilots between two rooms: character to `(site, until)`.
    transit: BTreeMap<i64, (i64, std::time::Instant)>,
}

impl CombatSites {
    /// The escalations waiting, oldest first.
    pub fn escalations(&self) -> &[Escalation] {
        &self.escalations
    }
}

/// The state of `site_id`'s live instance, made (with its entry room) on
/// first use and reset when the instance is a new one.
fn state(w: &mut Registry, site_id: i64) -> Option<&mut State> {
    let (seq, room, anchor) = {
        let i = w.ext::<Sites>().get(site_id)?;
        (i.seq, i.room_id?, i.anchor)
    };
    let s = w.ext_mut::<CombatSites>();
    let st = s.sites.entry(site_id).or_default();
    if st.rooms.is_empty() || st.seq != seq {
        *st = State {
            seq,
            rooms: vec![OpenRoom { room, anchor }],
            fresh: Vec::new(),
            cached: false,
        };
    }
    Some(st)
}

/// The rooms placed in `site_id`'s instance, entry first (empty for a site
/// with no instance).
pub fn open_rooms(w: &mut Registry, site_id: i64) -> Vec<OpenRoom> {
    state(w, site_id)
        .map(|s| s.rooms.clone())
        .unwrap_or_default()
}

/// The rooms placed since the last call, once: `site_spawn` gives each its
/// squads.
pub fn take_fresh(w: &mut Registry, site_id: i64) -> Vec<OpenRoom> {
    state(w, site_id)
        .map(|s| std::mem::take(&mut s.fresh))
        .unwrap_or_default()
}

/// How many of the dungeon's rooms are not placed yet: 0 for a single-room
/// dungeon, or one whose gates [`chain`] does not explain (they never open).
pub fn rooms_unopened(w: &mut Registry, site_id: i64) -> usize {
    let Some(dungeon) = w.ext::<Sites>().get(site_id).map(|i| i.dungeon_id) else {
        return 0;
    };
    let Some(order) = dungeon_of(w, dungeon).and_then(|d| chain(&d).map(|h| h.len() + 1)) else {
        return 0;
    };
    let open = state(w, site_id).map_or(0, |s| s.rooms.len());
    order.saturating_sub(open)
}

fn dungeon_of(w: &Registry, id: i64) -> Option<Dungeon> {
    w.content()
        .try_get::<dungeon_data::Table>()?
        .dungeons
        .get(&id)
        .cloned()
}

// ---------------------------------------------------------------------------
// Activating a gate
// ---------------------------------------------------------------------------

/// Why a gate stays shut.
#[derive(Debug, Clone, PartialEq)]
pub enum Shut {
    /// Not a gate of any live site, or the pilot is not in space there.
    NotAGate,
    /// Too far; the client approaches first, so this is a stale park.
    OutOfRange,
    /// `DunShipCannotWarp`.
    NotListed { ship_type: i64, whitelist: String },
    /// `DunBlacklistCannotWarp`.
    Blacklisted { ship_type: i64 },
    /// A warp guard (scrambled, entosis, CONCORD).
    Guard(GuardRefusal),
    /// The instance has no room left to place the next room in.
    NoRoom,
}

impl Shut {
    /// The refusal as the client's `UserError`.
    pub fn into_error(self) -> ServiceError {
        use crate::pyutil::{bstr, pypairs};
        match self {
            Shut::NotListed {
                ship_type,
                whitelist,
            } => ServiceError::Raise(eve_proto::user_error(
                "DunShipCannotWarp",
                pypairs(vec![
                    ("ship", PyValue::Int(ship_type)),
                    ("whitelist", bstr(&whitelist)),
                ]),
            )),
            Shut::Blacklisted { ship_type } => ServiceError::Raise(eve_proto::user_error(
                "DunBlacklistCannotWarp",
                pypairs(vec![
                    ("ship", PyValue::Int(ship_type)),
                    ("list", bstr(BLACKLISTED_NAMES)),
                ]),
            )),
            Shut::Guard(g) => g.into_error(),
            Shut::NotAGate | Shut::OutOfRange | Shut::NoRoom => {
                let text = match self {
                    Shut::OutOfRange => "The gate is too far away to activate.",
                    Shut::NoRoom => "The gate leads nowhere: the site has no room left for it.",
                    _ => "That is not an acceleration gate of an open site.",
                };
                ServiceError::Raise(eve_proto::user_error(
                    "CustomNotify",
                    pypairs(vec![("notify", bstr(text))]),
                ))
            }
        }
    }
}

/// Where a pilot who went through a gate goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Through {
    /// The next room's warp-in point.
    pub aim: [f64; 3],
    /// How far short of it the warp stops.
    pub standoff: f64,
}

/// **`keeper.ActivateAccelerationGate(gate_item)`** for `actor` (module
/// header). On success the next room is placed (if it was not) and its
/// aim is answered; the caller starts the warp.
pub fn activate(w: &mut Registry, actor: i64, gate_item: i64) -> Result<Through, Shut> {
    w.advance_character(actor);
    let here = w.roster().get(actor).copied().ok_or(Shut::NotAGate)?;
    let (placed, system) = site::object(w, gate_item).ok_or(Shut::NotAGate)?;
    if system != here.solar_system_id {
        return Err(Shut::NotAGate);
    }
    let (site_id, dungeon_id, entry_anchor) = {
        let i = w.ext::<Sites>().of_item(gate_item).ok_or(Shut::NotAGate)?;
        (i.site_id, i.dungeon_id, i.anchor)
    };
    let d = dungeon_of(w, dungeon_id).ok_or(Shut::NotAGate)?;
    let hops = chain(&d).ok_or(Shut::NotAGate)?;
    let order = room_order(&d).ok_or(Shut::NotAGate)?;
    let hop = *hops
        .iter()
        .find(|h| h.gate_object == placed.dun_object_id)
        .ok_or(Shut::NotAGate)?;
    let gap = crate::space_state::distance(here.motion.position, placed.position) - placed.radius;
    if gap > crate::jump::MAX_JUMP_DISTANCE + GATE_SLACK {
        return Err(Shut::OutOfRange);
    }
    // The ship against the gate's list, by the client's own rule.
    let ship_type = w.character(actor).map_or(0, |c| c.ship_type_id);
    let row = w
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(ship_type));
    if let Some(row) = row {
        let lists = w
            .content()
            .try_get::<dungeon_data::Table>()
            .map(|t| t.typelists.clone())
            .unwrap_or_default();
        let gate = d.gates.iter().find(|g| g.from_object == hop.gate_object);
        match admits(&lists, gate, ship_type, row.group_id, row.category_id) {
            Admit::Yes => {}
            Admit::Blacklisted => return Err(Shut::Blacklisted { ship_type }),
            Admit::NotListed => {
                let list = gate
                    .and_then(|g| g.allowed_ships_list)
                    .and_then(|id| lists.get(&id));
                let whitelist = whitelist_text(w, list);
                return Err(Shut::NotListed {
                    ship_type,
                    whitelist,
                });
            }
        }
    }
    if let Some(g) = crate::movement_guard::check(&crate::module::plan().warp_guards, w, actor) {
        return Err(Shut::Guard(g));
    }
    let index = order
        .iter()
        .position(|r| *r == hop.to)
        .ok_or(Shut::NotAGate)?;
    let anchor = room_anchor(entry_anchor, index);
    let known = state(w, site_id)
        .ok_or(Shut::NotAGate)?
        .rooms
        .iter()
        .any(|r| r.room == hop.to);
    if !known {
        site::open_room(w, site_id, hop.to, anchor).ok_or(Shut::NoRoom)?;
        let st = state(w, site_id).ok_or(Shut::NotAGate)?;
        let open = OpenRoom {
            room: hop.to,
            anchor,
        };
        st.rooms.push(open);
        st.fresh.push(open);
    }
    w.ext_mut::<CombatSites>()
        .transit
        .insert(actor, (site_id, std::time::Instant::now() + TRANSIT_GRACE));
    Ok(Through {
        aim: anchor,
        standoff: crate::space_state::WARP_STANDOFF,
    })
}

/// The hulls a list admits, by name, for the refusal text: up to
/// [`WHITELIST_NAMES`] published ships, ascending by name.
fn whitelist_text(w: &Registry, list: Option<&TypeList>) -> String {
    let (Some(list), Some(types)) = (list, w.content().try_get::<crate::typedata::Installed>())
    else {
        return String::new();
    };
    let mut names: Vec<&str> = types
        .table()
        .types()
        .filter(|t| t.category_id == CATEGORY_SHIP && t.published)
        .filter(|t| list.contains(t.type_id, t.group_id, t.category_id))
        .map(|t| t.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.truncate(WHITELIST_NAMES);
    names.join(", ")
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A cache holds `difficulty - 1` pieces of equipment, so a difficulty-1 site
/// leaves none. `[A]`, falsified by a market flooded with the pools' modules
/// (too many) or a difficulty-5 site that pays like a belt (too few).
pub const PIECES_BASE: i64 = 1;

/// The loot size (`npc/loot.tsv`'s `size`, 1-3) of a difficulty: 1-2 small,
/// 3-4 medium, 5 large. `[A]`: the pools are keyed by hull size and the
/// dungeon's gate lists top out at frigate, destroyer, battlecruiser and
/// battleship classes by difficulty 1, 2, 3 and 4-5 **[V-data]**.
pub fn cache_size(difficulty: i64) -> i64 {
    ((difficulty + 1) / 2).clamp(1, 3)
}

/// Equipment is capped at the elite tier's meta level, as R7's elite
/// rats (`npc::loot::tier_cap`, 3). `[A]`.
pub const CACHE_META_CAP: i64 = 3;

/// The difficulties a cache may hold a faction, a deadspace and an officer
/// module from. `[A]`: R7's tiers by hull size (commander rats from
/// cruisers, deadspace from battleships) against the gate lists' class
/// ladder **[V-data]** (module header of `bot::site_runner::difficulty_cap`).
pub const FACTION_FROM: i64 = 3;
pub const DEADSPACE_FROM: i64 = 4;
pub const OFFICER_FROM: i64 = 5;

/// The chance of one faction module from [`FACTION_FROM`]: this times
/// `difficulty - FACTION_FROM + 1`. `[A]`: R7's commander chance (0.25,
/// `npc::loot::FACTION_CHANCE`).
pub const FACTION_STEP: f64 = 0.25;
/// The chance of one deadspace module from [`DEADSPACE_FROM`]: this times
/// `difficulty - DEADSPACE_FROM + 1`. `[A]`: R7's deadspace-battleship chance (0.10).
pub const DEADSPACE_STEP: f64 = 0.10;
/// The chance of one officer module from [`OFFICER_FROM`]. `[A]`: officers are
/// rare in every EVE economy; falsified by officer modules on the market
/// within a week of bots running difficulty-5 sites.
pub const OFFICER_CHANCE: f64 = 0.05;

const STREAM_TAG: u64 = 0xcab0;

/// **What the last room's cache holds**, as `(typeID, quantity)` stacks,
/// from `loot` (the bundle's `npc/loot.tsv`) for `faction`, drawn from the
/// world seed, the site and the instance so it is stable. `grantable` is
/// the store's reach; pools are filtered by it *before* a draw, as R7 does.
pub fn cache_stacks(
    loot: &[Loot],
    faction: i64,
    difficulty: i64,
    seed: u64,
    site_id: i64,
    seq: u64,
    grantable: impl Fn(i64) -> bool,
) -> Vec<(i64, i64)> {
    let mut s = stream(seed, Kind::Loot, &[STREAM_TAG, site_id as u64, seq]);
    let size = cache_size(difficulty);
    let pool = |kind: &str, tier: Tier, sized: bool, cap: Option<i64>| -> Vec<i64> {
        let mut out: Vec<i64> = loot
            .iter()
            .filter(|l| {
                l.faction_id == faction
                    && l.kind == kind
                    && l.tier == tier
                    && (!sized || l.size == size)
                    && cap.is_none_or(|c| l.meta_level.unwrap_or(0) <= c)
                    && grantable(l.type_id)
            })
            .map(|l| l.type_id)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    let mut got: Vec<i64> = Vec::new();
    let mut weapons = pool("weapon", Tier::Base, true, Some(CACHE_META_CAP));
    for _ in 0..(difficulty - PIECES_BASE).max(0) {
        if weapons.is_empty() {
            break;
        }
        let i = s.range(0, weapons.len() as u64) as usize;
        got.push(weapons.swap_remove(i));
    }
    let one = |s: &mut crate::seed::Stream, kind: &str, tier: Tier, p: f64| -> Option<i64> {
        let pool = pool(kind, tier, true, None);
        (s.chance(p) && !pool.is_empty()).then(|| pool[s.range(0, pool.len() as u64) as usize])
    };
    if difficulty >= FACTION_FROM {
        let p = FACTION_STEP * (difficulty - FACTION_FROM + 1) as f64;
        got.extend(one(&mut s, "faction", Tier::Commander, p));
    }
    if difficulty >= DEADSPACE_FROM {
        let p = DEADSPACE_STEP * (difficulty - DEADSPACE_FROM + 1) as f64;
        got.extend(one(&mut s, "deadspace", Tier::Deadspace, p));
    }
    if difficulty >= OFFICER_FROM {
        got.extend(one(&mut s, "officer", Tier::Officer, OFFICER_CHANCE));
    }
    got.into_iter().map(|type_id| (type_id, 1)).collect()
}

/// **Drop a finished site's cache** once: in its last room, owned by the
/// NPC corporation `owner`, a wreck of `wreck_type` holding the stacks of
/// [`cache_stacks`]. `false` when the instance is gone or already paid.
pub fn drop_cache(
    w: &mut Registry,
    site_id: i64,
    owner: i64,
    wreck_type: i64,
    now: BlueTime,
) -> bool {
    let Some((dungeon, system)) = w
        .ext::<Sites>()
        .get(site_id)
        .map(|i| (i.dungeon_id, i.system))
    else {
        return false;
    };
    let Some(d) = dungeon_of(w, dungeon) else {
        return false;
    };
    let Some(faction) = d.faction else {
        return false;
    };
    let seq = w.ext::<Sites>().get(site_id).map_or(0, |i| i.seq);
    let Some(st) = state(w, site_id) else {
        return false;
    };
    if st.cached {
        return false;
    }
    st.cached = true;
    let at = st.rooms.last().map_or([0.0; 3], |r| r.anchor);
    let stacks = match w.content().try_get::<crate::npc_data::Table>() {
        Some(t) => cache_stacks(
            &t.loot,
            faction,
            d.difficulty.unwrap_or(1),
            w.world().seed,
            site_id,
            seq,
            |t| crate::item::find_in(w.content(), t).is_some(),
        ),
        None => Vec::new(),
    };
    if stacks.is_empty() {
        return false;
    }
    let at = [at[0] + CACHE_OFFSET[0], at[1], at[2] + CACHE_OFFSET[2]];
    crate::combat::drop_cache(w, owner, wreck_type, system, at, &stacks, now).is_some()
}

/// Where the cache lies against the last room's warp-in point. `[A]`:
/// 2 km out, where a pilot who landed 1 km short of the point flies.
pub const CACHE_OFFSET: [f64; 3] = [2_000.0, 0.0, 2_000.0];

// ---------------------------------------------------------------------------
// The escalation seam
// ---------------------------------------------------------------------------

/// The chance a completed site escalates. `[A]`: nothing in the client's
/// data or code says (the server decides on TQ). Falsified by the owner's
/// "too many" or "never" after a test round; Y-ESCAL may replace it.
pub const ESCALATION_CHANCE: f64 = 0.25;

/// The chance a cleared **escalation** opens the next rung of its ladder
/// (Y-ESCAL). `[A]`, lower than [`ESCALATION_CHANCE`] so a ladder is short;
/// the data holds difficulties 3 to 5 only, so a ladder has three rungs at
/// most. Falsified by the owner's "never past the first" or "endless".
pub const ESCALATION_STEP_CHANCE: f64 = 0.15;

/// The highest rung an escalation has: the bundle's archetype 31 tops out
/// at difficulty 5 **[V-data]**.
pub const ESCALATION_MAX_DIFFICULTY: i64 = 5;

/// The lowest difficulty an escalation has: the bundle's archetype 31 holds
/// difficulties 3 to 5 only **[V-data]**.
pub const ESCALATION_MIN_DIFFICULTY: i64 = 3;

/// The archetype-31 dungeons a completed `from` may escalate to: its
/// faction, difficulty `max(from, 3)`. `[A]` (the pairing is the server's on
/// TQ). Ascending ids.
pub fn escalation_candidates(t: &dungeon_data::Table, from: &Dungeon) -> Vec<i64> {
    // A cleared escalation (Y-ESCAL) climbs its faction's ladder one rung.
    let want = if from.archetype == ESCALATION_ARCHETYPE {
        from.difficulty.unwrap_or(ESCALATION_MIN_DIFFICULTY) + 1
    } else {
        from.difficulty.unwrap_or(1).max(ESCALATION_MIN_DIFFICULTY)
    };
    t.dungeons
        .values()
        .filter(|d| {
            d.archetype == ESCALATION_ARCHETYPE
                && d.faction.is_some()
                && d.faction == from.faction
                && d.difficulty == Some(want)
        })
        .map(|d| d.id)
        .collect()
}

/// Whether `from` escalates this time, and to what: [`ESCALATION_CHANCE`],
/// then a candidate, both drawn from the world seed, the site and the
/// instance. Combat sites escalate ([`COMBAT_ARCHETYPE`]) and so does a
/// cleared escalation, one rung up ([`ESCALATION_STEP_CHANCE`]).
pub fn escalation_for(
    t: &dungeon_data::Table,
    from: &Dungeon,
    seed: u64,
    site_id: i64,
    seq: u64,
) -> Option<i64> {
    let chance = match from.archetype {
        COMBAT_ARCHETYPE => ESCALATION_CHANCE,
        ESCALATION_ARCHETYPE => ESCALATION_STEP_CHANCE,
        _ => return None,
    };
    let mut s = stream(seed, Kind::Loot, &[STREAM_TAG + 1, site_id as u64, seq]);
    let cands = escalation_candidates(t, from);
    (s.chance(chance) && !cands.is_empty()).then(|| cands[s.range(0, cands.len() as u64) as usize])
}

/// **A site completed**: roll its escalation and queue it for Y-ESCAL.
pub fn on_complete(w: &mut Registry, site_id: i64, ft: BlueTime) {
    // Y-ESCAL: a cleared escalation leaves its pilots' journals.
    crate::escalation::completed(w, site_id);
    let Some((dungeon, system, seq)) = w
        .ext::<Sites>()
        .get(site_id)
        .map(|i| (i.dungeon_id, i.system, i.seq))
    else {
        return;
    };
    let Some(from) = dungeon_of(w, dungeon) else {
        return;
    };
    let seed = w.world().seed;
    let Some(to) = w
        .content()
        .try_get::<dungeon_data::Table>()
        .and_then(|t| escalation_for(t, &from, seed, site_id, seq))
    else {
        return;
    };
    let q = &mut w.ext_mut::<CombatSites>().escalations;
    q.push(Escalation {
        from_site: site_id,
        from_dungeon: dungeon,
        system,
        dungeon_id: to,
        when: ft,
    });
    if q.len() > ESCALATION_QUEUE {
        q.remove(0);
    }
    crate::outln!("[combat_site] site {site_id} in {system} escalates to dungeon {to}");
}

/// The escalations queued so far, once. Y-ESCAL's entry.
pub fn take_escalations(w: &mut Registry) -> Vec<Escalation> {
    std::mem::take(&mut w.ext_mut::<CombatSites>().escalations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Gate, Object, Room};

    /// Invented ids throughout.
    const DUNGEON: i64 = 999_701;
    const FACTION: i64 = 599_701;
    const LIST: i64 = 999_711;
    /// The entry room has the *highest* id, as in the bundle.
    const ENTRY: i64 = 30;
    const MIDDLE: i64 = 10;
    const LAST: i64 = 11;
    const GATE_ENTRY: i64 = 2;
    const GATE_MIDDLE: i64 = 4;

    fn object(room: i64, id: i64, x: f64) -> Object {
        Object {
            room,
            id,
            type_id: 990_701,
            pos: [x, 0.0, 0.0],
            ..Object::default()
        }
    }

    fn gate(from_object: i64, list: Option<i64>) -> Gate {
        Gate {
            from_object,
            allowed_ships_list: list,
            ..Gate::default()
        }
    }

    /// Entry (a beacon and a gate), a middle room (a gate) and a last one.
    fn dungeon() -> Dungeon {
        Dungeon {
            id: DUNGEON,
            archetype: COMBAT_ARCHETYPE,
            faction: Some(FACTION),
            difficulty: Some(3),
            entry_object: Some(1),
            rooms: [ENTRY, MIDDLE, LAST]
                .into_iter()
                .map(|id| Room {
                    id,
                    ..Room::default()
                })
                .collect(),
            objects: vec![
                object(ENTRY, 1, 0.0),
                object(ENTRY, GATE_ENTRY, 5_000.0),
                object(MIDDLE, 3, 0.0),
                object(MIDDLE, GATE_MIDDLE, 8_000.0),
                object(LAST, 5, 0.0),
            ],
            gates: vec![gate(GATE_ENTRY, Some(LIST)), gate(GATE_MIDDLE, None)],
            ..Dungeon::default()
        }
    }

    #[test]
    fn the_rooms_are_ordered_entry_first_then_by_id() {
        assert_eq!(room_order(&dungeon()), Some(vec![ENTRY, MIDDLE, LAST]));
    }

    #[test]
    fn a_gate_leads_to_the_next_room_in_that_order() {
        let hops = chain(&dungeon()).expect("a chain");
        assert_eq!(
            hops,
            vec![
                Hop {
                    gate_object: GATE_ENTRY,
                    from: ENTRY,
                    to: MIDDLE
                },
                Hop {
                    gate_object: GATE_MIDDLE,
                    from: MIDDLE,
                    to: LAST
                },
            ]
        );
    }

    /// The rule is accepted only when it explains every room.
    #[test]
    fn a_dungeon_the_rule_does_not_explain_has_no_chain() {
        let mut two_in_a_room = dungeon();
        two_in_a_room.objects.push(object(MIDDLE, 6, 1_000.0));
        two_in_a_room.gates.push(gate(6, None));
        assert_eq!(chain(&two_in_a_room), None);
        let mut gate_in_last = dungeon();
        gate_in_last.objects.push(object(LAST, 7, 0.0));
        gate_in_last.gates.push(gate(7, None));
        assert_eq!(chain(&gate_in_last), None);
        let mut no_gate = dungeon();
        no_gate.gates.truncate(1);
        assert_eq!(chain(&no_gate), None);
        let mut single = dungeon();
        single.rooms.truncate(1);
        single.objects.retain(|o| o.room == ENTRY);
        assert_eq!(chain(&single), None);
        assert_eq!(room_order(&single), None);
    }

    #[test]
    fn rooms_stand_a_grid_apart_and_the_entry_room_at_the_anchor() {
        let a = [1.0e11, 2.0e9, -3.0e10];
        assert_eq!(room_anchor(a, 0), a);
        for k in 1..4 {
            let d = crate::space_state::distance(room_anchor(a, k), room_anchor(a, k - 1));
            assert!(d > crate::grid::GRID_LEAVE, "{d}");
        }
    }

    fn lists() -> BTreeMap<i64, TypeList> {
        BTreeMap::from([(
            LIST,
            TypeList {
                inc_groups: vec![25],
                exc_types: vec![990_799],
                ..TypeList::default()
            },
        )])
    }

    /// The client's rule: a list admits its ships and no others; no list
    /// admits all but the blacklisted groups.
    #[test]
    fn a_gate_admits_by_the_clients_rule() {
        let g = gate(GATE_ENTRY, Some(LIST));
        let l = lists();
        assert_eq!(admits(&l, Some(&g), 990_798, 25, 6), Admit::Yes);
        assert_eq!(admits(&l, Some(&g), 990_799, 25, 6), Admit::NotListed);
        assert_eq!(admits(&l, Some(&g), 990_798, 26, 6), Admit::NotListed);
        let open = gate(GATE_MIDDLE, None);
        assert_eq!(admits(&l, Some(&open), 990_798, 26, 6), Admit::Yes);
        for group in BLACKLISTED_GROUPS {
            assert_eq!(
                admits(&l, Some(&open), 990_798, group, 6),
                Admit::Blacklisted
            );
        }
        // A list a bundle does not carry restricts nothing.
        let lost = gate(GATE_ENTRY, Some(LIST + 1));
        assert_eq!(admits(&l, Some(&lost), 990_798, 26, 6), Admit::Yes);
        assert_eq!(admits(&l, None, 990_798, 26, 6), Admit::Yes);
    }

    #[test]
    fn the_refusals_are_the_clients_own_keys() {
        let key = |s: Shut| match s.into_error() {
            ServiceError::Raise(PyValue::ObjectEx { header, .. }) => match *header {
                PyValue::Tuple(t) => match t.get(1) {
                    Some(PyValue::Tuple(a)) => match a.first() {
                        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                        other => panic!("{other:?}"),
                    },
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        assert_eq!(
            key(Shut::NotListed {
                ship_type: 1,
                whitelist: String::new()
            }),
            "DunShipCannotWarp"
        );
        assert_eq!(
            key(Shut::Blacklisted { ship_type: 1 }),
            "DunBlacklistCannotWarp"
        );
        assert_eq!(key(Shut::OutOfRange), "CustomNotify");
    }

    fn loot(kind: &str, tier: Tier, size: i64, type_id: i64, meta: i64) -> Loot {
        Loot {
            faction_id: FACTION,
            size,
            kind: kind.to_owned(),
            tier,
            type_id,
            meta_level: Some(meta),
        }
    }

    /// Invented pools: eight weapons per size 1-3 (metas 0-7), and one
    /// faction, deadspace and officer module per size.
    fn pools() -> Vec<Loot> {
        let mut v = Vec::new();
        for size in 1..=3 {
            for k in 0..8 {
                v.push(loot(
                    "weapon",
                    Tier::Base,
                    size,
                    991_000 + size * 100 + k,
                    k,
                ));
            }
            v.push(loot("faction", Tier::Commander, size, 992_000 + size, 0));
            v.push(loot("deadspace", Tier::Deadspace, size, 993_000 + size, 0));
            v.push(loot("officer", Tier::Officer, size, 994_000 + size, 0));
        }
        v
    }

    fn stacks(difficulty: i64, site: i64) -> Vec<(i64, i64)> {
        cache_stacks(&pools(), FACTION, difficulty, 7, site, 1, |_| true)
    }

    /// The size is a loot size (1-3), never shrinks as the difficulty
    /// grows, and the hardest site's is the largest.
    #[test]
    fn the_size_follows_the_difficulty() {
        let got: Vec<i64> = (1..=5).map(cache_size).collect();
        assert!(got.iter().all(|s| (1..=3).contains(s)));
        assert!(got.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(got[4], 3);
        assert_eq!(cache_size(i64::MIN / 4), 1);
        assert_eq!(cache_size(i64::MAX / 4), 3);
    }

    #[test]
    fn a_difficulty_one_site_leaves_no_cache() {
        assert!(stacks(PIECES_BASE, 1).is_empty());
    }

    /// Distinct pieces, at the difficulty's size, under the meta cap, one
    /// fewer than the difficulty.
    #[test]
    fn a_cache_holds_distinct_pieces_of_its_size_under_the_cap() {
        for site in 0..40 {
            let got = stacks(PIECES_BASE + 1, site);
            assert_eq!(got.len(), 1);
            let got = stacks(OFFICER_FROM, site);
            let weapons: Vec<i64> = got
                .iter()
                .map(|(t, _)| *t)
                .filter(|t| (991_000..992_000).contains(t))
                .collect();
            assert_eq!(weapons.len(), (OFFICER_FROM - PIECES_BASE) as usize);
            let mut dedup = weapons.clone();
            dedup.sort_unstable();
            dedup.dedup();
            assert_eq!(dedup.len(), weapons.len(), "no piece twice");
            for t in weapons {
                let l = pools()
                    .into_iter()
                    .find(|l| l.type_id == t)
                    .expect("pooled");
                assert_eq!(l.size, cache_size(OFFICER_FROM));
                assert!(l.meta_level.unwrap_or(0) <= CACHE_META_CAP);
            }
            assert!(got.iter().all(|(_, q)| *q == 1));
        }
    }

    /// The rare kinds appear only where the difficulty allows them, and
    /// at the rates the constants name.
    #[test]
    fn faction_deadspace_and_officer_modules_come_by_difficulty() {
        let has = |d: i64, site: i64, range: std::ops::Range<i64>| {
            stacks(d, site).iter().any(|(t, _)| range.contains(t))
        };
        let n = 4_000;
        for site in 0..n {
            assert!(!has(FACTION_FROM - 1, site, 992_000..995_000));
            assert!(!has(DEADSPACE_FROM - 1, site, 993_000..995_000));
            assert!(!has(OFFICER_FROM - 1, site, 994_000..995_000));
        }
        let rate = |d: i64, range: std::ops::Range<i64>| {
            (0..n).filter(|s| has(d, *s, range.clone())).count() as f64 / n as f64
        };
        let near = |got: f64, want: f64| (got - want).abs() < 0.03;
        assert!(near(rate(FACTION_FROM, 992_000..993_000), FACTION_STEP));
        assert!(near(rate(DEADSPACE_FROM, 993_000..994_000), DEADSPACE_STEP));
        assert!(near(rate(OFFICER_FROM, 994_000..995_000), OFFICER_CHANCE));
    }

    #[test]
    fn a_cache_is_stable_and_never_holds_what_the_store_cannot() {
        assert_eq!(stacks(OFFICER_FROM, 9), stacks(OFFICER_FROM, 9));
        assert_ne!(stacks(OFFICER_FROM, 9), stacks(OFFICER_FROM, 10));
        let none = cache_stacks(&pools(), FACTION, 5, 7, 1, 1, |_| false);
        assert!(none.is_empty());
        let other = cache_stacks(&pools(), FACTION + 1, 5, 7, 1, 1, |_| true);
        assert!(other.is_empty(), "another faction's pools are not ours");
    }

    fn escalation_table() -> dungeon_data::Table {
        let mut t = dungeon_data::Table::default();
        let mut add = |id: i64, archetype: i64, faction: i64, difficulty: i64| {
            t.dungeons.insert(
                id,
                Dungeon {
                    id,
                    archetype,
                    faction: Some(faction),
                    difficulty: Some(difficulty),
                    ..Dungeon::default()
                },
            );
        };
        let floor = ESCALATION_MIN_DIFFICULTY;
        add(1, COMBAT_ARCHETYPE, FACTION, floor);
        add(2, COMBAT_ARCHETYPE, FACTION, floor - 1);
        add(3, ESCALATION_ARCHETYPE, FACTION, floor);
        add(4, ESCALATION_ARCHETYPE, FACTION, floor + 1);
        add(5, ESCALATION_ARCHETYPE, FACTION + 1, floor);
        add(6, ESCALATION_ARCHETYPE, FACTION, floor);
        t
    }

    /// Same faction, difficulty `max(own, 3)`, archetype 31.
    #[test]
    fn an_escalation_is_the_same_factions_at_the_same_difficulty() {
        let t = escalation_table();
        assert_eq!(escalation_candidates(&t, &t.dungeons[&1]), vec![3, 6]);
        assert_eq!(
            escalation_candidates(&t, &t.dungeons[&2]),
            vec![3, 6],
            "below the escalations' own floor they start at it"
        );
        let mut hard = t.dungeons[&1].clone();
        hard.difficulty = Some(ESCALATION_MIN_DIFFICULTY + 1);
        assert_eq!(escalation_candidates(&t, &hard), vec![4]);
        hard.difficulty = Some(ESCALATION_MIN_DIFFICULTY + 2);
        assert!(escalation_candidates(&t, &hard).is_empty());
    }

    #[test]
    fn only_combat_sites_escalate_and_the_chance_is_the_constants() {
        let t = escalation_table();
        let from = &t.dungeons[&1];
        let n = 4_000;
        let hits = (0..n)
            .filter(|s| escalation_for(&t, from, 9, *s, 1).is_some())
            .count() as f64
            / n as f64;
        assert!((hits - ESCALATION_CHANCE).abs() < 0.03, "{hits}");
        for s in 0..n {
            if let Some(to) = escalation_for(&t, from, 9, s, 1) {
                assert!([3, 6].contains(&to));
            }
        }
        let mut other = from.clone();
        other.archetype = 27;
        assert!((0..n).all(|s| escalation_for(&t, &other, 9, s, 1).is_none()));
        assert_eq!(
            escalation_for(&t, from, 9, 5, 1),
            escalation_for(&t, from, 9, 5, 1)
        );
    }
}
