//! The dungeon catalogue: every dungeon, its rooms, objects, gates, spawns
//! and resources, and the C3 §4.2 draw filters over them.
//!
//! W12 packet CP16 (`docs/content/plan.md` §6.2). Spec: C1 §2.5 (the
//! files), C2 §2.4 (`allowedTypes`), C3 §4.2 (the filters); headers from
//! `docs/content/schema.md` (X1), written by CP10 (`content_dungeons.py`)
//! and CP12 (`types/typelists.tsv`). Plan §3.2 makes this module the one
//! owner of the catalogue: CP21 draws from [`candidates`], CP28a/b place
//! [`Dungeon::objects`] and [`Dungeon::spawns`], CP29 reads the tracker
//! fields. Pure: no I/O past [`Table::from_bundle`], no clock, no hash order.
//!
//! What this module does **not** own: the pick among candidates (CP21,
//! `seed::mix`), the system's rat faction (CP20, passed in as
//! [`Slot::faction`]), dungeon names (no bundle file carries them; see
//! [`candidates`]) and a type's group and category (typedata, passed in to
//! [`allowed_types`]).

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "dungeon_data",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The bundle's rows, or the fixture with no bundle (client data, class B).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::fixture(),
    }))
}

/// The files, and their header lines copied from `docs/content/schema.md`.
pub const DUNGEONS: (&str, &str) = (
    "dungeons/dungeons.tsv",
    "#dungeon_id\tarchetype_id\tfaction_id\tdifficulty\tentry_object_id\tentry_type_id\tobjectives_id\tname_id\tcontent_tags",
);
pub const ROOMS: (&str, &str) = (
    "dungeons/rooms.tsv",
    "#dungeon_id\troom_id\tempty_reset_delay\tmessage_header_id\tmessage_text_id\tshort_name",
);
pub const OBJECTS: (&str, &str) = (
    "dungeons/objects.tsv",
    "#dungeon_id\troom_id\tobject_id\ttype_id\tx\ty\tz\tradius\tyaw\tpitch\troll\tobject_min_respawn\tobject_max_respawn\tobject_delay_spawn\tobject_trigger_spawn\tname_id",
);
pub const GATES: (&str, &str) = (
    "dungeons/gates.tsv",
    "#dungeon_id\tfrom_object_id\tallowed_ships_list\tdisallowed_fitted_types_list\tallowed_ship_races",
);
pub const SPAWNS: (&str, &str) = (
    "dungeons/spawns.tsv",
    "#dungeon_id\troom_id\tobject_id\tentity_id\tnpc_grouping_id\tbehavior_name\tgroup_behavior_name\ttrigger_spawn\tadditional_loot_id\tentity_min_respawn\tentity_max_respawn\tspawn_once\tdelay_spawn\taggression_range\torbit_range\tjitter_radius\tentity_name_id",
);
pub const ARCHETYPES: (&str, &str) = (
    "dungeons/archetypes.tsv",
    "#archetype_id\ttitle_id\tcontent_tags\tlocation",
);
pub const RESOURCES: (&str, &str) = (
    "dungeons/resources.tsv",
    "#dungeon_id\tkind\ttype_ids\tore_band",
);
pub const ROOM_ROCKS: (&str, &str) = (
    "dungeons/room-rocks.tsv",
    "#dungeon_id\troom_id\ttype_id\tcount",
);
pub const TYPELISTS: (&str, &str) = (
    "types/typelists.tsv",
    "#list_id\tinc_types\tinc_groups\tinc_cats\texc_types\texc_groups\texc_cats",
);

/// `typeCosmicAnomaly` and the signature entry type [V, C3 §4.2, schema.md].
pub const ENTRY_ANOMALY: i64 = 28356;
pub const ENTRY_SIGNATURE: i64 = 19728;
/// The archetypes whose dungeons carry a pirate faction, so a slot draws
/// only the system's rat faction (C3 §4.2 step 1: combat 24, relic 44,
/// data 45). [A] Falsified by a faction-less candidate set for one of them.
pub const FACTION_ARCHETYPES: [i64; 3] = [24, 44, 45];
/// [A] C3 §4.2 step 2: the mercenary faction without fleets until C6 has
/// them. Falsified by C6 shipping its squads.
pub const EXCLUDED_FACTIONS: [i64; 1] = [500021];
/// [A] C3 §4.2 step 2: name fragments of unfinished or event content.
/// Falsified by an excluded name a human wants, or an included broken one.
pub const EXCLUDED_NAME_PARTS: [&str; 7] = [
    "DO NOT",
    "QA",
    "TEST",
    "Test",
    "Placeholder",
    " OLD",
    "Training",
];
/// [A] Fullerite-C28 … C540, the wormhole gases: a gas site holding one is a
/// wormhole "Reservoir" site (C3 §4.2 step 2). Falsified by a Reservoir
/// site whose `resources.tsv` gas row has none of these, or by typedata
/// naming one of them other than "Fullerite-*".
pub const EXCLUDED_GAS_TYPES: [i64; 9] = [
    30370, 30371, 30372, 30373, 30374, 30375, 30376, 30377, 30378,
];

/// One `dungeons.tsv` row, with its child rows attached, each sorted by key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dungeon {
    pub id: i64,
    pub archetype: i64,
    pub faction: Option<i64>,
    pub difficulty: Option<i64>,
    pub entry_object: Option<i64>,
    pub entry_type: Option<i64>,
    pub objectives: Option<i64>,
    pub name_id: Option<i64>,
    pub content_tags: Vec<String>,
    pub rooms: Vec<Room>,
    pub objects: Vec<Object>,
    pub gates: Vec<Gate>,
    pub spawns: Vec<Spawn>,
    pub resources: Vec<Resource>,
    pub room_rocks: Vec<RoomRock>,
}

