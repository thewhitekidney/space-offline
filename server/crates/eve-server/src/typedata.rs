//! The client's own type and dogma tables, loaded at startup from an extract.
//!
//! Owned by the W9 data-breadth lane (`docs/lanes/typedata.md`).
//!
//! # Why this exists
//!
//! The server's type knowledge was hand-copied rows: ~20 hulls
//! (`crate::destiny::hull::HULLS`), ~20 modules (`crate::modules::TYPES`), the
//! fitting numbers per hull (`crate::fitting::HULLS`) and the rows
//! `/createitem` can grant (`crate::item::SPAWNABLE`). So most ships could not
//! be boarded or fitted and most modules could not be created. Every one of
//! those numbers came from the client's own tables anyway -- `types`,
//! `groups`, `typedogma`, `dogmaattributes`, `dogmaeffects` -- so this module
//! reads **all** of them, for every ship, module, charge and drone, from the
//! file `tools/typedata.py` writes -- and, since CP12, every commodity
//! (category 17: tags, salvage, trade goods) and asteroid (category 25: ore),
//! which are plain stackable items here.
//!
//! # Fall-through, not replacement
//!
//! Every existing lookup asks its hand-made table **first** and this one
//! second: `item::find_in`/`find_type_in`, `modules::find_in`,
//! `destiny::hull::find_in`, `fitting::hull_in`. The hand rows are the client's numbers too, they are
//! what every existing test was written against, and they are the whole
//! world when the server starts without `--typedata`. The one reader that
//! changes for a type the hand tables already know is
//! `damage::Profile::from_hull_in`, which takes its resonances from here when
//! an extract is loaded -- the hand hull table never had resistances at all.
//!
//! # CCP's data stays out of the repository
//!
//! Like `--celestials` and `--planets`: the extract is generated into `out/`
//! (gitignored) from the player's own install, and loaded by flag. Nothing in
//! this file is a number from the client; the tests use a fixture they write.
//!
//! # In `Content` (CP58a, ruling 3), and only there (X2)
//!
//! [`Installed`] is a content table ([`TABLE`], plan §1.5): built from the
//! bundle's `types/typedata.tsv`, or from the invented [`FIXTURE`] without
//! a bundle, and read through `reg.content()` with the `*_in(&Content, ..)`
//! accessors ([`item_in`], [`hull_in`], ...). `--typedata FILE` replaces
//! it before the world is built ([`override_in`]). The process-wide
//! `OnceLock` and its free accessors are gone (X2b): every reader has a
//! registry or is handed a `&Content`.

use crate::combat::{ChargeType, Weapon};
use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::damage::Damage;
use crate::destiny::hull::Hull;
use crate::fitting::{Hardpoint, HullFit, Rack};
use crate::item::ItemType;
use crate::modules::{Effect, ModuleType};
use std::collections::{BTreeMap, HashMap};

/// `categoryShip` (`inventorycommon/const`) [V].
pub const CATEGORY_SHIP: i64 = 6;
/// `categoryModule` [V].
pub const CATEGORY_MODULE: i64 = 7;
/// `categoryCharge` [V].
pub const CATEGORY_CHARGE: i64 = 8;
/// `categoryDrone` [V].
pub const CATEGORY_DRONE: i64 = 18;
/// `categoryEntity` [V]: NPC types, extracted for `crate::npc::derive`.
pub const CATEGORY_ENTITY: i64 = 11;
/// `categorySkill` [V]. Extracted for the dogma engine (`crate::dogma`): a
/// skill's level reaches a ship only through the skill type's own effects.
/// **Not an item here** -- `crate::skill` owns skillbooks -- so
/// [`Installed::build`] leaves them out of every item view.
pub const CATEGORY_SKILL: i64 = 16;
/// `categoryCommodity` [V]: NPC loot tags, salvage and trade goods (C6 R7).
/// Added by CP12; a stackable item like a charge.
pub const CATEGORY_COMMODITY: i64 = 17;
/// `categoryAsteroid` [V]: ore and its compressed forms (C1 §2.3). Added by
/// CP12, so mined ore is a type the item lookups know.
pub const CATEGORY_ASTEROID: i64 = 25;
/// `categoryCelestial` [V]: only group 988 (wormholes) is extracted (FX7),
/// for `crate::wormhole`'s lifetime and mass attributes. Never an item.
pub const CATEGORY_CELESTIAL: i64 = 2;
/// `categoryStation` [V]: extracted for the type's radius
/// (`crate::destiny::station_radius`, FX7). Never an item.
pub const CATEGORY_STATION: i64 = 3;
/// `categoryBlueprint` [V], extracted since FX7. A singleton, like the
/// `item::SPAWNABLE` blueprint rows (`BlueprintInstance` derives
/// `singleton = quantity < 0`).
pub const CATEGORY_BLUEPRINT: i64 = 9;

/// The dogma attribute ids this module derives the other tables from. The
/// names are CCP's (`dogmaattributes`), read with `tools/dogma.py` [V].
pub mod attr {
    pub const POWER_OUTPUT: i64 = 11;
    pub const LOW_SLOTS: i64 = 12;
    pub const MED_SLOTS: i64 = 13;
    pub const HI_SLOTS: i64 = 14;
    pub const MAX_VELOCITY: i64 = 37;
    pub const CPU_OUTPUT: i64 = 48;
    pub const CAPACITOR_RECHARGE: i64 = 55;
    pub const DAMAGE_MULTIPLIER: i64 = 64;
    pub const AGILITY: i64 = 70;
    pub const MAX_TARGET_RANGE: i64 = 76;
    pub const HP: i64 = 9;
    pub const LAUNCHER_SLOTS: i64 = 101;
    pub const TURRET_SLOTS: i64 = 102;
    pub const CHARGE_SIZE: i64 = 128;
    pub const MAX_LOCKED_TARGETS: i64 = 192;
    pub const SHIELD_CAPACITY: i64 = 263;
    pub const ARMOR_HP: i64 = 265;
    pub const DRONE_CAPACITY: i64 = 283;
    pub const SHIELD_RECHARGE: i64 = 479;
    pub const CAPACITOR_CAPACITY: i64 = 482;
    pub const SIGNATURE_RADIUS: i64 = 552;
    pub const SCAN_RESOLUTION: i64 = 564;
    pub const WARP_SPEED_MULTIPLIER: i64 = 600;
    pub const RIG_SLOTS: i64 = 1137;
    pub const DRONE_BANDWIDTH: i64 = 1271;
    pub const BASE_WARP_SPEED: i64 = 1281;
    /// `explosionDelay`: a missile's flight time, ms (R-MISSILE §2.1).
    pub const EXPLOSION_DELAY: i64 = 281;
}

/// The slot and hardpoint effects (`dogma/const`) [V]: `loPower` 11,
/// `hiPower` 12, `medPower` 13, `launcherFitted` 40, `turretFitted` 42;
/// and a launcher's own `useMissiles` 101 (`dogmaeffects[101]`, no guid).
pub mod effect {
    pub const USE_MISSILES: i64 = 101;
    pub const LO_POWER: i64 = 11;
    pub const HI_POWER: i64 = 12;
    pub const MED_POWER: i64 = 13;
    pub const LAUNCHER_FITTED: i64 = 40;
    pub const TURRET_FITTED: i64 = 42;
}

/// One type, as `tools/typedata.py` wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRow {
    pub type_id: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub published: bool,
    pub portion_size: i64,
    pub volume: f64,
    pub capacity: f64,
    pub mass: f64,
    pub radius: f64,
    /// The `typedogma` effect flagged `isDefault`, if any.
    pub default_effect: Option<i64>,
    pub effects: Vec<i64>,
    /// Exactly the type's own `typedogma` rows, ascending by id. **Presence
    /// is information** -- a module carrying `chargeGroup1` takes charges --
    /// so the attribute defaults are kept apart, in [`Table::attribute`].
    pub attributes: Vec<(i64, f64)>,
    pub name: String,
}

impl TypeRow {
    /// The type's own value for `attribute_id`, if it carries one.
    pub fn attribute(&self, attribute_id: i64) -> Option<f64> {
        self.attributes
            .binary_search_by_key(&attribute_id, |(id, _)| *id)
            .ok()
            .map(|i| self.attributes[i].1)
    }

    pub fn has_effect(&self, effect_id: i64) -> bool {
        self.effects.contains(&effect_id)
    }
}

