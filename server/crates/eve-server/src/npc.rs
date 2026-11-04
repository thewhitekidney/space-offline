//! Hostile NPCs: server-side balls that see, lock, follow, orbit and shoot.
//!
//! Spec: `docs/lanes/npc.md`. Owned by the P4.1 lane.
//!
//! **An NPC is a ball in the roster under its own id.** `Presence::character_id`
//! and `Presence::ship_id` are both the NPC's id, in [`MIN_NPC_ID`]..
//! [`END_NPC_ID`], so everything that already walks the roster -- the
//! bootstrap, the lock graph, the range sweep, motion integration, combat,
//! the kill -- sees it, after one seam: `Registry::pilot`, which answers
//! "who flies this roster entry" for a character *or* an NPC ([`pilot_of`]).
//! A consumer that asks `character(p.character_id)` instead gets `None` for
//! an NPC and falls back in silence (`docs/lanes/npc.md` §5, §10 risk 1).
//!
//! **Why not an `eve-bot` client** (§1): the client calls a ball an NPC by
//! its `ownerID` alone -- `IsNPC = 9999 < ownerID < 90000000`
//! (`idCheckers.py:283`) **[V]** -- and a bot is a character, whose owner is a
//! player id. Red brackets, no attack confirmation and "hostile NPC" all hang
//! off the owner, so a hostile NPC has to be a ball the server owns.
//!
//! **The shape: a pure brain, a thin executor** (§4.1). [`decide`] is the
//! whole policy, takes a [`View`] and returns intents, and touches no world;
//! [`tick`] builds the views from the roster, runs it, and applies the
//! intents through the subsystems that already exist -- `targeting`'s lock
//! graph and `OnTarget`, `modules::SpecialFx`, `combat::fire` -- so a player
//! shot by an NPC sees exactly what a player shot by a player sees.
//!
//! **Everything the clock decides leaves on the clock** (`Registry::schedule`):
//! the tick is not a payload turn (`combat.rs`'s header). The one exception is
//! [`entity_slash`], which *is* a turn and so uses `Registry::push`.
//!
//! **Not persisted**, like the roster: a restart empties space.

use crate::damage::{BlueTime, Damage};
use crate::destiny::hull::Hull;
use crate::destiny::{self, Mode};
use crate::service::Registry;
use crate::space_state::{Motion, Presence};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

// W10.5 seam S4 (`docs/content/plan.md` §6.1): the brain, the loot roll and
// the bounty are their own files; every item keeps its `crate::npc::` path.
// `bounty` is public for its `kill_hooks::NPC` row (W17 CP50).
pub mod bounty;
mod brain;
mod loot;

pub use bounty::{REF_BOUNTY_PRIZE, pay_bounty};
use brain::apply;
pub use brain::{Behaviour, Candidate, Intent, Mind, View, decide};
pub use loot::{Drop, droppable, loot, loot_from, loot_in};

/// The `OnSpecialFX` start of `npc`'s gun while it fires (`to` empty), for a
/// late arrival's replay ([`crate::modules::active_fx`]).
pub fn running_gun_fx(npc: &Npc) -> Option<crate::modules::SpecialFx> {
    let target = npc.target.filter(|_| npc.firing)?;
    Some(brain::gun_fx(Vec::new(), npc, target, true))
}

/// The first id an NPC (or an NPC's wreck) gets.
///
/// `[A]` **7e9..8e9, above every client id class and below the fake ranges.**
/// `inventorycommon/const.py:7-100` **[V]** ends every class at or below
/// 2,147,483,647 except `minFakeItem` (9e18) and `minFakeClientItem`
/// (1.7e19); `maxPreInv64Items` = 2,099,999,999 names the pre-inv64 era, so
/// ids above it are ordinary 64-bit items **[I]**. Far from the item store's
/// allocator (100,000,000 up, by one), and **disjoint from every character
/// id**, which matters because the roster is keyed by character
/// (`docs/lanes/npc.md` §2). Falsified by a client path that mishandles a
/// ball id above 2³¹ (no bracket, a traceback); then the unused **[V]** gap
/// 80,100,000..80,999,999 (`const.py:76-77`) is the fallback.
pub const MIN_NPC_ID: i64 = 7_000_000_000;

/// One past the last id [`Npcs::next_id`] hands out.
pub const END_NPC_ID: i64 = 8_000_000_000;

/// `[A]` How far an idle NPC notices a player ship. A belt rat's number is
/// not in the client; CCP's dungeon `guardCommand.aggressionRange` is 30,000
/// on 6,824 objects (`docs/re/npc-behaviors.md` §6.1), and 20 km keeps a
/// 10 km band under the 30 km lock range. Falsified by "it does not feel like
/// a rat".
pub const AGGRESSION_RANGE: f64 = 20_000.0;

/// `[A]` Where `/entity deploy` puts one: inside [`AGGRESSION_RANGE`], far
/// enough to watch it come.
pub const SPAWN_DISTANCE: f64 = 15_000.0;

/// `[A]` The most one `/entity deploy` makes. The client's own prompt allows
/// 100 (`menusvc.py:1065`) **[V]**; ten is plenty for a sandbox.
pub const MAX_DEPLOY: i64 = 10;

/// The follow/orbit latch, EvEJS's numbers (`npcBehaviorLoop.js:4061-4064`;
/// **a hint, not a fact: EvEJS is often wrong, nothing here is checked
/// against the client, so every number is `[A]`**): an NPC that is not already following re-enters FOLLOW when
/// the gap to its target exceeds `orbit + max(5 km, 50% of orbit)`, and a
/// following NPC hands over to ORBIT once the gap is within `orbit +
/// max(1 km, 20% of orbit)`. In between it keeps whatever it was doing, so
/// the two orders never alternate tick by tick at a boundary (EvEJS
/// `client/jolt9.txt`: returning to orbit as soon as the ship is "not far
/// anymore" made visible FOLLOW -> ORBIT churn). Centre to centre here,
/// surface to surface there; the radii are tens of metres on a band of
/// kilometres. `[A]` that the same numbers suit our client; falsified by an
/// NPC that still alternates Follow and Orbit in a capture.
pub const ORBIT_REENTER_MIN: f64 = 5_000.0;
pub const ORBIT_REENTER_FRACTION: f64 = 0.5;
pub const ORBIT_SETTLE_MIN: f64 = 1_000.0;
pub const ORBIT_SETTLE_FRACTION: f64 = 0.2;

/// The latch: what an NPC at `gap` from its target, in `phase`, should be
/// doing, for an orbit range `orbit`.
pub fn orbit_latch(phase: Phase, gap: f64, orbit: f64) -> Phase {
    let reenter = orbit + ORBIT_REENTER_MIN.max(orbit * ORBIT_REENTER_FRACTION);
    let settle = orbit + ORBIT_SETTLE_MIN.max(orbit * ORBIT_SETTLE_FRACTION);
    if gap > reenter || (phase == Phase::Approach && gap > settle) {
        Phase::Approach
    } else {
        Phase::Orbit
    }
}

/// The most volleys one NPC lands in one tick. A stalled clock catches up a
/// little rather than all at once; anything further behind is dropped.
pub const MAX_SHOTS_PER_TICK: u32 = 8;

/// Blue time is 10,000 ticks to the millisecond (`damage::DGM_TAU_CONSTANT`).
const BLUE_PER_MS: i64 = 10_000;

/// One kind of NPC: its hull, its gun, and how it fights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcType {
    pub type_id: i64,
    pub name: &'static str,
    /// `types.groupID`: a faction's asteroid frigate, destroyer or cruiser
    /// group ([`Faction`]).
    pub group_id: i64,
    /// `ownerID` and `corpID` on the slim, and the wreck's owner.
    pub owner_corp_id: i64,
    /// `spaceComponentsByType.entityStandings` (hostile, friendly).
    pub hostile_threshold: f32,
    pub friendly_threshold: f32,
    pub hull: Hull,
    /// Attribute 51 `speed`, the gun's cycle, in milliseconds.
    pub weapon_period_ms: i64,
    /// Attributes 114/116/117/118, which an entity carries **on the hull**.
    pub damage: Damage,
    /// Attribute 64 `damageMultiplier`.
    pub damage_multiplier: f64,
    /// The default effect's `guid`, for `OnSpecialFX`.
    pub weapon_guid: &'static str,
    /// Attribute 247 `entityAttackRange`.
    pub attack_range: f64,
    /// Attribute 416 `entityFlyRange`: the orbit and follow range.
    pub orbit_range: f64,
    /// Attribute 508 `entityCruiseSpeed` over attribute 37 `maxVelocity`.
    pub cruise_fraction: f64,
    /// `types.wreckTypeID`.
    pub wreck_type_id: Option<i64>,
    /// Attribute 481 `entityKillBounty`: the ISK its killer is paid
    /// ([`pay_bounty`]).
    pub bounty: f64,
    /// Attribute `AI_IgnoreDronesBelowSignatureRadius`: a drone whose
    /// `signatureRadius` is below this is not worth shooting back at. The
    /// value is the type's own **[V-data]**; the meaning is **[I]**, from the
    /// name -- the server reads it, the client never does.
    pub ignore_drones_below_signature: f64,
    /// Attributes `entityEquipmentMin` / `entityEquipmentMax`: how many
    /// pieces of [`NpcType::equipment`] its wreck holds. Values **[V-data]**;
    /// meaning **[I]** from the names.
    pub equipment_min: u32,
    pub equipment_max: u32,
    /// The modules its wreck may hold. `[A]`: the client ships no NPC loot
    /// table -- loot is the server's -- so this is a list of what the
    /// entity plausibly flies, from types the store knows.
    pub equipment: &'static [i64],
    /// The stacks its wreck may also hold, each rolled on its own. `[A]`, as
    /// [`NpcType::equipment`].
    pub cargo: &'static [Cargo],
}

/// One stack an NPC's wreck may hold: `chance` of it at all, then a
/// quantity in `min..=max`. Every number `[A]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cargo {
    pub type_id: i64,
    pub chance: f64,
    pub min: i64,
    pub max: i64,
}

impl NpcType {
    /// What one cycle throws, pre-resistance: the hull's own damage times
    /// its `damageMultiplier` -- P2.2b's turret rule (`combat::volley_in`,
    /// `fittingDogmaLocationUtil:292-303` **[V]**), **[I]** that it applies
    /// to an entity whose damage sits on its hull.
    pub fn volley(&self) -> Damage {
        let m = self.damage_multiplier;
        Damage {
            em: self.damage.em * m,
            explosive: self.damage.explosive * m,
            kinetic: self.damage.kinetic * m,
            thermal: self.damage.thermal * m,
        }
    }

    /// The gun's cycle, in blue ticks.
    pub fn weapon_period(&self) -> BlueTime {
        self.weapon_period_ms * BLUE_PER_MS
    }
}

/// **The no-bundle rat** (CP61, plan §2.3): the one NPC type the server
/// carries by hand, so that a server with no `--content` still spawns belt
/// rats, answers `/entity deploy` and runs every NPC test. **Every number is
/// invented**: it is not a client row, and nothing the client ships is
/// copied here (tracker §66's rule). Its ids sit beside
/// `npc_data::Table::fixture`'s invented ones but are **not listed there**,
/// on purpose: a type the loaded rows list takes its owner and loot from
/// them (`npc::loot_in`), and the hand rat is the case with no row. A real
/// client loads the types from the bundle instead ([`resolve`],
/// [`derive_loaded`]); this one has no model it can draw.
///
/// It is **weak** on purpose, as the Angel Hijacker it replaces was: a slow
/// gun and a thin hull, so losing a ship to it takes a long time.
///
/// Two fields name rows the server itself carries, not client data: its
/// wreck is the item store's hand wreck ([`HAND_SMALL_WRECK`]) and its loot
/// kit is store items ([`FIXTURE_EQUIPMENT`], [`FIXTURE_CARGO`]), so a kill
/// leaves a wreck the store can hold.
pub const FIXTURE_RAT: NpcType = NpcType {
    type_id: 990_310_001,
    name: "Fixture Raider",
    group_id: FIXTURE_GROUP_FRIGATE,
    owner_corp_id: FIXTURE_CORP,
    hostile_threshold: RESPONSE_THRESHOLD,
    friendly_threshold: RESPONSE_THRESHOLD,
    hull: Hull {
        type_id: 990_310_001,
        mass: 1_500_000.0,
        radius: 40.0,
        max_velocity: 1_800.0,
        // A zero would freeze the server's copy of the ball (see
        // [`FALLBACK_AGILITY`]); the fixture takes the fallback.
        agility: FALLBACK_AGILITY,
        shield_capacity: 40.0,
        shield_recharge_rate: 1_500_000.0,
        armor_hp: 45.0,
        // Where `targeting::sweep_out_of_range` drops the NPC's lock; the
        // client never reads an NPC's.
        max_target_range: MIN_LOCK_RANGE,
        max_locked_targets: 1.0,
        scan_resolution: 1_200.0,
        signature_radius: 35.0,
        // NPCs do not warp.
        warp_speed_au: 0.0,
        hp: 50.0,
        capacitor_capacity: 40.0,
        capacitor_recharge_rate: 400_000.0,
    },
    weapon_period_ms: 3_000,
    damage: Damage {
        em: 1.5,
        explosive: 0.0,
        kinetic: 0.0,
        thermal: 1.5,
    },
    damage_multiplier: 0.5,
    weapon_guid: TURRET_FX_GUID,
    attack_range: 5_000.0,
    orbit_range: 2_000.0,
    cruise_fraction: 0.125,
    wreck_type_id: Some(HAND_SMALL_WRECK),
    bounty: 1_500.0,
    ignore_drones_below_signature: 40.0,
    equipment_min: 0,
    equipment_max: 1,
    equipment: FIXTURE_EQUIPMENT,
    cargo: FIXTURE_CARGO,
};

/// The fixture's owner, invented (`npc_data::Table::fixture`'s "Fixture
/// Corp" id). Inside the client's NPC owner range (`IsNPC = 9999 < ownerID <
/// 90000000`, `idCheckers.py:283` **[V]**), so the slim reads as hostile.
pub const FIXTURE_CORP: i64 = 1_999_011;

/// The fixture faction's groups, invented.
pub const FIXTURE_GROUP_FRIGATE: i64 = 99_111;
pub const FIXTURE_GROUP_DESTROYER: i64 = 99_113;
pub const FIXTURE_GROUP_CRUISER: i64 = 99_112;

/// The item store's hand-carried NPC wrecks (`item::WRECK_TYPES`, its own
/// rows until OWNER DECISION O3 moves them): a no-bundle kill needs a wreck
/// the store can hold. A reference to the store's table, not an NPC row.
pub const HAND_SMALL_WRECK: i64 = 26561;
pub const HAND_MEDIUM_WRECK: i64 = 26562;

/// `'effects.Laser'`, one of the client's `FX_TURRET_EFFECT_GUIDS`
/// (`fxSequencer.py:50-56`) **[V]**: the fixture's gun, and the visual a
/// loaded type falls back to when its default effect names none.
pub const TURRET_FX_GUID: &str = "effects.Laser";

/// `spaceComponentsByType.entityStandings` (hostile, friendly): **11.0,
/// 11.0 on every belt type checked** **[V-data]** (`out/npc/sc.py`). `[A]`
/// for every loaded type: the bundle carries the component's presence, not
/// its numbers. Falsified by a loaded type that ignores a neutral pilot.
pub const RESPONSE_THRESHOLD: f32 = 11.0;