/// One `rooms.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Room {
    pub id: i64,
    pub empty_reset_delay: Option<i64>,
    pub message_header_id: Option<i64>,
    pub message_text_id: Option<i64>,
    pub short_name: String,
}

/// One `objects.tsv` row; `pos` is room-local metres (`dungeon-model.md` §2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object {
    pub room: i64,
    pub id: i64,
    pub type_id: i64,
    pub pos: [f64; 3],
    pub radius: Option<f64>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub roll: Option<f64>,
    pub min_respawn: Option<i64>,
    pub max_respawn: Option<i64>,
    pub delay_spawn: bool,
    pub trigger_spawn: bool,
    pub name_id: Option<i64>,
}

/// One `gates.tsv` row: a `connections[]` element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gate {
    pub from_object: i64,
    pub allowed_ships_list: Option<i64>,
    pub disallowed_fitted_types_list: Option<i64>,
    pub allowed_ship_races: Vec<i64>,
}

/// One `spawns.tsv` row. `entity` is **not** a type id (schema.md).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spawn {
    pub room: i64,
    pub object: i64,
    pub entity: i64,
    pub npc_grouping: Option<i64>,
    pub behavior_name: Option<String>,
    pub group_behavior_name: Option<String>,
    pub trigger_spawn: bool,
    pub additional_loot: Option<i64>,
    pub min_respawn: Option<i64>,
    pub max_respawn: Option<i64>,
    pub spawn_once: bool,
    pub delay_spawn: bool,
    pub aggression_range: Option<f64>,
    pub orbit_range: Option<f64>,
    pub jitter_radius: Option<f64>,
    pub entity_name_id: Option<i64>,
}

/// `resources.tsv`'s `kind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    #[default]
    Ore,
    Gas,
    Ice,
}

/// `resources.tsv`'s `ore_band` (C3 §7.2 rule R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OreBand {
    High,
    Low,
    Null,
    Special,
}

/// The four basic ices **[V-data]** (Clear Icicle, White Glaze, Glacial
/// Mass, Blue Ice): the high-sec tier. The ice dungeons that hold only
/// these are the ones the client's own ice-anomaly list shows first
/// (archetype 28, difficulty 2, one ice each).
pub const ICE_HIGH_TIER: [i64; 4] = [16_262, 16_263, 16_264, 16_265];
/// Glare Crust **[V-data]**: the ice that lifts a field to the low-sec
/// tier (the dungeons that hold it with the basic ices and nothing finer).
pub const ICE_LOW_TIER: i64 = 16_266;

/// X-ICE: an ice dungeon's security band from the ice it holds, as C3 R4
/// does for ore (the highest band among its ices). **[A]** the tiering:
/// the four basic ices are high-sec, Glare Crust makes a field low-sec,
/// and any finer ice (Dark Glitter, Gelidus, Krystallos, the IV-grades) is
/// null-sec, as the game's own ice (EvEJS facts, Dark Glitter / Gelidus /
/// Krystallos in null-sec only). An ice in none of these (Azure Ice, the
/// event ices) makes the field `Special`, drawn only by a row that asks for
/// it. Falsified by a human who finds a field of the wrong tier in a band.
/// An empty list has no band.
pub fn ice_band(type_ids: &[i64]) -> Option<OreBand> {
    if type_ids.is_empty() {
        return None;
    }
    let mut band = OreBand::High;
    for t in type_ids {
        let b = if ICE_HIGH_TIER.contains(t) {
            OreBand::High
        } else if *t == ICE_LOW_TIER {
            OreBand::Low
        } else if ICE_FINER.contains(t) {
            OreBand::Null
        } else {
            return Some(OreBand::Special);
        };
        band = band.max(b);
    }
    Some(band)
}

/// The finer ices **[V-data]**: Dark Glitter, Gelidus, Krystallos and the
/// four IV-grades (thick blue, enriched clear, pristine white glaze, smooth
/// glacial mass).
pub const ICE_FINER: [i64; 7] = [16_267, 16_268, 16_269, 17_975, 17_976, 17_977, 17_978];

/// One `resources.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resource {
    pub kind: ResourceKind,
    pub type_ids: Vec<i64>,
    pub ore_band: Option<OreBand>,
}

/// One `room-rocks.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomRock {
    pub room: i64,
    pub type_id: i64,
    pub count: i64,
}

/// One `archetypes.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Archetype {
    pub id: i64,
    pub title_id: Option<i64>,
    pub content_tags: Vec<String>,
    pub location: String,
}

/// One `typelists.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeList {
    pub inc_types: Vec<i64>,
    pub inc_groups: Vec<i64>,
    pub inc_cats: Vec<i64>,
    pub exc_types: Vec<i64>,
    pub exc_groups: Vec<i64>,
    pub exc_cats: Vec<i64>,
}

impl TypeList {
    /// [I] Included by type, group or category, and excluded by none of the
    /// three (the client's `evetypes` typelist reading). Falsified by a gate
    /// the client lets a ship through that this refuses.
    pub fn contains(&self, type_id: i64, group: i64, category: i64) -> bool {
        let hit = |t: &[i64], g: &[i64], c: &[i64]| {
            t.contains(&type_id) || g.contains(&group) || c.contains(&category)
        };
        hit(&self.inc_types, &self.inc_groups, &self.inc_cats)
            && !hit(&self.exc_types, &self.exc_groups, &self.exc_cats)
    }
}

/// The catalogue.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub dungeons: BTreeMap<i64, Dungeon>,
    pub archetypes: BTreeMap<i64, Archetype>,
    pub typelists: BTreeMap<i64, TypeList>,
}