/// One `dogmaeffects` row, the columns the server reads.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectRow {
    pub effect_id: i64,
    /// `effectCategory`: 0 passive, 1 activation, 2 target, ...
    pub category: i64,
    pub is_offensive: bool,
    /// Each `0` when the effect has none.
    pub duration_attribute: i64,
    pub discharge_attribute: i64,
    pub range_attribute: i64,
    pub falloff_attribute: i64,
    pub tracking_speed_attribute: i64,
    pub name: String,
    pub guid: String,
    /// `isWarpSafe`: false stops the effect when the ship's warp starts
    /// (`EffectDeactivationWarping`). `None` for a table written before the
    /// column existed: nothing is stopped then.
    pub is_warp_safe: Option<bool>,
    /// `fittingUsageChanceAttributeID`: the attribute on a booster that holds the
    /// chance this effect (a side effect) is rolled in. `None` for a table written
    /// before the column existed; `Some(0)` when the effect has no chance.
    pub usage_chance_attribute: Option<i64>,
}

/// The `dogmaattributes` columns that decide how a modifier lands on an
/// attribute (`dogma/attributes/attribute.pyc` and `itemAttributeFactory.pyc`)
/// [V]. The `attrmeta` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttrMeta {
    /// `stackable`: **false** makes the client build a
    /// `StackingNurfedAttribute` (`itemAttributeFactory.GetAttributeInstance`).
    pub stackable: bool,
    pub high_is_good: bool,
    /// `maxAttributeID` / `minAttributeID`, `0` for none: the value is
    /// clamped to that attribute **of the same item** (`Attribute.Update`).
    pub max_attribute: i64,
    pub min_attribute: i64,
    /// `dataType`: 10, 11 and 12 are truncated with `int()` (`Update:379`).
    pub data_type: i64,
}

impl AttrMeta {
    /// What an attribute the extract has no `attrmeta` for is treated as:
    /// stackable, unclamped, a float. An extract from before the dogma lane
    /// carries no modifiers either, so nothing is ever applied to it.
    pub const PLAIN: AttrMeta = AttrMeta {
        stackable: true,
        high_is_good: true,
        max_attribute: 0,
        min_attribute: 0,
        data_type: 0,
    };
}

/// `modifierInfo[].domain`: whose item the modifier is aimed from. The seven
/// values the table holds (`docs/re/dogma.md` §5) [V], spelled as the client
/// spells them (`dogma/effects/modifiers.pyc` `BaseModifier.__init__`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// `itemID` -- and an absent domain, which `_GetDomainID` also reads as
    /// the item itself.
    Item,
    Ship,
    Char,
    /// `otherID`: a module's loaded charge, or a charge's module.
    Other,
    Structure,
    /// `targetID` and `target`: remote effects, not self-fit dogma.
    Target,
}

/// `modifierInfo[].func`, the six the table holds [V]
/// (`modifiers.pyc` `GetModifierClassByTypeString`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    ItemModifier,
    LocationModifier,
    LocationGroupModifier,
    LocationRequiredSkillModifier,
    OwnerRequiredSkillModifier,
    /// `NoOpModifier` on the client's side of this build.
    EffectStopper,
}

/// One `modifierInfo` element. The `modifier` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifier {
    pub domain: Domain,
    pub func: Func,
    pub modified_attribute: Option<i64>,
    pub modifying_attribute: Option<i64>,
    /// Signed: `-1` is `dgmAssPreAssignment` (`dogma/const.pyc:8`) [V].
    pub operation: Option<i64>,
    pub skill_type: Option<i64>,
    pub group: Option<i64>,
}

/// How many of each kind a table holds, for the startup line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub ships: usize,
    pub modules: usize,
    pub charges: usize,
    pub drones: usize,
    /// Of `modules`, the ones that have a rack and so can be fitted.
    pub fittable_modules: usize,
    pub effects: usize,
    pub attributes: usize,
    /// Skill types (category 16), extracted for their effects only.
    pub skills: usize,
    /// `modifierInfo` elements, over every extracted effect.
    pub modifiers: usize,
    /// Commodity types (category 17).
    pub commodities: usize,
    /// Asteroid types (category 25).
    pub asteroids: usize,
}

/// The whole extract.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    types: BTreeMap<i64, TypeRow>,
    effects: BTreeMap<i64, EffectRow>,
    /// `dogmaattributes[id].defaultValue`.
    defaults: BTreeMap<i64, f64>,
    /// `dogmaattributes` stacking and clamping columns.
    meta: BTreeMap<i64, AttrMeta>,
    /// `dogmaeffects[id].modifierInfo`, in the table's order.
    modifiers: BTreeMap<i64, Vec<Modifier>>,
}