/// `[A]` the fixture's wreck kit, store items (`item::SPAWNABLE`): a small
/// autocannon, a small shield extender and a civilian afterburner. The
/// client ships no NPC loot table -- loot is the server's.
pub const FIXTURE_EQUIPMENT: &[i64] = &[484, 377, 21857];

/// `[A]` two stacks of small projectile rounds, store items.
pub const FIXTURE_CARGO: &[Cargo] = &[
    Cargo {
        type_id: 185,
        chance: 0.5,
        min: 20,
        max: 60,
    },
    Cargo {
        type_id: 184,
        chance: 0.5,
        min: 20,
        max: 60,
    },
];

/// Every NPC type the server carries by hand, and so knows with no bundle:
/// the invented [`FIXTURE_RAT`].
pub const TYPES: &[NpcType] = &[FIXTURE_RAT];

/// **The test-only name of [`FIXTURE_RAT`]** (CP61). The Angel Hijacker's
/// hand row is gone; other modules' tests still say `ANGEL_HIJACKER`, and
/// get the invented fixture.
#[cfg(test)]
pub const ANGEL_HIJACKER: NpcType = FIXTURE_RAT;

/// **The test-only name of [`FIXTURE_FACTION`]** (CP61), as
/// [`ANGEL_HIJACKER`].
#[cfg(test)]
pub const ANGEL_CARTEL: Faction = FIXTURE_FACTION;

/// One hand NPC type by type id: [`TYPES`]. Registry-free, for callers with
/// no world (`destiny::hull::find`, the bot); a type the bundle defines is
/// [`resolve`]'s.
pub fn find(type_id: i64) -> Option<&'static NpcType> {
    TYPES
        .iter()
        .chain(crate::concord::FIXTURES.iter())
        .find(|t| t.type_id == type_id)
}

/// **One NPC type by type id, for a live world**: the hand table
/// ([`find`]), else the type the loaded `npc/` rows list, derived over the
/// content's typedata (`typedata::installed_in`, CP58b; [`derive_loaded`])
/// once and kept in [`Npcs`]. `None` for a type neither knows (a bundle
/// with no typedata, or no such entity).
pub fn resolve(w: &mut Registry, type_id: i64) -> Option<&'static NpcType> {
    if let Some(k) = find(type_id) {
        return Some(k);
    }
    if let Some(k) = w.npcs().kinds.get(&type_id) {
        return Some(*k);
    }
    let made = {
        let data = w.content().try_get::<crate::npc_data::Table>()?;
        let td = crate::typedata::installed_in(w.content()).table();
        derive_loaded(td.get(type_id)?, td, data)?
    };
    let k: &'static NpcType = Box::leak(Box::new(made));
    w.npcs_mut().kinds.insert(type_id, k);
    Some(k)
}

// ---------------------------------------------------------------------------
// Factions: which entities a belt holds with no bundle
// ---------------------------------------------------------------------------

/// One hull size a belt spawn draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Frigate,
    Destroyer,
    Cruiser,
}

/// A pirate faction as a belt sees it: who owns its ships, which groups it
/// flies in, and **which types of each size a belt spawn picks from**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Faction {
    pub name: &'static str,
    /// `ownerID`/`corpID` on every slim, and the wreck's owner.
    pub owner_corp_id: i64,
    /// `(group, its wreck)`, every group this faction's belt rats are in.
    pub groups: &'static [(i64, i64)],
    /// The types a spawn picks from, per size.
    pub frigates: &'static [i64],
    pub destroyers: &'static [i64],
    pub cruisers: &'static [i64],
    pub equipment: &'static [i64],
    pub cargo: &'static [Cargo],
}

impl Faction {
    /// The types of one size, as the server can spawn them with no world:
    /// those [`find`] resolves.
    pub fn of_size(&self, size: Size) -> Vec<&'static NpcType> {
        let ids = match size {
            Size::Frigate => self.frigates,
            Size::Destroyer => self.destroyers,
            Size::Cruiser => self.cruisers,
        };
        ids.iter().filter_map(|id| find(*id)).collect()
    }

    /// The wreck a type of `group_id` leaves, if the group is this faction's.
    pub fn wreck_of(&self, group_id: i64) -> Option<i64> {
        self.groups
            .iter()
            .find(|(g, _)| *g == group_id)
            .map(|(_, w)| *w)
    }
}

/// **The no-bundle faction** (CP61): [`FIXTURE_RAT`]'s, every id invented
/// but the store's wrecks. A belt in a system with no loaded pirate picks
/// from it (`npc_gen::hand_squad`).
pub const FIXTURE_FACTION: Faction = Faction {
    name: "Fixture Cartel",
    owner_corp_id: FIXTURE_CORP,
    groups: &[
        (FIXTURE_GROUP_FRIGATE, HAND_SMALL_WRECK),
        (FIXTURE_GROUP_DESTROYER, HAND_SMALL_WRECK),
        (FIXTURE_GROUP_CRUISER, HAND_MEDIUM_WRECK),
    ],
    frigates: &[FIXTURE_RAT.type_id],
    destroyers: &[],
    cruisers: &[],
    equipment: FIXTURE_EQUIPMENT,
    cargo: FIXTURE_CARGO,
};

/// Every hand faction: [`FIXTURE_FACTION`].
pub const FACTIONS: &[Faction] = &[FIXTURE_FACTION];

/// **The faction a system's belts hold when the loaded rows name none**
/// ([`pirate_of`]): the no-bundle [`FIXTURE_FACTION`], everywhere.
pub fn faction_for(_solar_system_id: i64) -> &'static Faction {
    &FIXTURE_FACTION
}
/// **The pirate faction a system's belts hold, from the loaded rows** (C6
/// R3, CP20): `npc_data`'s `pirate(system)` over the world's catalogue.
/// `None` when the rows name none -- which, with no bundle, is every real
/// system (the fixture's regions are invented), so callers keep
/// [`faction_for`]'s no-bundle fixture.
pub fn pirate_of(w: &Registry, solar_system_id: i64) -> Option<i64> {
    w.content()
        .get::<crate::npc_data::Table>()
        .pirate(w.celestials(), solar_system_id)
}

/// The dogma attributes an entity's numbers are read from (`dogmaattributes`
/// names, ids by `tools/dogma.py`) **[V-data]**.
pub mod attr {
    pub const SPEED: i64 = 51;
    pub const ENTITY_ATTACK_RANGE: i64 = 247;
    pub const ENTITY_FLY_RANGE: i64 = 416;
    pub const ENTITY_EQUIPMENT_MIN: i64 = 456;
    pub const ENTITY_EQUIPMENT_MAX: i64 = 457;
    pub const ENTITY_KILL_BOUNTY: i64 = 481;
    pub const ENTITY_CRUISE_SPEED: i64 = 508;
    pub const AI_IGNORE_DRONES_BELOW_SIGNATURE_RADIUS: i64 = 1855;
}

/// `[A]` An entity's agility when it carries none: belt types lack
/// attribute 70 and its default is 0.0 **[V-data]**, and a zero `tau` makes
/// `Motion::advanced_at` return the motion unchanged, so the server would
/// hold the NPC still while the client flies it. 0.5 gives tau about 1 s
/// for a frigate. Client and server agree on whatever this is, because it
/// rides in the ball blob; only realism is assumed. Falsified by an NPC that
/// turns implausibly.
pub const FALLBACK_AGILITY: f32 = 0.5;

/// `[A]` The least lock range an entity gets: none carries attribute 76
/// **[V-data]**, and the client never reads an NPC's. The larger of this and
/// the type's attack range, so a rat can always lock what it can shoot.
pub const MIN_LOCK_RANGE: f64 = 30_000.0;

/// **An NPC type from the client's own tables** (`--typedata`), for a type in
/// one of `faction`'s groups.
///
/// Every number is the type's own attribute (defaults applied) **[V-data]**,
/// hull from 37, 263,
/// 479, 265, 9, 192, 564, 552, 482, 55 and the row's mass and radius; the gun
/// from 51 and 114/116/117/118 times 64; ranges 247 and 416; cruise 508 over
/// 37; bounty 481; equipment counts 456/457 (a negative minimum is read as
/// zero). The default effect's `guid` is the gun's visual. `[A]` agility and
/// lock range when absent ([`FALLBACK_AGILITY`], [`MIN_LOCK_RANGE`]); the
/// thresholds are the faction's verified pair; the loot kit is the
/// faction's. `None` for a type outside the faction's groups.
pub fn derive(
    row: &crate::typedata::TypeRow,
    table: &crate::typedata::Table,
    faction: &Faction,
) -> Option<NpcType> {
    let wreck = faction.wreck_of(row.group_id)?;
    Some(derive_as(
        row,
        table,
        faction.owner_corp_id,
        Some(wreck),
        (faction.equipment, faction.cargo),
    ))
}

/// **An NPC type from the loaded `npc/` rows** (CP20, C6 R1-R2): [`derive`]
/// for a type `npc/types.tsv` lists, of **any** faction. The owner is the
/// faction row's ([`crate::npc_data::Faction::owner`]) and the wreck its
/// `wrecks[class]`. `[A]` the thresholds are [`RESPONSE_THRESHOLD`]
/// (the bundle carries `entityStandings` presence, not its numbers), and the
/// loot kit is empty until CP22's pools (C6 R7). `None` for a type the rows
/// do not list, or whose faction has no row.
pub fn derive_loaded(
    row: &crate::typedata::TypeRow,
    table: &crate::typedata::Table,
    data: &crate::npc_data::Table,
) -> Option<NpcType> {
    let t = data.types.get(&row.type_id)?;
    let f = data.factions.get(&t.faction_id)?;
    let wreck = f.wrecks.get(&t.class).copied();
    Some(derive_as(row, table, f.owner(), wreck, (&[], &[])))
}

/// **An entity type's ball numbers** from its own attributes, with the
/// [`FALLBACK_AGILITY`] and [`MIN_LOCK_RANGE`] it lacks. The one place the
/// hull of a belt or site NPC is made: [`derive_as`] keeps it in the
/// [`NpcType`], and `typedata::Installed` serves the same numbers to
/// `destiny::hull::find_in`, which is what the ball, the server's own
/// integration and the web factor read. NPC-CHASE: without the second a
/// derived NPC had **no** hull there (an entity is not a ship row), so every
/// loaded rat flew as [`crate::destiny::hull::FALLBACK`], 1 m/s, and sat
/// still on the server and on the client.
pub fn entity_hull(row: &crate::typedata::TypeRow, table: &crate::typedata::Table) -> Hull {
    use crate::typedata::attr as t;
    let a = |id: i64| table.attribute(row.type_id, id);
    let attack_range = a(attr::ENTITY_ATTACK_RANGE);
    let agility = a(t::AGILITY) as f32;
    Hull {
        type_id: row.type_id,
        mass: row.mass,
        radius: row.radius as f32,
        max_velocity: a(t::MAX_VELOCITY) as f32,
        agility: if agility > 0.0 {
            agility
        } else {
            FALLBACK_AGILITY
        },
        shield_capacity: a(t::SHIELD_CAPACITY),
        shield_recharge_rate: a(t::SHIELD_RECHARGE),
        armor_hp: a(t::ARMOR_HP),
        max_target_range: a(t::MAX_TARGET_RANGE).max(attack_range).max(MIN_LOCK_RANGE),
        max_locked_targets: a(t::MAX_LOCKED_TARGETS),
        scan_resolution: a(t::SCAN_RESOLUTION),
        signature_radius: a(t::SIGNATURE_RADIUS),
        // NPCs do not warp.
        warp_speed_au: 0.0,
        hp: a(t::HP),
        capacitor_capacity: a(t::CAPACITOR_CAPACITY),
        capacitor_recharge_rate: a(t::CAPACITOR_RECHARGE),
    }
}

/// [`derive`]'s numbers, with the owner, wreck and loot kit given.
fn derive_as(
    row: &crate::typedata::TypeRow,
    table: &crate::typedata::Table,
    owner_corp_id: i64,
    wreck: Option<i64>,
    (equipment, cargo): (&'static [i64], &'static [Cargo]),
) -> NpcType {
    use crate::typedata::attr as t;
    let a = |id: i64| table.attribute(row.type_id, id);
    let velocity = a(t::MAX_VELOCITY);
    let attack_range = a(attr::ENTITY_ATTACK_RANGE);
    let guid = row
        .default_effect
        .and_then(|e| table.effect(e))
        .map(|e| e.guid.as_str())
        .filter(|g| !g.is_empty())
        .unwrap_or(TURRET_FX_GUID);
    let count = |id: i64| u32::try_from(a(id).max(0.0) as i64).unwrap_or(0);
    NpcType {
        type_id: row.type_id,
        name: leak(&row.name),
        group_id: row.group_id,
        owner_corp_id,
        hostile_threshold: RESPONSE_THRESHOLD,
        friendly_threshold: RESPONSE_THRESHOLD,
        hull: entity_hull(row, table),
        weapon_period_ms: a(attr::SPEED) as i64,
        damage: Damage {
            em: a(crate::damage::attr::EM_DAMAGE),
            explosive: a(crate::damage::attr::EXPLOSIVE_DAMAGE),
            kinetic: a(crate::damage::attr::KINETIC_DAMAGE),
            thermal: a(crate::damage::attr::THERMAL_DAMAGE),
        },
        damage_multiplier: a(t::DAMAGE_MULTIPLIER),
        weapon_guid: leak(guid),
        attack_range,
        orbit_range: a(attr::ENTITY_FLY_RANGE),
        cruise_fraction: if velocity > 0.0 {
            (a(attr::ENTITY_CRUISE_SPEED) / velocity).clamp(0.0, 1.0)
        } else {
            0.0
        },
        wreck_type_id: wreck,
        bounty: a(attr::ENTITY_KILL_BOUNTY),
        ignore_drones_below_signature: a(attr::AI_IGNORE_DRONES_BELOW_SIGNATURE_RADIUS),
        equipment_min: count(attr::ENTITY_EQUIPMENT_MIN),
        equipment_max: count(attr::ENTITY_EQUIPMENT_MAX),
        equipment,
        cargo,
    }
}

/// A name for a derived type, kept for the process: [`NpcType`] holds
/// `&'static str`, as `crate::typedata` does for its own views.
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// [`derive`] over a whole table, for every faction. Pure.
pub fn derive_all(table: &crate::typedata::Table) -> BTreeMap<i64, NpcType> {
    let mut out = BTreeMap::new();
    for row in table.types() {
        if let Some(t) = FACTIONS.iter().find_map(|f| derive(row, table, f)) {
            out.insert(t.type_id, t);
        }
    }
    out
}