/// One data line, split, with typed accessors that name the column.
struct Fields<'a>(Vec<&'a str>);

impl<'a> Fields<'a> {
    fn s(&self, i: usize) -> &'a str {
        self.0[i]
    }
    fn i(&self, i: usize) -> Result<i64, String> {
        let v = self.0[i];
        v.parse()
            .map_err(|e| format!("column {} {v:?} is not an i64: {e}", i + 1))
    }
    fn oi(&self, i: usize) -> Result<Option<i64>, String> {
        (!self.0[i].is_empty()).then(|| self.i(i)).transpose()
    }
    fn f(&self, i: usize) -> Result<f64, String> {
        let v = self.0[i];
        v.parse()
            .map_err(|e| format!("column {} {v:?} is not an f64: {e}", i + 1))
    }
    fn of(&self, i: usize) -> Result<Option<f64>, String> {
        (!self.0[i].is_empty()).then(|| self.f(i)).transpose()
    }
    fn os(&self, i: usize) -> Option<String> {
        (!self.0[i].is_empty()).then(|| self.0[i].to_owned())
    }
    fn b(&self, i: usize) -> Result<bool, String> {
        match self.0[i] {
            "0" => Ok(false),
            "1" => Ok(true),
            v => Err(format!("column {} {v:?} is not 0 or 1", i + 1)),
        }
    }
    fn li(&self, i: usize) -> Result<Vec<i64>, String> {
        self.ls(i)
            .iter()
            .map(|x| {
                x.parse()
                    .map_err(|e| format!("column {} element {x:?}: {e}", i + 1))
            })
            .collect()
    }
    fn ls(&self, i: usize) -> Vec<String> {
        match self.0[i] {
            "" => Vec::new(),
            v => v.split(',').map(str::to_owned).collect(),
        }
    }
}

/// Every row of `file`, parsed; an absent file is no rows. Errors name the
/// file and the data row.
fn rows<T>(
    b: &Bundle,
    (rel, header): (&str, &str),
    parse: impl Fn(&Fields) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let want = header.split('\t').count();
    let lines = b.data_lines(rel, header)?;
    let mut out = Vec::with_capacity(lines.len());
    for (n, line) in lines.iter().enumerate() {
        let f = Fields(line.split('\t').collect());
        if f.0.len() != want {
            return Err(format!(
                "{rel} data row {}: {} fields, the header has {want}",
                n + 1,
                f.0.len()
            ));
        }
        out.push(parse(&f).map_err(|e| format!("{rel} data row {}: {e}", n + 1))?);
    }
    Ok(out)
}