impl Table {
    /// Read the file `tools/typedata.py` writes.
    pub fn load(path: &std::path::Path) -> Result<Table, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Table::parse(&text)
    }

    /// The same from text: one tab-separated record per line, `#` for
    /// comments (the tool's docstring is the format's definition):
    ///
    /// ```text
    ///     type       typeID groupID categoryID published portionSize volume
    ///                capacity mass radius defaultEffectID effectIDs attrs name
    ///     attribute  attributeID defaultValue name
    ///     effect     effectID category isOffensive durationAttributeID
    ///                dischargeAttributeID rangeAttributeID falloffAttributeID
    ///                trackingSpeedAttributeID name guid isWarpSafe
    ///     attrmeta   attributeID stackable highIsGood maxAttributeID
    ///                minAttributeID dataType
    ///     modifier   effectID domain func modifiedAttributeID
    ///                modifyingAttributeID operation skillTypeID groupID
    /// ```
    ///
    /// A `modifier`'s optional columns are empty when the client's field is
    /// absent; an unknown `domain` or `func` is an error like any other
    /// malformed column.
    ///
    /// **A malformed line is an error, not a skip**, for `crate::planet`'s
    /// reason: a type with a dropped column is a ship that flies
    /// *differently*, not one that is visibly missing. An unknown record tag
    /// is skipped, so a newer tool can add one.
    ///
    /// ```
    /// let t = eve_server::typedata::Table::parse(
    ///     "attribute\t37\t0.0\tmaxVelocity\n\
    ///      type\t9001\t25\t6\t1\t1\t100.0\t50.0\t1000.0\t20.0\t0\t\t37=300.0\tA Hull\n",
    /// )
    /// .unwrap();
    /// assert_eq!(t.attribute(9001, 37), 300.0);
    /// ```
    pub fn parse(text: &str) -> Result<Table, String> {
        fn int(f: &[&str], i: usize, line: usize) -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .ok_or_else(|| format!("line {line}: column {i}: expected an integer"))
        }
        fn real(f: &[&str], i: usize, line: usize) -> Result<f64, String> {
            f.get(i)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("line {line}: column {i}: expected a number"))
        }
        fn flag(f: &[&str], i: usize, line: usize) -> Result<bool, String> {
            match f.get(i).map(|v| v.trim()) {
                Some("1") => Ok(true),
                Some("0") => Ok(false),
                _ => Err(format!("line {line}: column {i}: expected 0 or 1")),
            }
        }
        fn string(f: &[&str], i: usize, line: usize) -> Result<String, String> {
            f.get(i)
                .map(|v| v.to_string())
                .ok_or_else(|| format!("line {line}: column {i} is missing"))
        }
        let mut t = Table::default();
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            match f[0] {
                "type" => {
                    let effects = f
                        .get(11)
                        .copied()
                        .unwrap_or("")
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| {
                            s.trim()
                                .parse::<i64>()
                                .map_err(|_| format!("line {n}: bad effect id {s:?}"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let mut attributes = f
                        .get(12)
                        .copied()
                        .unwrap_or("")
                        .split(';')
                        .filter(|s| !s.trim().is_empty())
                        .map(|kv| {
                            let (k, v) = kv.split_once('=').ok_or_else(|| {
                                format!("line {n}: expected id=value, got {kv:?}")
                            })?;
                            let k = k
                                .trim()
                                .parse::<i64>()
                                .map_err(|_| format!("line {n}: bad attribute id {k:?}"))?;
                            let v = v
                                .trim()
                                .parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite())
                                .ok_or_else(|| format!("line {n}: bad attribute value {v:?}"))?;
                            Ok((k, v))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    attributes.sort_by_key(|(k, _)| *k);
                    attributes.dedup_by_key(|(k, _)| *k);
                    let default_effect = int(&f, 10, n)?;
                    let row = TypeRow {
                        type_id: int(&f, 1, n)?,
                        group_id: int(&f, 2, n)?,
                        category_id: int(&f, 3, n)?,
                        published: flag(&f, 4, n)?,
                        portion_size: int(&f, 5, n)?,
                        volume: real(&f, 6, n)?,
                        capacity: real(&f, 7, n)?,
                        mass: real(&f, 8, n)?,
                        radius: real(&f, 9, n)?,
                        default_effect: (default_effect != 0).then_some(default_effect),
                        effects,
                        attributes,
                        name: string(&f, 13, n)?,
                    };
                    t.types.insert(row.type_id, row);
                }
                "attribute" => {
                    t.defaults.insert(int(&f, 1, n)?, real(&f, 2, n)?);
                }
                "effect" => {
                    let row = EffectRow {
                        effect_id: int(&f, 1, n)?,
                        category: int(&f, 2, n)?,
                        is_offensive: flag(&f, 3, n)?,
                        duration_attribute: int(&f, 4, n)?,
                        discharge_attribute: int(&f, 5, n)?,
                        range_attribute: int(&f, 6, n)?,
                        falloff_attribute: int(&f, 7, n)?,
                        tracking_speed_attribute: int(&f, 8, n)?,
                        name: string(&f, 9, n)?,
                        guid: f.get(10).copied().unwrap_or("").to_string(),
                        is_warp_safe: match f.get(11).map(|v| v.trim()) {
                            Some("1") => Some(true),
                            Some("0") => Some(false),
                            _ => None,
                        },
                        usage_chance_attribute: f
                            .get(12)
                            .and_then(|v| v.trim().parse::<i64>().ok()),
                    };
                    t.effects.insert(row.effect_id, row);
                }
                "attrmeta" => {
                    t.meta.insert(
                        int(&f, 1, n)?,
                        AttrMeta {
                            stackable: flag(&f, 2, n)?,
                            high_is_good: flag(&f, 3, n)?,
                            max_attribute: int(&f, 4, n)?,
                            min_attribute: int(&f, 5, n)?,
                            data_type: int(&f, 6, n)?,
                        },
                    );
                }
                "modifier" => {
                    let opt = |i: usize| -> Result<Option<i64>, String> {
                        match f.get(i).map(|v| v.trim()) {
                            None | Some("") => Ok(None),
                            Some(v) => v.parse::<i64>().map(Some).map_err(|_| {
                                format!("line {n}: column {i}: expected an integer or nothing")
                            }),
                        }
                    };
                    let domain = match f.get(2).map(|v| v.trim()) {
                        Some("itemID") | Some("") => Domain::Item,
                        Some("shipID") => Domain::Ship,
                        Some("charID") => Domain::Char,
                        Some("otherID") => Domain::Other,
                        Some("structureID") => Domain::Structure,
                        Some("targetID") | Some("target") => Domain::Target,
                        other => return Err(format!("line {n}: unknown domain {other:?}")),
                    };
                    let func = match f.get(3).map(|v| v.trim()) {
                        Some("ItemModifier") => Func::ItemModifier,
                        Some("LocationModifier") => Func::LocationModifier,
                        Some("LocationGroupModifier") => Func::LocationGroupModifier,
                        Some("LocationRequiredSkillModifier") => {
                            Func::LocationRequiredSkillModifier
                        }
                        Some("OwnerRequiredSkillModifier") => Func::OwnerRequiredSkillModifier,
                        Some("EffectStopper") => Func::EffectStopper,
                        other => return Err(format!("line {n}: unknown func {other:?}")),
                    };
                    let m = Modifier {
                        domain,
                        func,
                        modified_attribute: opt(4)?,
                        modifying_attribute: opt(5)?,
                        operation: opt(6)?,
                        skill_type: opt(7)?,
                        group: opt(8)?,
                    };
                    t.modifiers.entry(int(&f, 1, n)?).or_default().push(m);
                }
                _ => {}
            }
        }
        Ok(t)
    }

    pub fn get(&self, type_id: i64) -> Option<&TypeRow> {
        self.types.get(&type_id)
    }

    /// True when the table carries the `fittingUsageChanceAttributeID` column at
    /// all. Without it a booster's side effects cannot be told from its main ones
    /// and would apply for good, so boosters refuse on a table that lacks it.
    pub fn knows_usage_chance(&self) -> bool {
        self.effects
            .values()
            .any(|e| e.usage_chance_attribute.is_some())
    }

    pub fn effect(&self, effect_id: i64) -> Option<&EffectRow> {
        self.effects.get(&effect_id)
    }

    pub fn types(&self) -> impl Iterator<Item = &TypeRow> {
        self.types.values()
    }

    /// `dogmaeffects[effect_id].modifierInfo`; empty when it has none.
    pub fn modifiers(&self, effect_id: i64) -> &[Modifier] {
        self.modifiers.get(&effect_id).map_or(&[], |v| v.as_slice())
    }

    /// The attribute's stacking and clamping columns, [`AttrMeta::PLAIN`]
    /// when the extract has none for it.
    pub fn attr_meta(&self, attribute_id: i64) -> AttrMeta {
        self.meta
            .get(&attribute_id)
            .copied()
            .unwrap_or(AttrMeta::PLAIN)
    }

    /// `dogmaattributes[id].defaultValue`, `0.0` for an unknown attribute.
    pub fn default_value(&self, attribute_id: i64) -> f64 {
        self.defaults.get(&attribute_id).copied().unwrap_or(0.0)
    }

    /// What the client's `GetTypeAttribute` returns: the type's own value,
    /// else the attribute's `defaultValue`, else `0.0` for an attribute the
    /// extract does not define.
    pub fn attribute(&self, type_id: i64, attribute_id: i64) -> f64 {
        self.get(type_id)
            .and_then(|r| r.attribute(attribute_id))
            .or_else(|| self.defaults.get(&attribute_id).copied())
            .unwrap_or(0.0)
    }

    pub fn counts(&self) -> Counts {
        let of = |c: i64| self.types().filter(|r| r.category_id == c).count();
        Counts {
            ships: of(CATEGORY_SHIP),
            modules: of(CATEGORY_MODULE),
            charges: of(CATEGORY_CHARGE),
            drones: of(CATEGORY_DRONE),
            fittable_modules: self
                .types()
                .filter(|r| r.category_id == CATEGORY_MODULE && rack_of(r).is_some())
                .count(),
            effects: self.effects.len(),
            attributes: self.defaults.len(),
            skills: of(CATEGORY_SKILL),
            modifiers: self.modifiers.values().map(Vec::len).sum(),
            commodities: of(CATEGORY_COMMODITY),
            asteroids: of(CATEGORY_ASTEROID),
        }
    }
}

/// The rack a module's slot effect puts it in. `None` for a rig or a
/// subsystem, which this server does not fit.
fn rack_of(r: &TypeRow) -> Option<Rack> {
    if r.has_effect(effect::HI_POWER) {
        Some(Rack::High)
    } else if r.has_effect(effect::MED_POWER) {
        Some(Rack::Medium)
    } else if r.has_effect(effect::LO_POWER) {
        Some(Rack::Low)
    } else {
        None
    }
}

fn hardpoint_of(r: &TypeRow) -> Option<Hardpoint> {
    if r.has_effect(effect::TURRET_FITTED) {
        Some(Hardpoint::Turret)
    } else if r.has_effect(effect::LAUNCHER_FITTED) {
        Some(Hardpoint::Launcher)
    } else {
        None
    }
}

/// **A charge that flies** (R-MISSILE M4): every charge with damage, a
/// `maxVelocity` (37) and an `explosionDelay` (281) -- what a player's
/// launcher launches (its flight is `crate::missile::Charge::of`'s). Here
/// for its launch FX: the guid of the charge's **default** effect,
/// `missileLaunching` (9), `effects.MissileDeployment` [V-data], since the
/// launcher's own `useMissiles` (101) has none (R-MISSILE §1.6). A scan
/// probe is a launcher's charge too, but does no damage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MissileCharge {
    pub type_id: i64,
    /// Empty when the charge has no default effect.
    pub guid: &'static str,
    /// That effect's `isOffensive`: the FX marks the threat.
    pub offensive: bool,
}