/// A number in `[0, 1)` from two others, always the same for the same two:
/// `splitmix64`'s finaliser, as `combat::survives` rolls a drop.
pub(crate) fn roll(seed: i64, salt: u64) -> f64 {
    let mut z = (seed as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// An integer in `min..=max` from a roll.
pub(crate) fn pick(r: f64, min: i64, max: i64) -> i64 {
    let span = (max - min).max(0) + 1;
    min + ((r * span as f64) as i64).min(span - 1)
}

/// A hand NPC type's hull, which `destiny::hull::find_in` asks first.
pub fn hull(type_id: i64) -> Option<&'static Hull> {
    find(type_id).map(|t| &t.hull)
}

/// Who flies a roster entry: a character, or an NPC.
///
/// The answer to the seam (`docs/lanes/npc.md` §5): every place that turns a
/// presence into a hull, an owner or an audience asks `Registry::pilot`,
/// which is this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pilot {
    pub ship_type_id: i64,
    /// The character for a player; the NPC corporation for an NPC.
    pub owner_id: i64,
    pub corporation_id: i64,
    pub alliance_id: Option<i64>,
    /// The character, or `None` for an NPC -- which is also the test for
    /// "is anyone listening": only a character has a session.
    pub char_id: Option<i64>,
}

/// `Registry::pilot`'s body: the character first, then the NPC table.
pub fn pilot_of(w: &Registry, roster_key: i64) -> Option<Pilot> {
    if let Some(c) = w.character(roster_key) {
        return Some(Pilot {
            ship_type_id: c.ship_type_id,
            owner_id: c.character_id,
            corporation_id: c.corporation_id,
            alliance_id: c.alliance_id,
            char_id: Some(c.character_id),
        });
    }
    w.npcs().get(roster_key).map(|n| Pilot {
        ship_type_id: n.kind.type_id,
        owner_id: n.kind.owner_corp_id,
        corporation_id: n.kind.owner_corp_id,
        alliance_id: None,
        char_id: None,
    })
}

/// What an NPC is doing with its engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Stopped, or stopping.
    Idle,
    /// Closing on its target at full speed.
    Approach,
    /// Circling its target at cruise speed.
    Orbit,
}

/// One live NPC: what it is, and what it is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Npc {
    /// Its ball id, roster key and item id, all one number.
    pub id: i64,
    pub kind: &'static NpcType,
    pub solar_system_id: i64,
    pub phase: Phase,
    /// The ship it is fighting.
    pub target: Option<i64>,
    /// When its lock completes, on the blue clock.
    pub lock_ready_at: Option<BlueTime>,
    /// An `OnSpecialFX` start is outstanding.
    pub firing: bool,
    /// When its next volley lands, on the blue clock.
    pub next_shot_at: BlueTime,
    /// What its brain remembers: home, the switch and repair timers
    /// (`brain`, CP23).
    pub mind: Mind,
    /// `Some` for a ship of the law (CONCORD-1a, `crate::concord`): it sees
    /// only the pilot it was sent for ([`view_of`]).
    pub law: Option<Law>,
}

/// Whom a ship of the law was sent for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Law {
    /// The offender's character: the one pilot its brain may see.
    pub offender: i64,
}

impl Npc {
    /// A fresh one, idle.
    pub fn new(id: i64, kind: &'static NpcType, solar_system_id: i64) -> Npc {
        Npc {
            id,
            kind,
            solar_system_id,
            phase: Phase::Idle,
            target: None,
            lock_ready_at: None,
            firing: false,
            next_shot_at: 0,
            mind: Mind::default(),
            law: None,
        }
    }
}

/// Every live NPC, and the id allocator its NPCs and their wrecks share.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Npcs {
    live: BTreeMap<i64, Npc>,
    issued: i64,
    /// Every asteroid belt's rats: who is there, when they come back
    /// (`crate::belts`). Here rather than beside this table in
    /// `Registry` because it is NPC state with the same lifetime -- not
    /// persisted, emptied by a restart.
    belts: crate::belts::Belts,
    /// The loaded types [`resolve`] derived, once each, for the process.
    kinds: BTreeMap<i64, &'static NpcType>,
}

impl Npcs {
    /// The belt spawns (`crate::belts::tick_belts` is the writer).
    pub fn belts(&self) -> &crate::belts::Belts {
        &self.belts
    }

    pub fn belts_mut(&mut self) -> &mut crate::belts::Belts {
        &mut self.belts
    }