impl Table {
    /// Read the nine files. An absent file contributes no rows, so a bundle
    /// without the dungeons section is an empty catalogue. A wrong header, a
    /// bad field, a duplicate key or a child row whose dungeon is missing is
    /// an error naming the file.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let mut t = Table::default();
        for d in rows(b, DUNGEONS, |f| {
            Ok(Dungeon {
                id: f.i(0)?,
                archetype: f.i(1)?,
                faction: f.oi(2)?,
                difficulty: f.oi(3)?,
                entry_object: f.oi(4)?,
                entry_type: f.oi(5)?,
                objectives: f.oi(6)?,
                name_id: f.oi(7)?,
                content_tags: f.ls(8),
                ..Dungeon::default()
            })
        })? {
            let id = d.id;
            if t.dungeons.insert(id, d).is_some() {
                return Err(format!("{}: dungeon {id} twice", DUNGEONS.0));
            }
        }
        for a in rows(b, ARCHETYPES, |f| {
            Ok(Archetype {
                id: f.i(0)?,
                title_id: f.oi(1)?,
                content_tags: f.ls(2),
                location: f.s(3).to_owned(),
            })
        })? {
            let id = a.id;
            if t.archetypes.insert(id, a).is_some() {
                return Err(format!("{}: archetype {id} twice", ARCHETYPES.0));
            }
        }
        for (id, l) in rows(b, TYPELISTS, |f| {
            let l = TypeList {
                inc_types: f.li(1)?,
                inc_groups: f.li(2)?,
                inc_cats: f.li(3)?,
                exc_types: f.li(4)?,
                exc_groups: f.li(5)?,
                exc_cats: f.li(6)?,
            };
            Ok((f.i(0)?, l))
        })? {
            if t.typelists.insert(id, l).is_some() {
                return Err(format!("{}: list {id} twice", TYPELISTS.0));
            }
        }
        t.attach(b, ROOMS, |f| {
            let r = Room {
                id: f.i(1)?,
                empty_reset_delay: f.oi(2)?,
                message_header_id: f.oi(3)?,
                message_text_id: f.oi(4)?,
                short_name: f.s(5).to_owned(),
            };
            Ok((f.i(0)?, r))
        })?;
        t.attach(b, OBJECTS, |f| {
            let o = Object {
                room: f.i(1)?,
                id: f.i(2)?,
                type_id: f.i(3)?,
                pos: [f.f(4)?, f.f(5)?, f.f(6)?],
                radius: f.of(7)?,
                yaw: f.of(8)?,
                pitch: f.of(9)?,
                roll: f.of(10)?,
                min_respawn: f.oi(11)?,
                max_respawn: f.oi(12)?,
                delay_spawn: f.b(13)?,
                trigger_spawn: f.b(14)?,
                name_id: f.oi(15)?,
            };
            Ok((f.i(0)?, o))
        })?;
        t.attach(b, GATES, |f| {
            let g = Gate {
                from_object: f.i(1)?,
                allowed_ships_list: f.oi(2)?,
                disallowed_fitted_types_list: f.oi(3)?,
                allowed_ship_races: f.li(4)?,
            };
            Ok((f.i(0)?, g))
        })?;
        t.attach(b, SPAWNS, |f| {
            let s = Spawn {
                room: f.i(1)?,
                object: f.i(2)?,
                entity: f.i(3)?,
                npc_grouping: f.oi(4)?,
                behavior_name: f.os(5),
                group_behavior_name: f.os(6),
                trigger_spawn: f.b(7)?,
                additional_loot: f.oi(8)?,
                min_respawn: f.oi(9)?,
                max_respawn: f.oi(10)?,
                spawn_once: f.b(11)?,
                delay_spawn: f.b(12)?,
                aggression_range: f.of(13)?,
                orbit_range: f.of(14)?,
                jitter_radius: f.of(15)?,
                entity_name_id: f.oi(16)?,
            };
            Ok((f.i(0)?, s))
        })?;
        t.attach(b, RESOURCES, |f| {
            let kind = match f.s(1) {
                "ore" => ResourceKind::Ore,
                "gas" => ResourceKind::Gas,
                "ice" => ResourceKind::Ice,
                v => return Err(format!("kind {v:?} is not ore, gas or ice")),
            };
            let ore_band = match f.s(3) {
                "" => None,
                "high" => Some(OreBand::High),
                "low" => Some(OreBand::Low),
                "null" => Some(OreBand::Null),
                "special" => Some(OreBand::Special),
                v => return Err(format!("ore_band {v:?} is not high, low, null or special")),
            };
            let r = Resource {
                kind,
                type_ids: f.li(2)?,
                ore_band,
            };
            Ok((f.i(0)?, r))
        })?;
        t.attach(b, ROOM_ROCKS, |f| {
            let r = RoomRock {
                room: f.i(1)?,
                type_id: f.i(2)?,
                count: f.i(3)?,
            };
            Ok((f.i(0)?, r))
        })?;
        for d in t.dungeons.values_mut() {
            d.sort();
            if let Some(e) = d.duplicate() {
                return Err(format!("dungeon {}: {e} twice", d.id));
            }
        }
        Ok(t)
    }

    /// Read a child file and push each row onto its dungeon.
    fn attach<T>(
        &mut self,
        b: &Bundle,
        file: (&str, &str),
        parse: impl Fn(&Fields) -> Result<(i64, T), String>,
    ) -> Result<(), String>
    where
        Dungeon: Children<T>,
    {
        for (id, row) in rows(b, file, parse)? {
            let d = self
                .dungeons
                .get_mut(&id)
                .ok_or_else(|| format!("{}: dungeon {id} is not in {}", file.0, DUNGEONS.0))?;
            d.push(row);
        }
        Ok(())
    }

    /// Invented rows for tests (`docs/content/plan.md` §10): no client id
    /// except through the named rule constants.
    pub fn fixture() -> Table {
        let d = |id: i64, archetype: i64, faction, difficulty, entry| Dungeon {
            id,
            archetype,
            faction,
            difficulty,
            entry_type: Some(entry),
            name_id: Some(id + 800),
            ..Dungeon::default()
        };
        let pirate = FACTION_ARCHETYPES[0];
        let res = |kind, type_ids: &[i64], ore_band| Resource {
            kind,
            type_ids: type_ids.to_vec(),
            ore_band,
        };
        let mut combat = d(999101, pirate, Some(599001), Some(2), ENTRY_ANOMALY);
        combat.entry_object = Some(8001);
        combat.rooms = vec![
            Room {
                id: 1,
                short_name: "Room 1".into(),
                ..Room::default()
            },
            Room {
                id: 2,
                short_name: "Room 2".into(),
                ..Room::default()
            },
        ];
        let obj = |room, id, type_id, x| Object {
            room,
            id,
            type_id,
            pos: [x, 0.0, 0.0],
            radius: Some(2500.0),
            ..Object::default()
        };
        combat.objects = vec![
            obj(1, 8001, 990060, 0.0),
            obj(1, 8002, 990061, 5000.0),
            obj(2, 8003, 990060, 0.0),
            obj(2, 8004, 990061, 9000.0),
        ];
        combat.gates = vec![
            Gate {
                from_object: 8002,
                allowed_ships_list: Some(9901),
                ..Gate::default()
            },
            Gate {
                from_object: 8004,
                allowed_ships_list: Some(9902),
                ..Gate::default()
            },
        ];
        combat.spawns = vec![Spawn {
            room: 1,
            object: 8001,
            entity: 70001,
            min_respawn: Some(60),
            max_respawn: Some(120),
            ..Spawn::default()
        }];
        let mut ore = d(999102, 97, None, Some(1), ENTRY_ANOMALY);
        ore.resources = vec![res(
            ResourceKind::Ore,
            &[990003, 990006],
            Some(OreBand::Low),
        )];
        ore.room_rocks = vec![RoomRock {
            room: 1,
            type_id: 990003,
            count: 14,
        }];
        let mut special = d(999108, 97, None, Some(1), ENTRY_ANOMALY);
        special.resources = vec![res(ResourceKind::Ore, &[990007], Some(OreBand::Special))];
        let mut reservoir = d(999106, 96, None, Some(2), ENTRY_SIGNATURE);
        reservoir.resources = vec![res(ResourceKind::Gas, &[EXCLUDED_GAS_TYPES[0]], None)];
        let mut nebula = d(999107, 96, None, Some(2), ENTRY_SIGNATURE);
        nebula.resources = vec![res(ResourceKind::Gas, &[990070], None)];
        let dungeons = [
            combat,
            ore,
            d(999103, pirate, Some(599002), Some(2), ENTRY_ANOMALY),
            d(999104, pirate, Some(599001), None, ENTRY_ANOMALY),
            d(
                999105,
                98,
                Some(EXCLUDED_FACTIONS[0]),
                Some(2),
                ENTRY_ANOMALY,
            ),
            reservoir,
            nebula,
            special,
            d(999109, pirate, Some(599001), Some(3), ENTRY_SIGNATURE),
            d(999110, 98, Some(599002), Some(2), ENTRY_ANOMALY),
        ];
        let list =
            |inc_types: &[i64], inc_groups: &[i64], inc_cats: &[i64], exc: &[i64]| TypeList {
                inc_types: inc_types.to_vec(),
                inc_groups: inc_groups.to_vec(),
                inc_cats: inc_cats.to_vec(),
                exc_types: exc.to_vec(),
                ..TypeList::default()
            };
        Table {
            dungeons: dungeons.into_iter().map(|d| (d.id, d)).collect(),
            archetypes: [pirate, 96, 97, 98]
                .into_iter()
                .map(|id| {
                    let a = Archetype {
                        id,
                        location: "Dungeon Specific".into(),
                        ..Archetype::default()
                    };
                    (id, a)
                })
                .collect(),
            typelists: BTreeMap::from([
                (9901, list(&[990001], &[99001], &[9906], &[990005])),
                (9902, list(&[990004], &[], &[], &[])),
            ]),
        }
    }
}