/// The table and every view derived from it, shaped as the hand tables are
/// so the existing lookups can hand them out unchanged.
///
/// The views carry `&'static` names and attribute slices because the hand
/// tables were designed as consts. They are leaked once per type per process
/// -- a few megabytes, once, the trade `crate::planet::register_item_types`
/// already makes.
#[derive(Debug)]
pub struct Installed {
    table: Table,
    items: Vec<ItemType>,
    modules: Vec<ModuleType>,
    hulls: Vec<Hull>,
    fits: Vec<HullFit>,
    weapons: Vec<Weapon>,
    charges: Vec<ChargeType>,
    missiles: Vec<MissileCharge>,
    /// Lower-cased name of a **published** type -> index into `items`. The
    /// lowest type id wins a duplicate name.
    by_name: HashMap<String, usize>,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

impl Installed {
    /// Derive every view. Pure: the tests build one from a fixture.
    pub fn build(table: Table) -> Installed {
        let mut strings: HashMap<String, &'static str> = HashMap::new();
        let mut intern = |s: &str| -> &'static str {
            if let Some(v) = strings.get(s) {
                return v;
            }
            let v = leak(s);
            strings.insert(s.to_owned(), v);
            v
        };
        let mut items = Vec::new();
        let mut modules = Vec::new();
        let mut hulls = Vec::new();
        let mut fits = Vec::new();
        let mut weapons = Vec::new();
        let mut charges = Vec::new();
        let mut missiles = Vec::new();
        let mut by_name = HashMap::new();
        for r in table.types() {
            // An entity has a ball and no item: its hull numbers are served
            // here so `destiny::hull::find_in` knows a loaded NPC
            // (NPC-CHASE: it did not, and every one flew at 1 m/s).
            if r.category_id == CATEGORY_ENTITY {
                hulls.push(crate::npc::entity_hull(r, &table));
            }
            // The same for a drone (DRONE-AI): its ball is its own row's,
            // not the 1 m/s fallback.
            if r.category_id == CATEGORY_DRONE {
                hulls.push(crate::drones::row_hull(r, &table));
            }
            // A skill is in the table for its effects (`crate::dogma`); its
            // book is `crate::skill`'s item, not one of these. An entity (an
            // NPC, category 11) is `crate::npc::derive`'s, and nothing a
            // player may be granted.
            // A wormhole or a station type (FX7) is a ball's, never an item.
            // A gas cloud is the one celestial that is both (SOC-B2): the
            // harvested gas is the cloud's own type.
            // The four container groups are celestial too and are items: a
            // cargo container is assembled from a packaged one (#57).
            if matches!(
                r.category_id,
                CATEGORY_SKILL | CATEGORY_ENTITY | CATEGORY_CELESTIAL | CATEGORY_STATION
            ) && r.group_id != crate::mining::GROUP_HARVESTABLE_CLOUD
                && !crate::ship_config::CONTAINER_GROUPS.contains(&r.group_id)
            {
                continue;
            }
            let a = |id: i64| table.attribute(r.type_id, id);
            let name = intern(&r.name);
            if r.published {
                by_name
                    .entry(r.name.to_ascii_lowercase())
                    .or_insert(items.len());
            }
            items.push(ItemType {
                type_id: r.type_id,
                name,
                group_id: r.group_id,
                category_id: r.category_id,
                // A ship or a blueprint is one object; everything else here
                // stacks. The same split `item::SPAWNABLE` makes, row for row.
                singleton: matches!(r.category_id, CATEGORY_SHIP | CATEGORY_BLUEPRINT),
                volume: r.volume,
                capacity: r.capacity,
            });
            match r.category_id {
                CATEGORY_SHIP => {
                    hulls.push(Hull {
                        type_id: r.type_id,
                        mass: r.mass,
                        radius: r.radius as f32,
                        max_velocity: a(attr::MAX_VELOCITY) as f32,
                        agility: a(attr::AGILITY) as f32,
                        shield_capacity: a(attr::SHIELD_CAPACITY),
                        shield_recharge_rate: a(attr::SHIELD_RECHARGE),
                        armor_hp: a(attr::ARMOR_HP),
                        max_target_range: a(attr::MAX_TARGET_RANGE),
                        max_locked_targets: a(attr::MAX_LOCKED_TARGETS),
                        scan_resolution: a(attr::SCAN_RESOLUTION),
                        signature_radius: a(attr::SIGNATURE_RADIUS),
                        warp_speed_au: a(attr::BASE_WARP_SPEED) * a(attr::WARP_SPEED_MULTIPLIER),
                        hp: a(attr::HP),
                        capacitor_capacity: a(attr::CAPACITOR_CAPACITY),
                        capacitor_recharge_rate: a(attr::CAPACITOR_RECHARGE),
                    });
                    fits.push(HullFit {
                        type_id: r.type_id,
                        hi: a(attr::HI_SLOTS) as i64,
                        med: a(attr::MED_SLOTS) as i64,
                        low: a(attr::LOW_SLOTS) as i64,
                        rig: a(attr::RIG_SLOTS) as i64,
                        turrets: a(attr::TURRET_SLOTS) as i64,
                        launchers: a(attr::LAUNCHER_SLOTS) as i64,
                        cpu_output: a(attr::CPU_OUTPUT),
                        power_output: a(attr::POWER_OUTPUT),
                    });
                }
                CATEGORY_MODULE => {
                    let Some(rack) = rack_of(r) else { continue };
                    // The default effect, when it is one a click can start:
                    // `effectCategory` 1 (activation) or 2 (target). Anything
                    // else is a passive module, which is onlined and nothing
                    // more -- the client refuses the click itself (`smb:1084`).
                    // A Jump Portal Generator carries no default effect in
                    // the client's data (its effect list is 12,16,2152 or
                    // 3674, `defaultEffectID` 0): its one activation effect
                    // is the portal's, found in the list.
                    let default = r.default_effect.or_else(|| {
                        r.effects
                            .iter()
                            .copied()
                            .find(|id| crate::jump_portal::is_portal(*id))
                    });
                    let effect = default
                        .and_then(|id| table.effect(id))
                        .filter(|e| matches!(e.category, 1 | 2))
                        .map(|e| Effect {
                            effect_id: e.effect_id,
                            name: intern(&e.name),
                            category: e.category,
                            guid: intern(&e.guid),
                            is_offensive: e.is_offensive,
                            duration_attribute: e.duration_attribute,
                            warp_safe: e.is_warp_safe,
                        });
                    let hardpoint = hardpoint_of(r);
                    modules.push(ModuleType {
                        type_id: r.type_id,
                        name,
                        effect,
                        attributes: Box::leak(r.attributes.clone().into_boxed_slice()),
                        rack,
                        hardpoint,
                    });
                    // **[A]** a weapon is a turret with a damage multiplier:
                    // `crate::combat`'s arithmetic (`volley_loaded`) needs
                    // exactly those two inputs.
                    if hardpoint == Some(Hardpoint::Turret)
                        && let Some(multiplier) = r.attribute(attr::DAMAGE_MULTIPLIER)
                    {
                        let d = damage_of(r);
                        weapons.push(Weapon {
                            type_id: r.type_id,
                            name,
                            damage: (d.total() > 0.0).then_some(d),
                            multiplier,
                        });
                    } else if is_launcher_row(r) {
                        // **A launcher** (M4): `useMissiles` and a charge
                        // group. Its damage is its missile's, always; a
                        // cycle launches the missile (`crate::combat::
                        // on_cycles`) rather than landing a volley.
                        // **[A]** [`LAUNCHER_DAMAGE_MULTIPLIER`] when the row
                        // has no `damageMultiplier` of its own.
                        weapons.push(Weapon {
                            type_id: r.type_id,
                            name,
                            damage: None,
                            multiplier: r
                                .attribute(attr::DAMAGE_MULTIPLIER)
                                .unwrap_or(LAUNCHER_DAMAGE_MULTIPLIER),
                        });
                    }
                }
                CATEGORY_CHARGE => {
                    let d = damage_of(r);
                    if d.total() > 0.0
                        && a(attr::MAX_VELOCITY) > 0.0
                        && a(attr::EXPLOSION_DELAY) > 0.0
                    {
                        let launching = r.default_effect.and_then(|id| table.effect(id));
                        missiles.push(MissileCharge {
                            type_id: r.type_id,
                            guid: launching.map_or("", |e| intern(&e.guid)),
                            offensive: launching.is_some_and(|e| e.is_offensive),
                        });
                    }
                    if d.total() > 0.0 {
                        charges.push(ChargeType {
                            type_id: r.type_id,
                            name,
                            damage: d,
                            charge_size: r.attribute(attr::CHARGE_SIZE).unwrap_or(0.0),
                        });
                    }
                }
                _ => {}
            }
        }
        Installed {
            table,
            items,
            modules,
            hulls,
            fits,
            weapons,
            charges,
            missiles,
            by_name,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn item(&self, type_id: i64) -> Option<&ItemType> {
        by_id(&self.items, type_id, |t| t.type_id)
    }

    /// A **published** type by name, case-insensitively: what `/createitem
    /// rifter` asks. Unpublished types are reachable by id only.
    pub fn item_named(&self, name: &str) -> Option<&ItemType> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.items[i])
    }

    /// Whether `type_id` is a published type, i.e. one a player may be
    /// granted by name.
    pub fn is_published(&self, type_id: i64) -> bool {
        self.table.get(type_id).is_some_and(|r| r.published)
    }

    pub fn module(&self, type_id: i64) -> Option<&ModuleType> {
        by_id(&self.modules, type_id, |t| t.type_id)
    }

    pub fn hull(&self, type_id: i64) -> Option<&Hull> {
        by_id(&self.hulls, type_id, |t| t.type_id)
    }

    pub fn hull_fit(&self, type_id: i64) -> Option<&HullFit> {
        by_id(&self.fits, type_id, |t| t.type_id)
    }

    pub fn weapon(&self, type_id: i64) -> Option<&Weapon> {
        by_id(&self.weapons, type_id, |t| t.type_id)
    }

    pub fn charge(&self, type_id: i64) -> Option<&ChargeType> {
        by_id(&self.charges, type_id, |t| t.type_id)
    }