    pub fn get(&self, id: i64) -> Option<&Npc> {
        self.live.get(&id)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut Npc> {
        self.live.get_mut(&id)
    }

    pub fn insert(&mut self, npc: Npc) {
        self.live.insert(npc.id, npc);
    }

    pub fn remove(&mut self, id: i64) -> Option<Npc> {
        self.live.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Npc> {
        self.live.values()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// A fresh id in [`MIN_NPC_ID`]..[`END_NPC_ID`], for an NPC or for the
    /// wreck one leaves; `None` once a billion have been handed out.
    pub fn next_id(&mut self) -> Option<i64> {
        let id = MIN_NPC_ID.checked_add(self.issued)?;
        if id >= END_NPC_ID {
            return None;
        }
        self.issued += 1;
        Some(id)
    }
}

/// An NPC's ball: an ordinary free ball whose numbers come from its hull
/// through `destiny::hull::find_in`, which reads [`hull`] first.
pub fn ball(content: &crate::content::Content, n: &Npc, motion: &Motion) -> destiny::Ball {
    destiny::ego_ball_in(
        content,
        n.id,
        n.kind.type_id,
        i32::try_from(n.kind.owner_corp_id).unwrap_or(-1),
        // An NPC corporation has no alliance.
        -1,
        motion,
    )
}

/// An NPC's slim item (`docs/lanes/npc.md` §2).
///
/// - `ownerID` the NPC corporation, so `IsNPC(ownerID)` holds
///   (`idCheckers.py:283`) **[V]**;
/// - `charID` `None`: no pilot, so `uix.GetSlimItemName:488` names the ball
///   by its type's name **[V]**;
/// - both response thresholds, as floats: see `destiny::Slim`.
pub fn slim(n: &Npc) -> destiny::Slim {
    destiny::Slim {
        item_id: n.id,
        type_id: n.kind.type_id,
        owner_id: n.kind.owner_corp_id,
        char_id: None,
        corp_id: Some(n.kind.owner_corp_id),
        response_thresholds: Some((n.kind.hostile_threshold, n.kind.friendly_threshold)),
    }
}

// ---------------------------------------------------------------------------
// The executor
// ---------------------------------------------------------------------------

/// **The world clock's hook**, after `combat::tick` (`push.rs`): forget the
/// NPCs that have left the roster, then, one system at a time, bring every
/// ball to the present, let each NPC decide, and apply what it decided. All
/// of one system's movement goes out as **one** `ActionUpdate`
/// (`docs/re/npc-behaviors.md` T4).
pub fn tick(w: &mut Registry, now: BlueTime) {
    // The belts first: a pilot who has just arrived gets rats this tick, and
    // they look round in the same tick.
    crate::belts::tick_belts(w, now);
    if w.npcs().is_empty() {
        return;
    }
    // DEAD is not a state: `combat::kill` takes the ball out of the roster.
    let gone: Vec<i64> = w
        .npcs()
        .iter()
        .filter(|n| w.roster().get(n.id).is_none())
        .map(|n| n.id)
        .collect();
    for id in gone {
        w.npcs_mut().remove(id);
    }
    let mut systems: Vec<i64> = w.npcs().iter().map(|n| n.solar_system_id).collect();
    systems.sort_unstable();
    systems.dedup();
    for system in systems {
        // Under load a system with no human is skipped on some ticks
        // (W14 CP40, C7 §5.3); `MAX_SHOTS_PER_TICK` absorbs the catch-up.
        if !crate::liveness::due(w, system) {
            continue;
        }
        tick_system(w, system, now);
    }
}

/// **TICK-1: whether `system` has nobody to fight**: no pilot in space
/// ([`crate::tick_budget::Class`] below hot) and nothing in the roster but
/// NPCs, so every NPC's view has no candidates.
fn nobody_here(w: &Registry, system: i64) -> bool {
    crate::tick_budget::class_of(w, system) != crate::tick_budget::Class::Hot
        && w.roster()
            .in_system(system)
            .iter()
            .all(|p| w.npcs().get(p.character_id).is_some())
}

/// An NPC's behaviour: [`Behaviour::of`], and for a faction police ship the
/// tackle its group-182 type carries (NW-7: those types are not in
/// `npc/types.tsv`, so no profile gives the lead its scram and web).
fn behaviour_of(w: &Registry, npc: &Npc) -> Behaviour {
    let mut b = Behaviour::of(w, npc.kind.type_id);
    if npc.law.is_some()
        && b.scram.is_none()
        && b.web.is_none()
        && crate::concord::police_target(w, npc.id).is_some()
    {
        let e = crate::concord::police_ewar_of(
            crate::typedata::installed_in(w.content()).table(),
            npc.kind.type_id,
        );
        b.scram = e.scram;
        b.web = e.web;
    }
    b
}

/// **TICK-1: whether [`brain::decide`] is the identity for `npc`** when its
/// view has no candidates and no allies: it holds no target, is not on its
/// way home, remembers no locker, is not firing, has no repairer due (a
/// repairer with no due time yet is not idle: its first look sets one), and
/// has no remote repairer to look for wards. Then `decide` returns the same
/// NPC and no intent, `reaim` returns (no target), and `view_of` only reads,
/// so skipping the NPC changes nothing.
fn idle_alone(w: &Registry, npc: &Npc, now: BlueTime) -> bool {
    idle_with(npc, &behaviour_of(w, npc), now)
}

/// [`idle_alone`] over a given behaviour (the pure half, tested against
/// [`decide`] itself).
fn idle_with(npc: &Npc, b: &Behaviour, now: BlueTime) -> bool {
    npc.target.is_none()
        && !npc.firing
        && !npc.mind.returning
        && npc.mind.aggressors.iter().all(Option::is_none)
        && b.assist.is_none()
        && b.self_rep
            .is_none_or(|_| npc.mind.next_rep_at.is_some_and(|due| now < due))
}

fn tick_system(w: &mut Registry, system: i64, now: BlueTime) {
    let alone = nobody_here(w, system);
    let ids: Vec<i64> = w
        .npcs()
        .iter()
        .filter(|n| n.solar_system_id == system)
        // An NPC still flying in from its warp-in has no brain yet.
        .filter(|n| {
            !w.roster()
                .get(n.id)
                .is_some_and(|p| p.motion.warp_commanded())
        })
        .map(|n| n.id)
        .collect();
    // TICK-1: nobody here and every NPC idle: nothing below reads a
    // position or changes a thing, so the system is not carried to now (a
    // ball is advanced on every read, and the closed form composes).
    if alone
        && ids
            .iter()
            .all(|id| w.npcs().get(*id).is_none_or(|n| idle_alone(w, n, now)))
    {
        return;
    }
    w.advance_system(system);
    let mut actions = Vec::new();
    run_system(
        w,
        system,
        now,
        alone,
        ids,
        crate::par::threads(),
        &mut actions,
    );
    if actions.is_empty() {
        return;
    }
    let to = audience(w, system);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    if let Some(update) = destiny::ActionUpdate::new(to, stamp, actions) {
        w.schedule(Duration::ZERO, update);
    }
}

/// Entry visits (NPCs x scene rows) below which the plan stays on the
/// caller's thread. **[A]** ~0.3 ms of serial plan after the scene hoist, a
/// few spawns' worth (PA-6: 17-34 us each); falsified by `brawl_bench` at 500
/// pilots, where 8 threads must beat 1 by 1.5x at the gate.
const NPC_PAR_WORK: usize = 40_000;

/// What `run_system` did, for the tests and the bench.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // read by the golden test and the bench
pub(crate) struct RunStats {
    /// NPCs whose plan was used as it was made.
    pub planned: usize,
    /// NPCs whose plan was stale (or missing) and was worked out again.
    pub fresh: usize,
    /// The plan panicked and the whole system ran the legacy loop.
    pub fell_back: bool,
}

/// One roster entry of a system, with what [`view_of`] asks about it that
/// does not depend on which NPC is looking.
struct SceneRow {
    p: Presence,
    protected: bool,
    drone: Option<crate::drones::Drone>,
    pilot: Option<Pilot>,
}

/// **The system's roster, read once** (PA-8a): [`view_of`] copied it and
/// asked `ship_protected`, the drone table and `Registry::pilot` of every
/// entry for every NPC; that is O(NPCs x balls) of the same answers.
fn scene_of(w: &Registry, system: i64, now: BlueTime) -> Vec<SceneRow> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| SceneRow {
            // Out of reach: protected (UNDOCK-INVULN) or cloaked (CLOAK-1).
            protected: crate::invuln::ship_protected(w, p.ship_id, now)
                || crate::cloak::is_cloaked(w, p.ship_id)
                // In warp (WARP-LOCK #191): not there to be locked or
                // aggressed until the warp ends.
                || p.motion.warp_active(),
            drone: w.modules().drones().get(p.character_id).copied(),
            pilot: w.pilot(p.character_id),
            p,
        })
        .collect()
}

/// [`view_of`] over a scene built once: the same filters in the same order,
/// the same per-type cache filled by the first ship of each type in roster
/// order. **Pure**: it reads `w` and writes nothing, so it can run on any
/// thread (the contract of a plan).
fn view_in(w: &Registry, scene: &[SceneRow], npc: &Npc, now: BlueTime) -> Option<View> {
    let me = w.roster().get(npc.id)?.motion.position;
    let lockers = w.locks().targeters_of(npc.id);
    let ledger = w.ext::<crate::npc_threat::Threat>();
    let mut by_type: std::collections::BTreeMap<i64, (BlueTime, f64)> = Default::default();
    let candidates = scene
        .iter()
        .filter_map(|r| {
            let p = &r.p;
            if npc.law.is_some_and(|l| l.offender != p.character_id) {
                return None;
            }
            if r.protected {
                return None;
            }
            if let Some(d) = &r.drone {
                return drone_in_view(w.content(), npc, d).then(|| Candidate {
                    ship_id: p.ship_id,
                    position: p.motion.position,
                    is_capsule: false,
                    locking_me: true,
                    lock_delay: blue(crate::targeting::acquisition_delay(w, npc.id, p.ship_id)),
                    signature: brain::signature_of(w.content(), d.kind.type_id),
                    // The drone's acts are its owner's ship's (npc_threat).
                    aggressor: false,
                    threat: 0.0,
                });
            }
            let pilot = r.pilot?;
            pilot.char_id?;
            let (lock_delay, signature) = *by_type.entry(pilot.ship_type_id).or_insert_with(|| {
                (
                    blue(crate::targeting::acquisition_delay(w, npc.id, p.ship_id)),
                    brain::signature_of(w.content(), pilot.ship_type_id),
                )
            });
            let (aggressor, threat) = standing(ledger, npc, p.ship_id, now);
            Some(Candidate {
                ship_id: p.ship_id,
                position: p.motion.position,
                is_capsule: pilot.ship_type_id == crate::damage::CAPSULE_TYPE_ID,
                locking_me: lockers.contains(&p.ship_id),
                lock_delay,
                signature,
                aggressor,
                threat,
            })
        })
        .collect();
    let lock_held = npc
        .target
        .is_some_and(|t| w.locks().targets_of(npc.id).contains(&t));
    let mut behaviour = behaviour_of(w, npc);
    behaviour.gun_reach = crate::combat::entity_gun_reach(w, npc.id);
    let allies = if behaviour.assist.is_some() && npc.law.is_none() {
        allies_of(w, npc, now)
    } else {
        Vec::new()
    };
    let lock_range = w.roster().get(npc.id).map_or(0.0, |p| {
        w.max_target_range_of_ship(
            p.ship_id,
            w.pilot(p.character_id).map_or(0, |q| q.ship_type_id),
        ) * crate::ewar::range_factor(w, p.ship_id)
    });
    Some(View {
        me,
        candidates,
        allies,
        lock_held,
        lock_range,
        now,
        behaviour,
    })
}

/// What [`view_of`] filters candidates by besides the roster: the cloaks and
/// the undock/jump protections. Neither bumps the roster's revision, so a
/// plan read before one changed is stale; compared by value (both sets are
/// small, and empty almost always).
fn cover_marks(
    w: &Registry,
) -> (
    std::collections::BTreeSet<i64>,
    Vec<(i64, crate::invuln::Grant)>,
) {
    (crate::cloak::hidden_ids(w), crate::invuln::grants(w))
}

/// One NPC's plan: what it was planned from, and what it decided.
struct Plan {
    /// The NPC record it was planned from.
    input: Npc,
    /// Its view read other NPCs' health (a remote repairer's wards), which
    /// the apply loop changes through `SelfRep` and `Assist`.
    reads_allies: bool,
    /// The threat ledger's revision when it was planned (NPC-THREAT): a write
    /// since makes the plan stale.
    threat_rev: u64,
    /// The view, kept to compare with [`view_of`] in debug builds.
    #[cfg(debug_assertions)]
    view: Option<View>,
    decided: Option<(Npc, Vec<Intent>)>,
}

fn plan_one(
    w: &Registry,
    scene: &[SceneRow],
    npc: &Npc,
    now: BlueTime,
    #[cfg(test)] fault: Option<i64>,
) -> Plan {
    #[cfg(test)]
    assert!(
        fault != Some(npc.id),
        "planted fault in the plan of {}",
        npc.id
    );
    let view = view_in(w, scene, npc, now);
    Plan {
        input: *npc,
        threat_rev: w.ext::<crate::npc_threat::Threat>().revision(),
        reads_allies: view
            .as_ref()
            .is_some_and(|v| v.behaviour.assist.is_some() && npc.law.is_none()),
        decided: view.as_ref().map(|v| decide(npc, v)),
        #[cfg(debug_assertions)]
        view,
    }
}

/// **The NPC loop of one system, planned in parallel and applied in order**
/// (PA-8a). Every NPC's view and decision is a pure function of the world as
/// `advance_system` left it, so they are made against one [`scene_of`], on
/// `threads` threads; then the NPCs are applied in id order, as before. A plan
/// is used only while what it read is unchanged -- the roster's revision, the
/// NPC's own record, and (for a remote repairer) the loop's own `bars_dirty`
/// -- else that NPC is worked out again from the live world, so the result is
/// byte-identical to [`run_system_legacy`] at any thread count. A plan that
/// panics sends the whole system to the legacy loop.
pub(crate) fn run_system(
    w: &mut Registry,
    system: i64,
    now: BlueTime,
    alone: bool,
    ids: Vec<i64>,
    threads: usize,
    actions: &mut Vec<destiny::Action>,
) -> RunStats {
    let tuning = Tuning {
        threads,
        gate: NPC_PAR_WORK,
        #[cfg(test)]
        fault: None,
    };
    run_planned(w, system, now, alone, ids, tuning, actions)
}

/// How a run plans: the thread count, the work below which it stays on the
/// caller's thread, and (tests) an NPC whose plan panics.
#[derive(Clone, Copy)]
struct Tuning {
    threads: usize,
    gate: usize,
    #[cfg(test)]
    fault: Option<i64>,
}

fn run_planned(
    w: &mut Registry,
    system: i64,
    now: BlueTime,
    alone: bool,
    ids: Vec<i64>,
    tuning: Tuning,
    actions: &mut Vec<destiny::Action>,
) -> RunStats {
    let revision0 = w.roster().revision();
    let cover0 = cover_marks(w);
    let inputs: Vec<Npc> = ids
        .iter()
        .filter_map(|id| w.npcs().get(*id).copied())
        .filter(|n| !(alone && idle_alone(w, n, now)))
        .collect();
    let scene = scene_of(w, system, now);
    let big = inputs.len().saturating_mul(scene.len()) >= tuning.gate;
    let planned = crate::par::par_map_with(if big { tuning.threads } else { 1 }, &inputs, 1, |n| {
        plan_one(
            w,
            &scene,
            n,
            now,
            #[cfg(test)]
            tuning.fault,
        )
    });
    let mut plans: std::collections::BTreeMap<i64, Plan> = match planned {
        Ok(v) => v.into_iter().map(|p| (p.input.id, p)).collect(),
        Err(e) => {
            eprintln!("[npc] system {system}: {e}; running the serial loop");
            run_system_legacy(w, now, alone, ids, actions);
            return RunStats {
                fell_back: true,
                ..RunStats::default()
            };
        }
    };
    drop(scene);
    let mut stats = RunStats::default();
    let mut bars_dirty = false;
    for id in ids {
        let Some(npc) = w.npcs().get(id).copied() else {
            continue;
        };
        if alone && idle_alone(w, &npc, now) {
            continue;
        }
        reaim(w, &npc);
        let plan = plans.remove(&id);
        let stale = plan.as_ref().is_none_or(|p| {
            p.input != npc
                || w.roster().revision() != revision0
                || cover_marks(w) != cover0
                || w.ext::<crate::npc_threat::Threat>().revision() != p.threat_rev
                || (bars_dirty && p.reads_allies)
        });
        let decided = match plan {
            Some(p) if !stale => {
                stats.planned += 1;
                // The witness says the world this plan read is the world the
                // legacy loop would read now; check it in every debug test.
                #[cfg(debug_assertions)]
                debug_assert_eq!(
                    p.view,
                    view_of(w, &npc, now),
                    "npc {}: the planned view differs from the live one",
                    npc.id
                );
                p.decided
            }
            _ => {
                stats.fresh += 1;
                view_of(w, &npc, now).map(|v| decide(&npc, &v))
            }
        };
        let Some((next, intents)) = decided else {
            continue;
        };
        if let Some(slot) = w.npcs_mut().get_mut(id) {
            *slot = next;
        }
        for intent in intents {
            bars_dirty |= matches!(intent, Intent::SelfRep { .. } | Intent::Assist { .. });
            apply(w, &npc, intent, now, actions);
        }
    }
    stats
}

/// The NPC loop as it was before PA-8a: one live [`view_of`] per NPC, in
/// order. The oracle of the golden test and the fallback of a plan that
/// panics.
pub(crate) fn run_system_legacy(
    w: &mut Registry,
    now: BlueTime,
    alone: bool,
    ids: Vec<i64>,
    actions: &mut Vec<destiny::Action>,
) {
    for id in ids {
        let Some(npc) = w.npcs().get(id).copied() else {
            continue;
        };
        if alone && idle_alone(w, &npc, now) {
            continue;
        }
        reaim(w, &npc);
        let Some(view) = view_of(w, &npc, now) else {
            continue;
        };
        let (next, intents) = decide(&npc, &view);
        if let Some(slot) = w.npcs_mut().get_mut(id) {
            *slot = next;
        }
        for intent in intents {
            apply(w, &npc, intent, now, actions);
        }
    }
}

/// **The server's record of an approach is a goto at the target's current
/// position, re-aimed every tick** -- while the wire says `FollowBall`.
///
/// `Registry::advance_character` reconciles a `Follow` by placing the ball
/// at its follow range from the target on every read (`Motion::tethered_to`),
/// which for a player who has just clicked "approach" is a small error and
/// for an NPC 15 km out is a teleport: the server would believe it in gun
/// range a moment after the order, while every client watches it fly in for
/// seven seconds -- and would order the orbit, at cruise speed, from there.
/// A goto is integrated by the ordinary drag law instead, which is the
/// client's own (`Motion::advanced_at`), so the server's NPC arrives when
/// the client's does.
///
/// `[A]` that a goto at the target's position, refreshed at the tick rate,
/// tracks `EvolveFollow` closely enough for range decisions. The cost is
/// that a pilot who binds mid-approach is handed a ball in `GOTO` toward
/// where the target was, not `FOLLOW`; the next order corrects it.
fn reaim(w: &mut Registry, npc: &Npc) {
    if npc.phase != Phase::Approach {
        return;
    }
    let Some(t) = npc.target else { return };
    let Some(at) = w.roster().by_ship(t).map(|p| p.motion.position) else {
        return;
    };
    if let Some(p) = w.roster_mut().get_mut(npc.id) {
        p.motion.mode = Mode::Goto(at);
    }
}

/// What one NPC can see: every player ship in its system, and **the drones
/// that matter to it** ([`drone_in_view`]).
fn view_of(w: &Registry, npc: &Npc, now: BlueTime) -> Option<View> {
    let me = w.roster().get(npc.id)?.motion.position;
    // **Per NPC, not per pair** (PERF-STAGES): who has this NPC locked is one
    // scan of the locks rather than an allocation per candidate, and a
    // candidate's lock time and signature depend on the NPC and on the
    // *type* of the ship it looks at, so each type is worked out once. A
    // belt of 500 miners in a handful of hull types cost 500 times the work
    // of a handful.
    let lockers = w.locks().targeters_of(npc.id);
    let ledger = w.ext::<crate::npc_threat::Threat>();
    let mut by_type: std::collections::BTreeMap<i64, (BlueTime, f64)> = Default::default();
    let candidates = w
        .roster()
        .in_system(npc.solar_system_id)
        .into_iter()
        .filter_map(|p| {
            // A ship of the law sees its offender and nobody else, drones
            // and bystanders included (CONCORD-1a, R-CONCORD section 4.4).
            if npc.law.is_some_and(|l| l.offender != p.character_id) {
                return None;
            }
            // UNDOCK-INVULN: a protected ship is not there to be aggressed.
            if crate::invuln::ship_protected(w, p.ship_id, now)
                || crate::cloak::is_cloaked(w, p.ship_id)
                || p.motion.warp_active()
            {
                return None;
            }
            if let Some(d) = w.modules().drones().get(p.character_id) {
                return drone_in_view(w.content(), npc, d).then(|| Candidate {
                    ship_id: p.ship_id,
                    position: p.motion.position,
                    is_capsule: false,
                    locking_me: true,
                    lock_delay: blue(crate::targeting::acquisition_delay(w, npc.id, p.ship_id)),
                    signature: brain::signature_of(w.content(), d.kind.type_id),
                    // The drone's acts are its owner's ship's (npc_threat).
                    aggressor: false,
                    threat: 0.0,
                });
            }
            let pilot = w.pilot(p.character_id)?;
            // Players only: NPCs do not fight each other.
            pilot.char_id?;
            let (lock_delay, signature) = *by_type.entry(pilot.ship_type_id).or_insert_with(|| {
                (
                    blue(crate::targeting::acquisition_delay(w, npc.id, p.ship_id)),
                    brain::signature_of(w.content(), pilot.ship_type_id),
                )
            });
            let (aggressor, threat) = standing(ledger, npc, p.ship_id, now);
            Some(Candidate {
                ship_id: p.ship_id,
                position: p.motion.position,
                is_capsule: pilot.ship_type_id == crate::damage::CAPSULE_TYPE_ID,
                locking_me: lockers.contains(&p.ship_id),
                lock_delay,
                signature,
                aggressor,
                threat,
            })
        })
        .collect();
    let lock_held = npc
        .target
        .is_some_and(|t| w.locks().targets_of(npc.id).contains(&t));
    let mut behaviour = behaviour_of(w, npc);
    behaviour.gun_reach = crate::combat::entity_gun_reach(w, npc.id);
    let allies = if behaviour.assist.is_some() && npc.law.is_none() {
        allies_of(w, npc, now)
    } else {
        Vec::new()
    };
    // What the range sweep will use for this NPC (`targeting::sweep_out_of_range`).
    let lock_range = w.roster().get(npc.id).map_or(0.0, |p| {
        w.max_target_range_of_ship(
            p.ship_id,
            w.pilot(p.character_id).map_or(0, |q| q.ship_type_id),
        ) * crate::ewar::range_factor(w, p.ship_id)
    });
    Some(View {
        me,
        candidates,
        allies,
        lock_held,
        lock_range,
        now,
        behaviour,
    })
}

/// Whether a ship has acted on `npc` and how much it threatens it
/// (NPC-THREAT); the law has no ledger.
fn standing(
    ledger: &crate::npc_threat::Threat,
    npc: &Npc,
    ship: i64,
    now: BlueTime,
) -> (bool, f64) {
    if npc.law.is_some() {
        return (false, 0.0);
    }
    ledger
        .standing(npc.id, ship, now)
        .map_or((false, 0.0), |t| (true, t))
}

/// The other NPCs in `npc`'s system that have taken damage (K-RATAI): a
/// remote repairer's wards. An NPC no shot has hit has no combat record and
/// is whole. The law is not repaired by rats, nor rats by the law.
fn allies_of(w: &Registry, npc: &Npc, now: BlueTime) -> Vec<brain::Ally> {
    w.npcs()
        .iter()
        .filter(|a| a.id != npc.id && a.solar_system_id == npc.solar_system_id)
        .filter(|a| a.law.is_none())
        .filter_map(|a| {
            let position = w.roster().get(a.id)?.motion.position;
            let bars = w.combat().ship(a.id)?.bars(now);
            Some(brain::Ally {
                id: a.id,
                position,
                shield: bars.shield.map(|s| s.fraction),
                armour: bars.armour,
            })
        })
        .collect()
}

/// **A drone is a candidate while it is attacking this NPC** (its order is
/// `Engage(npc)`), or while it is already the NPC's target -- and only when
/// its `signatureRadius` is at least the type's
/// [`NpcType::ignore_drones_below_signature`] **[I]**. It is offered as
/// "locking me" -- a drone holds no lock in this model, but an attacking
/// drone is the same provocation -- so [`decide`]'s retaliation rule is the
/// one that picks it. `[A]` the rule: no switching away from a player to a
/// drone mid-fight beyond what `decide` already does (one target, kept
/// until it is gone).
///
/// For a frigate rat whose threshold is above a light drone's signature
/// this is never true: such a rat ignores light drones.
fn drone_in_view(content: &crate::content::Content, npc: &Npc, d: &crate::drones::Drone) -> bool {
    let provoked = d.order == crate::drones::Order::Engage(npc.id) || npc.target == Some(d.id);
    let signature = crate::destiny::hull::find_in(content, d.kind.type_id)
        .map_or(d.kind.hull.signature_radius, |h| h.signature_radius);
    provoked && signature >= npc.kind.ignore_drones_below_signature
}

/// A `Duration` in blue ticks.
fn blue(d: Duration) -> BlueTime {
    i64::try_from(d.as_millis())
        .unwrap_or(i64::MAX / BLUE_PER_MS)
        .saturating_mul(BLUE_PER_MS)
}

/// Everyone who should be told: the **characters** in a system. An NPC is in
/// the roster too and has no session (§4.3).
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Put one NPC of `kind` in space at `position`, idle, and answer its id.
/// Nobody is told: [`entity_slash`] tells everyone at once.
pub fn spawn(
    w: &mut Registry,
    kind: &'static NpcType,
    solar_system_id: i64,
    position: [f64; 3],
) -> Option<i64> {
    let id = w.npcs_mut().next_id()?;
    w.roster_mut().arrive(Presence::arriving(
        id,
        id,
        solar_system_id,
        Motion::arrived(position, [0.0; 3]),
    ));
    let mut npc = Npc::new(id, kind, solar_system_id);
    npc.mind = Mind::at(position);
    w.npcs_mut().insert(npc);
    Some(id)
}

/// How far out a warping-in NPC starts its flight, in metres **[A]**: the
/// real game's rats drop in from outside the grid's near field; 100 km keeps
/// the start inside the belt range ([`crate::belts::BELT_RANGE`]) so every
/// pilot there is told the ball at once. Falsified by a client that shows the
/// ball popping at the start point instead of flying in (`fci_npc_warpin`).
pub const WARP_IN_DISTANCE: f64 = 100_000.0;

/// **[`spawn`] an NPC that warps in**: the ball starts `WARP_IN_DISTANCE`
/// away from `position`, on the line from `from_dir` (a unit-less direction
/// from `position` to the start), in `DSTBALL_WARP` already active, bound for
/// `position`. The client's `ship.py:_SetInitialState` **[V]** fires
/// `effects.WarpIn` for any ship ball it is handed in `DSTBALL_WARP`
/// (not its own), so nothing but the mode has to be right. The brain does not
/// plan for it until the warp is over (`tick_system`). `position` stays the
/// NPC's home.
pub fn spawn_warping_in(
    w: &mut Registry,
    kind: &'static NpcType,
    solar_system_id: i64,
    position: [f64; 3],
    from_dir: [f64; 3],
) -> Option<i64> {
    let id = spawn(w, kind, solar_system_id, position)?;
    let dir = crate::space_state::unit(from_dir).unwrap_or([1.0, 0.0, 0.0]);
    let start = [
        position[0] + dir[0] * WARP_IN_DISTANCE,
        position[1] + dir[1] * WARP_IN_DISTANCE,
        position[2] + dir[2] * WARP_IN_DISTANCE,
    ];
    let factor = destiny::warp_factor_in(w.content(), kind.type_id);
    if let Some(p) = w.roster_mut().get_mut(id) {
        p.motion.position = start;
        p.motion.speed_fraction = 1.0;
        p.motion.mode = destiny::Mode::Warp {
            goto: position,
            effect_stamp: crate::space_state::now_seconds() as i32 - 1,
            total_distance: WARP_IN_DISTANCE,
            stop_distance: 0.0,
            warp_factor: factor,
        };
    }
    Some(id)
}

/// [`spawn`] a ship of the law: it never leashes (no home) and its brain
/// sees only `law.offender` (CONCORD-1a).
pub fn spawn_bound(
    w: &mut Registry,
    kind: &'static NpcType,
    solar_system_id: i64,
    position: [f64; 3],
    law: Law,
) -> Option<i64> {
    let id = spawn(w, kind, solar_system_id, position)?;
    bind_law(w, id, law);
    Some(id)
}

/// [`spawn_bound`] for a ship that **warps in** ([`spawn_warping_in`]): the
/// police and CONCORD squads that arrive on an occupied grid.
pub fn spawn_bound_warping_in(
    w: &mut Registry,
    kind: &'static NpcType,
    solar_system_id: i64,
    position: [f64; 3],
    law: Law,
    from_dir: [f64; 3],
) -> Option<i64> {
    let id = spawn_warping_in(w, kind, solar_system_id, position, from_dir)?;
    bind_law(w, id, law);
    Some(id)
}

fn bind_law(w: &mut Registry, id: i64, law: Law) {
    if let Some(n) = w.npcs_mut().get_mut(id) {
        n.law = Some(law);
        n.mind.home = None;
    }
}

/// **How long before its due time a warp-in must start to land on it**, in
/// blue ticks: the flight over [`WARP_IN_DISTANCE`] by the destiny warp model
/// (`destiny::warp_flight_seconds`, which rounds up, so a squad lands at or a
/// little before its due time, never after). `[A]` one hull's factor stands
/// for the whole squad (the default hull, type 0); falsified by a squad whose
/// ships land seconds apart in `fci_npc_warpin`.
pub fn warp_in_lead(w: &Registry) -> BlueTime {
    let factor = destiny::warp_factor_in(w.content(), 0);
    blue(Duration::from_secs_f64(destiny::warp_flight_seconds(
        WARP_IN_DISTANCE,
        factor,
        0.0,
    )))
}

/// Hand a ship of the law a new offender (a gate checkpoint taking a crime on
/// its grid, `crate::concord`). `false` when it is gone.
pub fn set_law(w: &mut Registry, id: i64, law: Law) -> bool {
    w.npcs_mut()
        .get_mut(id)
        .map(|n| n.law = Some(law))
        .is_some()
}

/// **One `AddBalls2` to every character in the system** for NPCs that are
/// already in the roster: [`entity_slash`]'s tell. `on_clock` sends it by
/// the clock (a tick); otherwise with the turn that asked (`push`).
pub fn announce(w: &mut Registry, solar_system_id: i64, ids: &[i64], on_clock: bool) {
    let now = crate::pyutil::filetime_now();
    let mut balls = Vec::new();
    let mut extra = Vec::new();
    for id in ids {
        let (Some(n), Some(p)) = (w.npcs().get(*id), w.roster().get(*id)) else {
            continue;
        };
        balls.push(ball(w.content(), n, &p.motion));
        extra.push((
            slim(n),
            Some(crate::combat::damage_state(w, *id, now).to_pyvalue()),
        ));
    }
    let to = audience(w, solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(added) = destiny::BallsAdded::new(
        to,
        stamp,
        destiny::Park {
            current_time: stamp,
            balls,
        },
        extra,
    ) {
        if on_clock {
            w.schedule(Duration::ZERO, added);
        } else {
            w.push(added);
        }
    }
}

/// NPCs leave space (not killed): out of every lock, the roster and the NPC
/// table, and one `RemoveBalls` to the system's characters. The tell of a
/// despawn.
pub fn despawn_ids(w: &mut Registry, solar_system_id: i64, ids: &[i64]) {
    let ids: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| w.roster().get(*id).is_some())
        .collect();
    if ids.is_empty() {
        return;
    }
    for id in &ids {
        crate::targeting::left_space(
            w,
            *id,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::OnTheClock,
        );
        w.roster_mut().leave(*id);
        w.npcs_mut().remove(*id);
    }
    let to = audience(w, solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(gone) = destiny::BallsRemoved::new(to, stamp, ids) {
        w.schedule(Duration::ZERO, gone);
    }
}

/// **NPCs moved to new places** in their system without being respawned (a
/// police squad rejoining a pilot), so their damage and locks stay: each ball
/// is placed and stopped, as a GM `/tp` does (`SetBallPosition`,
/// `SetBallVelocity`, `Stop`), the system's characters are told in one
/// `ActionUpdate`, and the grid is brought up to the new places.
pub fn move_ids(w: &mut Registry, solar_system_id: i64, moves: &[(i64, [f64; 3])]) {
    let mut actions = Vec::new();
    for (id, point) in moves {
        if w.roster().get(*id).is_none() {
            continue;
        }
        let ball = [
            destiny::Action::SetBallPosition {
                ball: *id,
                point: *point,
            },
            destiny::Action::SetBallVelocity {
                ball: *id,
                velocity: [0.0; 3],
            },
            destiny::Action::Stop { ball: *id },
        ];
        if let Some(p) = w.roster_mut().get_mut(*id) {
            for a in &ball {
                p.motion.after(*a);
            }
        }
        actions.extend(ball);
    }
    if actions.is_empty() {
        return;
    }
    let to = audience(w, solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(update) = destiny::ActionUpdate::new(to, stamp, actions) {
        w.schedule(Duration::ZERO, update);
    }
    crate::grid::reconcile_in(w, solar_system_id);
}

/// **A CONCORD entity type** (group 301), derived from the typedata alone:
/// the NPC rows do not list it. Owner CONCORD, no wreck. Cached with the
/// other derived kinds. `None` for a type the typedata lacks.
pub fn resolve_concord(w: &mut Registry, type_id: i64) -> Option<&'static NpcType> {
    resolve_law(w, type_id, crate::concord::CONCORD_CORP)
}

/// [`resolve_concord`] for any owner: a faction police type (CONCORD-1c) is
/// derived the same way, owned by its faction. The first owner to resolve a
/// type fixes its kind.
pub fn resolve_law(w: &mut Registry, type_id: i64, owner: i64) -> Option<&'static NpcType> {
    if let Some(k) = find(type_id) {
        return Some(k);
    }
    if let Some(k) = w.npcs().kinds.get(&type_id) {
        return Some(*k);
    }
    let made = {
        let td = crate::typedata::installed_in(w.content()).table();
        derive_as(td.get(type_id)?, td, owner, None, (&[], &[]))
    };
    let k: &'static NpcType = Box::leak(Box::new(made));
    w.npcs_mut().kinds.insert(type_id, k);
    Some(k)
}

/// A type derived for `owner` and **not** cached by type id: the same type
/// can then belong to different owners (the sentry guns of NW-8, one
/// type per empire but several factions that share it). The caller keeps
/// the result. `None` for a type the typedata lacks.
pub fn derive_owned(w: &Registry, type_id: i64, owner: i64) -> Option<&'static NpcType> {
    let td = crate::typedata::installed_in(w.content()).table();
    let made = derive_as(td.get(type_id)?, td, owner, None, (&[], &[]));
    Some(Box::leak(Box::new(made)))
}