/// A child row type of [`Dungeon`], so `attach` can push it generically.
trait Children<T> {
    fn push(&mut self, row: T);
}
macro_rules! children {
    ($($t:ty => $f:ident),*) => {$(
        impl Children<$t> for Dungeon {
            fn push(&mut self, row: $t) {
                self.$f.push(row);
            }
        }
    )*};
}
children!(Room => rooms, Object => objects, Gate => gates, Spawn => spawns,
          Resource => resources, RoomRock => room_rocks);

impl Dungeon {
    fn sort(&mut self) {
        self.rooms.sort_by_key(|r| r.id);
        self.objects.sort_by_key(|o| o.id);
        self.gates.sort_by_key(|g| g.from_object);
        self.spawns.sort_by_key(|s| (s.object, s.entity));
        self.resources.sort_by_key(|r| r.kind);
        self.room_rocks.sort_by_key(|r| (r.room, r.type_id));
    }

    /// The first repeated key, after [`Self::sort`] (schema.md's keys).
    fn duplicate(&self) -> Option<String> {
        fn rep<K: PartialEq + std::fmt::Debug>(what: &str, k: Vec<K>) -> Option<String> {
            k.windows(2)
                .find(|w| w[0] == w[1])
                .map(|w| format!("{what} {:?}", w[0]))
        }
        rep("room", self.rooms.iter().map(|r| r.id).collect())
            .or_else(|| rep("object", self.objects.iter().map(|o| o.id).collect()))
            .or_else(|| rep("gate", self.gates.iter().map(|g| g.from_object).collect()))
            .or_else(|| {
                let k = self.spawns.iter().map(|s| (s.object, s.entity)).collect();
                rep("spawn", k)
            })
            .or_else(|| rep("resource", self.resources.iter().map(|r| r.kind).collect()))
            .or_else(|| {
                let k = self
                    .room_rocks
                    .iter()
                    .map(|r| (r.room, r.type_id))
                    .collect();
                rep("room-rock", k)
            })
    }

    /// The dungeon's ore band (C3 §7.2 R4), from its `ore` resource row.
    ///
    /// X-ICE: an ice dungeon has no `ore_band` in `resources.tsv` (the
    /// bundle leaves it empty for ice), so its band is [`ice_band`] of its
    /// ice row, and a `*` ice slot draws the ice of its security band.
    pub fn ore_band(&self) -> Option<OreBand> {
        self.resources
            .iter()
            .find(|r| r.kind == ResourceKind::Ore)
            .and_then(|r| r.ore_band)
            .or_else(|| {
                self.resources
                    .iter()
                    .find(|r| r.kind == ResourceKind::Ice)
                    .and_then(|r| ice_band(&r.type_ids))
            })
    }

    /// [I] The first gate (C2 §2.4): the one leaving the lowest room, then
    /// the lowest object id. A gate whose object is not listed sorts last.
    /// Falsified by a client whose `allowedTypes` names a later gate's list.
    pub fn first_gate(&self) -> Option<&Gate> {
        let room = |g: &Gate| {
            self.objects
                .iter()
                .find(|o| o.id == g.from_object)
                .map_or(i64::MAX, |o| o.room)
        };
        self.gates.iter().min_by_key(|g| (room(g), g.from_object))
    }
}

/// A slot's `entry` (`population.tsv`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Anomaly,
    Signature,
}

impl Entry {
    /// The `entry_type_id` it requires.
    pub fn type_id(self) -> i64 {
        match self {
            Entry::Anomaly => ENTRY_ANOMALY,
            Entry::Signature => ENTRY_SIGNATURE,
        }
    }
}

/// A slot's `difficulties` (`population.tsv`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Difficulties<'a> {
    /// The dungeon's difficulty is one of these; a missing one never is.
    In(&'a [i64]),
    /// `*`: a dungeon with an ore band must have this band; one without (ice,
    /// gas) passes [A] (C3 §4.1 names `*` for ore and ice rows alike).
    /// Falsified by an ice row drawing a dungeon a human calls wrong-tier.
    Star(OreBand),
}

/// What one population slot asks for (C3 §4.2 step 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot<'a> {
    pub archetypes: &'a [i64],
    pub entry: Entry,
    pub difficulties: Difficulties<'a>,
    /// The system's rat faction (CP20), applied to [`FACTION_ARCHETYPES`]
    /// only; `None` applies no faction filter.
    pub faction: Option<i64>,
}

/// C3 §4.2 step 2's exclusions, as data so a test can name them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exclusions<'a> {
    pub factions: &'a [i64],
    pub gas_types: &'a [i64],
    pub name_parts: &'a [&'a str],
}