    pub fn missile(&self, type_id: i64) -> Option<&MissileCharge> {
        by_id(&self.missiles, type_id, |t| t.type_id)
    }

    /// `(shield, armour, hull)` resonances, attribute defaults applied: a
    /// type that carries no resonance row resists nothing (the default is
    /// 1.0), which is the client's own `GetTypeAttribute` answer.
    pub fn resonances(&self, type_id: i64) -> Option<[Damage; 3]> {
        self.table.get(type_id)?;
        let layer = |ids: [i64; 4]| Damage {
            em: self.table.attribute(type_id, ids[0]),
            explosive: self.table.attribute(type_id, ids[1]),
            kinetic: self.table.attribute(type_id, ids[2]),
            thermal: self.table.attribute(type_id, ids[3]),
        };
        Some([
            layer(crate::damage::attr::SHIELD_RESONANCE),
            layer(crate::damage::attr::ARMOR_RESONANCE),
            layer(crate::damage::attr::HULL_RESONANCE),
        ])
    }
}

/// **[A]** a launcher row without a `damageMultiplier` (64) of its own --
/// every one in the client's table -- throws its missile's damage times
/// this. The pilot's `missileDamageMultiplier` (212) is the real factor;
/// its default is 1.0 [V-data] and its skill bonuses are the dogma sheet's
/// (`crate::combat::volley_fitted`). Falsified by the fitting window's
/// per-missile figure for an unskilled pilot disagreeing with the charge's.
pub const LAUNCHER_DAMAGE_MULTIPLIER: f64 = 1.0;

/// A launcher: a module with `useMissiles` (101) and a `chargeGroup1`
/// (604). A probe launcher is one too; its charges do no damage.
fn is_launcher_row(r: &TypeRow) -> bool {
    r.has_effect(effect::USE_MISSILES)
        && r.attribute(crate::modules::attr::CHARGE_GROUP1)
            .is_some_and(|g| g > 0.0)
}

/// The four damage attributes (`emDamage` 114, `explosiveDamage` 116,
/// `kineticDamage` 117, `thermalDamage` 118), absent ones as zero.
fn damage_of(r: &TypeRow) -> Damage {
    let a = |id| r.attribute(id).unwrap_or(0.0);
    Damage {
        em: a(crate::damage::attr::EM_DAMAGE),
        explosive: a(crate::damage::attr::EXPLOSIVE_DAMAGE),
        kinetic: a(crate::damage::attr::KINETIC_DAMAGE),
        thermal: a(crate::damage::attr::THERMAL_DAMAGE),
    }
}

/// Every view is built in ascending type-id order (the table is a
/// `BTreeMap`), so a binary search finds a row.
fn by_id<T>(rows: &[T], type_id: i64, key: impl Fn(&T) -> i64) -> Option<&T> {
    rows.binary_search_by_key(&type_id, key)
        .ok()
        .map(|i| &rows[i])
}

/// The invented table, in the tool's own format: [`TABLE`]'s fixture and
/// the tests'. **Not the client's numbers**: type ids from 990_000_001 up,
/// so that installing it process-wide (the fall-through tests must) cannot
/// change what any other test sees -- except row 34, which shadows a
/// hand-made row on purpose, to prove the hand row wins. One hull, three
/// modules, a charge, a drone, an ore and two tags.
pub const FIXTURE: &str = "\
# a comment, then a blank line

attribute\t9\t0.0\thp
attribute\t37\t0.0\tmaxVelocity
attribute\t109\t1.0\tkineticDamageResonance
attribute\t110\t1.0\tthermalDamageResonance
attribute\t111\t1.0\texplosiveDamageResonance
attribute\t113\t1.0\temDamageResonance
attribute\t267\t1.0\tarmorEmDamageResonance
attribute\t268\t1.0\tarmorExplosiveDamageResonance
attribute\t269\t1.0\tarmorKineticDamageResonance
attribute\t270\t1.0\tarmorThermalDamageResonance
attribute\t271\t1.0\tshieldEmDamageResonance
attribute\t272\t1.0\tshieldExplosiveDamageResonance
attribute\t273\t1.0\tshieldKineticDamageResonance
attribute\t274\t1.0\tshieldThermalDamageResonance
attribute\t552\t100.0\tsignatureRadius
type\t990000001\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;271=0.0;272=0.5;273=0.6;274=0.8;479=600000.0;482=260.0;564=640.0;600=5.0;1137=3.0;1281=1.0\tFixture Frigate
type\t990000002\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=83.0\tFixture Autocannon
type\t990000003\t38\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t13,16\t30=3.0;50=15.0\tFixture Shield Extender
type\t990000004\t773\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t2663\t50=0.0\tFixture Rig
type\t990000005\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t114=7.0;117=3.0;128=1.0\tFixture Ammo S
type\t990000006\t100\t18\t0\t1\t5.0\t0.0\t3000.0\t25.0\t0\t\t\tFixture Unpublished Drone
type\t990000007\t99701\t25\t1\t100\t0.1\t0.0\t1e+35\t1.0\t0\t\t790=1.0\tFixture Ore
type\t990000008\t99702\t17\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Copper Tag
type\t990000009\t99702\t17\t0\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Retired Tag
type\t34\t18\t8\t1\t1\t99.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Shadow Of Tritanium
effect\t34\t2\t1\t51\t6\t54\t158\t160\tprojectileFired\teffects.ProjectileFired
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
some-future-record\tthat this build does not know
";

// ---------------------------------------------------------------------------
// The content table (CP58a)
// ---------------------------------------------------------------------------

/// The registration row in `content_tables::TABLES` (coordinator-only).
/// The table is an [`Installed`]: the extract and every view derived from it.
pub const TABLE: TableSpec = TableSpec {
    name: "typedata",
    load,
    fixture: || Box::new(fixture()) as Boxed,
};

/// [`FIXTURE`], built. Every call leaks its few interned names (see
/// [`Installed`]); a test registry builds one per `Content::fixture()`.
pub fn fixture() -> Installed {
    Installed::build(Table::parse(FIXTURE).expect("the typedata fixture parses"))
}

/// The bundle's `types/typedata.tsv`, or [`FIXTURE`] without a bundle.
///
/// A bundle without the file is an empty table: the hand rows alone, which
/// is what a server with no typedata file had before the bundle. A file
/// that is there but malformed, or holds no ship or no module, is an `Err`
/// naming it, for the reason `cli.rs` refuses the same file.
fn load(src: &Sources) -> Result<Boxed, String> {
    let Some(bundle) = src.bundle else {
        return Ok(Box::new(fixture()));
    };
    src.note(Origin::Bundle);
    let Some(path) = bundle.path(crate::content::TYPEDATA) else {
        return Ok(Box::new(Installed::build(Table::default())));
    };
    Ok(Box::new(from_file(&path)?))
}

/// One extract file, built; an error names the file.
fn from_file(path: &std::path::Path) -> Result<Installed, String> {
    let table = Table::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let c = table.counts();
    if c.ships == 0 || c.modules == 0 {
        return Err(format!(
            "{}: no ships or no modules. Re-extract with tools/up.sh",
            path.display()
        ));
    }
    Ok(Installed::build(table))
}

/// `--typedata FILE`: replace the bundle's (or the fixture's) table with
/// this file's, before the world is built (CP59b, as
/// `planet::override_in`). Returns the startup line. Refused as [`TABLE`]'s
/// `load` refuses a bundle file: malformed, or no ship or no module.
pub fn override_in(content: &mut Content, path: &std::path::Path) -> Result<String, String> {
    let t = from_file(path).map_err(|e| format!("--typedata {e}"))?;
    let c = t.table.counts();
    content.insert(TABLE.name, t);
    Ok(format!(
        "types: {} ships, {} modules ({} fittable), {} charges, {} drones, \
         {} effects from {}",
        c.ships,
        c.modules,
        c.fittable_modules,
        c.charges,
        c.drones,
        c.effects,
        path.display()
    ))
}

/// The content table. Panics, as `Content::get` does, when [`TABLE`] is
/// not registered.
pub fn installed_in(content: &Content) -> &Installed {
    content.get::<Installed>()
}

/// One loaded type as an item type (`item::find_in`'s fall-through), from `content`.
pub fn item_in(content: &Content, type_id: i64) -> Option<&ItemType> {
    installed_in(content).item(type_id)
}

/// A published type, by id or by name (`item::find_type_in`'s fall-through), from `content`.
pub fn grantable_in<'c>(content: &'c Content, needle: &str) -> Option<&'c ItemType> {
    grantable_from(installed_in(content), needle)
}

/// One loaded module (`modules::find_in`'s fall-through), from `content`.
pub fn module_in(content: &Content, type_id: i64) -> Option<&ModuleType> {
    installed_in(content).module(type_id)
}