/// `/entity deploy <qty> <typeID>`: NPCs in space beside the caller.
///
/// The client's own GM string, `'/entity deploy %%d %d' % typeID` through
/// `_wrapMulti(..., '/Entity', 100)`, offered for `categoryEntity` types in
/// space (`menusvc.py:1058-1065`) **[V]**: quantity first, type second.
///
/// Each one is [`SPAWN_DISTANCE`] from the caller in the XZ plane, spaced
/// evenly round it, and **one** `AddBalls2` tells every character in the
/// system about all of them -- `push`, because a slash command is a turn.
///
/// The type is [`resolve`]'s: a bundle's entity with `--content` and
/// `--typedata`, the invented [`FIXTURE_RAT`] with neither.
pub fn entity_slash(w: &mut Registry, actor: Option<i64>, args: &[&str]) -> PyValue {
    const USAGE: &str = "usage: /entity deploy <quantity> <typeID>";
    let bstr = crate::pyutil::bstr;
    let (qty, type_id) = match args {
        [verb, qty, type_id, ..] if verb.eq_ignore_ascii_case("deploy") => {
            match (qty.parse::<i64>(), type_id.parse::<i64>()) {
                (Ok(q), Ok(t)) => (q, t),
                _ => return bstr(USAGE),
            }
        }
        _ => return bstr(USAGE),
    };
    let Some(kind) = resolve(w, type_id) else {
        let known: Vec<String> = TYPES
            .iter()
            .map(|t| format!("{} ({})", t.type_id, t.name))
            .collect();
        return bstr(&format!(
            "no NPC data for type {type_id} (a bundle's types need --content and \
             --typedata); with no bundle: {}",
            known.join(", ")
        ));
    };
    let Some(actor) = actor else {
        return bstr("you must be in space");
    };
    w.advance_character(actor);
    let Some(here) = w.roster().get(actor).copied() else {
        return bstr("you must be in space");
    };
    let qty = qty.clamp(1, MAX_DEPLOY);
    let mut ids = Vec::new();
    for i in 0..qty {
        let angle = std::f64::consts::TAU * i as f64 / qty as f64;
        let at = here.motion.position;
        let position = [
            at[0] + SPAWN_DISTANCE * angle.cos(),
            at[1],
            at[2] + SPAWN_DISTANCE * angle.sin(),
        ];
        match spawn(w, kind, here.solar_system_id, position) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    announce(w, here.solar_system_id, &ids, false);
    let names: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    bstr(&format!(
        "deployed {} x {}: {}",
        ids.len(),
        kind.name,
        names.join(", ")
    ))
}

#[cfg(test)]
mod par_tests;

#[cfg(test)]
mod threat_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Profile, ShipDamage, Shot};
    use std::time::Instant;

    pub(super) const ALICE: i64 = 90_000_001;
    pub(super) const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;
    pub(super) const ALICE_SHIP: i64 = 100_000_001;
    pub(super) const BOB_SHIP: i64 = 100_000_002;
    const CAROL_SHIP: i64 = 100_000_003;
    const RIFTER: i64 = 587;
    const SYSTEM: i64 = 30_000_001;

    /// Alice, Bob and Carol in Rifters at the one station; `in_space` of
    /// them undocked into the system with their parks bound, every push
    /// that took drained.
    pub(super) fn world(in_space: &[i64]) -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: RIFTER,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
                pilot(CAROL, "Carol", CAROL_SHIP),
            ],
            ..crate::model::SandboxConfig::default()
        });
        for who in in_space {
            destiny::on_ballpark_bind(&mut reg, Some(*who));
        }
        let _ = reg.take_pending();
        reg
    }

    pub(super) fn ship_of(reg: &Registry, who: i64) -> i64 {
        reg.roster().get(who).expect("in space").ship_id
    }

    fn position_of(reg: &Registry, roster_key: i64) -> [f64; 3] {
        reg.roster()
            .get(roster_key)
            .expect("in space")
            .motion
            .position
    }

    /// One fixture rat `offset` metres from `who` along x, at rest.
    pub(super) fn hijacker_near(reg: &mut Registry, who: i64, offset: f64) -> i64 {
        let at = position_of(reg, who);
        let id = spawn(reg, &FIXTURE_RAT, SYSTEM, [at[0] + offset, at[1], at[2]]).expect("an id");
        // `spawn` tells nobody ([`entity_slash`] does): the grid's reconcile
        // is what puts the rat in the parks around it, as the clock would.
        crate::grid::reconcile_in(reg, SYSTEM);
        let _ = reg.take_due(Instant::now());
        id
    }

    /// [`hijacker_near`] of a given kind.
    pub(super) fn hijacker_near_kind(
        reg: &mut Registry,
        who: i64,
        offset: f64,
        kind: &'static NpcType,
    ) -> i64 {
        let at = position_of(reg, who);
        let id = spawn(reg, kind, SYSTEM, [at[0] + offset, at[1], at[2]]).expect("an id");
        crate::grid::reconcile_in(reg, SYSTEM);
        let _ = reg.take_due(Instant::now());
        id
    }

    /// Everything on the clock queue, however far ahead, as `(audience,
    /// name, args)`.
    pub(super) fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// The `(funcName, args)` of every entry in a `DoDestinyUpdate`.
    fn entries(args: &[PyValue]) -> Vec<(String, Vec<PyValue>)> {
        let PyValue::List(entries) = &args[0] else {
            panic!("a batch is a list")
        };
        entries
            .iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else {
                    panic!("(stamp, call)")
                };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!("(name, args)")
                };
                let PyValue::Bytes(name) = &call[0] else {
                    panic!("a str name")
                };
                let PyValue::Tuple(a) = &call[1] else {
                    panic!("args")
                };
                (String::from_utf8_lossy(name).into_owned(), a.clone())
            })
            .collect()
    }

    /// A dict's value by `str` key (`PyValue::Dict` is `(value, key)`).
    fn key<'a>(dict: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        let PyValue::Dict(pairs) = dict else {
            return None;
        };
        pairs
            .iter()
            .find(|(_, k)| *k == crate::pyutil::bstr(name))
            .map(|(v, _)| v)
    }

    pub(super) fn idle() -> Npc {
        Npc::new(MIN_NPC_ID, &FIXTURE_RAT, SYSTEM)
    }

    // ---- §8.1 test 1-3: the data ------------------------------------------

    /// **The no-bundle rat is the invented fixture, whole** (CP61): the
    /// loaded fixture rows do not list it (so it is the no-row case), the
    /// type and the hull lookup find it, its volley is its damage times its multiplier, it can lock
    /// what it can shoot, and its hull is never frozen.
    #[test]
    fn the_fixture_rat_is_the_invented_fixture_and_deployable() {
        let data = crate::npc_data::Table::fixture();
        assert!(!data.types.contains_key(&FIXTURE_RAT.type_id));
        assert_eq!(TYPES, &[FIXTURE_RAT]);
        let k = find(FIXTURE_RAT.type_id).expect("deployable");
        assert_eq!(k.hull.type_id, k.type_id);
        let m = k.damage_multiplier;
        assert_eq!(
            k.volley(),
            Damage {
                em: k.damage.em * m,
                explosive: k.damage.explosive * m,
                kinetic: k.damage.kinetic * m,
                thermal: k.damage.thermal * m,
            }
        );
        assert!(k.hull.max_target_range >= k.attack_range);
        assert!(k.hull.agility > 0.0, "a zero tau freezes the ball");
        assert!(k.orbit_range < k.attack_range, "it orbits inside its reach");
        assert_eq!(FIXTURE_FACTION.wreck_of(k.group_id), k.wreck_type_id);
        assert_eq!(
            destiny::hull::find_in(&crate::content::Content::fixture(), k.type_id),
            hull(k.type_id),
            "destiny's hull lookup falls through to the NPC table"
        );
        assert!(hull(k.type_id).is_some());
    }

    /// **`/entity deploy` of a type the hand table does not carry asks the
    /// bundle** ([`resolve`]): with none loaded it refuses, naming the flags
    /// and the fixture, and spawns nothing.
    #[test]
    fn deploying_an_unknown_type_with_no_bundle_names_the_flags() {
        let mut reg = world(&[ALICE]);
        let unknown = (FIXTURE_RAT.type_id + 1).to_string();
        let answer = entity_slash(&mut reg, Some(ALICE), &["deploy", "1", &unknown]);
        let PyValue::Bytes(text) = answer else {
            panic!("a message")
        };
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("--content"), "{text}");
        assert!(text.contains(&FIXTURE_RAT.type_id.to_string()), "{text}");
        assert!(reg.npcs().is_empty());
        assert_eq!(resolve(&mut reg, FIXTURE_RAT.type_id), Some(&FIXTURE_RAT));
    }

    /// `inventorycommon/const.py` [V]: the NPC range is clear of every id
    /// class the client tests for, and of the fake ranges.
    #[test]
    fn npc_ids_sit_outside_every_client_id_class() {
        let ours = MIN_NPC_ID..END_NPC_ID;
        let overlaps = |lo: i64, hi_inclusive: i64| ours.start <= hi_inclusive && lo < ours.end;
        assert!(!overlaps(90_000_000, 2_147_483_647), "player owners");
        assert!(!overlaps(70_000_000, 79_999_999), "asteroids");
        assert!(!overlaps(40_000_000, 49_999_999), "celestials");
        assert!(ours.end <= 9_000_000_000_000_000_000, "minFakeItem");
        assert!(ours.start > 2_099_999_999, "maxPreInv64Items");
        let mut n = Npcs::default();
        assert_eq!(n.next_id(), Some(MIN_NPC_ID));
        assert_eq!(n.next_id(), Some(MIN_NPC_ID + 1));
    }

    /// `IsNPC = 9999 < ownerID < 90000000` (`idCheckers.py:283`) and
    /// `IsCharacter` 90,000,000..=2,147,483,647 (`:318-319`) [V]; thresholds
    /// the type's own 11.0 [V-data], hostile because `11.0 > 0.0`
    /// (`is_npc_hostile`, `npcs/client/entitystandings.py:20-21`) [V].
    #[test]
    fn an_npc_slim_reads_as_a_hostile_npc() {
        let s = slim(&idle());
        assert!(9_999 < s.owner_id && s.owner_id < 90_000_000, "IsNPC");
        assert!(!(90_000_000..=2_147_483_647).contains(&s.owner_id));
        assert_eq!(s.char_id, None, "named by type, no pilot to prime");
        let (hostile, friendly) = s.response_thresholds.expect("both sent");
        assert_eq!(
            (hostile, friendly),
            (RESPONSE_THRESHOLD, RESPONSE_THRESHOLD)
        );
        assert!(f64::from(hostile) > 0.0, "is_npc_hostile");
        // And on the wire, as floats.
        let n = idle();
        let added = destiny::BallsAdded::new(
            vec![ALICE],
            1,
            destiny::Park {
                current_time: 1,
                balls: vec![ball(
                    &crate::content::Content::fixture(),
                    &n,
                    &Motion::arrived([0.0; 3], [0.0; 3]),
                )],
            },
            vec![(s, None)],
        )
        .expect("a batch");
        let (_, args) = crate::notify::Push::to_push(&added);
        let e = entries(&args);
        let PyValue::Tuple(inner) = &e[0].1[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::List(extra) = &inner[1] else {
            panic!("extra is a list")
        };
        assert_eq!(
            key(&extra[0], "hostile_response_threshold"),
            Some(&PyValue::Real(f64::from(RESPONSE_THRESHOLD)))
        );
        assert_eq!(
            key(&extra[0], "friendly_response_threshold"),
            Some(&PyValue::Real(f64::from(RESPONSE_THRESHOLD)))
        );
        assert_eq!(key(&extra[0], "ownerID"), Some(&PyValue::Int(FIXTURE_CORP)));
        assert_eq!(key(&extra[0], "charID"), Some(&PyValue::None));
    }

    /// §8.3: the seam answers a character as before and an NPC as its type
    /// and corporation, with no `charID`.
    #[test]
    fn the_pilot_of_a_roster_entry_is_a_character_or_an_npc() {
        let mut reg = world(&[ALICE]);
        let alice = reg.pilot(ALICE).expect("Alice");
        assert_eq!(
            (alice.ship_type_id, alice.owner_id, alice.char_id),
            (RIFTER, ALICE, Some(ALICE))
        );
        let npc = hijacker_near(&mut reg, ALICE, 10_000.0);
        let p = reg.pilot(npc).expect("the NPC");
        assert_eq!(
            (p.ship_type_id, p.owner_id, p.corporation_id, p.char_id),
            (FIXTURE_RAT.type_id, FIXTURE_CORP, FIXTURE_CORP, None)
        );
        assert_eq!(reg.pilot(12345), None);
    }

    // ---- §8.1 test 4: the gun's visual -------------------------------------

    /// `module_id == ship_id == the NPC's ball` [V] (`turrets.py:96`), the
    /// type's own guid, offensive.
    #[test]
    fn the_turret_effect_names_the_npc_ball_as_its_module() {
        let mut reg = world(&[ALICE]);
        let npc = hijacker_near(&mut reg, ALICE, 3_000.0);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        tick(&mut reg, t0 + 60_000 * BLUE_PER_MS);
        let fx: Vec<_> = due(&mut reg)
            .into_iter()
            .filter(|(_, n, _)| *n == "OnSpecialFX")
            .collect();
        assert!(!fx.is_empty(), "the gun started");
        for (_, _, a) in fx {
            assert_eq!(a[0], PyValue::Int(npc), "shipID");
            assert_eq!(a[1], PyValue::Int(npc), "moduleID is the ball");
            assert_eq!(a[3], PyValue::Int(ship_of(&reg, ALICE)), "targetID");
            assert_eq!(a[5], PyValue::Bytes(TURRET_FX_GUID.as_bytes().to_vec()));
            assert_eq!(a[6], PyValue::Bool(true), "isOffensive");
        }
    }

    // ---- §8.1 tests 6-14: the world ----------------------------------------

    /// Someone who binds after the spawn is handed the NPC in `SetState`:
    /// ball, slim and bars (`destiny::bootstrap_for`'s NPC branch).
    #[test]
    fn a_late_joiner_sees_the_npc() {
        let mut reg = world(&[ALICE]);
        let answer = entity_slash(
            &mut reg,
            Some(ALICE),
            &["deploy", "1", &FIXTURE_RAT.type_id.to_string()],
        );
        assert!(matches!(answer, PyValue::Bytes(_)));
        let npc = reg.npcs().iter().next().expect("one NPC").id;
        destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let state = destiny::bootstrap_for(&mut reg, BOB).expect("Bob's state");
        assert!(state.park.balls.iter().any(|b| b.ball_id == npc));
        let s = state
            .slims
            .iter()
            .find(|s| s.item_id == npc)
            .expect("its slim");
        assert_eq!(
            (s.owner_id, s.type_id, s.char_id),
            (FIXTURE_CORP, FIXTURE_RAT.type_id, None)
        );
        assert!(state.damage.iter().any(|(id, _)| *id == npc), "its bars");
        assert!(state.every_slim_has_a_ball());
    }

    #[test]
    fn deploy_is_one_batch_to_everyone_and_refuses_cleanly() {
        let mut reg = world(&[ALICE, BOB]);
        let before = reg.roster().len();
        entity_slash(
            &mut reg,
            Some(ALICE),
            &["deploy", "2", &FIXTURE_RAT.type_id.to_string()],
        );
        assert_eq!(reg.roster().len(), before + 2);
        let ids: Vec<i64> = reg.npcs().iter().map(|n| n.id).collect();
        let pushes: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect();
        assert_eq!(pushes.len(), 1, "one batch");
        let (mut to, name, args) = pushes[0].clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "everyone in the system, NPCs not");
        assert_eq!(name, "DoDestinyUpdate");
        let e = entries(&args);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "AddBalls2");
        let PyValue::Tuple(inner) = &e[0].1[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::Bytes(blob) = &inner[0] else {
            panic!("blob")
        };
        let park = destiny::Park::decode(blob).expect("our own blob");
        let mut balls: Vec<i64> = park.balls.iter().map(|b| b.ball_id).collect();
        balls.sort_unstable();
        assert_eq!(balls, ids, "both, in one blob");

        // Docked: refused, nothing made.
        let n = reg.roster().len();
        let answer = entity_slash(
            &mut reg,
            Some(CAROL),
            &["deploy", "1", &FIXTURE_RAT.type_id.to_string()],
        );
        assert_eq!(answer, crate::pyutil::bstr("you must be in space"));
        assert_eq!(reg.roster().len(), n);
        // Not an NPC type: refused, nothing made.
        let PyValue::Bytes(answer) = entity_slash(&mut reg, Some(ALICE), &["deploy", "1", "587"])
        else {
            panic!("a str")
        };
        assert!(String::from_utf8_lossy(&answer).contains("no NPC data for type 587"));
        assert_eq!(reg.roster().len(), n);
        assert!(reg.take_pending().is_empty());
    }

    /// The premise of the end goal: with nobody calling anything, one tick
    /// moves the NPC and starts its lock -- on the clock queue, not on
    /// `pending`, which only a payload turn drains.
    #[test]
    fn the_npc_acts_on_the_clock_alone() {
        let mut reg = world(&[ALICE]);
        let npc = hijacker_near(&mut reg, ALICE, AGGRESSION_RANGE / 2.0);
        tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.take_pending().is_empty(), "nothing waits for a turn");
        let out = due(&mut reg);
        assert!(
            out.iter().any(|(to, n, a)| *n == "DoDestinyUpdate"
                && to == &vec![ALICE]
                && entries(a)
                    .iter()
                    .any(|(f, args)| f == "FollowBall" && args[0] == PyValue::Int(npc))),
            "the approach"
        );
        assert!(
            out.iter().any(|(to, n, a)| *n == "OnTarget"
                && to == &vec![ALICE]
                && a[0] == PyValue::Bytes(b"otheradd".to_vec())
                && a[1] == PyValue::Int(npc)),
            "Alice is told she is targeted"
        );
        assert_eq!(reg.locks().targets_of(npc), vec![ship_of(&reg, ALICE)]);
    }

    /// WARP-LOCK (#191): a rat does not start a lock, or aggro, on a pilot
    /// in an activated warp; it does once the warp has ended.
    #[test]
    fn an_npc_does_not_lock_a_pilot_in_warp_until_it_ends() {
        let mut reg = world(&[ALICE]);
        let npc = hijacker_near(&mut reg, ALICE, AGGRESSION_RANGE / 2.0);
        let mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 - 1,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = mode;
        tick(&mut reg, crate::pyutil::filetime_now());
        let _ = due(&mut reg);
        assert!(reg.locks().targets_of(npc).is_empty(), "no lock in warp");
        assert_eq!(reg.npcs().get(npc).and_then(|n| n.target), None, "no aggro");
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = crate::destiny::Mode::Stop;
        tick(&mut reg, crate::pyutil::filetime_now());
        let _ = due(&mut reg);
        assert_eq!(reg.locks().targets_of(npc), vec![ship_of(&reg, ALICE)]);
    }

    /// `docs/re/npc-behaviors.md` T4: one batch per tick, not one per NPC.
    #[test]
    fn one_tick_one_action_batch() {
        let mut reg = world(&[ALICE]);
        let a = hijacker_near(&mut reg, ALICE, AGGRESSION_RANGE / 2.0);
        let b = hijacker_near(&mut reg, ALICE, -AGGRESSION_RANGE / 2.0);
        tick(&mut reg, crate::pyutil::filetime_now());
        let batches: Vec<_> = due(&mut reg)
            .into_iter()
            .filter(|(_, n, a)| {
                *n == "DoDestinyUpdate" && entries(a).iter().any(|(f, _)| f == "FollowBall")
            })
            .collect();
        assert_eq!(batches.len(), 1);
        let balls: Vec<PyValue> = entries(&batches[0].2)
            .into_iter()
            .filter(|(f, _)| f == "FollowBall")
            .map(|(_, args)| args[0].clone())
            .collect();
        assert_eq!(balls, vec![PyValue::Int(a), PyValue::Int(b)]);
    }

    /// An NPC's volley goes through P2.2b's own path: the pilot's ship
    /// record is what an independent `ShipDamage` says one volley does, and
    /// the bars are on the clock.
    #[test]
    fn an_npc_volley_lands_like_a_player_volley() {
        let mut reg = world(&[ALICE]);
        let alice_ship = ship_of(&reg, ALICE);
        let npc = hijacker_near(&mut reg, ALICE, 3_000.0);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        let ready = reg
            .npcs()
            .get(npc)
            .and_then(|n| n.lock_ready_at)
            .expect("locking");
        let _ = due(&mut reg);
        assert!(
            reg.combat().ship(alice_ship).is_none(),
            "not before the lock"
        );
        tick(&mut reg, ready);
        let got = reg.combat().ship(alice_ship).expect("Alice was hit");
        let rifter = *destiny::hull::find_in(reg.content(), RIFTER).expect("a Rifter");
        let rifter = &rifter;
        let mut want = ShipDamage::full(Profile::from_hull_in(reg.content(), rifter), ready);
        want.apply(FIXTURE_RAT.volley(), ready);
        assert_eq!(got.shield_at(ready), want.shield_at(ready));
        assert_eq!(got.armour_left(), want.armour_left());
        assert_eq!(got.hull_left(), want.hull_left());
        assert!(got.shield_at(ready) < rifter.shield_capacity);
        let out = due(&mut reg);
        assert!(out.iter().any(|(to, n, a)| *n == "DoDestinyUpdate"
            && to.contains(&ALICE)
            && entries(a).iter().any(
                |(f, args)| f == "OnDamageStateChange" && args[0] == PyValue::Int(alice_ship)
            )));
        assert!(
            out.iter()
                .any(|(to, n, _)| *n == "OnDamageMessage" && to == &vec![ALICE]),
            "Alice's combat log"
        );
    }

    /// A player locks an NPC with the client's own lock time (the type's
    /// signature, `bracketMgr.py:1305` [V]) and kills it: one kill batch
    /// with the entity's wreck -- a store item -- the lock released with
    /// `'Exploding'`, the NPC gone from the roster and the table.
    #[test]
    fn a_player_can_lock_and_kill_an_npc() {
        let mut reg = world(&[ALICE]);
        let alice_ship = ship_of(&reg, ALICE);
        let npc = hijacker_near(&mut reg, ALICE, 3_000.0);
        crate::targeting::lock(&mut reg, ALICE, npc).expect("not refused");
        let scan = reg.scan_resolution_of(RIFTER);
        let sig = FIXTURE_RAT.hull.signature_radius;
        let want = (40.0e6 / scan / sig.asinh().powi(2)).min(180_000.0);
        assert_eq!(
            crate::targeting::acquisition_delay(&reg, ALICE, npc),
            Duration::from_millis(want as u64)
        );
        let _ = reg.take_pending();
        let _ = due(&mut reg);
        let t0 = crate::pyutil::filetime_now();
        for i in 0..200 {
            let Some(victim) = reg.roster().get(npc).copied() else {
                break;
            };
            let shot = Shot {
                at: t0,
                attacker_ship_id: alice_ship,
                weapon_item_id: 1,
                weapon_type_id: 3640,
                target_ship_id: npc,
                damage: crate::combat::volley_in(reg.content(), 3640).expect("a civilian gun"),
                hit_quality: crate::combat::HIT_QUALITY,
            };
            crate::combat::fire(&mut reg, shot, ALICE, victim, t0);
            assert!(i < 199, "160 HP should not take 200 volleys");
        }
        let out = due(&mut reg);
        let kill = out
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate" && entries(a).iter().any(|(f, _)| f == "RemoveBall")
            })
            .expect("a kill batch");
        assert_eq!(kill.0, vec![ALICE], "the park, which is characters");
        let e = entries(&kill.2);
        assert_eq!(
            e.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(),
            vec!["AddBalls2", "RemoveBall"],
            "the wreck, the removal, and no capsule"
        );
        assert_eq!(e[1].1, vec![PyValue::Int(npc), PyValue::Bool(true)]);
        let PyValue::Tuple(inner) = &e[0].1[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::List(extra) = &inner[1] else {
            panic!("extra")
        };
        assert_eq!(
            key(&extra[0], "typeID"),
            Some(&PyValue::Int(HAND_SMALL_WRECK))
        );
        assert_eq!(key(&extra[0], "launcherID"), Some(&PyValue::Int(npc)));
        let lost = out
            .iter()
            .find(|(to, n, a)| {
                *n == "OnTarget" && to == &vec![ALICE] && a[0] == PyValue::Bytes(b"lost".to_vec())
            })
            .expect("Alice's target box is told");
        assert_eq!(lost.2[1], PyValue::Int(npc));
        assert_eq!(lost.2[2], PyValue::Bytes(b"Exploding".to_vec()));
        assert!(
            out.iter().all(|(to, _, _)| !to.contains(&npc)),
            "nothing addressed to the dead NPC"
        );
        tick(&mut reg, t0);
        assert!(reg.npcs().is_empty());
        assert!(reg.roster().get(npc).is_none());
        let wreck = reg.combat().wrecks().next().copied().expect("a wreck");
        assert_eq!((wreck.type_id, wreck.ship_id), (HAND_SMALL_WRECK, npc));
        let item = reg.store().get(wreck.wreck_id).expect("a store item");
        assert_eq!(
            (item.type_id, item.owner_id),
            (HAND_SMALL_WRECK, FIXTURE_CORP)
        );
        assert_eq!(
            key(&extra[0], "isEmpty"),
            Some(&PyValue::Bool(
                reg.store().contents_of(wreck.wreck_id).next().is_none()
            )),
            "the slim agrees with the store"
        );
        // And a late joiner sees the wreck, not the NPC.
        destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let state = destiny::bootstrap_for(&mut reg, BOB).expect("Bob's state");
        let ids: Vec<i64> = state.park.balls.iter().map(|b| b.ball_id).collect();
        assert!(ids.contains(&wreck.wreck_id) && !ids.contains(&npc));
        assert!(state.every_slim_has_a_ball());
    }

    #[test]
    fn a_target_that_leaves_space_ends_the_engagement() {
        let mut reg = world(&[ALICE, BOB]);
        // Bob far out of range: the audience, not a candidate. Still on the
        // grid, or he would be no audience at all (`crate::grid`).
        if let Some(p) = reg.roster_mut().get_mut(BOB) {
            p.motion.position[0] += crate::grid::GRID_ENTER / 2.0;
        }
        let npc = hijacker_near(&mut reg, ALICE, 3_000.0);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        let ready = reg
            .npcs()
            .get(npc)
            .and_then(|n| n.lock_ready_at)
            .expect("locking");
        tick(&mut reg, ready);
        assert!(reg.npcs().get(npc).is_some_and(|n| n.firing));
        let _ = due(&mut reg);
        crate::targeting::left_space(
            &mut reg,
            ALICE,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::OnTheClock,
        );
        reg.roster_mut().leave(ALICE);
        let _ = due(&mut reg);
        tick(&mut reg, ready + BLUE_PER_MS);
        let n = reg.npcs().get(npc).copied().expect("alive");
        assert_eq!((n.target, n.firing, n.phase), (None, false, Phase::Idle));
        assert!(reg.locks().targets_of(npc).is_empty());
        let out = due(&mut reg);
        assert!(
            out.iter().any(|(to, n, a)| *n == "OnSpecialFX"
                && to == &vec![BOB]
                && a[7] == PyValue::Int(0))
        );
        assert!(out.iter().any(|(_, n, a)| {
            *n == "DoDestinyUpdate"
                && entries(a)
                    .iter()
                    .any(|(f, args)| f == "Stop" && args[0] == PyValue::Int(npc))
        }));
    }

    /// The server integrates an approaching NPC by the client's own drag
    /// law -- it closes, and it does not arrive in an instant.
    #[test]
    fn the_server_moves_the_npc_too() {
        let mut reg = world(&[ALICE]);
        let npc = hijacker_near(&mut reg, ALICE, AGGRESSION_RANGE / 2.0);
        tick(&mut reg, crate::pyutil::filetime_now());
        assert_eq!(reg.npcs().get(npc).map(|n| n.phase), Some(Phase::Approach));
        let gap = |reg: &Registry| {
            crate::space_state::distance(position_of(reg, npc), position_of(reg, ALICE))
        };
        let before = gap(&reg);
        if let Some(p) = reg.roster_mut().get_mut(npc) {
            p.since -= 2.0;
        }
        reg.advance_system(SYSTEM);
        let after = gap(&reg);
        assert!(after < before, "{after} < {before}");
        let top = 2.0 * f64::from(FIXTURE_RAT.hull.max_velocity);
        assert!(
            before - after <= top + 1_000.0,
            "no faster than top speed (plus Alice's own drift): {before} -> {after}"
        );
    }

    /// Every push an NPC causes is addressed to characters only, through a
    /// whole fight and a death.
    #[test]
    fn no_npc_push_is_addressed_to_an_npc() {
        let mut reg = world(&[ALICE, BOB]);
        entity_slash(
            &mut reg,
            Some(ALICE),
            &["deploy", "3", &FIXTURE_RAT.type_id.to_string()],
        );
        let npcs: Vec<i64> = reg.npcs().iter().map(|n| n.id).collect();
        let mut seen: Vec<Vec<i64>> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.audience().unwrap_or_default())
            .collect();
        let t0 = crate::pyutil::filetime_now();
        for step in 0..40 {
            tick(&mut reg, t0 + step * 1_000 * BLUE_PER_MS);
            seen.extend(due(&mut reg).into_iter().map(|(to, _, _)| to));
        }
        // And one dies to Alice.
        let victim = reg.roster().get(npcs[0]).copied().expect("alive");
        for _ in 0..100 {
            if reg.roster().get(npcs[0]).is_none() {
                break;
            }
            let shot = Shot {
                at: t0,
                attacker_ship_id: ship_of(&reg, ALICE),
                weapon_item_id: 1,
                weapon_type_id: 3640,
                target_ship_id: npcs[0],
                damage: crate::combat::volley_in(reg.content(), 3640).expect("a gun"),
                hit_quality: crate::combat::HIT_QUALITY,
            };
            crate::combat::fire(&mut reg, shot, ALICE, victim, t0);
        }
        seen.extend(due(&mut reg).into_iter().map(|(to, _, _)| to));
        assert!(seen.len() > 10, "something happened");
        for to in seen {
            assert!(!to.is_empty());
            for id in to {
                assert!(reg.character(id).is_some(), "{id} is not a character");
            }
        }
    }

    // ---- Belt rats: types from the client's tables, and the bounty -------

    /// **`resolve` derives a bundle type over the content's typedata**
    /// (CP58b): an `npc/types.tsv` row ([`crate::npc_data::Table::fixture`])
    /// whose entity is in `reg.content()`'s typedata comes back derived, with
    /// the typedata row's own numbers, and is kept.
    #[test]
    fn resolve_derives_a_bundle_type_from_the_content_typedata() {
        let listed = *crate::npc_data::Table::fixture()
            .types
            .keys()
            .next()
            .expect("the npc fixture lists a type");
        let text = ENTITIES.replace("990200001", &listed.to_string());
        let table = crate::typedata::Table::parse(&text).expect("the fixture parses");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut reg = world(&[]).with_content(std::sync::Arc::new(content));
        let want = reg
            .content()
            .get::<crate::typedata::Installed>()
            .table()
            .get(listed)
            .expect("the row")
            .mass;
        let k = resolve(&mut reg, listed).expect("derived from the content typedata");
        assert_eq!(k.type_id, listed);
        assert_eq!(k.hull.mass, want);
        assert!(std::ptr::eq(k, resolve(&mut reg, listed).expect("kept")));
        assert!(
            resolve(&mut world(&[]), listed).is_none(),
            "not in the fixture typedata"
        );
    }

    /// A fixture in `tools/typedata.py`'s format. **Not the client's
    /// numbers**: every value is invented, so that what the test checks is
    /// the mapping -- each field reads its own attribute -- and not a copy
    /// of CCP's data. Type 990_200_001 is in the fixture faction's frigate
    /// group (`FIXTURE_GROUP_FRIGATE`), 002 in its cruiser group with no agility and a negative equipment minimum,
    /// 003 in a group no faction flies.
    const ENTITIES: &str = "\
attribute\t9\t0.0\thp
attribute\t37\t0.0\tmaxVelocity
attribute\t51\t0.0\tspeed
attribute\t55\t0.0\trechargeRate
attribute\t64\t1.0\tdamageMultiplier
attribute\t70\t0.0\tagility
attribute\t76\t0.0\tmaxTargetRange
attribute\t114\t0.0\temDamage
attribute\t116\t0.0\texplosiveDamage
attribute\t117\t0.0\tkineticDamage
attribute\t118\t0.0\tthermalDamage
attribute\t192\t0.0\tmaxLockedTargets
attribute\t247\t0.0\tentityAttackRange
attribute\t263\t0.0\tshieldCapacity
attribute\t265\t0.0\tarmorHP
attribute\t416\t0.0\tentityFlyRange
attribute\t456\t0.0\tentityEquipmentMin
attribute\t457\t0.0\tentityEquipmentMax
attribute\t479\t0.0\tshieldRechargeRate
attribute\t481\t0.0\tentityKillBounty
attribute\t482\t0.0\tcapacitorCapacity
attribute\t508\t0.0\tentityCruiseSpeed
attribute\t552\t0.0\tsignatureRadius
attribute\t564\t0.0\tscanResolution
attribute\t1855\t0.0\tAI_IgnoreDronesBelowSignatureRadius
effect\t10\t2\t1\t51\t0\t54\t158\t160\ttargetAttack\teffects.Fixture
type\t990200001\t99111\t11\t0\t1\t10.0\t0.0\t1234.5\t41.0\t10\t10\t9=11.0;37=1000.0;51=3000.0;55=12.0;64=1.5;70=0.25;114=1.0;116=2.0;117=3.0;118=4.0;192=3.0;247=6000.0;263=13.0;265=14.0;416=2100.0;456=1.0;457=3.0;479=15.0;481=7777.0;482=16.0;508=250.0;552=33.0;564=17.0;1855=18.0\tFixture Frigate
type\t990200002\t99112\t11\t0\t1\t10.0\t0.0\t9999.0\t140.0\t10\t10\t37=500.0;247=45000.0;456=-2.0;457=4.0;481=50000.0\tFixture Cruiser
type\t990200003\t1\t11\t0\t1\t10.0\t0.0\t1.0\t1.0\t10\t10\t481=1.0\tFixture Stranger
";

    /// **Every field of a derived type is its own attribute** -- the
    /// fixture's invented numbers come back where they belong -- and the
    /// wreck is the faction's for the group.
    #[test]
    fn a_derived_type_reads_each_number_from_its_own_attribute() {
        let table = crate::typedata::Table::parse(ENTITIES).expect("the fixture parses");
        let row = table.get(990_200_001).expect("a row");
        let t = derive(row, &table, &FIXTURE_FACTION).expect("a fixture frigate");
        assert_eq!(t.name, "Fixture Frigate");
        assert_eq!(t.group_id, FIXTURE_GROUP_FRIGATE);
        assert_eq!(t.owner_corp_id, FIXTURE_CORP);
        let h = t.hull;
        assert_eq!((h.type_id, h.mass, h.radius), (990_200_001, 1234.5, 41.0));
        assert_eq!((h.max_velocity, h.agility), (1000.0, 0.25));
        assert_eq!((h.hp, h.shield_capacity, h.armor_hp), (11.0, 13.0, 14.0));
        assert_eq!(h.shield_recharge_rate, 15.0);
        assert_eq!(
            (h.capacitor_capacity, h.capacitor_recharge_rate),
            (16.0, 12.0)
        );
        assert_eq!((h.scan_resolution, h.signature_radius), (17.0, 33.0));
        assert_eq!(h.max_locked_targets, 3.0);
        assert_eq!(t.weapon_period_ms, 3000);
        assert_eq!(
            t.volley(),
            Damage {
                em: 1.5,
                explosive: 3.0,
                kinetic: 4.5,
                thermal: 6.0,
            }
        );
        assert_eq!(
            t.weapon_guid, "effects.Fixture",
            "the default effect's guid"
        );
        assert_eq!((t.attack_range, t.orbit_range), (6000.0, 2100.0));
        assert_eq!(t.cruise_fraction, 250.0 / 1000.0);
        assert_eq!(t.bounty, 7777.0);
        assert_eq!(t.ignore_drones_below_signature, 18.0);
        assert_eq!((t.equipment_min, t.equipment_max), (1, 3));
        assert_eq!(t.wreck_type_id, Some(HAND_SMALL_WRECK));
        // A rat can always lock what it can shoot.
        assert!(h.max_target_range >= t.attack_range);
    }

    /// The cruiser: the medium wreck, a lock range that covers its long gun,
    /// a negative equipment minimum read as none, and -- carrying no agility
    /// -- the named fallback, never a frozen zero.
    #[test]
    fn a_derived_cruiser_keeps_its_own_wreck_and_reach() {
        let table = crate::typedata::Table::parse(ENTITIES).expect("the fixture parses");
        let t = derive(
            table.get(990_200_002).expect("a row"),
            &table,
            &FIXTURE_FACTION,
        )
        .expect("a fixture cruiser");
        assert_eq!(t.wreck_type_id, Some(HAND_MEDIUM_WRECK));
        assert_eq!(t.bounty, 50_000.0);
        assert!(t.hull.max_target_range >= 45_000.0);
        assert_eq!(t.equipment_min, 0);
        assert_eq!(t.hull.agility, FALLBACK_AGILITY);
        assert!(
            derive(
                table.get(990_200_003).expect("a row"),
                &table,
                &FIXTURE_FACTION
            )
            .is_none()
        );
        let all = derive_all(&table);
        assert_eq!(
            all.keys().copied().collect::<Vec<_>>(),
            vec![990_200_001, 990_200_002],
            "only the faction's groups"
        );
    }

    /// **A type from the loaded rows** (CP20): [`derive`]'s numbers, with the
    /// faction row's owner and its wreck for the row's class; a type the
    /// rows do not list is `None`, and a class with no wreck has none.
    #[test]
    fn a_loaded_type_takes_its_faction_rows_owner_and_class_wreck() {
        let table = crate::typedata::Table::parse(ENTITIES).expect("the fixture parses");
        let mut data = crate::npc_data::Table::fixture();
        let mut listed = data.types[&990_300_001].clone();
        listed.type_id = 990_200_001;
        data.types.insert(listed.type_id, listed.clone());
        listed.type_id = 990_200_002;
        listed.class = "Unwrecked".into();
        data.types.insert(listed.type_id, listed);
        let row = table.get(990_200_001).expect("a row");
        let t = derive_loaded(row, &table, &data).expect("a listed type");
        let f = &data.factions[&data.types[&990_200_001].faction_id];
        assert_eq!(t.owner_corp_id, f.owner());
        assert_eq!(t.wreck_type_id, f.wrecks.get("Frigate").copied());
        let hand = derive(row, &table, &FIXTURE_FACTION).expect("a fixture frigate");
        assert_eq!(
            (t.hull, t.bounty, t.damage),
            (hand.hull, hand.bounty, hand.damage)
        );
        let unwrecked = derive_loaded(table.get(990_200_002).expect("a row"), &table, &data);
        assert_eq!(unwrecked.expect("listed").wreck_type_id, None);
        assert!(derive_loaded(table.get(990_200_003).expect("a row"), &table, &data).is_none());
    }

    /// **Belt rats keep working with no bundle**: the fixture's regions are
    /// invented, so no real system has a loaded pirate and the fixture
    /// faction stands.
    #[test]
    fn with_no_bundle_no_system_has_a_loaded_pirate() {
        let reg = world(&[]);
        let here = reg.roster().get(ALICE).map_or(0, |p| p.solar_system_id);
        assert_eq!(pirate_of(&reg, here), None);
        assert!(!faction_for(here).of_size(Size::Frigate).is_empty());
    }

    /// **The fixture rat is its faction's frigate** (CP61): the faction
    /// owns it, leaves its wreck, and spawns it with no `--typedata` at all;
    /// and the test-only Angel names are the fixture's.
    #[test]
    fn the_fixture_rat_is_its_factions_belt_frigate() {
        assert!(FIXTURE_FACTION.frigates.contains(&FIXTURE_RAT.type_id));
        assert_eq!(FIXTURE_FACTION.owner_corp_id, FIXTURE_RAT.owner_corp_id);
        assert_eq!(
            FIXTURE_FACTION
                .of_size(Size::Frigate)
                .iter()
                .map(|t| t.type_id)
                .collect::<Vec<_>>(),
            vec![FIXTURE_RAT.type_id]
        );
        assert_eq!(faction_for(0), &FIXTURE_FACTION);
        assert_eq!(FACTIONS, &[FIXTURE_FACTION]);
        assert_eq!(ANGEL_HIJACKER, FIXTURE_RAT);
        assert_eq!(ANGEL_CARTEL, FIXTURE_FACTION);
        assert_eq!(REF_BOUNTY_PRIZE, 17);
    }

    // ---- W10.5 S4's pin -------------------------------------------------

    /// **Every item the S4 split moved is still at its old path**
    /// (`docs/content/plan.md` §6.1, S4): a compile-time use list. Dropping
    /// a `pub use` from `npc.rs` breaks this test's build before it breaks a
    /// caller's (`combat.rs` calls `crate::npc::pay_bounty` and
    /// `crate::npc::loot`).
    #[test]
    fn reexports_cover_every_moved_item() {
        use crate::npc::{Candidate, Intent, REF_BOUNTY_PRIZE, View, decide, loot, pay_bounty};
        let _: fn(&Npc, &View) -> (Npc, Vec<Intent>) = decide;
        let _: fn(&mut Registry, &Npc, Intent, BlueTime, &mut Vec<crate::destiny::Action>) =
            crate::npc::apply;
        let _: fn(&NpcType, i64) -> Vec<(i64, i64)> = loot;
        let _: fn(&mut Registry, i64, i64) -> Option<(i64, f64)> = pay_bounty;
        let _: Option<(Candidate, View)> = None;
        let _: i64 = REF_BOUNTY_PRIZE;
    }
}