impl Exclusions<'static> {
    /// C3 §4.2 as written.
    pub const C3: Exclusions<'static> = Exclusions {
        factions: &EXCLUDED_FACTIONS,
        gas_types: &EXCLUDED_GAS_TYPES,
        name_parts: &EXCLUDED_NAME_PARTS,
    };
}

/// Whether `name` contains an excluded fragment (case-sensitive, as C3
/// lists both `TEST` and `Test`).
pub fn excluded_name(name: &str, parts: &[&str]) -> bool {
    parts.iter().any(|p| name.contains(p))
}

/// Whether `d` may fill `slot`: C3 §4.2 steps 1 and 2. `name` is the
/// dungeon's English name, if the caller has one.
pub fn admits(d: &Dungeon, slot: &Slot, ex: &Exclusions, name: Option<&str>) -> bool {
    let difficulty = match slot.difficulties {
        Difficulties::In(list) => d.difficulty.is_some_and(|x| list.contains(&x)),
        Difficulties::Star(band) => d.ore_band().is_none_or(|b| b == band),
    };
    let faction = match slot.faction {
        Some(f) if FACTION_ARCHETYPES.contains(&d.archetype) => d.faction == Some(f),
        _ => true,
    };
    let bad_gas = d.resources.iter().any(|r| {
        r.kind == ResourceKind::Gas && r.type_ids.iter().any(|t| ex.gas_types.contains(t))
    });
    slot.archetypes.contains(&d.archetype)
        && d.entry_type == Some(slot.entry.type_id())
        && difficulty
        && faction
        && !d.faction.is_some_and(|f| ex.factions.contains(&f))
        && !bad_gas
        && !name.is_some_and(|n| excluded_name(n, ex.name_parts))
}

/// Every dungeon id that may fill `slot`, ascending, so CP21's
/// `candidates[mix % len]` is reproducible. Empty is a valid answer
/// (C3 §4.2 step 4).
///
/// `name_of` maps a `name_id` to its English text. **No bundle file carries
/// dungeon names** (`dungeons.tsv` has `name_id` only), so until one does a
/// caller passes `&|_| None` and the name exclusion is inert.
pub fn candidates(
    t: &Table,
    slot: &Slot,
    ex: &Exclusions,
    name_of: &dyn Fn(i64) -> Option<String>,
) -> Vec<i64> {
    t.dungeons
        .values()
        .filter(|d| {
            let name = d.name_id.and_then(name_of);
            admits(d, slot, ex, name.as_deref())
        })
        .map(|d| d.id)
        .collect()
}