/// One loaded hull's ball numbers (`destiny::hull::find_in`'s fall-through), from `content`.
pub fn hull_in(content: &Content, type_id: i64) -> Option<&Hull> {
    installed_in(content).hull(type_id)
}

/// One loaded hull's fitting numbers (`fitting::hull_in`'s fall-through), from `content`.
pub fn hull_fit_in(content: &Content, type_id: i64) -> Option<&HullFit> {
    installed_in(content).hull_fit(type_id)
}

/// The capital ship groups [V]: `eveCfg.GetShipGroupByClassType()
/// [GROUP_CAPITALSHIPS]` -- Carrier 547, Command Carrier 5120, Capital
/// Industrial 883, Dreadnought 485, Titan 30, Supercarrier 659, Force
/// Auxiliary 1538, Lancer Dreadnought 4594 (`inventorycommon/const.py`).
/// **Freighters (513) and Jump Freighters (902) are industrial class, not
/// capital**, by that same table: the module-size gate applies to them.
pub const CAPITAL_GROUPS: [i64; 8] = [547, 5120, 883, 485, 30, 659, 1538, 4594];

/// `categoryStructure` [V]: a structure is exempt from the module-size gate.
pub const CATEGORY_STRUCTURE: i64 = 65;

/// Whether a ship group is a capital one ([`CAPITAL_GROUPS`]).
pub fn is_capital_group(group_id: i64) -> bool {
    CAPITAL_GROUPS.contains(&group_id)
}

/// Whether a ship type is a capital hull, by its group in `content`.
pub fn is_capital_hull_in(content: &Content, type_id: i64) -> bool {
    item_in(content, type_id)
        .is_some_and(|t| t.category_id == CATEGORY_SHIP && is_capital_group(t.group_id))
}

/// What a hull lacks to be flown as an ordinary ship end to end: boarding
/// needs the type, the ball needs mass, hit points and a warp speed, the
/// fitting window needs slots. Empty for a complete hull; used by the
/// capital-hull check against a real bundle and by the tests.
pub fn hull_gaps_in(content: &Content, type_id: i64) -> Vec<&'static str> {
    let mut gaps = Vec::new();
    if item_in(content, type_id).is_none_or(|t| t.category_id != CATEGORY_SHIP) {
        return vec!["not a ship type"];
    }
    match hull_in(content, type_id) {
        None => gaps.push("no ball numbers"),
        Some(h) => {
            if h.mass <= 0.0 {
                gaps.push("no mass");
            }
            if h.hp <= 0.0 && h.armor_hp <= 0.0 {
                gaps.push("no hit points");
            }
            if h.warp_speed_au <= 0.0 {
                gaps.push("no warp speed");
            }
            if h.max_velocity <= 0.0 {
                gaps.push("no speed");
            }
        }
    }
    match hull_fit_in(content, type_id) {
        None => gaps.push("no fitting numbers"),
        Some(f) => {
            if f.hi + f.med + f.low == 0 {
                gaps.push("no slots");
            }
        }
    }
    gaps
}

/// One loaded gun (`combat::weapon_in`'s fall-through), from `content`.
pub fn weapon_in(content: &Content, type_id: i64) -> Option<&Weapon> {
    installed_in(content).weapon(type_id)
}

/// One loaded charge (`combat::charge_in`'s fall-through), from `content`.
pub fn charge_in(content: &Content, type_id: i64) -> Option<&ChargeType> {
    installed_in(content).charge(type_id)
}

/// One loaded flying charge (M4: what a launcher cycle launches), from `content`.
pub fn missile_in(content: &Content, type_id: i64) -> Option<&MissileCharge> {
    installed_in(content).missile(type_id)
}

/// One loaded hull's resonances (`damage::Profile::from_hull_in`'s), from `content`.
pub fn resonances_in(content: &Content, type_id: i64) -> Option<[Damage; 3]> {
    installed_in(content).resonances(type_id)
}

/// Any attribute of any loaded type in `content`, defaults applied
/// (`None` when the type is not in the table).
pub fn attribute_in(content: &Content, type_id: i64, attribute_id: i64) -> Option<f64> {
    attribute_from(installed_in(content), type_id, attribute_id)
}

/// The type's **own** value for an attribute, no default applied: `None`
/// when the type does not carry it (or is not in the table). The client's
/// `GetCanFitShipGroups` asks exactly this ("is the attribute there").
pub fn own_attribute_in(content: &Content, type_id: i64, attribute_id: i64) -> Option<f64> {
    installed_in(content)
        .table()
        .get(type_id)?
        .attribute(attribute_id)
}

fn grantable_from<'t>(t: &'t Installed, needle: &str) -> Option<&'t ItemType> {
    match needle.parse::<i64>() {
        Ok(id) => t.item(id).filter(|i| t.is_published(i.type_id)),
        Err(_) => t.item_named(needle),
    }
}