/// TICK-1: the idle skip is exactly the identity of the brain.
#[cfg(test)]
mod idle_skip {
    use super::brain::{Assist, Rep};
    use super::*;
    use std::time::Instant;

    const SYSTEM: i64 = 30_000_001;
    const NOW: BlueTime = 5_000_000_000;

    fn rep() -> Rep {
        Rep {
            armor: true,
            amount: 100.0,
            period: 10_000_000,
            delay_chance: 0.0,
        }
    }

    fn assist() -> Assist {
        Assist {
            armor: true,
            amount: 7.0,
            period: 10_000_000,
            chance: 1.0,
            threshold: 0.5,
            range: 10_000.0,
            max_targets: 1,
        }
    }

    fn view(b: Behaviour) -> View {
        View {
            me: [0.0; 3],
            candidates: Vec::new(),
            allies: Vec::new(),
            lock_held: false,
            lock_range: 0.0,
            now: NOW,
            behaviour: b,
        }
    }

    /// Every combination of what the predicate reads: `idle_with` says yes
    /// only when `decide` over a view with nobody in it returns the same NPC
    /// and no intent.
    #[test]
    fn idle_with_implies_decide_is_the_identity() {
        let mut idle_seen = 0;
        for target in [None, Some(7)] {
            for firing in [false, true] {
                for returning in [false, true] {
                    for locker in [false, true] {
                        for rep_state in [None, Some(NOW + 1), Some(NOW), Some(NOW - 1)] {
                            for (has_rep, has_assist) in
                                [(false, false), (true, false), (false, true)]
                            {
                                let mut npc = Npc::new(1, &FIXTURE_RAT, SYSTEM);
                                npc.target = target;
                                npc.firing = firing;
                                npc.mind.returning = returning;
                                npc.mind.next_rep_at = rep_state;
                                if locker {
                                    npc.mind.aggressors[0] = Some((9, NOW - 1));
                                }
                                let b = Behaviour {
                                    self_rep: has_rep.then(rep),
                                    assist: has_assist.then(assist),
                                    ..Behaviour::default()
                                };
                                let (next, out) = decide(&npc, &view(b));
                                if idle_with(&npc, &b, NOW) {
                                    idle_seen += 1;
                                    assert!(out.is_empty(), "{npc:?} {b:?}");
                                    assert_eq!(next, npc, "{b:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(idle_seen > 0, "the predicate is not vacuous");
    }

    /// A repairer whose first look has not happened is not idle (the look
    /// sets its due time), one not yet due is, one due is not.
    #[test]
    fn a_repairer_is_idle_only_between_cycles() {
        let b = Behaviour {
            self_rep: Some(rep()),
            ..Behaviour::default()
        };
        let mut npc = Npc::new(1, &FIXTURE_RAT, SYSTEM);
        assert!(!idle_with(&npc, &b, NOW), "no due time yet");
        npc.mind.next_rep_at = Some(NOW + 1);
        assert!(idle_with(&npc, &b, NOW));
        npc.mind.next_rep_at = Some(NOW);
        assert!(!idle_with(&npc, &b, NOW), "due");
    }

    /// **Catch-up**: with nobody here the clock leaves an idle NPC exactly
    /// as the full path would, and the first pilot's arrival makes the
    /// system tick in full on that very tick -- the rat sees him.
    #[test]
    fn a_system_with_nobody_is_skipped_and_a_pilot_ends_the_skip() {
        use super::tests::{ALICE, world};
        let mut reg = world(&[]);
        let rat = spawn(&mut reg, &FIXTURE_RAT, SYSTEM, [0.0; 3]).expect("an id");
        let before = *reg.npcs().get(rat).expect("spawned");
        // The clock's first stage, then the rest of it.
        for k in 0..20 {
            crate::liveness::refresh(&mut reg, Instant::now(), NOW + k * 2_500_000);
            assert!(nobody_here(&reg, SYSTEM));
            tick(&mut reg, NOW + k * 2_500_000);
        }
        assert_eq!(*reg.npcs().get(rat).expect("still there"), before);
        // A pilot undocks beside it.
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::liveness::refresh(&mut reg, Instant::now(), 2);
        assert!(!nobody_here(&reg, SYSTEM));
    }
}

/// NPC-CHASE: a loaded NPC has a hull in the lookup the ball and the
/// server's integration read, so it flies at its own speed and follows a
/// pilot who burns away.
#[cfg(test)]
mod chase {
    use super::tests::{self, ALICE, hijacker_near_kind};
    use super::*;

    const SYSTEM: i64 = 30_000_001;
    const FAST: i64 = 990_200_011;
    const SLOW: i64 = 990_200_012;
    /// Invented. The pilot's Rifter flies at the fixture's speed; FAST is
    /// well above it and SLOW well below.
    const TABLE: &str = "\
attribute\t37\t0.0\tmaxVelocity
attribute\t51\t0.0\tspeed
attribute\t64\t1.0\tdamageMultiplier
attribute\t70\t0.0\tagility
attribute\t117\t0.0\tkineticDamage
attribute\t247\t0.0\tentityAttackRange
attribute\t416\t0.0\tentityFlyRange
attribute\t508\t0.0\tentityCruiseSpeed
type\t990200011\t99111\t11\t0\t1\t10.0\t0.0\t1000000.0\t40.0\t10\t10\t37=2000.0;51=3000.0;117=3.0;247=6000.0;416=2100.0;508=250.0\tFast Fixture
type\t990200012\t99111\t11\t0\t1\t10.0\t0.0\t1000000.0\t40.0\t10\t10\t37=60.0;51=3000.0;117=3.0;247=6000.0;416=2100.0;508=20.0\tSlow Fixture
";

    fn world_with_table() -> (Registry, &'static NpcType, &'static NpcType) {
        let table = crate::typedata::Table::parse(TABLE).expect("parses");
        let kind = |id: i64| -> &'static NpcType {
            let row = table.get(id).expect("row");
            Box::leak(Box::new(
                derive(row, &table, &FIXTURE_FACTION).expect("derives"),
            ))
        };
        let (fast, slow) = (kind(FAST), kind(SLOW));
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let reg = tests::world(&[ALICE]).with_content(std::sync::Arc::new(content));
        (reg, fast, slow)
    }

    fn gap(reg: &Registry, npc: i64) -> f64 {
        let at = |k: i64| reg.roster().get(k).expect("in space").motion.position;
        crate::space_state::distance(at(npc), at(ALICE))
    }

    /// Carry the system `seconds` forward a second at a time, letting the
    /// rats decide each second; the largest gap seen.
    fn run(reg: &mut Registry, rat: i64, seconds: u32, now: &mut BlueTime) -> f64 {
        let mut worst: f64 = 0.0;
        for _ in 0..seconds {
            let ids: Vec<i64> = reg
                .roster()
                .in_system(SYSTEM)
                .iter()
                .map(|p| p.character_id)
                .collect();
            for id in ids {
                if let Some(p) = reg.roster_mut().get_mut(id) {
                    p.since -= 1.0;
                }
            }
            *now += BLUE_PER_MS * 1000;
            tick(reg, *now);
            worst = worst.max(gap(reg, rat));
        }
        worst
    }

    fn burn_away(reg: &mut Registry) {
        let from = reg.roster().get(ALICE).expect("in space").motion.position;
        let to = [from[0] + 5.0e7, from[1], from[2]];
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        let ball = p.ship_id;
        p.motion
            .after(crate::destiny::Action::GotoPoint { ball, point: to });
        p.motion.after(crate::destiny::Action::SetSpeedFraction {
            ball,
            fraction: 1.0,
        });
    }

    /// The defect: a derived entity had no row in `find_in`, so its ball and
    /// the server's integration both read the 1 m/s fallback.
    #[test]
    fn a_loaded_npc_flies_at_its_own_speed() {
        let (reg, fast, slow) = world_with_table();
        for k in [fast, slow] {
            let h = crate::destiny::hull::find_in(reg.content(), k.type_id).expect("a hull");
            assert_eq!(h.max_velocity, k.hull.max_velocity);
            assert!(h.agility > 0.0, "a zero tau freezes the ball");
            assert_eq!(h, &k.hull);
        }
    }

    /// A pilot lands near a rat and burns away at full speed: a faster rat
    /// stays within its reach of him; a slower one is left behind.
    #[test]
    fn a_faster_rat_follows_a_pilot_who_burns_away_and_a_slower_one_does_not() {
        for (kind_of, follows) in [(0, true), (1, false)] {
            let (mut reg, fast, slow) = world_with_table();
            let kind = if kind_of == 0 { fast } else { slow };
            let rat = hijacker_near_kind(&mut reg, ALICE, 3_000.0, kind);
            let mut now = crate::pyutil::filetime_now();
            tick(&mut reg, now);
            run(&mut reg, rat, 5, &mut now);
            burn_away(&mut reg);
            run(&mut reg, rat, 30, &mut now);
            let worst = run(&mut reg, rat, 30, &mut now);
            let margin = 2_000.0;
            if follows {
                assert!(
                    worst <= kind.attack_range + margin,
                    "followed: {worst} within {} + {margin}",
                    kind.attack_range
                );
            } else {
                assert!(
                    gap(&reg, rat) > 2.0 * kind.attack_range,
                    "left behind: {}",
                    gap(&reg, rat)
                );
            }
        }
    }
}