/// The dungeon's `allowedTypes` (C2 §2.4): its first gate's
/// `allowed_ships_list`, expanded over `types` = `(type_id, group,
/// category)` (typedata), ascending. `None` when the dungeon is unknown, has
/// no gate, its first gate has no list, or the list is not in
/// `typelists.tsv`.
pub fn allowed_types(
    t: &Table,
    dungeon: i64,
    types: impl IntoIterator<Item = (i64, i64, i64)>,
) -> Option<Vec<i64>> {
    let list_id = t.dungeons.get(&dungeon)?.first_gate()?.allowed_ships_list?;
    let list = t.typelists.get(&list_id)?;
    let mut out: Vec<i64> = types
        .into_iter()
        .filter(|&(ty, g, c)| list.contains(ty, g, c))
        .map(|(ty, _, _)| ty)
        .collect();
    out.sort_unstable();
    out.dedup();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "eve-cp16-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dungeons")).unwrap();
        std::fs::create_dir_all(d.join("types")).unwrap();
        Tmp(d)
    }
    fn write(t: &Tmp, (rel, header): (&str, &str), rows: &[&str]) {
        let mut s = format!("{header}\n# a comment\n");
        for r in rows {
            s.push_str(r);
            s.push('\n');
        }
        std::fs::write(t.0.join(rel), s).unwrap();
    }

    /// Every file, with schema.md's invented example rows plus a few more.
    fn synthetic() -> Tmp {
        let t = tmp();
        let rows = [
            format!("999101\t98\t599001\t2\t8001\t{ENTRY_ANOMALY}\t\t999901\tactivity_combat"),
            format!("999102\t97\t\t\t\t{ENTRY_ANOMALY}\t\t\t"),
        ];
        write(&t, DUNGEONS, &[&rows[0], &rows[1]]);
        write(
            &t,
            ROOMS,
            &["999101\t1\t\t\t\tRoom 1", "999101\t0\t60\t1\t2\tR0 entry"],
        );
        write(
            &t,
            OBJECTS,
            &[
                "999101\t1\t8002\t990061\t1.5\t-2.0\t3e-05\t\t\t\t\t\t\t0\t1\t",
                "999101\t1\t8001\t990060\t0.0\t0.0\t0.0\t2500.0\t\t\t\t\t\t0\t0\t",
            ],
        );
        write(
            &t,
            GATES,
            &["999101\t8002\t9901\t\t", "999101\t8001\t\t9902\t1,2"],
        );
        write(
            &t,
            SPAWNS,
            &[
                "999101\t1\t8001\t70001\t9901\t\t\t0\t\t60\t120\t0\t0\t30000.0\t5000.0\t\t",
                "999101\t1\t8001\t70002\t\tguard\tpack\t1\t5\t\t\t1\t1\t\t\t250.0\t77",
            ],
        );
        write(&t, ARCHETYPES, &["98\t\tactivity_combat\tDungeon Specific"]);
        write(
            &t,
            RESOURCES,
            &["999102\tore\t990003,990006\tlow", "999102\tgas\t990070\t"],
        );
        write(&t, ROOM_ROCKS, &["999102\t1\t990003\t14"]);
        write(&t, TYPELISTS, &["9901\t\t99001,99002\t\t990001\t\t"]);
        t
    }
    fn bundle(t: &Tmp) -> Bundle {
        Bundle { dir: t.0.clone() }
    }

    #[test]
    fn loads_a_synthetic_bundle() {
        let t = synthetic();
        let c = Table::from_bundle(&bundle(&t)).unwrap();
        assert_eq!(c.dungeons.len(), 2);
        let d = &c.dungeons[&999101];
        assert_eq!(
            (d.archetype, d.faction, d.difficulty),
            (98, Some(599001), Some(2))
        );
        let entry = (d.entry_object, d.entry_type, d.objectives);
        assert_eq!(entry, (Some(8001), Some(ENTRY_ANOMALY), None));
        assert_eq!(d.content_tags, vec!["activity_combat".to_owned()]);
        assert_eq!(
            d.rooms.iter().map(|r| r.id).collect::<Vec<_>>(),
            [0, 1],
            "sorted"
        );
        assert_eq!(d.rooms[0].short_name, "R0 entry");
        assert_eq!(d.rooms[0].empty_reset_delay, Some(60));
        let o = &d.objects[1];
        assert_eq!((o.id, o.pos, o.radius), (8002, [1.5, -2.0, 3e-05], None));
        assert!(o.trigger_spawn && !o.delay_spawn);
        assert_eq!(d.gates[0].allowed_ship_races, [1, 2]);
        assert_eq!(d.gates[0].disallowed_fitted_types_list, Some(9902));
        let s = &d.spawns[1];
        assert_eq!(s.behavior_name.as_deref(), Some("guard"));
        assert_eq!(
            (s.additional_loot, s.jitter_radius, s.entity_name_id),
            (Some(5), Some(250.0), Some(77))
        );
        assert!(s.trigger_spawn && s.spawn_once && s.delay_spawn);
        assert_eq!(d.spawns[0].npc_grouping, Some(9901));
        let ore = &c.dungeons[&999102];
        assert_eq!((ore.difficulty, ore.name_id), (None, None));
        assert_eq!(ore.ore_band(), Some(OreBand::Low));
        assert_eq!(ore.resources[1].kind, ResourceKind::Gas);
        assert_eq!(ore.room_rocks[0].count, 14);
        assert_eq!(c.archetypes[&98].location, "Dungeon Specific");
        assert_eq!(c.typelists[&9901].inc_groups, [99001, 99002]);
        // Through the registered spec, as the server loads it.
        let dir = crate::authored::Dir::compiled_only();
        let b = bundle(&t);
        let boxed = (TABLE.load)(&Sources::new(Some(&b), &dir)).unwrap();
        assert_eq!(boxed.downcast_ref::<Table>(), Some(&c));
    }

    #[test]
    fn absent_files_give_an_empty_catalogue() {
        let t = tmp();
        assert_eq!(Table::from_bundle(&bundle(&t)).unwrap(), Table::default());
        // No bundle at all: the fixture (client data, class B).
        let dir = crate::authored::Dir::compiled_only();
        let boxed = (TABLE.load)(&Sources::new(None, &dir)).unwrap();
        assert_eq!(boxed.downcast_ref::<Table>(), Some(&Table::fixture()));
    }

    #[test]
    fn a_bad_header_is_rejected() {
        let t = synthetic();
        std::fs::write(
            t.0.join(GATES.0),
            "#dungeon_id\tfrom_object_id\n999101\t8002\n",
        )
        .unwrap();
        let e = Table::from_bundle(&bundle(&t)).unwrap_err();
        assert!(e.contains(GATES.0) && e.contains("header"), "{e}");
    }

    #[test]
    fn bad_rows_orphans_and_duplicates_are_rejected() {
        let cases: [((&str, &str), &str, &str); 5] = [
            (ROOM_ROCKS, "999101\t1\t990003", "fields"),
            (ROOM_ROCKS, "999101\t1\t990003\tmany", "i64"),
            (RESOURCES, "999101\tmud\t\t", "kind"),
            (ROOMS, "999999\t1\t\t\t\tlost", "999999"),
            (ROOMS, "999101\t1\t\t\t\tagain", "twice"),
        ];
        for (file, row, want) in cases {
            let t = synthetic();
            let path = t.0.join(file.0);
            let mut s = std::fs::read_to_string(&path).unwrap();
            s.push_str(row);
            s.push('\n');
            std::fs::write(&path, s).unwrap();
            let e = Table::from_bundle(&bundle(&t)).unwrap_err();
            assert!(e.contains(want), "{row:?}: {e}");
        }
    }

    fn slot<'a>(archetypes: &'a [i64], entry: Entry, d: Difficulties<'a>) -> Slot<'a> {
        Slot {
            archetypes,
            entry,
            difficulties: d,
            faction: None,
        }
    }
    fn none(_: i64) -> Option<String> {
        None
    }

    #[test]
    fn the_filters_match_archetype_entry_and_difficulty() {
        let t = Table::fixture();
        let pirate = [FACTION_ARCHETYPES[0]];
        let s = slot(&pirate, Entry::Anomaly, Difficulties::In(&[2]));
        // 999104 has no difficulty, 999109 is a signature: neither is drawn.
        assert_eq!(candidates(&t, &s, &Exclusions::C3, &none), [999101, 999103]);
        let s = slot(&pirate, Entry::Signature, Difficulties::In(&[2, 3]));
        assert_eq!(candidates(&t, &s, &Exclusions::C3, &none), [999109]);
        let s = slot(&pirate, Entry::Anomaly, Difficulties::In(&[5]));
        assert!(
            candidates(&t, &s, &Exclusions::C3, &none).is_empty(),
            "an empty slot"
        );
    }

    #[test]
    fn the_faction_filter_applies_to_faction_archetypes_only() {
        let t = Table::fixture();
        let arch = [FACTION_ARCHETYPES[0], 98];
        let mut s = slot(&arch, Entry::Anomaly, Difficulties::In(&[2]));
        s.faction = Some(599001);
        // 999110 (archetype 98, faction 599002) passes: 98 carries no faction.
        assert_eq!(candidates(&t, &s, &Exclusions::C3, &none), [999101, 999110]);
        s.faction = Some(599002);
        assert_eq!(candidates(&t, &s, &Exclusions::C3, &none), [999103, 999110]);
    }

    #[test]
    fn the_exclusions_drop_faction_gas_and_names() {
        let t = Table::fixture();
        let s = slot(&[98], Entry::Anomaly, Difficulties::In(&[2]));
        let keep = Exclusions {
            factions: &[],
            ..Exclusions::C3
        };
        assert_eq!(candidates(&t, &s, &keep, &none), [999105, 999110]);
        assert_eq!(candidates(&t, &s, &Exclusions::C3, &none), [999110]);
        let gas = slot(&[96], Entry::Signature, Difficulties::In(&[2]));
        assert_eq!(candidates(&t, &gas, &Exclusions::C3, &none), [999107]);
        // A name with any listed fragment is dropped; the rule, per fragment.
        for part in EXCLUDED_NAME_PARTS {
            let named = |id: i64| (id == 999107 + 800).then(|| format!("Example{part}Nebula"));
            assert!(
                candidates(&t, &gas, &Exclusions::C3, &named).is_empty(),
                "{part}"
            );
            assert!(excluded_name(&format!("x{part}y"), &EXCLUDED_NAME_PARTS));
        }
        let fine = |_| Some("Example Nebula".to_owned());
        assert_eq!(candidates(&t, &gas, &Exclusions::C3, &fine), [999107]);
    }

    #[test]
    fn star_draws_by_ore_band() {
        let t = Table::fixture();
        let ore = |b| slot(&[97], Entry::Anomaly, Difficulties::Star(b));
        assert_eq!(
            candidates(&t, &ore(OreBand::Low), &Exclusions::C3, &none),
            [999102]
        );
        assert!(candidates(&t, &ore(OreBand::High), &Exclusions::C3, &none).is_empty());
        // `special` is drawn only by a row that asks for it.
        assert_eq!(
            candidates(&t, &ore(OreBand::Special), &Exclusions::C3, &none),
            [999108]
        );
        // A dungeon with no ore row passes `*` (gas here).
        let gas = slot(&[96], Entry::Signature, Difficulties::Star(OreBand::Null));
        assert_eq!(candidates(&t, &gas, &Exclusions::C3, &none), [999107]);
    }

    /// X-ICE: an ice dungeon's band follows the ice it holds, and a `*` ice
    /// slot draws only the ice of its own band.
    #[test]
    fn ice_band_follows_the_ice_held() {
        let (basic, glare, finer) = (ICE_HIGH_TIER[0], ICE_LOW_TIER, ICE_FINER[0]);
        assert_eq!(ice_band(&[]), None);
        assert_eq!(ice_band(&[basic, ICE_HIGH_TIER[3]]), Some(OreBand::High));
        assert_eq!(ice_band(&[basic, glare]), Some(OreBand::Low));
        assert_eq!(ice_band(&[glare, finer]), Some(OreBand::Null));
        assert_eq!(ice_band(&[glare, finer, ICE_FINER[6]]), Some(OreBand::Null));
        // A type in no tier (invented here) makes the field special.
        assert_eq!(ice_band(&[basic, 990_999]), Some(OreBand::Special));

        let ice = |id, ices: &[i64]| Dungeon {
            id,
            archetype: 28,
            entry_type: Some(ENTRY_ANOMALY),
            resources: vec![Resource {
                kind: ResourceKind::Ice,
                type_ids: ices.to_vec(),
                ore_band: None,
            }],
            ..Dungeon::default()
        };
        let mut t = Table::default();
        for d in [
            ice(1, &[basic]),
            ice(2, &[basic, glare]),
            ice(3, &[glare, finer]),
            ice(4, &[990_999]),
        ] {
            t.dungeons.insert(d.id, d);
        }
        let drawn = |b| {
            let s = slot(&[28], Entry::Anomaly, Difficulties::Star(b));
            candidates(&t, &s, &Exclusions::C3, &none)
        };
        assert_eq!(drawn(OreBand::High), [1]);
        assert_eq!(drawn(OreBand::Low), [2]);
        assert_eq!(drawn(OreBand::Null), [3]);
        assert_eq!(drawn(OreBand::Special), [4]);
    }

    #[test]
    fn allowed_types_resolves_through_typelists() {
        let t = Table::fixture();
        // (type, group, category): in by type, by group, by category; out by
        // an exclusion; out by nothing matching.
        let types = [
            (990001, 1, 1),
            (990002, 99001, 6),
            (990003, 5, 9906),
            (990005, 99001, 6),
            (990004, 7, 7),
        ];
        // The gate leaving room 1 (list 9901), not the room-2 gate (9902).
        assert_eq!(
            t.dungeons[&999101].first_gate().unwrap().allowed_ships_list,
            Some(9901)
        );
        assert_eq!(
            allowed_types(&t, 999101, types),
            Some(vec![990001, 990002, 990003])
        );
        assert_eq!(allowed_types(&t, 999102, types), None, "no gate");
        assert_eq!(allowed_types(&t, 1, types), None, "no dungeon");
        let mut u = t.clone();
        u.typelists.remove(&9901);
        assert_eq!(allowed_types(&u, 999101, types), None, "list not loaded");
    }
}