fn attribute_from(t: &Installed, type_id: i64, attribute_id: i64) -> Option<f64> {
    t.table.get(type_id)?;
    Some(t.table.attribute(type_id, attribute_id))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_loader_parses_what_the_tool_writes() {
        let t = Table::parse(FIXTURE).unwrap();
        let c = t.counts();
        assert_eq!(
            c,
            Counts {
                ships: 1,
                modules: 3,
                charges: 2,
                drones: 1,
                fittable_modules: 2,
                effects: 2,
                attributes: 15,
                skills: 0,
                modifiers: 0,
                commodities: 2,
                asteroids: 1,
            }
        );
        let f = t.get(990_000_001).unwrap();
        assert_eq!(f.name, "Fixture Frigate");
        assert_eq!((f.group_id, f.category_id), (25, 6));
        assert!(f.published);
        assert_eq!(
            (f.volume, f.capacity, f.mass, f.radius),
            (20000.0, 130.0, 1_200_000.0, 33.0)
        );
        assert_eq!(f.default_effect, None);
        assert_eq!(f.effects, vec![5000]);
        assert_eq!(f.attribute(37), Some(355.0));
        let gun = t.get(990_000_002).unwrap();
        assert_eq!(gun.default_effect, Some(34));
        assert_eq!(t.get(990_000_005).unwrap().portion_size, 100);
        assert!(!t.get(990_000_006).unwrap().published);
        let e = t.effect(34).unwrap();
        assert_eq!(
            (e.category, e.is_offensive, e.duration_attribute),
            (2, true, 51)
        );
        assert_eq!((e.discharge_attribute, e.range_attribute), (6, 54));
        assert_eq!(
            (e.falloff_attribute, e.tracking_speed_attribute),
            (158, 160)
        );
        assert_eq!(
            (e.name.as_str(), e.guid.as_str()),
            ("projectileFired", "effects.ProjectileFired")
        );
        assert_eq!(t.effect(16).unwrap().guid, "");
    }

    #[test]
    fn the_dogma_records_parse_and_skills_are_not_items() {
        let t = Table::parse(
            "attrmeta\t37\t0\t1\t2033\t0\t4\n\
             modifier\t7\tshipID\tLocationRequiredSkillModifier\t51\t460\t6\t3302\t\n\
             modifier\t7\titemID\tItemModifier\t326\t64\t-1\t\t\n\
             modifier\t8\ttarget\tEffectStopper\t\t\t\t\t\n\
             type\t990000101\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t7\t292=5.0\tA Skill\n",
        )
        .unwrap();
        let m = t.attr_meta(37);
        assert!(!m.stackable && m.high_is_good);
        assert_eq!(
            (m.max_attribute, m.min_attribute, m.data_type),
            (2033, 0, 4)
        );
        assert_eq!(t.attr_meta(9), AttrMeta::PLAIN);
        let mods = t.modifiers(7);
        assert_eq!(mods.len(), 2);
        assert_eq!(
            mods[0],
            Modifier {
                domain: Domain::Ship,
                func: Func::LocationRequiredSkillModifier,
                modified_attribute: Some(51),
                modifying_attribute: Some(460),
                operation: Some(6),
                skill_type: Some(3302),
                group: None,
            }
        );
        assert_eq!(
            (mods[1].domain, mods[1].operation),
            (Domain::Item, Some(-1))
        );
        assert_eq!(t.modifiers(8)[0].func, Func::EffectStopper);
        assert_eq!(t.modifiers(8)[0].modified_attribute, None);
        assert!(t.modifiers(9).is_empty());
        assert_eq!((t.counts().skills, t.counts().modifiers), (1, 3));
        let i = Installed::build(t);
        assert!(
            i.item(990_000_101).is_none(),
            "a skill is crate::skill's item"
        );
        assert!(i.table().get(990_000_101).is_some());
        for bad in [
            "modifier\t7\tnowhereID\tItemModifier\t1\t2\t0\t\t",
            "modifier\t7\titemID\tMagicModifier\t1\t2\t0\t\t",
            "modifier\t7\titemID\tItemModifier\tx\t2\t0\t\t",
            "attrmeta\t37\t2\t1\t0\t0\t4",
        ] {
            assert!(Table::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_missing_attribute_reads_as_its_default_and_presence_is_kept() {
        let t = Table::parse(FIXTURE).unwrap();
        // The frigate carries no armour resonance row: the default.
        assert_eq!(t.attribute(990_000_001, 267), 1.0);
        // Nor a hull hp default it does not need: its own value.
        assert_eq!(t.attribute(990_000_001, 9), 310.0);
        // Neither a row nor a default.
        assert_eq!(t.attribute(990_000_001, 123_456), 0.0);
        // The row itself does not grow a defaulted attribute.
        assert_eq!(t.get(990_000_001).unwrap().attribute(267), None);
    }

    #[test]
    fn a_malformed_line_is_an_error() {
        for bad in [
            "type\tnot-a-number\t25\t6\t1\t1\t0\t0\t0\t1\t0\t\t\tX",
            "type\t1\t25\t6\tyes\t1\t0\t0\t0\t1\t0\t\t\tX",
            "type\t1\t25\t6\t1\t1\t0\t0\t0\t1\t0\t\t37:5\tX",
            "type\t1\t25\t6\t1\t1\t0\t0\t0\t1\t0\t\t37=inf\tX",
            "type\t1\t25\t6\t1\t1\t0\t0\t0\t1\t0\t\t37=1",
            "attribute\t37",
            "effect\t34\t2\t1",
        ] {
            assert!(Table::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn a_hull_and_its_fitting_numbers_come_from_the_attributes() {
        let i = Installed::build(Table::parse(FIXTURE).unwrap());
        let h = i.hull(990_000_001).unwrap();
        assert_eq!(h.mass, 1_200_000.0);
        assert_eq!(h.radius, 33.0);
        assert_eq!(h.max_velocity, 355.0);
        assert_eq!(h.agility, 3.1);
        assert_eq!(
            (h.shield_capacity, h.shield_recharge_rate),
            (420.0, 600_000.0)
        );
        assert_eq!((h.armor_hp, h.hp), (440.0, 310.0));
        assert_eq!((h.max_target_range, h.max_locked_targets), (21_000.0, 5.0));
        assert_eq!((h.scan_resolution, h.signature_radius), (640.0, 100.0));
        assert_eq!(h.warp_speed_au, 5.0);
        assert_eq!(
            (h.capacitor_capacity, h.capacitor_recharge_rate),
            (260.0, 150_000.0)
        );
        let f = i.hull_fit(990_000_001).unwrap();
        assert_eq!(
            (f.hi, f.med, f.low, f.rig, f.turrets, f.launchers),
            (3, 3, 4, 3, 2, 1)
        );
        assert_eq!((f.cpu_output, f.power_output), (125.0, 45.0));
        assert!(i.hull(990_000_002).is_none(), "a module is not a hull");
    }

    #[test]
    fn an_effect_row_carries_is_warp_safe_when_the_column_is_there() {
        let t = Table::parse(
            "effect\t990001\t1\t0\t73\t0\t0\t0\t0\tfixtureA\t\t1\n\
             effect\t990002\t1\t0\t73\t0\t0\t0\t0\tfixtureB\t\t0\n\
             effect\t990003\t1\t0\t73\t0\t0\t0\t0\tfixtureC\t\n",
        )
        .unwrap();
        let warp_safe = |id| t.effect(id).unwrap().is_warp_safe;
        assert_eq!(warp_safe(990_001), Some(true));
        assert_eq!(warp_safe(990_002), Some(false));
        assert_eq!(warp_safe(990_003), None, "an old extract has no column");
    }

    #[test]
    fn a_module_gets_its_rack_hardpoint_effect_and_own_attributes() {
        let i = Installed::build(Table::parse(FIXTURE).unwrap());
        let gun = i.module(990_000_002).unwrap();
        assert_eq!(gun.rack, Rack::High);
        assert_eq!(gun.hardpoint, Some(Hardpoint::Turret));
        let e = gun.effect.unwrap();
        assert_eq!(
            (e.effect_id, e.name, e.category),
            (34, "projectileFired", 2)
        );
        assert_eq!(
            (e.guid, e.is_offensive, e.duration_attribute),
            ("effects.ProjectileFired", true, 51)
        );
        assert_eq!(gun.duration_ms(), 2500);
        assert_eq!((gun.cpu(), gun.power()), (4.0, 2.0));
        assert!(crate::modules::takes_charges(gun));
        let ext = i.module(990_000_003).unwrap();
        assert_eq!(
            (ext.rack, ext.hardpoint, ext.effect),
            (Rack::Medium, None, None)
        );
        assert!(!crate::modules::takes_charges(ext));
        assert!(ext.attribute(267).is_none(), "no defaults on a module row");
        assert!(i.module(990_000_004).is_none(), "a rig has no rack");
    }

    #[test]
    fn weapons_charges_and_resonances() {
        let i = Installed::build(Table::parse(FIXTURE).unwrap());
        let w = i.weapon(990_000_002).unwrap();
        assert_eq!((w.multiplier, w.damage), (2.5, None));
        assert!(i.weapon(990_000_003).is_none());
        let c = i.charge(990_000_005).unwrap();
        assert_eq!(
            (c.damage.em, c.damage.kinetic, c.damage.thermal),
            (7.0, 3.0, 0.0)
        );
        assert_eq!(c.charge_size, 1.0);
        assert!(i.charge(34).is_none(), "no damage, no charge");
        let [shield, armour, hull] = i.resonances(990_000_001).unwrap();
        assert_eq!(
            shield,
            Damage {
                em: 0.0,
                explosive: 0.5,
                kinetic: 0.6,
                thermal: 0.8
            }
        );
        assert_eq!(armour, Damage::UNRESISTED);
        assert_eq!(hull, Damage::UNRESISTED);
        assert!(i.resonances(1).is_none());
    }

    /// **A launcher is a weapon whose damage is its missile's** (M4), and
    /// a charge that flies keeps its default effect's guid for the launch
    /// FX. Invented rows: a launcher, a missile, and a probe (no damage),
    /// which flies nowhere.
    #[test]
    fn a_launcher_is_a_weapon_and_a_flying_charge_keeps_its_guid() {
        let text = "\
effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t
effect\t9\t2\t1\t0\t0\t0\t0\t0\tmissileLaunching\teffects.FixtureLaunch
type\t990470001\t99471\t7\t1\t1\t5.0\t0.6\t0.0\t1.0\t101\t12,16,40,101\t51=4000.0;604=99472.0\tFixture Launcher
type\t990470002\t99472\t8\t1\t100\t0.015\t0.0\t700.0\t300.0\t9\t9\t37=3000.0;117=20.0;281=5000.0\tFixture Missile
type\t990470003\t99472\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t37=3000.0;281=5000.0\tFixture Probe
";
        let i = Installed::build(Table::parse(text).unwrap());
        let launcher = i.module(990_470_001).unwrap();
        assert_eq!(launcher.hardpoint, Some(Hardpoint::Launcher));
        assert!(crate::modules::is_launcher(launcher));
        let w = i.weapon(990_470_001).expect("a launcher is a weapon");
        assert_eq!((w.damage, w.multiplier), (None, LAUNCHER_DAMAGE_MULTIPLIER));
        let m = i.missile(990_470_002).expect("the missile flies");
        assert_eq!((m.guid, m.offensive), ("effects.FixtureLaunch", true));
        assert!(i.missile(990_470_003).is_none(), "no damage, no missile");
    }

    /// **A portal generator has no default effect in the client's data**
    /// (`defaultEffectID` 0, effects `12,16,<portal>`): the loader still
    /// gives it the portal effect, or `jump_portal` finds no generator and
    /// answers `JumpNoPortal` to a fitted, online one. Invented ids.
    #[test]
    fn a_portal_generator_gets_its_portal_effect_without_a_default() {
        let text = format!(
            "effect\t{p}\t1\t0\t1002\t6\t0\t0\t0\tfixturePortal\teffects.JumpPortal\n\
             type\t990480001\t99480\t7\t1\t1\t10.0\t0.0\t1.0\t1.0\t0\t12,16,{p}\t6=10.0\tFixture Portal\n\
             type\t990480002\t99481\t7\t1\t1\t10.0\t0.0\t1.0\t1.0\t0\t12,16\t6=10.0\tFixture Plain\n",
            p = crate::jump_portal::EFFECT_JUMP_PORTAL
        );
        let i = Installed::build(Table::parse(&text).unwrap());
        let e = i.module(990_480_001).unwrap().effect.expect("an effect");
        assert_eq!(e.effect_id, crate::jump_portal::EFFECT_JUMP_PORTAL);
        assert!(i.module(990_480_002).unwrap().effect.is_none());
    }

    #[test]
    fn items_stack_except_ships_and_names_are_published_only() {
        let i = Installed::build(Table::parse(FIXTURE).unwrap());
        assert!(i.item(990_000_001).unwrap().singleton);
        let ammo = i.item(990_000_005).unwrap();
        assert!(!ammo.singleton);
        assert_eq!(
            (ammo.group_id, ammo.category_id, ammo.volume),
            (83, 8, 0.0025)
        );
        assert_eq!(i.item_named("fixture ammo s").unwrap().type_id, 990_000_005);
        assert!(i.item_named("Fixture Unpublished Drone").is_none());
        assert!(i.item(990_000_006).is_some(), "by id it is still a type");
    }

    /// CP12 (plan §6.2, review finding 19): the asteroid and commodity
    /// categories install as stackable items, grantable by id and by name
    /// when published -- and, like every other category, not when not.
    #[test]
    fn asteroid_and_commodity_rows_install_as_stackable_grantable_items() {
        let i = Installed::build(Table::parse(FIXTURE).unwrap());
        for (id, cat, name) in [
            (990_000_007, CATEGORY_ASTEROID, "fixture ore"),
            (990_000_008, CATEGORY_COMMODITY, "FIXTURE COPPER TAG"),
        ] {
            let t = i.item(id).unwrap();
            assert_eq!(t.category_id, cat);
            assert!(!t.singleton, "category {cat} stacks");
            assert_eq!(i.item_named(name).unwrap().type_id, id);
            assert!(i.module(id).is_none() && i.hull(id).is_none());
            assert!(i.charge(id).is_none() && i.weapon(id).is_none());
        }
        assert_eq!(i.item(990_000_007).unwrap().volume, 0.1);
        assert!(i.item_named("Fixture Retired Tag").is_none(), "unpublished");
        let c = Content::fixture();
        assert_eq!(
            crate::item::find_type_in(&c, "Fixture Ore")
                .unwrap()
                .type_id,
            990_000_007
        );
        assert_eq!(
            crate::item::find_type_in(&c, "990000008").unwrap().name,
            "Fixture Copper Tag"
        );
        assert!(
            crate::item::find_type_in(&c, "990000009").is_none(),
            "unpublished"
        );
        assert!(!crate::item::find_in(&c, 990_000_009).unwrap().singleton);
    }

    /// The fall-through: hand rows first, the extract second.
    #[test]
    fn the_existing_lookups_fall_through_to_the_loaded_table() {
        let c = Content::fixture();
        assert_eq!(
            crate::item::find_in(&c, 990_000_005).unwrap().name,
            "Fixture Ammo S"
        );
        assert_eq!(
            crate::item::find_type_in(&c, "FIXTURE FRIGATE")
                .unwrap()
                .type_id,
            990_000_001
        );
        assert_eq!(
            crate::item::find_type_in(&c, "990000002").unwrap().name,
            "Fixture Autocannon"
        );
        assert!(
            crate::item::find_type_in(&c, "990000006").is_none(),
            "unpublished"
        );
        assert!(crate::item::find_type_in(&c, "Fixture Unpublished Drone").is_none());
        assert_eq!(
            crate::modules::find_in(&c, 990_000_002).unwrap().rack,
            Rack::High
        );
        assert_eq!(
            crate::destiny::hull::find_in(&c, 990_000_001).unwrap().mass,
            1_200_000.0
        );
        assert_eq!(
            crate::fitting::hull_in(&c, 990_000_001).unwrap().cpu_output,
            125.0
        );
        // The hand row shadows the fixture's row 34.
        let t = crate::item::find_in(&c, 34).unwrap();
        assert_eq!((t.name, t.category_id), ("Tritanium", 4));
        assert_eq!(
            crate::item::find_type_in(&c, "34").unwrap().name,
            "Tritanium"
        );
        // Resonances reach the damage profile.
        let h = crate::destiny::hull::find_in(&c, 990_000_001).unwrap();
        let p = crate::damage::Profile::from_hull_in(&c, h);
        assert_eq!(p.shield_resonance.explosive, 0.5);
        assert_eq!(attribute_in(&c, 990_000_001, 1281), Some(1.0));
        assert_eq!(attribute_in(&c, 1, 1281), None);
    }
    /// Without a bundle the table is the fixture; the registered spec's
    /// fixture is the same table.
    #[test]
    fn without_a_bundle_the_content_table_is_the_fixture() {
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = (TABLE.load)(&src).expect("loads");
        let loaded = loaded.downcast_ref::<Installed>().expect("an Installed");
        let fixed = (TABLE.fixture)();
        let fixed = fixed.downcast_ref::<Installed>().expect("an Installed");
        assert_eq!(loaded.table(), fixed.table());
        assert_eq!(loaded.table(), &Table::parse(FIXTURE).unwrap());
    }

    /// The registration line (`content_tables::TABLES`) puts it in every
    /// test registry's `Content`.
    #[test]
    fn the_fixture_content_carries_the_typedata_table() {
        let c = Content::fixture();
        assert_eq!(installed_in(&c).table(), &Table::parse(FIXTURE).unwrap());
    }

    /// The bundle's file: a good one builds, a malformed one or one with no
    /// ship is an error naming the file.
    #[test]
    fn the_typedata_flag_replaces_the_content_table() {
        // CP59b: `--typedata` reaches the table `reg.content()` reads.
        let dir = std::env::temp_dir().join(format!("eve-typedata-cp59b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.tsv");
        std::fs::write(&good, FIXTURE).unwrap();
        let mut c = Content::default();
        c.insert(TABLE.name, Installed::build(Table::default()));
        assert!(hull_in(&c, 990_000_001).is_none());
        let line = override_in(&mut c, &good).expect("overrides");
        assert!(line.contains(&good.display().to_string()), "{line}");
        assert!(
            hull_in(&c, 990_000_001).is_some(),
            "the file's hull answers"
        );
        let bad = dir.join("bad.tsv");
        std::fs::write(&bad, "type\t1\n").unwrap();
        let e = override_in(&mut c, &bad).expect_err("refused");
        assert!(e.starts_with("--typedata ") && e.contains("bad.tsv"), "{e}");
        assert!(
            hull_in(&c, 990_000_001).is_some(),
            "a refused file changes nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bundle_file_builds_and_a_bad_one_is_an_error_naming_it() {
        let dir = std::env::temp_dir().join(format!("eve-typedata-cp58a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.tsv");
        std::fs::write(&good, FIXTURE).unwrap();
        let i = from_file(&good).expect("the fixture file builds");
        assert_eq!(i.table(), &Table::parse(FIXTURE).unwrap());
        let shipless = dir.join("shipless.tsv");
        std::fs::write(
            &shipless,
            "type\t990000003\t38\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t\tX\n",
        )
        .unwrap();
        let bad = dir.join("bad.tsv");
        std::fs::write(&bad, "attribute\t37\n").unwrap();
        for p in [&shipless, &bad, &dir.join("absent.tsv")] {
            let e = from_file(p).expect_err("refused");
            assert!(e.contains(&p.display().to_string()), "{e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// FX7: a blueprint installs as a singleton item; a station type and a
    /// wormhole type are no items (not grantable by id or name, published or
    /// not), while their rows and attributes stay readable. Invented rows.
    #[test]
    fn a_blueprint_is_a_singleton_and_stations_and_wormholes_are_no_items() {
        let t = Table::parse(
            "type\t990000201\t99105\t9\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Blueprint
type\t990000202\t99015\t3\t1\t1\t0.0\t0.0\t0.0\t42000.0\t0\t\t\tFixture Station
type\t990000203\t988\t2\t1\t1\t0.0\t0.0\t0.0\t3000.0\t0\t\t1382=60.0\tFixture Hole
type\t990000204\t99300\t20\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t379=1.0\tFixture Implant
",
        )
        .unwrap();
        let i = Installed::build(t);
        assert!(i.item(990_000_201).unwrap().singleton, "a blueprint");
        assert!(!i.item(990_000_204).unwrap().singleton, "an implant stacks");
        assert!(grantable_from(&i, "Fixture Blueprint").is_some());
        for (id, name) in [
            (990_000_202, "Fixture Station"),
            (990_000_203, "Fixture Hole"),
        ] {
            assert!(i.item(id).is_none(), "{name} is no item");
            assert!(grantable_from(&i, name).is_none(), "{name} by name");
            assert!(
                grantable_from(&i, &id.to_string()).is_none(),
                "{name} by id"
            );
            assert!(i.table().get(id).is_some(), "{name} is still in the table");
        }
        assert_eq!(i.table().get(990_000_202).unwrap().radius, 42_000.0);
        assert_eq!(attribute_from(&i, 990_000_203, 1382), Some(60.0));
    }
}
