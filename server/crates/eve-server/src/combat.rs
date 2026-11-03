//! The glue between a module cycle and a damage volley: who hit whom, for how much, and what the clients are told.
//!
//! Owned by the P2.2b lane (`docs/lanes/damage-death.md`). Spec:
//! `docs/re/damage-death.md`; the model is `crate::damage`, which knows
//! nothing about connections, and this is the half that does.
//!
//! **One input, four outputs.** The input is `modules::Cycle` -- "this
//! module's effect cycled at `at` against `target_id`" -- drained by [`tick`]
//! from the world clock right after `modules::tick` produced it. For a cycle
//! of a weapon that throws something -- its loaded charge's damage, or its
//! own for the civilian guns ([`volley_loaded_in`]) -- against a ship in space, the
//! victim's [`ShipDamage`] takes the volley and four things go out
//! (`docs/re/damage-death.md` §0.1 steps 4-7):
//!
//! 1. **the bars**, `OnDamageStateChange` in a `DoDestinyUpdate`, to every
//!    client whose park holds the victim (`destiny::DamageStateChange`);
//! 2. **the rings**, one `OnModuleAttributeChange` per layer that moved, to
//!    the victim alone, `ownerID` = the victim's **character** ([`RingUpdate`]);
//! 3. **the combat log**, `OnDamageMessage` to the shooter and to the victim,
//!    two different dicts ([`CombatMessage`]);
//! 4. **a death**, when the hull reaches zero: the wreck, the terminal
//!    removal and the pilot's capsule in one batch, the locks released with
//!    `'Exploding'`, the dead ship gone from the store and each item in it
//!    dropped into the wreck or destroyed, the wreck a store item in the
//!    system, and the pilot moved into the capsule by a `shipid` session
//!    change ([`kill`]).
//!
//! **And loot** (P2.4): a wreck lists its contents to anyone in its system,
//! a take into the active ship's hold is [`plan_loot`] (reached through
//! `fitting::plan_to_cargo`), and [`looted`] turns a looted-out wreck empty on
//! every screen.
//!
//! **And consequences** (`crate::crimewatch`): every volley is aggression --
//! [`fire`] starts the weapons and log-off timers -- and a take without loot
//! rights is theft ([`looted`]). Crimewatch's state lives here
//! ([`Combat::crimewatch`]) and runs on [`tick`].
//!
//! **Everything leaves on the clock queue** (`Registry::schedule`), never
//! `Registry::push`: the tick is not a payload turn, and `pending` is drained
//! only by a turn -- a hit would otherwise wait until the victim happened to
//! click something. `Push::restamp` binds the Destiny stamp as it leaves.
//!
//! Both gauge families come from one [`ShipDamage`] per ship, so they cannot
//! disagree (`crate::damage`'s header, §1.4).

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::damage::{
    self, BlueTime, Damage, DamageMessage, MessageValue, Profile, RingChange, ShipDamage, Shot,
};
use crate::destiny::{self, DamageState};
use crate::modules::Cycle;
use crate::notify::Push;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// One weapon type: what one cycle throws, before resistances.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weapon {
    pub type_id: i64,
    pub name: &'static str,
    /// The module's **own** damage (attributes 114/116/117/118), or `None`
    /// when its damage lives on a charge we do not model.
    pub damage: Option<Damage>,
    /// `damageMultiplier` (64).
    pub multiplier: f64,
}

// ---------------------------------------------------------------------------
// Weapons, charges and wrecks: the content's, not baked (CP58d, O3)
// ---------------------------------------------------------------------------
//
// **The hand tables are retired** (W18 CP58d, owner decision O3, ruling 3):
// every turret and charge is `crate::typedata`'s view of the client's own
// `typedogma` rows, read through `reg.content()`, and every hull's wreck is
// the bundle's `types/market.tsv` `wreck_type_id` ([`WRECK_TABLE`]). Without
// a bundle the content is the invented fixture, so `--content` is needed to
// *play* and never to *test*. Ids and protocol constants stay in code.

/// One weapon by type id, from the content (W18 CP58d, ruling 3): every
/// turret with a `damageMultiplier` in the bundle's type table
/// (`crate::typedata::weapon_in`), then, with no bundle only, the invented
/// [`FIXTURE_WEAPONS`].
pub fn weapon_in(content: &Content, type_id: i64) -> Option<&Weapon> {
    crate::typedata::weapon_in(content, type_id).or_else(|| {
        if content.is_bundle() {
            None
        } else {
            FIXTURE_WEAPONS.iter().find(|w| w.type_id == type_id)
        }
    })
}

/// One charge type by id, from the content, as [`weapon_in`]: then
/// [`FIXTURE_CHARGES`] with no bundle.
pub fn charge_in(content: &Content, type_id: i64) -> Option<&ChargeType> {
    crate::typedata::charge_in(content, type_id).or_else(|| {
        if content.is_bundle() {
            None
        } else {
            FIXTURE_CHARGES.iter().find(|c| c.type_id == type_id)
        }
    })
}

/// The weapons a server with no bundle can fire: every gun of
/// `crate::modules::FIXTURE`, its **invented** multiplier and own damage
/// as that table has them (`tests::the_weapon_table_agrees_with_the_module_table`
/// holds the two together). The baked rows are retired (O3); a gun whose
/// damage lives on its charge carries none of its own.
pub const FIXTURE_WEAPONS: &[Weapon] = &[
    fixture_weapon(454, "Fixture Beam Laser", None, 4.0),
    fixture_weapon(484, "Fixture Projectile Turret S", None, 2.5),
    fixture_weapon(486, "Fixture Projectile Turret M", None, 3.0),
    fixture_weapon(
        3634,
        "Fixture Civilian Pulse",
        Some([3.5, 0.0, 0.0, 1.5]),
        1.5,
    ),
    fixture_weapon(
        3636,
        "Fixture Civilian Autocannon",
        Some([0.0, 3.5, 1.5, 0.0]),
        1.5,
    ),
    fixture_weapon(
        3638,
        "Fixture Civilian Railgun",
        Some([0.0, 0.0, 3.5, 1.5]),
        1.5,
    ),
    fixture_weapon(
        3640,
        "Fixture Civilian Blaster",
        Some([0.0, 0.0, 1.5, 3.5]),
        1.5,
    ),
];

const fn fixture_weapon(
    type_id: i64,
    name: &'static str,
    damage: Option<[f64; 4]>,
    multiplier: f64,
) -> Weapon {
    Weapon {
        type_id,
        name,
        damage: match damage {
            Some([em, explosive, kinetic, thermal]) => Some(Damage {
                em,
                explosive,
                kinetic,
                thermal,
            }),
            None => None,
        },
        multiplier,
    }
}

/// The charges a server with no bundle can load, by the type ids
/// `/createitem` grants (`crate::item::SPAWNABLE`), with **invented**
/// damage; `chargeSize` 1 to fit the fixture's small guns.
pub const FIXTURE_CHARGES: &[ChargeType] = &[
    ChargeType {
        type_id: 184,
        name: "Fixture Round A",
        damage: Damage {
            em: 0.0,
            explosive: 0.0,
            kinetic: 3.0,
            thermal: 8.0,
        },
        charge_size: 1.0,
    },
    ChargeType {
        type_id: 185,
        name: "Fixture Round B",
        damage: Damage {
            em: 7.0,
            explosive: 3.0,
            kinetic: 1.0,
            thermal: 0.0,
        },
        charge_size: 1.0,
    },
];

/// One charge type: what it adds to a gun's cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChargeType {
    pub type_id: i64,
    pub name: &'static str,
    /// `emDamage` 114, `explosiveDamage` 116, `kineticDamage` 117,
    /// `thermalDamage` 118.
    pub damage: Damage,
    /// `chargeSize` (128): must equal the gun's own for the round to fit.
    pub charge_size: f64,
}

/// What one cycle of `type_id` loaded with `charge_type_id` throws,
/// pre-resistance, or `None` if it throws nothing we can compute.
///
/// **The client's own turret arithmetic**: `_GetTurretDamagePerShot`
/// (`shipfitting/fittingDogmaLocationUtil.pyc:292-303`) [V] takes the
/// damage from the **charge** when one is loaded (`if chargeKey is not
/// None`, `:293-294`) and from the **module** only when none is (`:296`),
/// and multiplies it by the module's `damageMultiplier` (`:298`, `:302`).
/// So a loaded charge *replaces* the module's own numbers rather than adding
/// to them, and a gun whose damage lives on its charge throws nothing empty.
/// That is the fitting window's number, so `[I]` it is also the server's; no
/// skills, no ship bonuses yet (`[A]`, falsified by a combat log that
/// disagrees with the fitting window's per-shot figure for the same fit).
///
/// This is the **pre-resistance, pre-application** volley. Whether it lands,
/// and for how much of it, is [`application`]'s (DOC-0, behind
/// [`Combat::turret_application`]).
pub fn volley_loaded_in(
    content: &Content,
    type_id: i64,
    charge_type_id: Option<i64>,
) -> Option<Damage> {
    throw(
        weapon_in(content, type_id)?,
        charge_type_id.map(|c| charge_in(content, c).copied()),
    )
}

/// [`volley_loaded_in`] with nothing loaded: what a gun throws on its own
/// damage alone, and nothing for a gun whose damage lives on its charge.
#[cfg(test)]
pub fn volley_in(content: &Content, type_id: i64) -> Option<Damage> {
    volley_loaded_in(content, type_id, None)
}

/// `_GetTurretDamagePerShot`'s product. `charge` is `None` for an empty
/// gun and `Some(None)` for a round that is not a charge.
fn throw(w: &Weapon, charge: Option<Option<ChargeType>>) -> Option<Damage> {
    let d = match charge {
        Some(c) => c?.damage,
        None => w.damage?,
    };
    Some(Damage {
        em: d.em * w.multiplier,
        explosive: d.explosive * w.multiplier,
        kinetic: d.kinetic * w.multiplier,
        thermal: d.thermal * w.multiplier,
    })
}

/// [`volley_loaded_in`] for a gun **fitted to a ship**, with the ship's dogma
/// applied (`crate::dogma`): the same `_GetTurretDamagePerShot` arithmetic
/// on the **modified** values -- the loaded charge's damage attributes as
/// the charge's dogma item has them, times the module's modified
/// `damageMultiplier` (Surgical Strike, the turret skill, the hull bonus,
/// gyrostabilizers). Exactly [`volley_loaded_in`] when the ship has no sheet.
pub fn volley_fitted(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    charge_type_id: Option<i64>,
) -> Option<Damage> {
    let base = volley_loaded_in(w.content(), type_id, charge_type_id)?;
    let Some(sheet) = crate::dogma::sheet(w, ship_id) else {
        return Some(base);
    };
    // A launcher has no `damageMultiplier` of its own (M4): its factor is
    // the pilot's `missileDamageMultiplier` as the sheet has it (K-MSKILL:
    // ballistic control systems, rigs, implants), and its missile's damage
    // is the sheet's.
    let multiplier = if is_launcher_type(w.content(), type_id) {
        crate::missile_skills::launcher_multiplier(w, &sheet, module_id)
            .or_else(|| weapon_in(w.content(), type_id).map(|t| t.multiplier))?
    } else {
        match sheet.module(module_id, crate::damage::attr::DAMAGE_MULTIPLIER_ID) {
            Some(m) => m,
            None => return Some(base),
        }
    };
    let d = |id: i64, unmodified: f64| match charge_type_id {
        // The charge on the sheet is the one loaded *now*; a cycle recorded
        // with another type (the round fired was the stack's last) keeps
        // that type's own number.
        Some(c) if sheet_charge_is(&sheet, w, ship_id, module_id, c) => {
            sheet.charge(module_id, id).unwrap_or(unmodified)
        }
        Some(_) => unmodified,
        None => sheet.module(module_id, id).unwrap_or(unmodified),
    };
    let raw = match charge_type_id {
        Some(c) => charge_in(w.content(), c)?.damage,
        None => weapon_in(w.content(), type_id)?.damage?,
    };
    Some(Damage {
        em: d(crate::damage::attr::EM_DAMAGE, raw.em) * multiplier,
        explosive: d(crate::damage::attr::EXPLOSIVE_DAMAGE, raw.explosive) * multiplier,
        kinetic: d(crate::damage::attr::KINETIC_DAMAGE, raw.kinetic) * multiplier,
        thermal: d(crate::damage::attr::THERMAL_DAMAGE, raw.thermal) * multiplier,
    })
}

/// Whether `type_id` is a missile launcher (`crate::modules::is_launcher`).
fn is_launcher_type(content: &Content, type_id: i64) -> bool {
    crate::modules::find_in(content, type_id).is_some_and(crate::modules::is_launcher)
}

/// Whether the charge on `sheet` for this module is of type `charge_type_id`.
fn sheet_charge_is(
    sheet: &crate::dogma::Sheet,
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    charge_type_id: i64,
) -> bool {
    sheet
        .charge(module_id, crate::damage::attr::EM_DAMAGE)
        .is_some()
        && w.item(module_id)
            .and_then(|m| crate::modules::loaded(w.store(), ship_id, m.flag_id))
            .is_some_and(|c| c.type_id == charge_type_id)
}

/// Every hull's wreck: `(hull typeID -> wreck typeID)`, the `types` table's
/// own `wreckTypeID` column as the bundle's `types/market.tsv` carries it
/// (`schema.md`, CP12) [V-data]. A hull with none (the capsule) dies the
/// capsule's death and leaves no wreck (`damage::Death::Capsule`).
///
/// The content table [`WRECK_TABLE`] (CP58d, O3): it retires the hand
/// table, so every hull the bundle knows leaves its own wreck.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wrecks {
    by_hull: BTreeMap<i64, i64>,
}

impl Wrecks {
    /// The wreck a hull leaves, if the table names one.
    pub fn of(&self, hull_type_id: i64) -> Option<i64> {
        self.by_hull.get(&hull_type_id).copied()
    }

    /// Every `(hull, wreck)` pair, ascending by hull.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.by_hull.iter().map(|(h, w)| (*h, *w))
    }

    /// A table of these pairs: the fixture's, and a test's own.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (i64, i64)>) -> Wrecks {
        Wrecks {
            by_hull: pairs.into_iter().collect(),
        }
    }

    /// `types/market.tsv` data lines: every ship row (category 6) with a
    /// `wreck_type_id`. A malformed line is an error naming the file and
    /// the line, for `crate::typedata`'s reason: a hull whose wreck went
    /// missing dies without one, silently.
    pub fn parse(lines: &[String]) -> Result<Wrecks, String> {
        let (file, _) = crate::reprocess::MARKET;
        let mut by_hull = BTreeMap::new();
        for (n, line) in lines.iter().enumerate() {
            let f: Vec<&str> = line.split('\t').collect();
            let int = |i: usize| -> Result<Option<i64>, String> {
                match f.get(i).map(|v| v.trim()) {
                    Some("") => Ok(None),
                    Some(v) => v
                        .parse::<i64>()
                        .map(Some)
                        .map_err(|_| format!("{file}: data line {}: column {i}", n + 1)),
                    None => Err(format!("{file}: data line {}: column {i} missing", n + 1)),
                }
            };
            let (Some(type_id), Some(category)) = (int(0)?, int(2)?) else {
                return Err(format!("{file}: data line {}: no type or category", n + 1));
            };
            if category != crate::typedata::CATEGORY_SHIP {
                continue;
            }
            if let Some(wreck) = int(11)? {
                by_hull.insert(type_id, wreck);
            }
        }
        Ok(Wrecks { by_hull })
    }
}

/// The wrecks a server with no bundle leaves: `(hull, wreck)` by **type
/// id** only -- every hull of `destiny::hull::FIXTURE` but the capsule, each
/// to a wreck `crate::item::WRECK_TYPES` holds, so a kill's
/// `Edit::Create` is a type the store knows. Ids are protocol (O3); there
/// is no number here.
pub const FIXTURE_WRECKS: &[(i64, i64)] = &[
    (587, 26550),
    (594, 26528),
    (597, 26484),
    (601, 26510),
    (603, 26506),
    (621, 26495),
    (629, 26539),
    (639, 26536),
    (641, 26514),
    (642, 26470),
    (648, 26507),
    (657, 26529),
    (1944, 26485),
    (2006, 26473),
    (16227, 26491),
    (16240, 26518),
    (16242, 26540),
    (24702, 26535),
];

/// The registration row in `content_tables::TABLES` (coordinator-only).
pub const WRECK_TABLE: TableSpec = TableSpec {
    name: "wrecks",
    load: load_wrecks,
    fixture: || Box::new(Wrecks::from_pairs(FIXTURE_WRECKS.iter().copied())) as Boxed,
};

/// The bundle's `types/market.tsv`, or the fixture without a bundle. A
/// bundle without the file has no wrecks: every hull dies without one.
fn load_wrecks(src: &Sources) -> Result<Boxed, String> {
    let Some(bundle) = src.bundle else {
        return Ok((WRECK_TABLE.fixture)());
    };
    src.note(Origin::Bundle);
    let (file, header) = crate::reprocess::MARKET;
    Ok(Box::new(Wrecks::parse(&bundle.data_lines(file, header)?)?))
}

/// The wreck a hull leaves, if the content names one.
pub fn wreck_type_in(content: &Content, hull_type_id: i64) -> Option<i64> {
    content.get::<Wrecks>().of(hull_type_id)
}

/// `hitQuality` for a hit: `3`, whose label is `UI/Inflight/HitQuality3` =
/// "Hits" [V] (`tools/loctable.py`). `[A]` that a plain hit is the honest
/// word while every shot lands for full damage; falsified by nothing until
/// tracking exists, when the quality should follow the roll.
pub const HIT_QUALITY: u8 = 3;

/// `groupWreck` (`inventorycommon/const.py:1053`) **[V]**.
pub const GROUP_WRECK: i64 = 186;

/// `flagNone`: the flag of an item lying in space. `[A]` -- the store has no
/// in-space address of its own yet, and 0 is what every other location-only
/// item would carry. Falsified by a wreck the solar-system `invMgr` lists in
/// the wrong place.
pub const FLAG_IN_SPACE: i64 = 0;

/// How long a wreck stays. `[A]` two hours, TQ's behaviour as players know
/// it; nothing in the client says. Falsified by nothing a client can see
/// except a wreck that outlives the evening.
/// Blue time is 10,000 ticks to the millisecond (`damage::DGM_TAU_CONSTANT`).
pub const WRECK_LIFETIME: BlueTime = 2 * 60 * 60 * 1_000 * 10_000;

/// The flag an item lying in a wreck carries. `[A]` `flagNone`: the client
/// never reads it -- `invCtrl.ItemWreck` keeps `BaseInvContainer`'s
/// `locationFlag = None` (`invControllers.py:80`), so its listing is a bare
/// `List()` and `IsItemHere` is `item.locationID == wreckID` alone
/// (`:1499`) **[V]**.
pub const FLAG_IN_WRECK: i64 = 0;

/// The chance one item in a dead ship survives into the wreck. `[A]` one in
/// two, per stack and per fitted module, whole or not at all -- TQ's rule as
/// players know it; nothing in the client says, because the roll is the
/// server's. Rolled by [`survives`], so it is reproducible.
pub const DROP_CHANCE: f64 = 0.5;

/// How close a ship must be to take from a wreck, surface to surface:
/// `const.maxCargoContainerTransferDistance` = 2500 m (`appConst.py:1726`)
/// **[V]**, the client's own fallback in
/// `BaseCelestialContainer.GetOperationalDistance` (`invControllers.py:1525`)
/// for a type with no `maxOperationalDistance` -- and no wreck type has one
/// (`tools/dogma.py type 26550`: four attributes, none of them that) **[V]**.
/// The client refuses beyond it before calling (`IsInRange`, `:1516`) and
/// approaches first (`menusvc.OpenCargo:3588`) **[V]**.
pub const LOOT_RANGE: f64 = 2_500.0;

/// What the server adds to [`LOOT_RANGE`] before refusing. `[A]`: the client
/// measures on its own extrapolated park and we on ours, and a take the
/// client allowed at 2,499 m should not bounce off a few metres of drift.
pub const LOOT_RANGE_SLACK: f64 = 500.0;

/// A wreck in space, as the server remembers it. **The item is in the
/// store** (`wreck_id` is its item id, owner the victim, location the
/// solar system, [`FLAG_IN_SPACE`]); this is the half the store cannot hold
/// -- where it is, what it came from, and when it goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wreck {
    pub wreck_id: i64,
    pub type_id: i64,
    pub solar_system_id: i64,
    pub position: [f64; 3],
    /// The ship that died, `launcherID`.
    pub ship_id: i64,
    pub owner_id: i64,
    pub corp_id: i64,
    /// The owner's fleet **when the wreck was made** (`lootRights[2]`), a
    /// snapshot: a fleet joined later grants nothing, and leaving keeps it.
    /// `[A]` the snapshot (EvEJS does it; the client reads whatever the slim
    /// says); falsifier: a TQ capture where a late joiner can take freely.
    pub fleet_id: Option<i64>,
    /// `lootRights[3]`: the owner gave it up (`beyonce.CmdAbandonLoot`).
    pub abandoned: bool,
    /// When [`tick`] removes it, on the blue clock.
    pub expires_at: BlueTime,
}

/// Every ship's health and every wreck.
///
/// **Not persisted**, like `modules::Modules` and `targeting::Locks`, and
/// for their reason: a restart takes every ship out of space. A ship's record
/// is forgotten when it leaves space ([`tick`]) -- except across a jump, where
/// the pilot is only between two parks. **Docking recharges the shield and
/// the capacitor but repairs nothing**: armour and structure damage is stashed
/// as a `hulldmg` row at the dock and seeded back at the undock
/// (`crate::repair`), and only the station's repair shop mends it (GAP-REPAIR).
/// The godma prime (`Registry::capsule_dogma_row`) reads this record, so rings and bars
/// agree on arrival.
///
/// **Wrecks are store items, so they outlive this** -- and a restart forgets
/// where they were. The first [`tick`] after a start therefore destroys every
/// wreck item it has no record of (`swept`): `[A]`, the same rule as ships,
/// "a restart takes everything out of space".
#[derive(Debug, Clone, PartialEq)]
pub struct Combat {
    /// DOC-0: turrets roll to hit. Default **on** (owner, 2026-10-28);
    /// `EVE_TURRET_APPLICATION=0` or [`Combat::set_turret_application`]
    /// turns it off, and every turret shot hits for the full amount again.
    turret_off: bool,
    ships: BTreeMap<i64, ShipDamage>,
    wrecks: BTreeMap<i64, Wreck>,
    swept: bool,
    /// GAP-LOOT: per NPC, the damage each character (a drone's owner for a
    /// drone) has done to it, to name the wreck's owner. Dropped when the
    /// NPC dies or [`tick`] finds it gone.
    npc_damage: BTreeMap<i64, BTreeMap<i64, f64>>,
    crimewatch: crate::crimewatch::Crimewatch,
    /// The last system's characters, for [`audience`]. Not state.
    audience: Option<Audience>,
}

impl Default for Combat {
    fn default() -> Combat {
        Combat {
            turret_off: std::env::var("EVE_TURRET_APPLICATION").is_ok_and(|v| v == "0"),
            ships: BTreeMap::new(),
            wrecks: BTreeMap::new(),
            swept: false,
            npc_damage: BTreeMap::new(),
            crimewatch: crate::crimewatch::Crimewatch::default(),
            audience: None,
        }
    }
}

impl Combat {
    /// Whether turrets roll to hit (DOC-0 `Features::turret_application`).
    pub fn turret_application(&self) -> bool {
        !self.turret_off
    }

    /// Switch turret application on or off.
    pub fn set_turret_application(&mut self, on: bool) {
        self.turret_off = !on;
    }

    /// A ship's health, if anything has ever hit it while in space.
    pub fn ship(&self, ship_id: i64) -> Option<&ShipDamage> {
        self.ships.get(&ship_id)
    }

    /// A ship's health, to restore (K-LOGI's reps, `crate::remote_rep`).
    pub(crate) fn ship_mut(&mut self, ship_id: i64) -> Option<&mut ShipDamage> {
        self.ships.get_mut(&ship_id)
    }

    /// Give a ship a health record, as if it had been hit (tests).
    #[cfg(test)]
    pub(crate) fn set_ship(&mut self, ship_id: i64, record: ShipDamage) {
        self.ships.insert(ship_id, record);
    }

    /// Give a ship a health record as it leaves a station with damage
    /// (`crate::repair::on_undock`), the non-test twin of `set_ship`.
    pub(crate) fn seed_ship(&mut self, ship_id: i64, record: ShipDamage) {
        self.ships.insert(ship_id, record);
    }

    /// Take a ship's record out as it docks (`crate::repair::on_dock`).
    pub(crate) fn take_ship(&mut self, ship_id: i64) -> Option<ShipDamage> {
        self.ships.remove(&ship_id)
    }

    /// Every wreck, by id.
    pub fn wrecks(&self) -> impl Iterator<Item = &Wreck> {
        self.wrecks.values()
    }

    /// One wreck in space, by its item id.
    pub fn wreck(&self, wreck_id: i64) -> Option<&Wreck> {
        self.wrecks.get(&wreck_id)
    }

    /// Every pilot's crimewatch timers and flags. Here rather than on
    /// `Registry` because combat is what moves them, and because it keeps
    /// the timers' lifetime this struct's: not persisted.
    pub fn crimewatch(&self) -> &crate::crimewatch::Crimewatch {
        &self.crimewatch
    }

    pub(crate) fn crimewatch_mut(&mut self) -> &mut crate::crimewatch::Crimewatch {
        &mut self.crimewatch
    }
}

/// One wreck as a ball and its slim, as the kill batch, a later `SetState`
/// and a looted-out [`destiny::SlimItemChange`] send it. `isEmpty` is read
/// off the store.
pub fn wreck_parts(w: &Registry, wr: &Wreck) -> (destiny::Ball, destiny::WreckSlim) {
    let mut ball = destiny::wreck_ball(wr.wreck_id, wr.position);
    // NW-2: an anchorable unit's ball is free until it is anchored.
    crate::anchor::shape_ball(w, wr.wreck_id, &mut ball);
    (
        ball,
        destiny::WreckSlim {
            item_id: wr.wreck_id,
            type_id: wr.type_id,
            owner_id: wr.owner_id,
            corp_id: wr.corp_id,
            launcher_id: wr.ship_id,
            is_empty: w.store().contents_of(wr.wreck_id).next().is_none(),
            fleet_id: wr.fleet_id,
            abandoned: wr.abandoned,
        },
    )
}

/// Every wreck in a system, for `destiny::bootstrap_for`.
pub fn wrecks_in(w: &Registry, solar_system_id: i64) -> Vec<(destiny::Ball, destiny::WreckSlim)> {
    w.combat()
        .wrecks
        .values()
        .filter(|wr| wr.solar_system_id == solar_system_id)
        .map(|wr| wreck_parts(w, wr))
        .collect()
}

/// The bars to send for `ship_id` at `now`: its record's, regenerating at
/// its own `tau`, or [`DamageState::FULL`] for a ship nothing has hit.
///
/// What `destiny::bootstrap_for` and `destiny::on_ballpark_bind` put in
/// `SetState` and `AddBalls2`, so a pilot who arrives mid-fight sees the
/// truth rather than a full ship.
pub fn damage_state(w: &Registry, ship_id: i64, now: BlueTime) -> DamageState {
    w.combat()
        .ship(ship_id)
        .map_or(DamageState::FULL, |s| DamageState::from(s.bars(now)))
}

/// **The world clock's hook**, beside `modules::tick` and after it: every
/// cycle started since the last tick, applied in order, then every record of
/// a ship that has left space forgotten, then every wreck whose time is up
/// removed, then every crimewatch timer that changed phase told
/// (`crimewatch::tick`).
pub fn tick(w: &mut Registry, now: BlueTime) {
    if !w.combat().swept {
        sweep_orphan_wrecks(w);
    }
    let cycles = w.modules_mut().take_cycles();
    on_cycles(w, cycles, now);
    despawn_wrecks(w, now);
    // #82: the MTUs' cycles.
    crate::mtu::tick(w, now);
    // #172: the anchoring units' changes.
    crate::anchor::tick(w, now);
    // **A ship in a jump is out of the roster but not out of space.** Gate,
    // wormhole and jump-drive jumps all take the pilot out of the roster
    // for the tunnel (`jump::arm_at` is the shared leg) and put them back at
    // the bind; the record rides along, and `service::capsule_dogma_row`
    // primes the destination's rings from it. A jumper who loses their
    // connection has their leg dropped by `jump::tick`, and the record goes
    // on the next tick like any other ship's.
    let in_tunnel: std::collections::BTreeSet<i64> = w
        .jumps()
        .characters()
        .filter_map(|c| w.character(c).map(|c| c.ship_id))
        .collect();
    let gone: Vec<i64> = w
        .combat()
        .ships
        .keys()
        .copied()
        .filter(|ship| w.roster().by_ship(*ship).is_none() && !in_tunnel.contains(ship))
        .collect();
    for ship in gone {
        w.combat_mut().ships.remove(&ship);
    }
    if !w.combat().npc_damage.is_empty() {
        let dead: Vec<i64> = w
            .combat()
            .npc_damage
            .keys()
            .copied()
            .filter(|n| w.npcs().get(*n).is_none())
            .collect();
        for n in dead {
            w.combat_mut().npc_damage.remove(&n);
        }
    }
    crate::crimewatch::tick(w, now);
}

/// Turn module cycles into volleys, oldest first.
///
/// A cycle is a shot when all of these hold, and is otherwise ignored here
/// (it is still a cycle -- the module lane already paid for it):
///
/// - it throws something ([`volley_loaded_in`]): the charge it cycled with, or a
///   damage source of the module's own;
/// - it names a target, and the target is **a ship in space** -- a rock or a
///   celestial is not a `ShipDamage`;
/// - the shooter is in space, in the target's system, and is not the target.
///
/// The lock is not re-checked: `modules::advance` already ends a cycle whose
/// lock is gone, before it is recorded.
///
/// **A launcher's cycle launches a missile** (R-MISSILE M4) instead of
/// landing a volley: the same checks, then `crate::missile::launch` with
/// the missile the cycle fired (`crate::missile::Charge::of`) and the
/// volley it will carry ([`volley_fitted`]). A launcher whose charge does
/// not fly, or does no damage (a probe), does nothing here.
pub fn on_cycles(w: &mut Registry, cycles: Vec<Cycle>, now: BlueTime) {
    // **A smartbomb's cycle is a volley at everything in its field**
    // (BUBBLES-SB, `crate::smartbomb`): it names no target, so it is taken
    // out of the batch and each of its victims is hit in turn.
    let (bombs, cycles): (Vec<Cycle>, Vec<Cycle>) = cycles
        .into_iter()
        .partition(|c| crate::smartbomb::is_smartbomb(c.effect_id));
    for c in &bombs {
        crate::smartbomb::on_cycle(w, c, now);
    }
    // NW-9a: a legacy doomsday's cycle books its delayed strike.
    let (legacy, cycles): (Vec<Cycle>, Vec<Cycle>) = cycles
        .into_iter()
        .partition(|c| crate::superweapon::is_legacy(c.effect_id));
    for c in &legacy {
        crate::superweapon::on_legacy_cycle(w, c, now);
    }
    on_cycles_with(w, cycles, now, crate::par::threads(), COMBAT_PAR_MIN);
}

/// Units in one batch from which their plans are made in parallel (~3 us a
/// unit: a dogma sheet read, four attributes, the aim; PA-8c). [A] untuned:
/// tune on `brawl_bench`.
const COMBAT_PAR_MIN: usize = 128;

/// What one volley unit does, worked out from the world as it stands (PA-8c).
enum UnitPlan {
    /// Nothing: no damage, no target, a shooter who left, a charge that
    /// does not fly.
    Skip,
    /// A launcher's missile.
    Launch(Box<LaunchPlan>),
    /// A volley to land (boxed: the variants differ a lot in size).
    Fire(Box<FirePlan>),
    /// A volley on an anchorable deployable (`crate::anchor`, NW-3).
    Deployable { target: i64, points: f64 },
}

struct LaunchPlan {
    c: Cycle,
    launch_modules: Vec<i64>,
    damage: Damage,
    charge: crate::missile::Charge,
}

struct FirePlan {
    shot: Shot,
    shooter: i64,
    victim: crate::space_state::Presence,
    banked: bool,
}

/// [`on_cycles`] with explicit threads and gate (tests, benches).
///
/// **Plan, then apply.** [`plan_unit`] is a pure function of the registry;
/// the units are planned in parallel from the state before the first volley
/// when there are `gate` of them, and applied in order. A plan is used only
/// while the roster has not been written since (a kill or an arrival in this
/// batch could change what a later unit sees); otherwise the unit is planned
/// again, serially, at its own turn, which is what the old loop did for every
/// unit. So the result is the old loop's at any thread count.
/// [A] nothing a volley writes besides the roster changes what a plan reads
/// (the shooter's dogma, items, the world seed); falsified by the golden test
/// in `combat/par_tests.rs` failing on a hook that does.
fn on_cycles_with(
    w: &mut Registry,
    cycles: Vec<Cycle>,
    now: BlueTime,
    threads: usize,
    gate: usize,
) {
    let units = bank_units(w, cycles);
    let mut planned: Option<Vec<Option<UnitPlan>>> = if threads > 1 && units.len() >= gate {
        match crate::par::par_map_with(threads, &units, 1, |u| plan_unit(w, u)) {
            Ok(v) => Some(v.into_iter().map(Some).collect()),
            Err(e) => {
                crate::errln!("[par] combat plan: {e}; serial");
                None
            }
        }
    } else {
        None
    };
    let revision0 = w.roster().revision();
    for (i, unit) in units.iter().enumerate() {
        let plan = match planned.as_mut() {
            Some(v) if w.roster().revision() == revision0 => v[i].take(),
            _ => None,
        }
        .unwrap_or_else(|| plan_unit(w, unit));
        apply_unit(w, plan, now);
    }
}

/// What one volley unit does (see [`on_cycles`] for the rules); reads only.
fn plan_unit(w: &Registry, unit: &(Cycle, Vec<Cycle>)) -> UnitPlan {
    let (lead, members) = unit;
    let banked = members.len() > 1 || bank_master(w, lead).is_some();
    let c = *lead;
    let launcher = is_launcher_type(w.content(), c.type_id);
    // **A bank is one unit** (`OnDamageMessage.isBanked`): its members'
    // volleys are summed, and the sum is rolled and applied once.
    let mut damage = Damage::ZERO;
    let mut launch_modules: Vec<i64> = Vec::new();
    for m in members {
        let Some(d) = volley_fitted(w, m.ship_id, m.module_id, m.type_id, m.charge_type_id) else {
            continue;
        };
        damage = Damage {
            em: damage.em + d.em,
            explosive: damage.explosive + d.explosive,
            kinetic: damage.kinetic + d.kinetic,
            thermal: damage.thermal + d.thermal,
        };
        launch_modules.push(m.module_id);
    }
    if launch_modules.is_empty() {
        return UnitPlan::Skip;
    }
    let Some(target) = c.target_id else {
        return UnitPlan::Skip;
    };
    if target == c.ship_id {
        return UnitPlan::Skip;
    }
    let Some(victim) = w.roster().by_ship(target).copied() else {
        // A deployable (NW-2) takes turret, missile and drone fire as
        // points, no resists **[A]**. A missile arrives at once, with no
        // flight **[A]**. A turret's range counts (optimal and falloff
        // through the same roll as a ship's); its tracking does not, as a
        // fixed target crosses no line of sight **[A]**: falsifier, a gun
        // that misses a bubble it should hit, or one that hits at 3x its
        // falloff.
        let unit = w
            .combat()
            .wreck(target)
            .filter(|wr| crate::anchor::is_unit(w, wr.wreck_id))
            .and_then(|wr| {
                let s = w.roster().get(c.character_id)?;
                (s.ship_id == c.ship_id && s.solar_system_id == wr.solar_system_id)
                    .then_some((*s, wr.position))
            });
        let Some((shooter, at)) = unit else {
            return UnitPlan::Skip;
        };
        let multiplier = if launcher || !w.combat().turret_application() {
            1.0
        } else {
            turret_numbers(w, &c).map_or(1.0, |app| {
                let aim = crate::drone_skills::Aim {
                    distance: crate::space_state::distance(shooter.motion.position, at),
                    transversal: 0.0,
                    signature: 1.0,
                };
                application(&app, aim, c.module_id, c.at, w.world().seed).multiplier
            })
        };
        return UnitPlan::Deployable {
            target,
            points: damage.total() * multiplier,
        };
    };
    let Some(shooter) = w.roster().get(c.character_id).copied() else {
        return UnitPlan::Skip;
    };
    if shooter.ship_id != c.ship_id || shooter.solar_system_id != victim.solar_system_id {
        return UnitPlan::Skip;
    }
    if launcher {
        // A launcher's damage is its missile's: a charge that cannot fly
        // (a probe has no damage anyway) launches nothing.
        // With the pilot's dogma on it (K-MSKILL): flight time, velocity
        // and explosion as the sheet has them.
        let charge = c
            .charge_type_id
            .and_then(|t| crate::missile_skills::charge_for(w, c.ship_id, c.module_id, t));
        return match charge {
            Some(charge) => UnitPlan::Launch(Box::new(LaunchPlan {
                c,
                launch_modules,
                damage,
                charge,
            })),
            None => UnitPlan::Skip,
        };
    }
    // One roll for the bank (the master's), its members' damage summed.
    let verdict = if w.combat().turret_application() {
        turret_verdict(w, &c, &shooter, &victim)
    } else {
        None
    };
    let (damage, hit_quality) = match verdict {
        Some(v) => (crate::drone_skills::scaled(damage, v.multiplier), v.quality),
        None => (damage, HIT_QUALITY),
    };
    UnitPlan::Fire(Box::new(FirePlan {
        shot: Shot {
            at: c.at,
            attacker_ship_id: c.ship_id,
            weapon_item_id: c.module_id,
            weapon_type_id: c.type_id,
            target_ship_id: target,
            damage,
            hit_quality,
        },
        shooter: c.character_id,
        victim,
        banked,
    }))
}

/// Do what [`plan_unit`] decided: the only part of a unit that writes.
fn apply_unit(w: &mut Registry, plan: UnitPlan, now: BlueTime) {
    match plan {
        UnitPlan::Skip => {}
        UnitPlan::Launch(l) => {
            let LaunchPlan {
                c,
                launch_modules,
                damage,
                charge,
            } = *l;
            // A refusal (the target left the system) is a missile that
            // was never launched; the round is spent all the same.
            // A bank launches ONE missile carrying every member's
            // volley, its slim naming every launcher (`launchModules`,
            // as the client's missile ball takes a list).
            let _ = crate::missile::launch(
                w,
                crate::missile::Launch {
                    shooter: c.character_id,
                    launcher_ship: c.ship_id,
                    launcher_item_id: c.module_id,
                    launch_modules,
                    charge,
                    volley: damage,
                    target: c.target_id.unwrap_or_default(),
                    // `crate::modules` sends the launch FX at the cycle,
                    // with the charge as `otherTypeID` (`MissileFx`).
                    fx: None,
                },
                c.at,
            );
        }
        UnitPlan::Fire(f) => fire_banked(w, f.shot, f.shooter, f.victim, f.banked, now),
        UnitPlan::Deployable { target, points } => {
            let _ = crate::anchor::hit(w, target, points, now);
        }
    }
}

/// The pre-PA-8c loop, kept as the oracle of the golden test.
#[cfg(test)]
pub(crate) fn on_cycles_legacy(w: &mut Registry, cycles: Vec<Cycle>, now: BlueTime) {
    for unit in bank_units(w, cycles) {
        match plan_unit(w, &unit) {
            UnitPlan::Fire(f) => {
                fire_banked_in(w, f.shot, f.shooter, f.victim, f.banked, now, false);
            }
            other => apply_unit(w, other, now),
        }
    }
}

/// The master of the bank `c`'s module is in, when it is in one.
fn bank_master(w: &Registry, c: &Cycle) -> Option<i64> {
    crate::weapon_bank::master_in(w, c.ship_id, c.module_id)
}

/// The batch's cycles as volley units, oldest first: a bank's cycles in one
/// batch are one unit led by the master's (or, with no master cycle in the
/// batch, the first member's); any other cycle is a unit of its own.
/// **[A]** a bank's members start their cycles in the same tick (one click,
/// one `modules::activate` each, same duration); a member that straggles
/// into a later batch fires as a one-member unit (still `isBanked`).
fn bank_units(w: &Registry, cycles: Vec<Cycle>) -> Vec<(Cycle, Vec<Cycle>)> {
    let mut units: Vec<(Cycle, Vec<Cycle>)> = Vec::new();
    let mut by_master: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for c in cycles {
        let Some(master) = bank_master(w, &c) else {
            units.push((c, vec![c]));
            continue;
        };
        match by_master.get(&(c.ship_id, master)) {
            Some(&i) => {
                if c.module_id == master {
                    units[i].0 = c;
                }
                units[i].1.push(c);
            }
            None => {
                by_master.insert((c.ship_id, master), units.len());
                units.push((c, vec![c]));
            }
        }
    }
    units
}

// ---------------------------------------------------------------------------
// Turret application (DOC-0 K-TURRET)
// ---------------------------------------------------------------------------

/// `maxRange` (54): a turret's **optimal** [V-data].
pub const ATTR_OPTIMAL: i64 = 54;

/// `[A]` a turret's hard range cut, in falloffs past its optimal: nothing
/// lands beyond `optimal + RANGE_CUT_FALLOFFS * falloff` (the chance there is
/// already `0.5^9`, under 0.2 %). Falsified by a retail log with a hit beyond
/// three falloffs.
pub const RANGE_CUT_FALLOFFS: f64 = 3.0;

/// What a turret brings to a shot: its range, tracking and the signature it
/// is built to hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Application {
    pub optimal: f64,
    pub falloff: f64,
    pub tracking: f64,
    pub sig_resolution: f64,
}

impl Application {
    /// The chance to hit: [`crate::drone_skills::chance_to_hit`]'s formula
    /// (the client's `0.5 ^ ((angular * sigRes / (tracking * sig))^2 +
    /// (max(0, d - optimal) / falloff)^2)`), 0 past the hard cut.
    pub fn chance(&self, aim: crate::drone_skills::Aim) -> f64 {
        if aim.distance > self.optimal + RANGE_CUT_FALLOFFS * self.falloff {
            return 0.0;
        }
        crate::drone_skills::hit_chance(
            self.optimal,
            self.falloff,
            self.tracking,
            self.sig_resolution,
            aim,
        )
    }
}

/// One cycle's verdict, **deterministic per cycle and world**: the rolls are
/// the module's id, the cycle's start and the world's own seed (REVIEW-FIX2,
/// review item 16: the seed is server-side only, so a client that knows the
/// hash still cannot predict which cycles hit). A replay in one world rolls
/// the same.
pub fn application(
    app: &Application,
    aim: crate::drone_skills::Aim,
    module: i64,
    at: i64,
    seed: u64,
) -> crate::drone_skills::Verdict {
    crate::drone_skills::verdict(
        app.chance(aim),
        crate::drone_skills::roll(module, at, seed.wrapping_add(1)),
        crate::drone_skills::roll(module, at, seed.wrapping_add(2)),
    )
}

/// The turret's numbers as the ship's sheet has them (skills, rigs, the
/// loaded charge's multipliers), else the type's own; `None` for a module
/// with no `trackingSpeed` (not a turret in the table, or the fixture's):
/// that gun still hits every time.
fn turret_numbers(w: &Registry, c: &Cycle) -> Option<Application> {
    let get = |attr: i64| {
        crate::dogma::module_attribute(w, c.ship_id, c.module_id, attr)
            .or_else(|| crate::typedata::attribute_in(w.content(), c.type_id, attr))
    };
    let tracking = get(crate::drone_skills::ATTR_TRACKING).filter(|t| *t > 0.0)?;
    let app = Application {
        optimal: get(ATTR_OPTIMAL).unwrap_or(0.0),
        falloff: get(crate::drone_skills::ATTR_FALLOFF).unwrap_or(0.0),
        tracking,
        sig_resolution: get(crate::drone_skills::ATTR_SIG_RESOLUTION).unwrap_or(0.0),
    };
    // A tracking disruptor (or computer) on the shooter (EWAR-2B).
    Some(disrupted(app, crate::ewar::gun_factors(w, c.ship_id)))
}

/// `app` with a tracking disruptor's factors on its optimal, falloff and
/// tracking speed.
fn disrupted(app: Application, (optimal, falloff, tracking): (f64, f64, f64)) -> Application {
    Application {
        optimal: app.optimal * optimal,
        falloff: app.falloff * falloff,
        tracking: app.tracking * tracking,
        ..app
    }
}

/// **A tracking disruptor on an entity's guns** (#137): the factor its optimal
/// range was cut (or raised) to, `None` while nothing touches it. An entity's
/// reach is its type's `entityAttackRange`, which the brain scales by this.
/// The scale is on the optimal alone **[A]** (the type's reach is not split
/// into optimal and falloff); falsified by a disrupted rat that keeps
/// shooting well past `optimal * factor` in the live client.
pub(crate) fn entity_gun_reach(w: &Registry, ship: i64) -> Option<f64> {
    let (optimal, ..) = crate::ewar::gun_factors(w, ship);
    ((optimal - 1.0).abs() > f64::EPSILON).then_some(optimal)
}

/// The share of an entity's volleys that still land under a disruptor, from
/// the gun's `base` numbers: the disrupted chance over the undisrupted one,
/// so an entity untouched by any module keeps hitting as it always did and
/// only the disruptor takes shots away. `None` when `base` cannot hit this
/// aim at all (nothing to take away from) or the factors are all 1.
fn disrupted_share(
    base: Application,
    factors: (f64, f64, f64),
    aim: crate::drone_skills::Aim,
) -> Option<f64> {
    if factors == (1.0, 1.0, 1.0) {
        return None;
    }
    let before = base.chance(aim);
    (before > 0.0).then(|| (disrupted(base, factors).chance(aim) / before).clamp(0.0, 1.0))
}

/// The verdict for one volley of the entity `shooter` (a roster key and its
/// ship id) at `victim`, while a tracking disruptor or computer holds its
/// guns (#137): optimal range (attribute 54), falloff (158) and tracking
/// speed (160) of the entity's type, times the module's factors, through the
/// same hit formula a turret uses. `None` leaves the volley as it was: no
/// disruptor, a type with no tracking speed, or an aim it could not hit.
pub(crate) fn entity_gun_verdict(
    w: &Registry,
    shooter: &crate::space_state::Presence,
    type_id: i64,
    victim: &crate::space_state::Presence,
    at: i64,
) -> Option<crate::drone_skills::Verdict> {
    let factors = crate::ewar::gun_factors(w, shooter.ship_id);
    if factors == (1.0, 1.0, 1.0) {
        return None;
    }
    let get = |attr: i64| crate::typedata::attribute_in(w.content(), type_id, attr);
    let base = Application {
        optimal: get(ATTR_OPTIMAL).unwrap_or(0.0),
        falloff: get(crate::drone_skills::ATTR_FALLOFF).unwrap_or(0.0),
        tracking: get(crate::drone_skills::ATTR_TRACKING).filter(|t| *t > 0.0)?,
        sig_resolution: get(crate::drone_skills::ATTR_SIG_RESOLUTION).unwrap_or(0.0),
    };
    let share = disrupted_share(base, factors, turret_aim(w, shooter, victim))?;
    Some(crate::drone_skills::verdict(
        share,
        crate::drone_skills::roll(shooter.ship_id, at, w.world().seed.wrapping_add(1)),
        crate::drone_skills::roll(shooter.ship_id, at, w.world().seed.wrapping_add(2)),
    ))
}

/// How the target looks from the shooter's gun (as `drones::aim_of`).
fn turret_aim(
    w: &Registry,
    shooter: &crate::space_state::Presence,
    victim: &crate::space_state::Presence,
) -> crate::drone_skills::Aim {
    let signature = crate::dogma::hull(w, victim.ship_id)
        .map(|h| h.signature_radius)
        .or_else(|| {
            let ty = w.pilot(victim.character_id)?.ship_type_id;
            destiny::hull::find_in(w.content(), ty).map(|h| h.signature_radius)
        })
        .unwrap_or(destiny::hull::FALLBACK.signature_radius);
    crate::drone_skills::Aim {
        distance: crate::space_state::distance(shooter.motion.position, victim.motion.position),
        transversal: crate::drone_skills::transversal(
            shooter.motion.position,
            shooter.motion.velocity,
            victim.motion.position,
            victim.motion.velocity,
        ),
        signature,
    }
}

/// The verdict for a turret cycle, or `None` when the gun has no tracking.
fn turret_verdict(
    w: &Registry,
    c: &Cycle,
    shooter: &crate::space_state::Presence,
    victim: &crate::space_state::Presence,
) -> Option<crate::drone_skills::Verdict> {
    let app = turret_numbers(w, c)?;
    Some(application(
        &app,
        turret_aim(w, shooter, victim),
        c.module_id,
        c.at,
        w.world().seed,
    ))
}

/// One volley against one ship, and everything it tells everybody.
///
/// `shooter` and `victim.character_id` are **roster keys**: a character, or
/// an NPC's own id (`crate::npc`, which calls this for its own guns). Each is
/// turned into a hull and an owner through `Registry::pilot`, never
/// `character()` -- an NPC victim would otherwise take damage as the
/// fallback hull and die without a wreck, in silence. Only characters are
/// told anything: an NPC has no session, no rings and no combat log.
///
/// **Every volley is aggression**, hit or miss, and goes to
/// `crimewatch::aggression` first: the shooter's weapons timer, the log-off
/// timers of both -- before a killing blow takes the victim out of the
/// roster.
pub(crate) fn fire(
    w: &mut Registry,
    shot: Shot,
    shooter: i64,
    victim: crate::space_state::Presence,
    now: BlueTime,
) {
    fire_banked(w, shot, shooter, victim, false, now);
}

/// [`fire`] for a grouped bank's one volley: both combat-log lines carry
/// `isBanked` (the client's `...Banked` line and turret-set grouping).
pub(crate) fn fire_banked(
    w: &mut Registry,
    shot: Shot,
    shooter: i64,
    victim: crate::space_state::Presence,
    banked: bool,
    now: BlueTime,
) {
    fire_banked_in(w, shot, shooter, victim, banked, now, true);
}

/// [`fire_banked`]; `cached` false builds the audience the pre-PA-8c way
/// (the test oracle), true from the per-system cache ([`Combat::audience`]).
fn fire_banked_in(
    w: &mut Registry,
    shot: Shot,
    shooter: i64,
    victim: crate::space_state::Presence,
    banked: bool,
    now: BlueTime,
    cached: bool,
) {
    // UNDOCK-INVULN: a ship fresh out of a station or through a gate takes
    // nothing, and a volley at it is not aggression either.
    if crate::invuln::ship_protected(w, shot.target_ship_id, now) {
        return;
    }
    crate::crimewatch::aggression(w, shooter, victim.character_id, shot.at, now);
    let victim_pilot = w.pilot(victim.character_id);
    let hull_type = victim_pilot.map_or(0, |p| p.ship_type_id);
    let victim_is_character = victim_pilot.is_some_and(|p| p.char_id.is_some());
    let shooter_pilot = w.pilot(shooter);
    // The victim's **fitted** HP and resonances when `crate::dogma` has a
    // sheet for the ship (plates, extenders, damage controls, hull upgrades);
    // the hull table's otherwise. Taken when the ship is first hit.
    let profile = crate::dogma::profile(w, shot.target_ship_id).unwrap_or_else(|| {
        Profile::from_hull_in(w.content(), destiny::hull::of_in(w.content(), hull_type))
    });
    // A miss takes nothing off, so it leaves no record behind for a ship
    // nobody has hit: its bars are a full ship's.
    let (applied, bars) = if shot.is_miss() {
        let bars = match w.combat().ships.get(&shot.target_ship_id) {
            Some(r) => r.bars(now),
            None => ShipDamage::full(profile, shot.at.min(now)).bars(now),
        };
        (None, DamageState::from(bars))
    } else {
        let record = w
            .combat_mut()
            .ships
            .entry(shot.target_ship_id)
            .or_insert_with(|| ShipDamage::full(profile, shot.at.min(now)));
        let applied = record.apply(shot.damage, shot.at);
        (Some(applied), DamageState::from(record.bars(now)))
    };
    let dealt = applied.map_or(0.0, |a| a.message_damage());
    // SOC-C6: the shooter goes on the victim's next killmail.
    crate::killmail::on_volley(w, &shot, shooter, victim.character_id, dealt);
    book_npc_damage(w, shooter, victim.character_id, dealt);
    // NPC-THREAT: a rat's ledger of who hurt it.
    crate::npc_threat::on_act(w, shooter, victim.character_id, dealt, shot.at);

    // Everyone whose park holds the victim's ball, victim included --
    // **characters**: an NPC in the roster has no park.
    let park = if cached {
        audience(w, &victim)
    } else {
        let mut park = vec![victim.character_id];
        park.extend(
            w.roster()
                .others_in_system(victim.solar_system_id, victim.character_id),
        );
        park.retain(|id| w.character(*id).is_some());
        park
    };

    // 1. The bars.
    let stamp = w.next_destiny_stamp();
    if let Some(p) = destiny::DamageStateChange::new(park.clone(), stamp, shot.target_ship_id, bars)
    {
        w.schedule(Duration::ZERO, p);
    }
    // 2. The rings, from the same volley, so they move with the bars.
    if let Some(a) = applied.filter(|_| victim_is_character) {
        for change in a.ring_changes.iter() {
            w.schedule(
                Duration::ZERO,
                RingUpdate {
                    to: vec![victim.character_id],
                    owner_id: victim.character_id,
                    ship_id: shot.target_ship_id,
                    change,
                },
            );
        }
    }
    // 3. The two combat-log lines. Both name fallbacks are set: they cost
    // nothing, and without them a line whose ball the reader's park lacks is
    // dropped (`uilog.py:461-466`) [V].
    //
    // The owner fallback is the pilot's **owner**: the character for a
    // player, the NPC corporation for an NPC (1000124 for an Archangel, not
    // its 7e9 ball id). The victim's source fallback is the shooter's
    // `charID`, which an NPC has none of -- its ball is in the victim's park,
    // and the line names it from there (by type: `GetSlimItemName:488`).
    let mut to_shooter = shot.attacker_message(dealt);
    to_shooter.is_banked = banked;
    to_shooter.target_owner_id = Some(victim_pilot.map_or(victim.character_id, |p| p.owner_id));
    let mut to_victim = shot.victim_message(dealt);
    to_victim.is_banked = banked;
    to_victim.source_char_id = shooter_pilot.and_then(|p| p.char_id);
    if shooter_pilot.is_some_and(|p| p.char_id.is_some()) {
        w.schedule(
            Duration::ZERO,
            CombatMessage {
                to: vec![shooter],
                message: to_shooter,
            },
        );
    }
    if victim_is_character {
        w.schedule(
            Duration::ZERO,
            CombatMessage {
                to: vec![victim.character_id],
                message: to_victim,
            },
        );
    }
    // 4. A death. `damage::ShipDamage` cannot tell a pod from a ship; the
    // hull can (`Death::for_type`).
    if applied.is_some_and(|a| a.death.is_some()) {
        // Its side effects -- an NPC's bounty to `shooter` among them -- are
        // `kill_hooks` rows, fired by `kill` and `kill_npc` (seam S13).
        kill(
            w,
            damage::Death::for_type(hull_type),
            victim,
            park,
            shooter,
            now,
        );
    }
}

/// **A self-destruct** (`crate::self_destruct`): the pilot's own ship or
/// capsule blows up through the same death as a shot one -- wreck, capsule,
/// revive, kill hooks and killmail -- with the pilot as the killer. `false`
/// when `character` is not in space.
pub(crate) fn self_destruct_kill(w: &mut Registry, character: i64, now: BlueTime) -> bool {
    let Some(victim) = w.roster().get(character).copied() else {
        return false;
    };
    let hull_type = w.pilot(character).map_or(0, |p| p.ship_type_id);
    let mut park = vec![character];
    park.extend(
        w.roster()
            .others_in_system(victim.solar_system_id, character),
    );
    park.retain(|id| w.character(*id).is_some());
    kill(
        w,
        damage::Death::for_type(hull_type),
        victim,
        park,
        character,
        now,
    );
    true
}

/// The characters of one system, in roster order, remembered while the roster
/// and the character list are as they were (PA-8c).
#[derive(Debug, Clone)]
struct Audience {
    system: i64,
    revision: u64,
    characters: usize,
    ids: std::sync::Arc<Vec<i64>>,
}

/// A cache, not state: it never makes two combats unequal.
impl PartialEq for Audience {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Everyone whose park holds `victim`'s ball: the victim first, then the
/// other characters of its system in roster order. The same vector the
/// per-volley scan made, built once per system and roster revision.
/// [A] a character's existence changes only with the character list, which
/// the cache key counts; falsified by `audience_follows_arrivals_and_kills`.
fn audience(w: &mut Registry, victim: &crate::space_state::Presence) -> Vec<i64> {
    let (revision, characters) = (w.roster().revision(), w.character_count());
    let system = victim.solar_system_id;
    let hit = w.combat().audience.as_ref().is_some_and(|a| {
        a.system == system && a.revision == revision && a.characters == characters
    });
    if !hit {
        // `i64::MIN` is nobody's id: nothing is left out.
        let ids: Vec<i64> = w
            .roster()
            .others_in_system(system, i64::MIN)
            .into_iter()
            .filter(|id| w.character(*id).is_some())
            .collect();
        w.combat_mut().audience = Some(Audience {
            system,
            revision,
            characters,
            ids: std::sync::Arc::new(ids),
        });
    }
    let ids = match w.combat().audience.as_ref() {
        Some(a) => a.ids.clone(),
        None => return Vec::new(),
    };
    let mut park = Vec::with_capacity(ids.len() + 1);
    if w.character(victim.character_id).is_some() {
        park.push(victim.character_id);
    }
    park.extend(ids.iter().copied().filter(|id| *id != victim.character_id));
    park
}

/// One missile volley arriving, for CP45 (`npc_missile`), W16 CP44.
///
/// The caller owns the flight: it decides when the volley lands and what it
/// carries (the missile type's damage times the launcher's multiplier,
/// attribute 212). This is only the landing, and it is [`fire`]: the same
/// aggression, record, bars, rings, combat-log lines and death as a gun's
/// volley, so a missile kill fires the same `kill_hooks` rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MissileVolley {
    /// The roster key that launched: a character, an NPC's id or a drone.
    pub shooter: i64,
    /// The launching ship's ball, for the combat-log lines. A missile
    /// outlives its launcher, so this need not be in space any more.
    pub attacker_ship_id: i64,
    pub launcher_item_id: i64,
    /// The missile's type. **[A]** it is the combat log's `weapon` key, as
    /// a gun's own type is for a turret; falsified by a retail log line
    /// naming the launcher instead.
    pub missile_type_id: i64,
    pub target_ship_id: i64,
    /// Final pre-resistance damage (`damage::Shot::damage`).
    pub damage: Damage,
    /// When it lands, blue FILETIME.
    pub at: BlueTime,
}

/// Land one [`MissileVolley`] through [`fire`]. `false`, and nothing
/// recorded, when the target is no longer a ship in space or is the
/// launcher's own ball. Every missile hits, at [`HIT_QUALITY`] **[A]**
/// (no explosion-radius or velocity reduction yet; falsified by the first
/// retail log line with reduced missile damage).
pub fn missile_volley(w: &mut Registry, v: MissileVolley, now: BlueTime) -> bool {
    missile_volley_banked(w, v, false, now)
}

/// [`missile_volley`] for a bank's one missile (`isBanked` on its lines).
pub fn missile_volley_banked(
    w: &mut Registry,
    v: MissileVolley,
    banked: bool,
    now: BlueTime,
) -> bool {
    if v.target_ship_id == v.attacker_ship_id {
        return false;
    }
    let Some(victim) = w.roster().by_ship(v.target_ship_id).copied() else {
        return false;
    };
    let shot = Shot {
        at: v.at,
        attacker_ship_id: v.attacker_ship_id,
        weapon_item_id: v.launcher_item_id,
        weapon_type_id: v.missile_type_id,
        target_ship_id: v.target_ship_id,
        damage: v.damage,
        hit_quality: HIT_QUALITY,
    };
    fire_banked(w, shot, v.shooter, victim, banked, now);
    true
}

/// One missile that reached its fuse without touching the target (it was
/// outrun, or the target warped): the **miss** line to the launcher's pilot
/// and the target's, `hitQuality` 0 -- "Your <weapon> misses <target>
/// completely" -- through [`fire`], so it is aggression and sends the same
/// messages as a hit with nothing taken off. `false`, and nothing sent, when
/// the target is gone from the roster (no ball to name) or is the launcher.
/// **[A]** TQ prints a miss line for such a missile; falsified by a retail
/// log of an outrun missile with no line.
pub fn missile_miss(w: &mut Registry, v: MissileVolley, now: BlueTime) -> bool {
    missile_miss_banked(w, v, false, now)
}

/// [`missile_miss`] for a bank's one missile.
pub fn missile_miss_banked(
    w: &mut Registry,
    v: MissileVolley,
    banked: bool,
    now: BlueTime,
) -> bool {
    if v.target_ship_id == v.attacker_ship_id {
        return false;
    }
    let Some(victim) = w.roster().by_ship(v.target_ship_id).copied() else {
        return false;
    };
    let shot = Shot {
        at: v.at,
        attacker_ship_id: v.attacker_ship_id,
        weapon_item_id: v.launcher_item_id,
        weapon_type_id: v.missile_type_id,
        target_ship_id: v.target_ship_id,
        damage: v.damage,
        hit_quality: 0,
    };
    fire_banked(w, shot, v.shooter, victim, banked, now);
    true
}

/// An NPC's own repairer (`npc::brain`, CP23): up to `amount` back on one
/// layer of `ship_id`'s record ([`ShipDamage::repair`]), and the bars to
/// every character in its system when anything moved. A ship nothing has
/// hit has no record, and nothing to repair.
pub(crate) fn self_repair(
    w: &mut Registry,
    ship_id: i64,
    armour: bool,
    amount: f64,
    now: BlueTime,
) {
    let Some(record) = w.combat_mut().ships.get_mut(&ship_id) else {
        return;
    };
    if record.repair(armour, amount, now) <= 0.0 {
        return;
    }
    let bars = DamageState::from(record.bars(now));
    let Some(system) = w.roster().by_ship(ship_id).map(|p| p.solar_system_id) else {
        return;
    };
    let park = characters_in(w, system);
    if park.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    if let Some(p) = destiny::DamageStateChange::new(park, stamp, ship_id, bars) {
        w.schedule(Duration::ZERO, p);
    }
}

/// Walk `death.steps()`: everything a ship's death needs, and the half of a
/// capsule's that is not the clone jump.
///
/// **A ship** (`docs/re/damage-death.md` §0 steps 8-13, §4.2, §5.1, §6):
///
/// 1. **the store, in one plan** ([`death_plan`]): the pilot's capsule out of
///    the dead ship (it sat at `flagCapsule`), every item inside the ship
///    destroyed, the ship destroyed, and the wreck created in the system.
///    Its item pushes are addressed to the victim and put on the clock
///    ([`Addressed`]): the tick is not a turn, and an unaddressed push left
///    on `pending` would go to whoever calls next;
/// 2. **one `destiny::Kill`** to the park: wreck `AddBalls2`, `RemoveBall(ship,
///    True)`, capsule `AddBalls2`, one stamp;
/// 3. **`TargetLost`**: `targeting::left_space` with `Reason::Exploding`, to
///    every locker, and the victim's own locks dropped -- while the roster
///    still has the dead ship, which is how it finds the ball;
/// 4. **the pilot is in the capsule**: the roster entry re-made for it at the
///    death position, `Character.ship_id/ship_type_id` the capsule's, and
///    **`{'shipid': (deadShip, capsule)}` queued** -- the one message the
///    victim cannot live without (§4.2), drained by the world ticker
///    (`push::session_changes_due`). On arrival godma re-primes
///    (`ProcessSessionChange:1375-1407`, `Prime` on any `shipid` change)
///    **[V]**, so `GetAllInfo` serves the capsule's rows and `CapsuleDogma`
///    needs no push of its own;
/// 5. the dead ship's module state and health record forgotten.
///
/// Not sent: `OnPodOrCorvetteAssembly` (§5.1, cosmetic `[I]`; its handler
/// applies a skin, and an unasked skin lookup is not worth the risk).
///
/// **A capsule**: the terminal removal (no wreck, no capsule), `TargetLost`,
/// the pilot out of the roster, then [`revive`]: `OnPodKilled`,
/// `OnPlayerPodDeath`, and P5.9's `revive_in_clone` session change.
///
/// **An NPC** is [`kill_npc`]; **a drone** is `drones::destroyed`, a
/// [`crate::kill_hooks::PLAYER`] row.
///
/// **The kill hooks** (seam S13): one [`crate::kill_hooks::KillEvent`],
/// built here before anything is torn down, and `kill_hooks::PLAYER` fired
/// once with it -- so a hook still sees the dead ship, its contents and its
/// roster entry.
fn kill(
    w: &mut Registry,
    death: damage::Death,
    victim: crate::space_state::Presence,
    park: Vec<i64>,
    killer: i64,
    now: BlueTime,
) {
    let character_id = victim.character_id;
    // **An NPC dies another death**: no capsule, no store ship, no revive.
    if let Some(n) = w.npcs().get(character_id).copied() {
        kill_npc(w, n, victim, park, killer, now);
        return;
    }
    // **An abandoned ship dies another death too** (`crate::pilotless`): no
    // pilot, no capsule, no crime.
    if let Some(a) = crate::pilotless::get(w, character_id) {
        kill_pilotless(w, a, victim, park, killer, now);
        return;
    }
    let drone = w.modules().drones().get(character_id).is_some();
    let event = crate::kill_hooks::KillEvent {
        victim_character: character_id,
        victim_ship: victim.ship_id,
        ship_type: w.pilot(character_id).map_or(0, |p| p.ship_type_id),
        killer: Some(killer),
        system: victim.solar_system_id,
        npc: None,
        when: now,
        park,
    };
    crate::kill_hooks::fire(crate::kill_hooks::table_for(&event), w, &event);
    let park = event.park;
    // **A drone** is an item with no pilot: no capsule, no wreck, no revive.
    // Its death is the `drones::destroyed` hook that just ran.
    if drone {
        w.combat_mut().ships.remove(&victim.ship_id);
        return;
    }
    let Some(c) = w.character(character_id).cloned() else {
        return;
    };
    // Where it died is where it is *now*, not where it was last commanded.
    w.advance_character(character_id);
    let position = w
        .roster()
        .get(character_id)
        .map_or(victim.motion.position, |p| p.motion.position);

    // `death.steps()` is the order reference; the store half comes first
    // because the kill batch needs the ids it produces. A capsule's death
    // ejects nothing and leaves nothing (`leaves_capsule`, `leaves_wreck`).
    let done = if death.leaves_capsule() || death.leaves_wreck() {
        run_death_plan(w, &c, victim.ship_id, now, position)
    } else {
        None
    };
    let (capsule, wreck, inside) =
        done.map_or((None, None, Vec::new()), |d| (d.capsule, d.wreck, d.inside));

    // The kill batch: wreck, removal, capsule; one stamp.
    let corp_id = c.corporation_id;
    let alliance_id = c
        .alliance_id
        .and_then(|a| i32::try_from(a).ok())
        .unwrap_or(-1);
    let capsule_motion = crate::space_state::Motion::arrived(position, [0.0; 3]);
    let capsule_parts = capsule.map(|(capsule_id, type_id)| {
        (
            destiny::ego_ball_in(
                w.content(),
                capsule_id,
                type_id,
                i32::try_from(corp_id).unwrap_or(-1),
                alliance_id,
                &capsule_motion,
            ),
            destiny::Slim {
                item_id: capsule_id,
                type_id,
                owner_id: character_id,
                char_id: Some(character_id),
                corp_id: Some(corp_id),
                response_thresholds: None,
            },
            damage_state(w, capsule_id, now),
        )
    });
    let wreck_ball = wreck.map(|wr| wreck_parts(w, &wr));
    let stamp = w.next_destiny_stamp();
    if let Some(k) = destiny::Kill::new(park, stamp, victim.ship_id, wreck_ball, capsule_parts) {
        w.schedule(Duration::ZERO, k);
    }
    if let Some(wr) = wreck {
        w.combat_mut().wrecks.insert(wr.wreck_id, wr);
    }

    // Locks before the roster: `left_space` finds the ball through it. A
    // pilot ejected into a capsule stays in space, so no park release
    // clears their `targetedBy`: they are told of each locker too
    // (RAT-GHOST).
    if capsule.is_some() {
        crate::targeting::lost_ship_in_space(
            w,
            character_id,
            crate::targeting::Reason::Exploding,
            crate::targeting::Delivery::OnTheClock,
        );
    } else {
        crate::targeting::left_space(
            w,
            character_id,
            crate::targeting::Reason::Exploding,
            crate::targeting::Delivery::OnTheClock,
        );
    }
    // A podded pilot stays in the roster until `revive` has run:
    // `revive_in_clone` reads "was this from space" off the roster, and
    // without it the change would not undock-reverse the client
    // (`solarsystemid` going absent is what tears the park down). It leaves
    // the roster itself.
    if death != damage::Death::Capsule {
        w.roster_mut().leave(character_id);
    }
    w.combat_mut().ships.remove(&victim.ship_id);
    w.modules_mut().forget_ship(victim.ship_id, &inside);

    match capsule {
        Some((capsule_id, type_id)) => {
            // `[A]` the capsule leaves at rest; TQ's pod drifts a little.
            w.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    character_id,
                    capsule_id,
                    victim.solar_system_id,
                    capsule_motion,
                ));
            if let Some(ch) = w.character_mut(character_id) {
                ch.ship_id = capsule_id;
                ch.ship_type_id = type_id;
            }
            w.queue_session_change(character_id, shipid_change(victim.ship_id, capsule_id));
        }
        None if death == damage::Death::Capsule => {
            revive(w, character_id, victim.ship_id);
        }
        // A ship with no capsule in it: nothing to eject, and the pilot is
        // simply out of space, as before P2.2c.
        None => {}
    }
}

/// An NPC's death (`docs/lanes/npc.md` §4.2, "DEAD"): the wreck and the
/// terminal removal in one [`destiny::Kill`], the locks released with
/// `'Exploding'`, the ball out of the roster and the record out of the NPC
/// table. Its side effects are [`crate::kill_hooks::NPC`], fired once with
/// one [`crate::kill_hooks::KillEvent`] after the ball has left the roster
/// and before the record leaves the NPC table: the bounty
/// (`npc::pay_bounty`, which reads the record) and then the mission that NPC
/// belonged to (`mission::npc_killed`, which reads the roster), so a
/// pocket's last kill ticks the objective live.
///
/// **What it skips, and why each is right:** there is no capsule to eject
/// (an entity has no pilot), no store ship to destroy and nothing inside it
/// (an NPC was never an item), and no revive.
///
/// **The wreck is a store item** ([`npc_wreck`]) holding the NPC's loot
/// (`npc::loot`), so P2.4's whole path -- `invbroker` listing it to anyone
/// in the system, "Loot All", `isEmpty` -- is the one a player's wreck
/// takes. Owner and corp are the NPC corporation, as the slim's `ownerID`;
/// `HaveLootRight` then gives everybody the right (`michelle.py:1972`, an
/// NPC owner) **[V]**. If the store refuses the wreck, it falls back to a
/// [`Wreck`] record alone with an id from the NPC table's range -- a wreck
/// on screen with nothing in it, as before P2.4 -- rather than no wreck.
fn kill_npc(
    w: &mut Registry,
    n: crate::npc::Npc,
    victim: crate::space_state::Presence,
    park: Vec<i64>,
    killer: i64,
    now: BlueTime,
) {
    let event = crate::kill_hooks::KillEvent {
        victim_character: n.id,
        victim_ship: victim.ship_id,
        ship_type: n.kind.type_id,
        killer: Some(killer),
        system: victim.solar_system_id,
        npc: Some(n.kind),
        when: now,
        park,
    };
    w.advance_character(n.id);
    let position = w
        .roster()
        .get(n.id)
        .map_or(victim.motion.position, |p| p.motion.position);
    // GAP-LOOT: the wreck belongs to the mission's owner, else the character
    // who hurt it most; with neither, to the NPC corporation (free).
    let owner =
        npc_wreck_owner(w, n.id).unwrap_or((n.kind.owner_corp_id, n.kind.owner_corp_id, None));
    w.combat_mut().npc_damage.remove(&n.id);
    let wreck = n.kind.wreck_type_id.and_then(|type_id| {
        let record = |wreck_id| Wreck {
            wreck_id,
            type_id,
            solar_system_id: victim.solar_system_id,
            position,
            ship_id: victim.ship_id,
            owner_id: owner.0,
            corp_id: owner.1,
            fleet_id: owner.2,
            abandoned: false,
            expires_at: now.saturating_add(WRECK_LIFETIME),
        };
        match npc_wreck(w, &n, type_id, victim.solar_system_id) {
            Some(id) => Some(record(id)),
            None => Some(record(w.npcs_mut().next_id()?)),
        }
    });
    let wreck_ball = wreck.map(|wr| wreck_parts(w, &wr));
    let stamp = w.next_destiny_stamp();
    if let Some(k) = destiny::Kill::new(event.park.clone(), stamp, victim.ship_id, wreck_ball, None)
    {
        w.schedule(Duration::ZERO, k);
    }
    if let Some(wr) = wreck {
        w.combat_mut().wrecks.insert(wr.wreck_id, wr);
    }
    crate::targeting::left_space(
        w,
        n.id,
        crate::targeting::Reason::Exploding,
        crate::targeting::Delivery::OnTheClock,
    );
    w.roster_mut().leave(n.id);
    w.combat_mut().ships.remove(&victim.ship_id);
    crate::kill_hooks::fire(crate::kill_hooks::table_for(&event), w, &event);
    w.npcs_mut().remove(n.id);
}

/// **An abandoned ship's death** (BE-2, `crate::pilotless`): the wreck in the
/// ship's own system, owned by the hull's owner (who may be anywhere, or
/// offline), the hull and what it carried dropped or destroyed as a flown
/// ship's are ([`contents_plan`]), one [`destiny::Kill`] to the park, the
/// locks released with `'Exploding'`, the ball out of the roster and the
/// record and its row out of the table.
///
/// **Of the kill hooks only insurance, the killmail, its notification and the
/// fleet tag run**: the others read the victim as a pilot (crimewatch,
/// CONCORD, bounties, map statistics), and shooting a ship nobody flies is no crime [A: the
/// spec's open question 3; falsifier: a retail capture that flags a suspect].
/// A killmail for an empty ship is `killmail::on_kill_pilotless` (#132), called
/// first: the shooter's kill, the hull's owner the victim.
fn kill_pilotless(
    w: &mut Registry,
    a: crate::pilotless::Abandoned,
    victim: crate::space_state::Presence,
    park: Vec<i64>,
    killer: i64,
    now: BlueTime,
) {
    use crate::item::{Edit, Slot};
    let event = crate::kill_hooks::KillEvent {
        victim_character: a.owner_id,
        victim_ship: a.ship_id,
        ship_type: a.type_id,
        killer: Some(killer),
        system: a.system,
        npc: None,
        when: now,
        park,
    };
    w.advance_character(a.ship_id);
    let position = w
        .roster()
        .get(a.ship_id)
        .map_or(victim.motion.position, |p| p.motion.position);
    // #132: the shooter's kill, the hull's owner the victim.
    // The mail is written before the store plan, which empties the hull it
    // lists; a plan the store refuses leaves the ship, so a later kill can
    // file a second mail (logged below).
    crate::killmail::on_kill_pilotless(w, &event);
    crate::notifications::on_kill(w, &event);
    for id in [a.ship_id, a.owner_id] {
        let e = w.fleets_mut().untag_item(id);
        w.fleet_apply(e);
    }
    // The wreck first: nothing can be moved into an id the same plan mints.
    let mut wreck = None;
    if let Some(type_id) = wreck_type_in(w.content(), a.type_id) {
        let create = [Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(a.owner_id, a.system, FLAG_IN_SPACE),
        }];
        match apply_on_the_clock(w, a.owner_id, &create) {
            Ok(changes) => {
                wreck = changes
                    .iter()
                    .find_map(|ch| match ch {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                    .map(|wreck_id| Wreck {
                        wreck_id,
                        type_id,
                        solar_system_id: a.system,
                        position,
                        ship_id: a.ship_id,
                        owner_id: a.owner_id,
                        corp_id: a.corp_id,
                        fleet_id: fleet_of(w, a.owner_id),
                        abandoned: false,
                        expires_at: now.saturating_add(WRECK_LIFETIME),
                    });
            }
            Err(e) => crate::outln!(
                "[combat] the wreck of ship {} was refused: {e:?}",
                a.ship_id
            ),
        }
    }
    let (mut edits, inside) =
        contents_plan(w, a.owner_id, a.ship_id, wreck.map(|wr| wr.wreck_id), None);
    if w.store().get(a.ship_id).is_some() {
        edits.push(Edit::Destroy { item_id: a.ship_id });
    }
    if let Err(e) = apply_on_the_clock(w, a.owner_id, &edits) {
        crate::outln!(
            "[combat] the death of abandoned ship {} was refused by the store: {e:?} (its killmail is already filed; a later kill files another)",
            a.ship_id
        );
        if let Some(wr) = wreck {
            remove_wreck_item(w, wr.wreck_id, wr.owner_id);
        }
        return;
    }
    // Paid only once the hull is gone: a refused plan leaves it, and the next
    // kill would pay the base again (#134).
    crate::service::insurance::on_kill(w, &event);
    let wreck_ball = wreck.map(|wr| wreck_parts(w, &wr));
    let stamp = w.next_destiny_stamp();
    if let Some(k) = destiny::Kill::new(event.park.clone(), stamp, a.ship_id, wreck_ball, None) {
        w.schedule(Duration::ZERO, k);
    }
    if let Some(wr) = wreck {
        w.combat_mut().wrecks.insert(wr.wreck_id, wr);
    }
    // Locks before the roster: `left_space` finds the ball through it.
    crate::targeting::left_space(
        w,
        a.ship_id,
        crate::targeting::Reason::Exploding,
        crate::targeting::Delivery::OnTheClock,
    );
    crate::pilotless::release(w, a.ship_id);
    w.combat_mut().ships.remove(&a.ship_id);
    w.modules_mut().forget_ship(a.ship_id, &inside);
}

/// An NPC's wreck as a store item, with its loot in it: **two plans**, as a
/// ship's death is ([`run_death_plan`]), because nothing can be created
/// inside an id the same plan mints. The wreck is owned by the NPC
/// corporation, in the solar system at [`FLAG_IN_SPACE`]; each stack of
/// `npc::loot` is created in it at [`FLAG_IN_WRECK`], owned the same.
///
/// The store's pushes go to **nobody**: no character owns either, and a
/// looter's client lists the wreck when it opens it. A loot plan the store
/// refuses leaves the wreck empty (logged), never missing. `None` when the
/// store refuses the wreck itself -- a type it does not know.
fn npc_wreck(
    w: &mut Registry,
    n: &crate::npc::Npc,
    type_id: i64,
    solar_system_id: i64,
) -> Option<i64> {
    use crate::item::{Edit, Slot};
    let owner = n.kind.owner_corp_id;
    let create = [Edit::Create {
        type_id,
        quantity: 1,
        to: Slot::new(owner, solar_system_id, FLAG_IN_SPACE),
    }];
    let wreck_id = match apply_on_the_clock_to(w, &[], &create) {
        Ok(changes) => changes.iter().find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })?,
        Err(e) => {
            crate::outln!("[combat] the wreck of NPC {} was refused: {e:?}", n.id);
            return None;
        }
    };
    let loot: Vec<Edit> = crate::npc::loot_in(w, n.kind, n.id, solar_system_id)
        .into_iter()
        .map(|(type_id, quantity)| Edit::Create {
            type_id,
            quantity,
            to: Slot::new(owner, wreck_id, FLAG_IN_WRECK),
        })
        .collect();
    if !loot.is_empty()
        && let Err(e) = apply_on_the_clock_to(w, &[], &loot)
    {
        crate::outln!("[combat] the loot of NPC {} was refused: {e:?}", n.id);
    }
    Some(wreck_id)
}

/// **A loot cache in space** (Y-COMBAT, `combat_site::drop_cache`): a wreck
/// of `type_id` owned by the NPC corporation `owner`, in `system` at
/// `position`, holding `stacks` (`(typeID, quantity)`), and recorded like any
/// wreck so the grid reconcile shows it, `invbroker` lists it and Loot All
/// takes it (`HaveLootRight` gives everybody an NPC owner's wreck [V]).
/// Two plans, as [`npc_wreck`]. The stacks the store refuses leave it
/// empty (logged); `None` when the store refuses the wreck itself.
pub fn drop_cache(
    w: &mut Registry,
    owner: i64,
    type_id: i64,
    solar_system_id: i64,
    position: [f64; 3],
    stacks: &[(i64, i64)],
    now: BlueTime,
) -> Option<i64> {
    use crate::item::{Edit, Slot};
    let create = [Edit::Create {
        type_id,
        quantity: 1,
        to: Slot::new(owner, solar_system_id, FLAG_IN_SPACE),
    }];
    let wreck_id = match apply_on_the_clock_to(w, &[], &create) {
        Ok(changes) => changes.iter().find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })?,
        Err(e) => {
            crate::outln!("[combat] a loot cache was refused: {e:?}");
            return None;
        }
    };
    let loot: Vec<Edit> = stacks
        .iter()
        .map(|&(type_id, quantity)| Edit::Create {
            type_id,
            quantity,
            to: Slot::new(owner, wreck_id, FLAG_IN_WRECK),
        })
        .collect();
    if !loot.is_empty()
        && let Err(e) = apply_on_the_clock_to(w, &[], &loot)
    {
        crate::outln!("[combat] the stacks of cache {wreck_id} were refused: {e:?}");
    }
    let wreck = Wreck {
        wreck_id,
        type_id,
        solar_system_id,
        position,
        ship_id: wreck_id,
        owner_id: owner,
        corp_id: owner,
        fleet_id: None,
        abandoned: false,
        expires_at: now.saturating_add(WRECK_LIFETIME),
    };
    w.combat_mut().wrecks.insert(wreck_id, wreck);
    Some(wreck_id)
}

/// A podded pilot wakes up at home (`docs/re/damage-death.md` §5.2,
/// `docs/lanes/clones.md` §3): `OnPodKilled(podID)`, `OnPlayerPodDeath()`,
/// then the docked session change `revive_in_clone` builds -- in that order
/// **[V]**, because `gameui._ParseStateChange:506` calls the change a podding
/// only when its `shipid` old value is the pod `OnPodKilled` named, and the
/// ticker posts a tick's frames before its session changes.
///
/// Preconditions (clones §3): `Character.ship_id` is the dead pod -- true
/// here, since the pod is the ship this pilot was flying -- and, **unlike
/// what clones §3 says**, the pilot is still in the roster: the revive
/// decides "from space" by `roster().get`, and builds the dock's
/// `stationid`/`solarsystemid` pair only then. The locks and the ball are
/// already gone. The revive's own item pushes are addressed to the pilot,
/// like the ship death's.
fn revive(w: &mut Registry, character_id: i64, pod_id: i64) {
    w.schedule(
        Duration::ZERO,
        PodKilled {
            to: vec![character_id],
            pod_id,
        },
    );
    w.schedule(
        Duration::ZERO,
        PlayerPodDeath {
            to: vec![character_id],
        },
    );
    let before = w.take_pending();
    let revival = crate::service::revive_in_clone(w, character_id);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        w.schedule(
            Duration::ZERO,
            Addressed {
                to: Some(vec![character_id]),
                inner: p,
            },
        );
    }
    // Out of space whatever happened: `revive_in_clone` leaves the roster
    // itself, but not when it refuses.
    w.roster_mut().leave(character_id);
    if let Some(r) = revival {
        w.queue_session_change(character_id, r.change);
        // Waking in a structure: its guests hear it, its slim counts them.
        // (Off the addressed turn above, which is for the pilot alone.)
        crate::structure_dock::clone_moved(
            w,
            character_id,
            r.from_station,
            Some(r.station.station_id),
        );
    }
}

/// `{'shipid': (old, new)}` -- one key; the client derives `locationid`
/// (`docs/lanes/fitting.md` §1.2). The same shape as `ship.Board`'s.
pub fn shipid_change(old: i64, new: i64) -> Vec<(PyValue, PyValue)> {
    crate::pyutil::pypairs(vec![(
        "shipid",
        PyValue::Tuple(vec![PyValue::Int(old), PyValue::Int(new)]),
    )])
}

/// What [`run_death_plan`] did: the capsule now outside the ship, the wreck
/// created, and every item id that was inside the ship -- dropped or
/// destroyed, either way no longer fitted to anything.
struct DeathDone {
    capsule: Option<(i64, i64)>,
    wreck: Option<Wreck>,
    inside: Vec<i64>,
}

/// Apply `edits` from the world tick, its store pushes addressed to `owner`
/// and on the clock.
///
/// `Registry::apply_items` puts its pushes on `pending`, which only a payload
/// turn drains: anything already there belongs to somebody else's turn, so it
/// is set aside and put back, and what is left after the plan is exactly the
/// plan's (`docs/lanes/damage-death.md` P2.2c, "a house trap").
fn apply_on_the_clock(
    w: &mut Registry,
    owner: i64,
    edits: &[crate::item::Edit],
) -> Result<Vec<crate::item::Change>, crate::item::EditError> {
    apply_on_the_clock_to(w, &[owner], edits)
}

/// [`apply_on_the_clock`] to any audience; an empty one drops the plan's
/// pushes -- for items nobody's client holds, an NPC wreck's.
fn apply_on_the_clock_to(
    w: &mut Registry,
    to: &[i64],
    edits: &[crate::item::Edit],
) -> Result<Vec<crate::item::Change>, crate::item::EditError> {
    let before = w.take_pending();
    let applied = w.apply_items(edits);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    if applied.is_ok() && !to.is_empty() {
        for (_, p) in ours {
            w.schedule(
                Duration::ZERO,
                Addressed {
                    to: Some(to.to_vec()),
                    inner: p,
                },
            );
        }
    }
    applied
}

/// The store half of a ship's death, as **two** plans, because the second
/// needs the first's id:
///
/// 1. **the wreck** is created, owner the victim, in the solar system --
///    `Edit::Create` mints its id only when it applies, so nothing can be
///    moved into it in the same plan (`item::Edit::Grow`'s note);
/// 2. **the hull** ([`death_plan`]): the capsule out, every item inside
///    either dropped into the wreck or destroyed ([`survives`]), the ship
///    destroyed.
///
/// If the second is refused the wreck is removed again and the death is
/// refused whole (logged; nothing else applied). `None` then, as before.
fn run_death_plan(
    w: &mut Registry,
    c: &crate::model::Character,
    ship_id: i64,
    now: BlueTime,
    position: [f64; 3],
) -> Option<DeathDone> {
    use crate::item::{Edit, Slot};
    // **Where the ship died, not where its pilot's station is** (P3.3): the
    // roster still holds the victim here.
    let space = w
        .roster()
        .get(c.character_id)
        .map(|p| p.solar_system_id)
        .or(c.solar_system_id);
    let wreck_type = w
        .store()
        .get(ship_id)
        .map(|s| s.type_id)
        .or(Some(c.ship_type_id))
        .and_then(|hull| wreck_type_in(w.content(), hull))
        .filter(|_| c.solar_system_id.is_some());
    let mut wreck = None;
    if let (Some(type_id), Some(system)) = (wreck_type, space) {
        let create = [Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(c.character_id, system, FLAG_IN_SPACE),
        }];
        match apply_on_the_clock(w, c.character_id, &create) {
            Ok(changes) => {
                wreck = changes
                    .iter()
                    .find_map(|ch| match ch {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                    .map(|wreck_id| Wreck {
                        wreck_id,
                        type_id,
                        solar_system_id: system,
                        position,
                        ship_id,
                        owner_id: c.character_id,
                        corp_id: c.corporation_id,
                        fleet_id: fleet_of(w, c.character_id),
                        abandoned: false,
                        expires_at: now.saturating_add(WRECK_LIFETIME),
                    });
            }
            Err(e) => crate::outln!("[combat] the wreck of ship {ship_id} was refused: {e:?}"),
        }
    }
    let plan = death_plan(w, c, ship_id, wreck.map(|wr| wr.wreck_id));
    if let Err(e) = apply_on_the_clock(w, c.character_id, &plan.edits) {
        crate::outln!("[combat] the death of ship {ship_id} was refused by the store: {e:?}");
        if let Some(wr) = wreck {
            remove_wreck_item(w, wr.wreck_id, wr.owner_id);
        }
        return None;
    }
    Some(DeathDone {
        capsule: plan.capsule,
        wreck,
        inside: plan.inside,
    })
}

/// The edits of a ship's death, and what they will do.
struct DeathPlan {
    edits: Vec<crate::item::Edit>,
    capsule: Option<(i64, i64)>,
    /// Every item that was inside the ship, at any depth, the capsule
    /// excepted: for `Modules::forget_ship`.
    inside: Vec<i64>,
}

/// Does this item survive its ship's death? [`DROP_CHANCE`], rolled from
/// the ship and the item alone -- the same two ids always roll the same, so
/// a test can rely on *a* result without caring which, and a replayed kill
/// drops what it dropped.
///
/// `splitmix64` (Steele, Lea & Flood 2014's finaliser) over the two ids: a
/// few lines of arithmetic, not a dependency.
pub fn survives(ship_id: i64, item_id: i64) -> bool {
    let mut z = (ship_id as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(item_id as u64);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 53 bits as a fraction in [0, 1).
    ((z >> 11) as f64 / (1u64 << 53) as f64) < DROP_CHANCE
}

/// The hull's half of a death: the capsule out, everything inside the ship
/// dropped into `wreck` or destroyed, and the ship destroyed.
///
/// - the capsule (`fitting::capsule_of`, the pod at `flagCapsule` in the
///   ship) moves to the character's station hangar. `[A]`: the store has no
///   in-space address for a ship -- an undocked ship stays at its station
///   address, `undock_change` moves nothing -- so the pod goes exactly where
///   `ship.LeaveShip` puts it, and a later dock finds it where a fresh
///   character's is. godma ignores the move: neither flag is a fitting flag
///   or `flagPilot` (`OnItemChange:1462-1474`) **[V]**;
/// - **each item directly inside the ship** -- a fitted module, a loaded
///   charge, a cargo stack -- either moves into the wreck whole
///   ([`survives`], [`FLAG_IN_WRECK`], still the victim's) or is destroyed
///   with everything inside it. A container that survives keeps its
///   contents, because they are addressed to it and not to the ship. With no
///   wreck everything is destroyed;
/// - the ship is destroyed.
fn death_plan(
    w: &Registry,
    c: &crate::model::Character,
    ship_id: i64,
    wreck: Option<i64>,
) -> DeathPlan {
    use crate::item::{Edit, Slot};
    let store = w.store();
    let mut edits = Vec::new();
    let capsule = crate::fitting::capsule_of(store, c)
        .filter(|pod| pod.location_id == ship_id && pod.flag_id == crate::fitting::FLAG_CAPSULE)
        .map(|pod| (pod.item_id, pod.type_id));
    if let Some((pod, _)) = capsule {
        let to = match (c.station_id, c.solar_system_id) {
            (Some(station), _) => Slot::new(c.character_id, station, crate::model::FLAG_HANGAR),
            (None, Some(system)) => Slot::new(c.character_id, system, FLAG_IN_SPACE),
            (None, None) => Slot::new(c.character_id, ship_id, crate::fitting::FLAG_CAPSULE),
        };
        edits.push(Edit::Move { item_id: pod, to });
    }
    let (more, inside) = contents_plan(
        w,
        c.character_id,
        ship_id,
        wreck,
        capsule.map(|(pod, _)| pod),
    );
    edits.extend(more);
    if store.get(ship_id).is_some() {
        edits.push(Edit::Destroy { item_id: ship_id });
    }
    DeathPlan {
        edits,
        capsule,
        inside,
    }
}

/// What a ship's death does to what is inside it: each item directly inside
/// moves into `wreck` whole ([`survives`], [`FLAG_IN_WRECK`], still owned by
/// `owner`) or is destroyed with everything under it; with no wreck all of it
/// is destroyed. `skip` (the capsule) is left alone. Returns the edits and
/// every item id that was inside, at any depth, the skipped one excepted.
fn contents_plan(
    w: &Registry,
    owner: i64,
    ship_id: i64,
    wreck: Option<i64>,
    skip: Option<i64>,
) -> (Vec<crate::item::Edit>, Vec<i64>) {
    use crate::item::{Edit, Slot};
    let store = w.store();
    let mut edits = Vec::new();
    let mut inside = Vec::new();
    let top: Vec<i64> = store
        .contents_of(ship_id)
        .map(|i| i.item_id)
        .filter(|id| skip != Some(*id))
        .collect();
    for item_id in top {
        // Everything under this one, itself first.
        let mut under = vec![item_id];
        let mut open = vec![item_id];
        while let Some(container) = open.pop() {
            for i in store.contents_of(container) {
                under.push(i.item_id);
                open.push(i.item_id);
            }
        }
        match wreck {
            Some(wreck_id) if survives(ship_id, item_id) => edits.push(Edit::Move {
                item_id,
                to: Slot::new(owner, wreck_id, FLAG_IN_WRECK),
            }),
            _ => edits.extend(under.iter().map(|&item_id| Edit::Destroy { item_id })),
        }
        inside.extend(under);
    }
    (edits, inside)
}

/// Remove every wreck whose time is up: its item (and anything in it) from
/// the store, its ball from every park in the system.
///
/// `RemoveBalls([id])`, the plural with **one** argument
/// (`destiny::BallsRemoved`), not the terminal singular: a wreck that
/// expires vanishes rather than explodes. `[A]` that TQ does not play an
/// effect either.
fn despawn_wrecks(w: &mut Registry, now: BlueTime) {
    let due: Vec<Wreck> = w
        .combat()
        .wrecks
        .values()
        .filter(|wr| wr.expires_at <= now)
        .copied()
        .collect();
    for wr in due {
        remove_wreck(w, wr.wreck_id);
    }
}

/// **A wreck out of space, now** (X-SALV): its record, its item and
/// everything in it destroyed, and `RemoveBalls` to its system. What the
/// two-hour clock does ([`despawn_wrecks`]) and a successful salvage does.
/// `false` for a wreck this run has no record of.
pub fn remove_wreck(w: &mut Registry, wreck_id: i64) -> bool {
    let Some(wr) = w.combat_mut().wrecks.remove(&wreck_id) else {
        return false;
    };
    remove_wreck_item(w, wr.wreck_id, wr.owner_id);
    let to = characters_in(w, wr.solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = destiny::BallsRemoved::new(to, stamp, vec![wr.wreck_id]) {
        w.schedule(Duration::ZERO, p);
    }
    true
}

/// A wreck's record moved to `position` (X-SALV: a tractor beam's pull).
/// The ball is the caller's to send. `false` for an unknown wreck.
pub fn set_wreck_position(w: &mut Registry, wreck_id: i64, position: [f64; 3]) -> bool {
    match w.combat_mut().wrecks.get_mut(&wreck_id) {
        Some(wr) => {
            wr.position = position;
            true
        }
        None => false,
    }
}

/// **A can in space** (GAPS-2, `crate::jettison`): a record for an item
/// the store already holds at the system, so that the grid shows it, the
/// listing opens it and Loot All takes from it, as for any wreck. The
/// caller made the item; the [`WRECK_LIFETIME`] clock is the record's
/// `expires_at`.
pub(crate) fn add_wreck(w: &mut Registry, wreck: Wreck) {
    w.combat_mut().wrecks.insert(wreck.wreck_id, wreck);
}

/// **A wreck record out of space, its item kept** (GAPS-2, a scooped can):
/// [`remove_wreck`] without the destruction. `RemoveBalls` goes to the
/// system. `None` for an id this run has no record of.
pub(crate) fn release_wreck(w: &mut Registry, wreck_id: i64) -> Option<Wreck> {
    let wr = w.combat_mut().wrecks.remove(&wreck_id)?;
    let to = characters_in(w, wr.solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = destiny::BallsRemoved::new(to, stamp, vec![wr.wreck_id]) {
        w.schedule(Duration::ZERO, p);
    }
    Some(wr)
}

/// A wreck record without a death (tests of what lies in a wreck).
#[cfg(test)]
pub(crate) fn insert_wreck(w: &mut Registry, wreck: Wreck) {
    w.combat_mut().wrecks.insert(wreck.wreck_id, wreck);
}

/// Destroy every wreck item the store holds and this run has no record of:
/// wrecks from before a restart, whose positions died with the process.
fn sweep_orphan_wrecks(w: &mut Registry) {
    w.combat_mut().swept = true;
    // #82: containers launched whole, depots and MTUs come back first.
    crate::mtu::restore(w);
    // #77: ships left in space with no pilot come back where they were.
    crate::pilotless::restore(w);
    let orphans: Vec<(i64, i64)> = w
        .store()
        .items()
        .iter()
        .filter(|i| i.group_id == GROUP_WRECK || crate::jettison::is_can_in_space(i))
        .filter(|i| !w.combat().wrecks.contains_key(&i.item_id))
        .map(|i| (i.item_id, i.owner_id))
        .collect();
    for (wreck_id, owner_id) in orphans {
        remove_wreck_item(w, wreck_id, owner_id);
    }
}

/// One wreck item and its contents out of the store, the pushes to its owner
/// on the clock.
fn remove_wreck_item(w: &mut Registry, wreck_id: i64, _owner_id: i64) {
    // The store's owner, not the slim's: an NPC's wreck is owned by the
    // character who earned it (GAP-LOOT) but the item by the NPC corporation.
    let Some(owner_id) = w.store().get(wreck_id).map(|i| i.owner_id) else {
        return;
    };
    // The wreck and everything in it, at any depth: a container that dropped
    // whole still holds its own contents.
    let mut edits = Vec::new();
    let mut open = vec![wreck_id];
    while let Some(container) = open.pop() {
        edits.push(crate::item::Edit::Destroy { item_id: container });
        open.extend(w.store().contents_of(container).map(|i| i.item_id));
    }
    // An NPC corporation has no session to tell.
    let to: &[i64] = if is_npc_owner(owner_id) {
        &[]
    } else {
        &[owner_id]
    };
    if let Err(e) = apply_on_the_clock_to(w, to, &edits) {
        crate::outln!("[combat] wreck {wreck_id} could not be removed: {e:?}");
    }
}

/// Every character in a system: the audience of anything that happens to a
/// ball there. An NPC is in the roster too, and has no park.
fn characters_in(w: &Registry, solar_system_id: i64) -> Vec<i64> {
    w.roster()
        .in_system(solar_system_id)
        .iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

// ---------------------------------------------------------------------------
// Loot
// ---------------------------------------------------------------------------

/// `idCheckers.IsNPC`: `maxSystemItem < id < minPlayerOwner`
/// (`idCheckers.py:283`, bounds 9999 and 90000000) **[V]**.
fn is_npc_owner(id: i64) -> bool {
    9_999 < id && id < 90_000_000
}

/// `idCheckers.IsNPCCorporation`: `minNPCCorporation <= id <=
/// maxNPCCorporation` (`idCheckers.py:323`; 1000000 and 1999999,
/// `inventorycommon/const.py:10-11`) **[V]**.
fn is_npc_corporation(id: i64) -> bool {
    (1_000_000..=1_999_999).contains(&id)
}

/// May `character_id` take from this wreck without becoming a thief?
///
/// **The client's own rule, `Park.HaveLootRight`** (`michelle.py:1959-1993`)
/// **[V]**, read against the slim [`wreck_parts`] sends -- `ownerID` the
/// victim and `lootRights = (victim, victim's corp, None, False)`:
///
/// - the slim's owner (`:1968`), or an NPC owner (`:1972`);
/// - `abandoned` (`:1979`) -- never, here;
/// - the rights' owner (`:1981`);
/// - **a player corporation** that is the rights' owner or corp (`:1983`):
///   two pilots in the same *NPC* corporation are strangers to each other;
/// - the rights' fleet (`:1985`) -- `None`, here;
/// - `crimewatchSvc.CanAttackFreely(slim)` (`:1987`): `[A]` not modelled,
///   false -- it needs limited engagements, which nothing here has.
///
/// The client asks this only when its safety is `Full` (`crimewatchSvc
/// .CheckCanTakeItems:650-666`, the group in `containerGroupsWithLootRights`
/// at `crimewatch/const.py:110-113`) **[V]**, and then refuses before calling
/// us (`LootTheftDeniedSafetyPreventsSuspect`, `invControllers.py:609-610`
/// for a drag and `:1610` for "Loot All") **[V]**. With safety below `Full`
/// the take is sent, and the taker becomes a suspect -- see [`looted`].
///
/// The zero-sec branch of `CanAttackFreely` (`crimewatchSvc.py:541-543`,
/// everyone's rights in zero sec) is **not** here: it is about the system,
/// not the wreck, and `crimewatch::theft` applies it.
pub fn has_loot_right(w: &Registry, character_id: i64, wr: &Wreck) -> bool {
    if character_id == wr.owner_id || is_npc_owner(wr.owner_id) || wr.abandoned {
        return true;
    }
    // `fleetID is not None and fleetID == session.fleetid` (`:1985`): the
    // wreck's snapshot against the taker's fleet now. Not limited to one
    // system -- the client has no such test.
    if wr.fleet_id.is_some() && wr.fleet_id == fleet_of(w, character_id) {
        return true;
    }
    let Some(corp) = w.character(character_id).map(|c| c.corporation_id) else {
        return false;
    };
    !is_npc_corporation(corp) && (corp == wr.owner_id || corp == wr.corp_id)
}

/// The id of the fleet `character_id` sits in, if any.
pub fn fleet_of(w: &Registry, character_id: i64) -> Option<i64> {
    w.fleets().fleet_of(character_id).map(|f| f.fleet_id)
}

/// GAP-LOOT: the character behind roster key `shooter` (a drone's owner), if
/// there is one. An NPC shooter has none.
fn credited_character(w: &Registry, shooter: i64) -> Option<i64> {
    if let Some(d) = w.modules().drones().get(shooter) {
        return w.character(d.owner_id).map(|c| c.character_id);
    }
    w.pilot(shooter)?.char_id
}

/// Add a volley's damage against an NPC to the wreck-owner tally. A volley
/// at a character, or by an NPC, books nothing.
fn book_npc_damage(w: &mut Registry, shooter: i64, victim: i64, dealt: f64) {
    if dealt <= 0.0 || !dealt.is_finite() || w.npcs().get(victim).is_none() {
        return;
    }
    let Some(who) = credited_character(w, shooter) else {
        return;
    };
    *w.combat_mut()
        .npc_damage
        .entry(victim)
        .or_default()
        .entry(who)
        .or_insert(0.0) += dealt;
}

/// `(owner, corp, fleet)` of the character who owns NPC `npc_id`'s wreck: the
/// mission's owner when the NPC is in a mission pocket, else the character
/// who did it the most damage (a tie goes to the lower id) -- EvEJS
/// `combatDestruction.js:120-224`. `[A]` the whole rule for TQ's wording; the
/// client only reads the slim. `None` when no character hurt it.
fn npc_wreck_owner(w: &Registry, npc_id: i64) -> Option<(i64, i64, Option<i64>)> {
    let top = crate::mission::pocket_owner(w, npc_id).or_else(|| {
        w.combat()
            .npc_damage
            .get(&npc_id)?
            .iter()
            .fold(None::<(i64, f64)>, |best, (&who, &dmg)| match best {
                Some((_, d)) if d >= dmg => best,
                _ => Some((who, dmg)),
            })
            .map(|(who, _)| who)
    })?;
    let corp = w.character(top)?.corporation_id;
    Some((top, corp, fleet_of(w, top)))
}

/// `beyonce.CmdAbandonLoot(wreckIDs)`: the owner of each named wreck or can
/// in their system gives up the rights (`abandoned` on the slim); anyone
/// else's, and unknown ids, are ignored. `OnSlimItemChange` goes to the
/// system for each one that changed.
pub fn abandon_loot(w: &mut Registry, character_id: i64, ids: &[i64]) -> usize {
    let Some(system) = w.roster().get(character_id).map(|p| p.solar_system_id) else {
        return 0;
    };
    let mut n = 0;
    for &id in ids {
        let Some(wr) = w.combat_mut().wrecks.get_mut(&id) else {
            continue;
        };
        if wr.owner_id != character_id || wr.solar_system_id != system || wr.abandoned {
            continue;
        }
        wr.abandoned = true;
        let wr = *wr;
        let (_, slim) = wreck_parts(w, &wr);
        let to = characters_in(w, system);
        let stamp = w.next_destiny_stamp();
        if let Some(p) = destiny::SlimItemChange::new(to, stamp, slim) {
            w.schedule(Duration::ZERO, p);
        }
        n += 1;
    }
    n
}

/// Take `item` (or `quantity` of it) out of the wreck it lies in, into
/// `ship_id`'s cargo hold, or say why not.
///
/// The call is the hold's own: "Loot All" is `ShipCargo().AddItems(items)`
/// (`invControllers.LootAll:1600-1605`), and a drag onto the hold is the same
/// method -- one item becomes `Add(itemID, wreckID, qty=n, flag=flagCargo)`
/// (`_AddItem:287` -> `__AddItem`), several `MultiAdd([ids], wreckID,
/// flag=flagCargo)` (`AddItems:686-688`), both on the active ship's inventory
/// **[V]**. `fitting::plan_to_cargo` sends an item that lies in a wreck here.
///
/// - **only the active ship's hold, in space, in the wreck's system** -- the
///   client offers nothing else (`ShipCargo` is the active ship's);
/// - **within [`LOOT_RANGE`]** surface to surface, plus [`LOOT_RANGE_SLACK`];
///   further is `NotCloseEnoughToLoot` (`[A]` raise site; the key and its
///   `maxdist` argument are the client's, `findings/user-errors.tsv`). The
///   looter's position must be current: the caller advances it first;
/// - **within the hold's capacity** (`fitting::check_cargo_room_in`,
///   `NotEnoughCargoSpace` -- the key the client's quantity prompt retries
///   on, `invControllers.py:319-360`) **[V]**;
/// - **anyone, rights or not** -- the server does not refuse a thief, TQ
///   flags one ([`looted`]).
///
/// The item becomes the looter's, at `flagCargo`; so does anything inside
/// it, where it is.
pub fn plan_loot(
    w: &Registry,
    c: &crate::model::Character,
    ship_id: i64,
    item: &crate::item::Item,
    quantity: Option<i64>,
) -> Result<Vec<crate::item::Edit>, crate::fitting::Refusal> {
    use crate::fitting::Refusal;
    use crate::item::{Edit, Slot};
    // A wreck, or a hacked can (HK3) taken the same way.
    let wr = crate::hacking_loot::loot_source(w, item.location_id).ok_or(Refusal::NotAvailable)?;
    if ship_id != c.ship_id {
        return Err(Refusal::Unsupported(
            "Loot goes into your active ship's cargo hold.",
        ));
    }
    let me = w
        .roster()
        .get(c.character_id)
        .filter(|p| p.ship_id == ship_id && p.solar_system_id == wr.solar_system_id)
        .ok_or(Refusal::NotAvailable)?;
    let radius = destiny::hull::of_in(w.content(), c.ship_type_id).radius;
    let surface = crate::space_state::distance(me.motion.position, wr.position)
        - f64::from(radius)
        - crate::hacking_loot::can_radius(w, wr.wreck_id)
            .unwrap_or(f64::from(destiny::WRECK_RADIUS));
    if surface > LOOT_RANGE + LOOT_RANGE_SLACK {
        return Err(Refusal::TooFar {
            maxdist: LOOT_RANGE,
        });
    }
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    crate::fitting::check_cargo_room_in(
        w.content(),
        w.store(),
        ship_id,
        item.type_id,
        part.unwrap_or(item.quantity),
    )?;
    let to = Slot::new(c.character_id, ship_id, crate::fitting::FLAG_CARGO);
    let mut edits = vec![match part {
        Some(quantity) => Edit::Split {
            item_id: item.item_id,
            quantity,
            to,
        },
        None => Edit::Move {
            item_id: item.item_id,
            to,
        },
    }];
    if part.is_none() {
        // A container's contents: still inside it, now the looter's.
        let mut open = vec![item.item_id];
        while let Some(container) = open.pop() {
            for i in w.store().contents_of(container) {
                edits.push(Edit::Move {
                    item_id: i.item_id,
                    to: Slot::new(c.character_id, i.location_id, i.flag_id),
                });
                open.push(i.item_id);
            }
        }
    }
    Ok(edits)
}

/// After a take from `wreck_id` by `character_id`: what the rest of the
/// world hears.
///
/// - **The wreck looted out** turns empty on every screen in the system: a
///   [`destiny::SlimItemChange`] with `isEmpty = true`, on the clock. Only on
///   that edge -- a take needs something in the wreck, so it was not empty
///   before.
/// - **A take without loot rights** ([`has_loot_right`]) is theft: allowed,
///   and the taker a suspect where the system has suspects
///   (`crimewatch::theft`) -- the consequence the client's refusal text
///   names, "This action would make you a Suspect" (283609) **[V]**.
pub fn looted(w: &mut Registry, character_id: i64, wreck_id: i64) {
    let Some(wr) = w.combat().wreck(wreck_id).copied() else {
        return;
    };
    if !has_loot_right(w, character_id, &wr) {
        crate::crimewatch::theft_from(
            w,
            character_id,
            Some(wr.owner_id),
            wr.solar_system_id,
            crate::pyutil::filetime_now(),
        );
    }
    if w.store().contents_of(wreck_id).next().is_some() {
        return;
    }
    let (_, slim) = wreck_parts(w, &wr);
    let to = characters_in(w, wr.solar_system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = destiny::SlimItemChange::new(to, stamp, slim) {
        w.schedule(Duration::ZERO, p);
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// [`crate::notify::Addressed`], which started here: the store's pushes
/// leave a clock tick through it, `to` the victim.
pub use crate::notify::Addressed;

/// `OnPodKilled(destroyedPodID)`, one argument: `gameui:1203` stores it so
/// that the next session change is recognised as a podding (§5.2) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct PodKilled {
    pub to: Vec<i64>,
    pub pod_id: i64,
}

impl Push for PodKilled {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnPodKilled", vec![PyValue::Int(self.pod_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnPlayerPodDeath()`, **zero arguments**: the white-out
/// (`evespacescene/playerdeath.py:56`, §5.2) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPodDeath {
    pub to: Vec<i64>,
}

impl Push for PlayerPodDeath {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnPlayerPodDeath", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// One ring moved: `OnModuleAttributeChange(ownerID, itemKey, attributeID,
/// time, newValue, oldValue, wallclockTime)`, seven positionals
/// (`godma.py:1575`) [V].
///
/// - `ownerID` is the victim's **character**: `godma:1591` drops a change
///   unless `itemKey == session.charid or ownerID == session.charid`, after a
///   traceback saying you do not own your own ship [V].
/// - `time` is **the moment of sending**, a filetime. It was `None` on the
///   reasoning that `UpdateAttribute:2049` takes the client's own clock; that
///   is the PRIME path. A change goes `ApplyAttributeChange` ->
///   `CreateChargedAttribute(..., time, ...)` with the value as sent, so a
///   `None` made the shield's charged attribute `[value, None, tau, cap]` and
///   every HUD poll raised `TypeError: unsupported operand type(s) for -:
///   'NoneType' and 'long'` in `GetChargeValue` (116 tracebacks in one
///   `fci_doc_0` run, 2026-10-28: FCI-FIX2). A real time restarts the
///   shield's charged curve at arrival, which is what was wanted.
/// - Values are floats, the old value included (§23.5's float/int trap).
/// - `wallclockTime = None` always passes `_IsAttributeChangeRelevant:2036`
///   [V].
///
/// A plain notification rather than `DoDestinyUpdate`'s `dogmaMessages`:
/// both reach godma (§1.3), and this way every Destiny batch here keeps the
/// two-argument envelope the rest of `destiny.rs` pins. The cost is that the
/// rings and the bars arrive in two frames; both are polled (1-2 Hz), so the
/// player cannot see the difference (§0.2 point 2).
#[derive(Debug, Clone, PartialEq)]
pub struct RingUpdate {
    pub to: Vec<i64>,
    pub owner_id: i64,
    pub ship_id: i64,
    pub change: RingChange,
}

impl Push for RingUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnModuleAttributeChange",
            vec![
                PyValue::Int(self.owner_id),
                PyValue::Int(self.ship_id),
                PyValue::Int(self.change.attribute_id),
                PyValue::Long(crate::pyutil::filetime_now()),
                PyValue::Real(self.change.new),
                PyValue::Real(self.change.old),
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnDamageMessage(argsDict)`: one line of the combat log.
///
/// A plain notification: `godma`, `bracketMgr` and `targetInBar` all list it
/// in `__notifyevents__` (§2) [V]. The dict is `DamageMessage::entries()`,
/// encoded here: keys and `attackType` as Python `str`, `owner` as
/// `(UE_OWNERID, id)`, `damageAttributes` as `{attributeID: amount}`.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatMessage {
    pub to: Vec<i64>,
    pub message: DamageMessage,
}

impl CombatMessage {
    fn dict(&self) -> PyValue {
        let value = |v: MessageValue| match v {
            MessageValue::Int(i) => PyValue::Int(i),
            MessageValue::Real(f) => PyValue::Real(f),
            MessageValue::Bool(b) => PyValue::Bool(b),
            MessageValue::Str(s) => crate::pyutil::bstr(s),
            MessageValue::OwnerRef(id) => {
                PyValue::Tuple(vec![PyValue::Int(damage::UE_OWNERID), PyValue::Int(id)])
            }
            // `PyValue::Dict` is `(value, key)` on the wire.
            MessageValue::Amounts(rows) => PyValue::Dict(
                rows.into_iter()
                    .map(|(attr, amount)| (PyValue::Real(amount), PyValue::Int(attr)))
                    .collect(),
            ),
        };
        crate::pyutil::pydict(
            self.message
                .entries()
                .into_iter()
                .map(|(k, v)| (k, value(v)))
                .collect(),
        )
    }
}

impl Push for CombatMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnDamageMessage", vec![self.dict()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

#[cfg(test)]
mod par_tests;

#[cfg(test)]
pub(crate) mod tests {
    /// A ring change carries a real time: `None` made the shield's charged
    /// attribute `[value, None, ...]` and the HUD's `GetChargeValue` raised on
    /// every poll (FCI-FIX2, fci_doc_0).
    #[test]
    fn a_ring_update_carries_a_filetime_not_none() {
        let p = RingUpdate {
            to: vec![1],
            owner_id: 1,
            ship_id: 2,
            change: crate::damage::RingChange {
                attribute_id: 264,
                old: 450.0,
                new: 400.0,
            },
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "OnModuleAttributeChange");
        assert!(
            matches!(args[3], PyValue::Long(t) if t > 130_000_000_000_000_000),
            "{args:?}"
        );
    }

    // ---- DOC-0 K-TURRET: invented numbers throughout ----

    /// A small gun's invented numbers.
    const APP: Application = Application {
        optimal: 10_000.0,
        falloff: 5_000.0,
        tracking: 0.04,
        sig_resolution: 40.0,
    };

    fn aim(distance: f64, transversal: f64, signature: f64) -> crate::drone_skills::Aim {
        crate::drone_skills::Aim {
            distance,
            transversal,
            signature,
        }
    }

    /// An invented world seed for the tests.
    const SEED: u64 = 0x5eed_0000_0000_0001;

    /// #137: a disruptor takes shots away from an entity's gun by the ratio
    /// of its disrupted chance to its own, and nothing else does.
    #[test]
    fn a_disruptor_takes_a_share_of_an_entitys_volleys() {
        let near = aim(5_000.0, 100.0, 40.0);
        assert_eq!(disrupted_share(APP, (1.0, 1.0, 1.0), near), None);
        let base = APP.chance(near);
        let slow = disrupted_share(APP, (1.0, 1.0, 0.5), near).expect("a share");
        assert!(slow < 1.0 && slow > 0.0, "{slow}");
        let want = disrupted(APP, (1.0, 1.0, 0.5)).chance(near) / base;
        assert!((slow - want).abs() < 1e-12);
        // Past the falloff cut the gun could not hit: nothing to take away.
        assert_eq!(
            disrupted_share(APP, (0.5, 0.5, 0.5), aim(1.0e6, 0.0, 40.0)),
            None
        );
        // Help (a factor above 1) cannot raise the share past 1.
        assert_eq!(disrupted_share(APP, (1.0, 1.0, 2.0), near), Some(1.0));
        // Halving optimal and falloff at range costs more than tracking alone.
        let far = aim(12_000.0, 0.0, 40.0);
        let ranged = disrupted_share(APP, (0.5, 0.5, 1.0), far).expect("a share");
        assert!(ranged < 0.5, "{ranged}");
    }

    /// #137: the reach factor follows the entity's told optimal factor, and
    /// is `None` while nothing disrupts it.
    #[test]
    fn an_entitys_reach_follows_its_optimal_factor() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        assert_eq!(entity_gun_reach(&reg, 12), None);
        reg.ext_mut::<crate::ewar::Ewars>().factors_for_test(
            12,
            crate::ewar::Factors {
                optimal: 0.6,
                ..crate::ewar::Factors::default()
            },
        );
        assert_eq!(entity_gun_reach(&reg, 12), Some(0.6));
        assert_eq!(entity_gun_reach(&reg, 13), None);
    }

    /// REVIEW-FIX2: the world's seed changes which cycles hit.
    #[test]
    fn review_fix2_the_roll_depends_on_the_worlds_seed() {
        let a = aim(8_000.0, 800.0, 100.0);
        let hits = |seed: u64| -> Vec<bool> {
            (0..400)
                .map(|at| application(&APP, a, 5, at, seed).quality > 0)
                .collect()
        };
        assert_eq!(hits(SEED), hits(SEED), "a world replays the same");
        assert_ne!(
            hits(SEED),
            hits(SEED ^ 0xdead_beef),
            "another world rolls another"
        );
    }

    #[test]
    fn a_turret_shot_inside_optimal_at_rest_always_hits() {
        let a = aim(5_000.0, 0.0, 40.0);
        assert_eq!(APP.chance(a), 1.0);
        for at in 0..200 {
            let v = application(&APP, a, 77, at, SEED);
            assert!(v.quality > 0 && v.multiplier > 0.0, "cycle {at} missed");
        }
    }

    #[test]
    fn a_turret_far_beyond_falloff_misses() {
        let a = aim(
            APP.optimal + (RANGE_CUT_FALLOFFS + 1.0) * APP.falloff,
            0.0,
            400.0,
        );
        assert_eq!(APP.chance(a), 0.0, "the hard cut");
        for at in 0..200 {
            assert_eq!(application(&APP, a, 77, at, SEED).quality, 0);
        }
        // Just inside the cut the chance is small but not zero.
        let edge = aim(
            APP.optimal + (RANGE_CUT_FALLOFFS - 0.1) * APP.falloff,
            0.0,
            40.0,
        );
        let c = APP.chance(edge);
        assert!(c > 0.0 && c < 0.01, "{c}");
    }

    #[test]
    fn a_fast_small_target_beats_slow_tracking() {
        // Same distance and speed across the line: a big slow-turning target
        // is hit far more often than a small one that is no bigger than the
        // gun's resolution.
        let big = APP.chance(aim(8_000.0, 800.0, 400.0));
        let small = APP.chance(aim(8_000.0, 800.0, 30.0));
        assert!(big > 0.9, "{big}");
        assert!(small < 0.001, "{small}");
        let hits = |a| {
            (0..2000)
                .filter(|at| application(&APP, a, 5, *at, SEED).quality > 0)
                .count()
        };
        assert!(hits(aim(8_000.0, 800.0, 400.0)) > 1500);
        assert!(hits(aim(8_000.0, 800.0, 30.0)) < 20);
    }

    #[test]
    fn a_wrecking_hit_scales_the_damage() {
        use crate::drone_skills::{WRECK_MULTIPLIER, WRECK_ROLL};
        let v = crate::drone_skills::verdict(1.0, 0.0, WRECK_ROLL / 2.0);
        assert_eq!((v.quality, v.multiplier), (6, WRECK_MULTIPLIER));
        let volley = Damage {
            em: 0.0,
            explosive: 0.0,
            kinetic: 10.0,
            thermal: 20.0,
        };
        let d = crate::drone_skills::scaled(volley, v.multiplier);
        assert_eq!((d.kinetic, d.thermal), (30.0, 60.0));
    }

    #[test]
    fn the_roll_is_deterministic_per_cycle() {
        let a = aim(12_000.0, 100.0, 100.0);
        for at in [1_i64, 5_000, 123_456_789] {
            assert_eq!(
                application(&APP, a, 9, at, SEED),
                application(&APP, a, 9, at, SEED)
            );
        }
        // ...and different cycles roll differently.
        let distinct = (0..50)
            .map(|at| application(&APP, a, 9, at, SEED).multiplier.to_bits())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(distinct.len() > 5);
    }

    /// EWAR-2B: a tracking disruptor lowers the chance to hit a moving
    /// target, and a computer (factors above 1) raises it.
    #[test]
    fn a_tracking_disruptor_lowers_the_chance_to_hit() {
        let a = aim(12_000.0, 600.0, 100.0);
        let plain = APP.chance(a);
        let hurt = disrupted(APP, (0.8, 0.7, 0.6)).chance(a);
        let helped = disrupted(APP, (1.3, 1.3, 1.3)).chance(a);
        assert!(hurt < plain && plain < helped, "{hurt} {plain} {helped}");
        assert_eq!(disrupted(APP, (1.0, 1.0, 1.0)).chance(a), plain);
        // Range cuts follow the disrupted falloff.
        let far = aim(
            APP.optimal + RANGE_CUT_FALLOFFS * APP.falloff - 1.0,
            0.0,
            100.0,
        );
        assert!(APP.chance(far) > 0.0);
        assert_eq!(disrupted(APP, (0.8, 0.7, 1.0)).chance(far), 0.0);
    }

    #[test]
    fn a_gun_with_no_tracking_still_hits_and_the_switch_defaults_on() {
        let reg = alice_has_bob_locked();
        assert!(reg.combat().turret_application());
        let c = blaster_cycle(1, 0);
        assert!(
            turret_numbers(&reg, &c).is_none(),
            "the fixture gun states no tracking"
        );
    }

    use super::*;
    use std::time::Instant;

    pub(crate) const ALICE: i64 = 1_000_001;
    pub(crate) const BOB: i64 = 1_000_002;
    pub(crate) const ALICE_SHIP: i64 = 100_000_001;
    pub(crate) const BOB_SHIP: i64 = 100_000_002;
    const BLASTER: i64 = 100_000_050;
    const BOB_GUN: i64 = 100_000_051;
    pub(crate) const ALICE_POD: i64 = 100_000_060;
    pub(crate) const BOB_POD: i64 = 100_000_061;
    const RIFTER: i64 = 587;
    pub(crate) const SYSTEM: i64 = 30_000_001;

    /// Alice and Bob in Rifters, both undocked into the same system, each
    /// with their capsule inside at `flagCapsule` (what `ship.Board` leaves,
    /// `docs/lanes/fitting.md` §1.2), Alice with Bob locked and a Civilian
    /// Light Electron Blaster fitted to high slot 0 as a singleton -- the
    /// shape `modules::alice_with_a_gun` uses -- and Bob with one too.
    pub(crate) fn alice_has_bob_locked() -> Registry {
        locked_with(&[])
    }

    /// [`alice_has_bob_locked`] with more pilots undocked beside them, each
    /// `(character, ship)` in a Rifter with nothing fitted.
    fn locked_with(others: &[(i64, i64)]) -> Registry {
        locked_on(others, None)
    }

    /// [`locked_with`] on other loaded data (`Registry::with_content`).
    pub(crate) fn locked_on(
        others: &[(i64, i64)],
        content: Option<std::sync::Arc<crate::content::Content>>,
    ) -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: RIFTER,
            ..crate::model::Character::default()
        };
        let mut characters = vec![
            pilot(ALICE, "Alice", ALICE_SHIP),
            pilot(BOB, "Bob", BOB_SHIP),
        ];
        characters.extend(others.iter().map(|&(c, ship)| pilot(c, "Other", ship)));
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters,
            ..crate::model::SandboxConfig::default()
        });
        if let Some(content) = content {
            reg = reg.with_content(content);
        }
        let rifter = *crate::item::find_in(reg.content(), RIFTER).expect("Rifter is spawnable");
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = rifter.type_id;
            i.group_id = rifter.group_id;
        }
        for (gun, owner, ship) in [(BLASTER, ALICE, ALICE_SHIP), (BOB_GUN, BOB, BOB_SHIP)] {
            items.push(crate::item::Item {
                item_id: gun,
                type_id: 3640,
                owner_id: owner,
                location_id: ship,
                // `flagHiSlot0` [V].
                flag_id: 27,
                quantity: 1,
                group_id: 53,
                category_id: 7,
                singleton: true,
            });
        }
        for (pod, owner, ship) in [(ALICE_POD, ALICE, ALICE_SHIP), (BOB_POD, BOB, BOB_SHIP)] {
            items.push(crate::item::Item {
                item_id: pod,
                type_id: damage::CAPSULE_TYPE_ID,
                owner_id: owner,
                location_id: ship,
                flag_id: crate::fitting::FLAG_CAPSULE,
                quantity: 1,
                group_id: crate::fitting::GROUP_CAPSULE,
                category_id: crate::model::CATEGORY_SHIP,
                singleton: true,
            });
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: BOB_POD + 1,
            ..crate::store::Snapshot::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        for &(c, _) in others {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        let _ = reg.take_pending();
        reg
    }

    /// **A fight on another grid reaches nobody on this one** (GRID-B,
    /// `docs/lanes/grid-research.md` §5 "Kill"). Alice kills Bob 1 AU from
    /// the station where Carol sits: the bars, the guns' effects and the
    /// kill batch go to the two of them and not to her, and the wreck is
    /// never added to her park, then or at the next reconcile.
    #[test]
    fn an_off_grid_fight_reaches_no_one_off_grid() {
        const CAROL: i64 = 1_000_003;
        const CAROL_SHIP: i64 = 100_000_003;
        let mut reg = locked_with(&[(CAROL, CAROL_SHIP)]);
        let far = [1.0e12, 0.0, 0.0];
        for (who, off) in [(ALICE, 0.0), (BOB, 2_000.0)] {
            let p = reg.roster_mut().get_mut(who).expect("in space");
            p.motion = crate::space_state::Motion::arrived([far[0] + off, 0.0, 0.0], [0.0; 3]);
            p.since = crate::space_state::now_seconds();
        }
        crate::grid::reconcile_in(&mut reg, SYSTEM);
        let _ = reg.take_due(Instant::now() + Duration::from_secs(1));
        let carols = reg.ext::<crate::grid::Parks>().of(CAROL).cloned();
        let carols = carols.expect("Carol's park is modelled");
        assert!(!carols.contains(&ALICE_SHIP) && !carols.contains(&BOB_SHIP));

        kill_bob(&mut reg);
        let mut out = due(&mut reg);
        out.extend(reg.take_pending().iter().map(|(_, p)| {
            let (n, a) = p.to_push();
            (p.audience().unwrap_or_default(), n, a)
        }));
        let fight: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate" || *n == "OnSpecialFX")
            .collect();
        assert!(
            fight.iter().any(|(_, _, a)| entries(a)
                .iter()
                .any(|(n, _)| *n == crate::pyutil::bstr("RemoveBall"))),
            "setup: Bob died"
        );
        for (to, n, _) in &fight {
            assert!(!to.contains(&CAROL), "{n} reached Carol 1 AU away");
            assert!(to.contains(&ALICE) || to.contains(&BOB), "{n} to {to:?}");
        }
        crate::grid::reconcile_in(&mut reg, SYSTEM);
        for p in reg.take_due(Instant::now() + Duration::from_secs(1)) {
            assert!(
                !p.audience().unwrap_or_default().contains(&CAROL),
                "Carol is told nothing of a wreck on another grid: {p:?}"
            );
        }
    }

    /// One cycle of Alice's civilian blaster at Bob, `index` cycles in.
    fn blaster_cycle(at: i64, index: i64) -> Cycle {
        Cycle {
            character_id: ALICE,
            ship_id: ALICE_SHIP,
            module_id: BLASTER,
            type_id: 3640,
            effect_id: 10,
            target_id: Some(BOB_SHIP),
            at,
            index,
            charge_type_id: None,
        }
    }

    /// Everything on the clock queue, as `(audience, name, args)`.
    pub(crate) fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(1))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// The `(funcName, args)` of every destiny entry in a batch.
    fn entries(args: &[PyValue]) -> Vec<(PyValue, Vec<PyValue>)> {
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
                let PyValue::Tuple(a) = &call[1] else {
                    panic!("args")
                };
                (call[0].clone(), a.clone())
            })
            .collect()
    }

    fn real(v: &PyValue) -> f64 {
        match v {
            PyValue::Real(f) => *f,
            other => panic!("a float, got {other:?}"),
        }
    }

    /// **End to end through the seam**: Alice clicks the gun
    /// (`dogmaIM.Activate`), the world clock runs `modules::tick` and then
    /// [`tick`], and Bob's bar update is on the clock queue. This is the
    /// whole server path a human test exercises, minus the sockets.
    #[test]
    fn activating_a_civilian_gun_on_a_locked_ship_damages_it_on_the_next_tick() {
        let mut reg = alice_has_bob_locked();
        reg.modules_mut().set_online(BLASTER, true);
        let now = crate::pyutil::filetime_now();
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            BLASTER,
            "targetAttack",
            Some(BOB_SHIP),
            1000,
            now,
        )
        .expect("the gun activates");
        let _ = reg.take_pending();
        crate::modules::tick(&mut reg, now);
        tick(&mut reg, now);
        assert!(
            reg.combat().ship(BOB_SHIP).is_some(),
            "Bob has a damage record"
        );
        let hit = due(&mut reg).into_iter().any(|(_, n, args)| {
            n == "DoDestinyUpdate"
                && entries(&args).iter().any(|(name, a)| {
                    *name == crate::pyutil::bstr("OnDamageStateChange")
                        && a[0] == PyValue::Int(BOB_SHIP)
                })
        });
        assert!(hit, "Bob's bars were sent by the clock");
    }

    /// A turret hits a target that is aligning for a warp: the activation
    /// stamp is ahead, so Bob is still in the open.
    #[test]
    fn a_turret_damages_a_target_that_is_aligning_for_warp() {
        let mut reg = alice_has_bob_locked();
        reg.roster_mut().get_mut(BOB).expect("in space").motion.mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 + 3_600,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        reg.modules_mut().set_online(BLASTER, true);
        let now = crate::pyutil::filetime_now();
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            BLASTER,
            "targetAttack",
            Some(BOB_SHIP),
            1000,
            now,
        )
        .expect("the gun activates");
        let _ = reg.take_pending();
        crate::modules::tick(&mut reg, now);
        tick(&mut reg, now);
        assert!(
            reg.combat().ship(BOB_SHIP).is_some(),
            "Bob, still aligning, has a damage record"
        );
    }

    /// **A turret's volley lands on an anchorable deployable** (#172, NW-3):
    /// the unit loses hull points, and is gone when they run out.
    #[test]
    fn a_turret_volley_takes_hull_points_off_a_deployable() {
        const UNIT_TYPE: i64 = 990_172_050;
        const HULL_POINTS: f64 = 5_000.0;
        let rows = format!(
            "{}type\t{UNIT_TYPE}\t{}\t22\t1\t1\t10.0\t50.0\t0.0\t1.0\t0\t\t{}={HULL_POINTS}\tInvented Unit\n",
            crate::typedata::FIXTURE,
            crate::anchor::GROUP_MOBILE_DISRUPTOR,
            crate::anchor::ATTR_HULL_HP,
        );
        let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut reg = locked_on(&[], Some(std::sync::Arc::new(content)));
        let stack = {
            let changes = reg
                .apply_items(&[crate::item::Edit::Create {
                    type_id: UNIT_TYPE,
                    quantity: 1,
                    to: crate::item::Slot::new(ALICE, ALICE_SHIP, crate::fitting::FLAG_CARGO),
                }])
                .expect("a known type");
            changes
                .iter()
                .find_map(|c| match c {
                    crate::item::Change::Created { item_id, .. } => Some(*item_id),
                    _ => None,
                })
                .expect("created")
        };
        let unit = crate::anchor::launch_one(&mut reg, ALICE, stack).expect("launched");
        reg.locks_mut().add(ALICE_SHIP, unit);
        let t0 = crate::pyutil::filetime_now();
        let shot = Cycle {
            target_id: Some(unit),
            ..blaster_cycle(t0, 0)
        };
        on_cycles(&mut reg, vec![shot], t0);
        let left = crate::anchor::hull_left(&reg, unit).expect("still there");
        assert!(left < HULL_POINTS, "the volley took points: {left}");
        // Enough volleys and it is gone.
        for i in 0..100_000 {
            if crate::anchor::hull_left(&reg, unit).is_none() {
                break;
            }
            on_cycles(
                &mut reg,
                vec![Cycle {
                    index: i + 1,
                    ..shot
                }],
                t0,
            );
        }
        assert!(!crate::anchor::is_unit(&reg, unit), "destroyed");
        assert!(reg.item(unit).is_none());
    }

    /// **A volley is aggression**: one cycle of Alice's gun at Bob, through
    /// the same seam the clock uses, arms Alice's weapons timer and both
    /// pilots' capsuleer log-off timer (`crate::crimewatch`'s tooltips,
    /// 283268/283269) [V] -- and the station then refuses Alice and not Bob.
    #[test]
    fn a_volley_at_a_capsuleer_starts_the_crimewatch_timers() {
        use crate::crimewatch::{Timer, refuse_dock};
        let mut reg = alice_has_bob_locked();
        let t0 = crate::pyutil::filetime_now();
        on_cycles(&mut reg, vec![blaster_cycle(t0, 0)], t0);
        let cw = reg.combat().crimewatch();
        assert!(cw.expiry(ALICE, Timer::Weapons, t0).is_some());
        assert!(cw.expiry(ALICE, Timer::Pvp, t0).is_some());
        assert!(cw.expiry(BOB, Timer::Pvp, t0).is_some());
        assert!(cw.expiry(BOB, Timer::Weapons, t0).is_none());
        assert!(refuse_dock(&reg, Some(ALICE), t0).is_err());
        assert!(refuse_dock(&reg, Some(BOB), t0).is_ok());
    }

    /// The weapon fixture and the module fixture are one set of facts:
    /// every offensive module of `modules::FIXTURE` has a weapon row with
    /// its multiplier, and a module's own damage attributes are the row's
    /// damage. Both are invented now (O3); this stops them drifting apart.
    #[test]
    fn the_weapon_table_agrees_with_the_module_table() {
        let c = crate::content::Content::fixture();
        for t in crate::modules::FIXTURE
            .iter()
            .filter(|t| t.effect.is_some_and(|e| e.is_offensive))
        {
            let w = weapon_in(&c, t.type_id).unwrap_or_else(|| panic!("{} has no row", t.name));
            assert_eq!(Some(w.multiplier), t.attribute(64), "{}", t.name);
            let own = [114, 116, 117, 118].map(|a| t.attribute(a));
            match w.damage {
                Some(d) => assert_eq!(
                    own,
                    [
                        Some(d.em),
                        Some(d.explosive),
                        Some(d.kinetic),
                        Some(d.thermal)
                    ],
                    "{}",
                    t.name
                ),
                None => assert_eq!(own, [None; 4], "{} has damage of its own", t.name),
            }
        }
    }

    /// **A loaded gun throws its charge's damage times its own multiplier**,
    /// the client's formula (`fittingDogmaLocationUtil.pyc:292-303`) [V]:
    /// a projectile turret with each round the fixture grants.
    #[test]
    fn a_loaded_gun_throws_the_charges_damage_times_its_multiplier() {
        let c = crate::content::Content::fixture();
        let m = weapon_in(&c, 484).expect("a gun").multiplier;
        for round in [184, 185] {
            let d = charge_in(&c, round).expect("a charge").damage;
            let v = volley_loaded_in(&c, 484, Some(round)).expect("it throws");
            assert_eq!(
                [v.em, v.explosive, v.kinetic, v.thermal],
                [d.em * m, d.explosive * m, d.kinetic * m, d.thermal * m]
            );
        }
    }

    /// **A gun whose damage lives on its charge throws nothing empty** --
    /// `:296` reads the module's own damage, and 484/486/454 have none.
    /// A civilian gun is unchanged: its damage is its own.
    #[test]
    fn an_empty_gun_throws_nothing_and_a_civilian_gun_needs_no_charge() {
        let c = crate::content::Content::fixture();
        assert_eq!(volley_in(&c, 484), None);
        assert_eq!(volley_in(&c, 486), None);
        assert_eq!(volley_in(&c, 454), None);
        assert!(volley_in(&c, 3640).is_some(), "the civilian blaster");
        assert_eq!(
            volley_loaded_in(&c, 484, Some(3640)),
            None,
            "a module is not a charge"
        );
    }

    /// Every charge of [`FIXTURE_CHARGES`] is one the store can hold, as a
    /// stackable charge of a group some fixture gun takes -- a row that
    /// could never be loaded would be a row nobody tested.
    #[test]
    fn every_charge_is_spawnable_and_fits_a_gun() {
        let content = crate::content::Content::fixture();
        for c in FIXTURE_CHARGES {
            let t = crate::item::find_in(&content, c.type_id)
                .unwrap_or_else(|| panic!("{} is not spawnable", c.name));
            assert_eq!(t.category_id, crate::item::CATEGORY_CHARGE, "{}", c.name);
            assert!(!t.singleton, "{} stacks", c.name);
            assert!(t.volume > 0.0, "{} has a volume to count rounds by", c.name);
            assert!(
                crate::modules::FIXTURE
                    .iter()
                    .any(|m| m.attribute(604) == Some(t.group_id as f64)
                        && m.attribute(128) == Some(c.charge_size)),
                "{} fits no gun",
                c.name
            );
        }
    }

    /// **Through the seam**: a cycle of a 125mm Gatling that fired EMP S hurts
    /// Bob, and the same cycle with nothing loaded does not touch him.
    #[test]
    fn only_a_cycle_that_fired_a_round_does_damage() {
        let mut reg = alice_has_bob_locked();
        let t0 = crate::pyutil::filetime_now();
        let gatling = |charge_type_id| Cycle {
            type_id: 484,
            effect_id: 34,
            charge_type_id,
            ..blaster_cycle(t0, 0)
        };
        on_cycles(&mut reg, vec![gatling(None)], t0);
        assert!(reg.combat().ship(BOB_SHIP).is_none(), "an empty gun");
        on_cycles(&mut reg, vec![gatling(Some(185))], t0);
        assert!(reg.combat().ship(BOB_SHIP).is_some(), "a loaded one");
    }

    /// Every hull a pilot can fly has a wreck, except the capsule, which
    /// dies the capsule's death and leaves none (`types.wreckTypeID` is
    /// absent for 670) [V] -- on the fixture content.
    #[test]
    fn every_hull_but_the_capsule_leaves_a_wreck() {
        let c = crate::content::Content::fixture();
        for h in destiny::hull::FIXTURE {
            assert_eq!(
                wreck_type_in(&c, h.type_id).is_some(),
                h.type_id != damage::CAPSULE_TYPE_ID,
                "hull {}",
                h.type_id
            );
        }
    }

    /// Every wreck a fixture hull leaves is a type the store can hold, in
    /// `groupWreck` -- so `Edit::Create` never refuses a kill's plan.
    #[test]
    fn every_wreck_is_a_type_the_store_knows() {
        let c = crate::content::Content::fixture();
        for (_, wreck) in FIXTURE_WRECKS {
            let t = crate::item::find_in(&c, *wreck).unwrap_or_else(|| panic!("{wreck}"));
            assert_eq!(t.group_id, GROUP_WRECK, "{}", t.name);
            assert!(
                crate::item::find_type_in(&c, &wreck.to_string()).is_none(),
                "not grantable"
            );
        }
    }

    /// `types/market.tsv` gives every **ship** its `wreck_type_id` (W18
    /// CP58d): another category's column is not read, a ship with none has
    /// none, a malformed number is an error naming the file, and no file is
    /// no wrecks. Invented rows.
    #[test]
    fn market_rows_give_each_ship_its_wreck() {
        let row = |id: i64, cat: i64, wreck: &str| {
            format!("{id}\t99001\t{cat}\t\t1.0\t\t\t1\t\t\t1\t{wreck}\t1\t1")
        };
        let w = Wrecks::parse(&[
            row(990_000_001, 6, "990000901"),
            row(990_000_002, 6, ""),
            row(990_000_003, 7, "990000902"),
        ])
        .expect("parses");
        assert_eq!(
            w.iter().collect::<Vec<_>>(),
            vec![(990_000_001, 990_000_901)]
        );
        assert_eq!(w.of(990_000_003), None, "not a ship");
        let bad = Wrecks::parse(&[row(990_000_001, 6, "x")]).unwrap_err();
        assert!(bad.contains("types/market.tsv"), "{bad}");
        assert!(Wrecks::parse(&[]).expect("empty").iter().next().is_none());
    }

    /// A ship nothing has hit reads as three full bars -- the shape every
    /// undock sends -- and not as "no gauges".
    #[test]
    fn a_ship_nobody_shot_is_full_health() {
        let reg = alice_has_bob_locked();
        assert_eq!(damage_state(&reg, BOB_SHIP, 0), DamageState::FULL);
    }

    /// **One volley moves the bars for the whole park and the rings for the
    /// victim, from one number, and it all leaves by the clock.**
    ///
    /// - nothing on `pending`: the tick is not a payload turn, and a hit
    ///   parked there would wait for the victim's next click;
    /// - `OnDamageStateChange` in a `DoDestinyUpdate`, to both pilots,
    ///   naming Bob's ship (§0 step 4) [V];
    /// - `OnModuleAttributeChange` to Bob only, seven arguments, ownerID =
    ///   Bob's **character**, itemKey = his ship (`godma:1591`) [V];
    /// - and the bar is the ring's value over its capacity (§8.2): the two
    ///   families agree.
    #[test]
    fn a_hit_moves_bars_and_rings_together_on_the_clock() {
        let mut reg = alice_has_bob_locked();
        let now = crate::pyutil::filetime_now();
        on_cycles(&mut reg, vec![blaster_cycle(now, 0)], now);
        assert!(
            reg.take_pending().is_empty(),
            "the clock's work must not wait for a payload turn"
        );
        let out = due(&mut reg);

        let bars: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .flat_map(|(to, _, args)| entries(args).into_iter().map(move |e| (to.clone(), e)))
            .filter(|(_, (name, _))| *name == crate::pyutil::bstr("OnDamageStateChange"))
            .collect();
        assert_eq!(bars.len(), 1, "one bar update: {out:?}");
        let (to, (_, args)) = &bars[0];
        let mut to = to.clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "everyone whose park holds Bob");
        assert_eq!(args[0], PyValue::Int(BOB_SHIP));
        let PyValue::Tuple(state) = &args[1] else {
            panic!("a per-ball state")
        };
        let PyValue::Tuple(shield) = &state[0] else {
            panic!("a shown shield is a sequence")
        };
        let bar = real(&shield[0]);
        assert!(bar < 1.0, "the shield bar moved");

        let rings: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "OnModuleAttributeChange")
            .collect();
        assert!(!rings.is_empty(), "the victim's rings moved");
        for (to, _, args) in &rings {
            assert_eq!(to, &vec![BOB], "rings are the victim's own HUD");
            assert_eq!(args.len(), 7);
            assert_eq!(args[0], PyValue::Int(BOB), "ownerID is the character");
            assert_eq!(args[1], PyValue::Int(BOB_SHIP), "itemKey is the ship");
        }
        let shield_ring = rings
            .iter()
            .find(|(_, _, a)| a[2] == PyValue::Int(damage::attr::SHIELD_CHARGE))
            .expect("the shield took the hit first");
        let capacity = destiny::hull::of_in(reg.content(), RIFTER).shield_capacity;
        assert!(
            (real(&shield_ring.2[4]) / capacity - bar).abs() < 1e-6,
            "the bar and the ring are one number"
        );
        assert!(
            real(&shield_ring.2[4]) < real(&shield_ring.2[5]),
            "new < old"
        );
    }

    /// The two combat-log lines are different dicts (§0 steps 6-7) [V]: the
    /// shooter's says `attackType 'me'` and names the `target` and the
    /// `weapon` type; the victim's says `'otherPlayerWeapons'` and names the
    /// `source` and the `attackerID`. `hitQuality`, `isBanked` and `damage`
    /// are on both, being bare subscripts.
    #[test]
    fn the_shooter_and_the_victim_read_different_lines() {
        let mut reg = alice_has_bob_locked();
        let now = crate::pyutil::filetime_now();
        on_cycles(&mut reg, vec![blaster_cycle(now, 0)], now);
        let out = due(&mut reg);
        let line = |who: i64| {
            out.iter()
                .find(|(to, n, _)| *n == "OnDamageMessage" && to == &vec![who])
                .map(|(_, _, a)| a[0].clone())
                .unwrap_or_else(|| panic!("a line for {who}"))
        };
        let get = |d: &PyValue, k: &str| crate::pyutil::dict_get(d, k.as_bytes()).cloned();
        for d in [line(ALICE), line(BOB)] {
            for k in ["hitQuality", "isBanked", "damage"] {
                assert!(get(&d, k).is_some(), "{k} is mandatory");
            }
        }
        let a = line(ALICE);
        assert_eq!(get(&a, "attackType"), Some(crate::pyutil::bstr("me")));
        assert_eq!(get(&a, "target"), Some(PyValue::Int(BOB_SHIP)));
        assert_eq!(get(&a, "weapon"), Some(PyValue::Int(3640)), "a type id");
        let b = line(BOB);
        assert_eq!(
            get(&b, "attackType"),
            Some(crate::pyutil::bstr("otherPlayerWeapons"))
        );
        assert_eq!(get(&b, "source"), Some(PyValue::Int(ALICE_SHIP)));
        assert_eq!(get(&b, "attackerID"), Some(PyValue::Int(ALICE_SHIP)));
    }

    /// **The killing volley removes the ship and leaves a wreck in one batch,
    /// and the shooter's lock is released as an explosion** (§0 steps 8, 9,
    /// 13) [V].
    #[test]
    fn the_killing_volley_removes_the_ball_with_its_wreck_and_drops_the_lock() {
        let mut reg = alice_has_bob_locked();
        let t0 = crate::pyutil::filetime_now();
        // Enough cycles to empty a Rifter, all in one instant so the shield
        // cannot regenerate in between.
        let cycles: Vec<Cycle> = (0..1_000).map(|i| blaster_cycle(t0, i)).collect();
        on_cycles(&mut reg, cycles, t0);
        let out = due(&mut reg);

        let kills: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .filter(|(_, _, a)| {
                entries(a)
                    .iter()
                    .any(|(n, _)| *n == crate::pyutil::bstr("RemoveBall"))
            })
            .collect();
        assert_eq!(kills.len(), 1, "one death, one batch");
        let (to, _, args) = kills[0];
        let mut to = to.clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "the whole park sees it go");
        let e = entries(args);
        assert_eq!(
            e.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            vec![
                crate::pyutil::bstr("AddBalls2"),
                crate::pyutil::bstr("RemoveBall"),
                crate::pyutil::bstr("AddBalls2")
            ],
            "the wreck, the terminal removal, then the capsule"
        );
        let PyValue::List(batch) = &args[0] else {
            unreachable!()
        };
        let stamps: Vec<_> = batch
            .iter()
            .map(|e| match e {
                PyValue::Tuple(e) => e[0].clone(),
                other => panic!("(stamp, call), got {other:?}"),
            })
            .collect();
        assert!(stamps.windows(2).all(|p| p[0] == p[1]), "one stamp");
        assert_eq!(
            e[1].1,
            vec![PyValue::Int(BOB_SHIP), PyValue::Bool(true)],
            "RemoveBall(ballID, terminal=True)"
        );

        let lost = out
            .iter()
            .find(|(to, n, _)| *n == "OnTarget" && to == &vec![ALICE])
            .expect("Alice's target box is told");
        assert_eq!(lost.2[0], PyValue::Bytes(b"lost".to_vec()));
        assert_eq!(lost.2[1], PyValue::Int(BOB_SHIP));
        assert_eq!(
            lost.2[2],
            PyValue::Bytes(b"Exploding".to_vec()),
            "the reason that routes to ClearDeadTarget"
        );
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty());
    }

    /// **RAT-GHOST: a pilot podded in space is told every locker of the dead
    /// ship has let go.** The capsule keeps the park, and the client's
    /// in-space `shipid` change leaves `targetedBy` alone
    /// (`docs/re/targeting.md` §5.1) [V]; without an `'otherlost'` per locker
    /// Bob's `GetTargetedBy()` lists his killers for ever (fci_hunter_bot,
    /// 2026-11-03). The rat is an invented NPC id in the lock graph only.
    #[test]
    fn a_podded_pilot_is_told_every_locker_of_the_dead_ship_let_go() {
        const RAT: i64 = 7_000_000_901;
        let mut reg = alice_has_bob_locked();
        reg.locks_mut().add(RAT, BOB_SHIP);
        kill_bob(&mut reg);
        let out = due(&mut reg);
        assert!(
            reg.character(BOB).is_some_and(|c| c.ship_id == BOB_POD),
            "setup: Bob is in his capsule"
        );
        let mut told: Vec<i64> = out
            .iter()
            .filter(|(to, n, a)| {
                *n == "OnTarget"
                    && to == &vec![BOB]
                    && a[0] == PyValue::Bytes(b"otherlost".to_vec())
            })
            .map(|(_, _, a)| match a[1] {
                PyValue::Int(ship) => ship,
                ref other => panic!("a ship id, got {other:?}"),
            })
            .collect();
        told.sort_unstable();
        assert_eq!(told, vec![ALICE_SHIP, RAT], "one 'otherlost' per locker");
        assert!(reg.locks().targeters_of(BOB_SHIP).is_empty());
        assert!(reg.locks().targeters_of(BOB_POD).is_empty());
    }

    /// Kill Bob with Alice's blaster: enough cycles in one instant that the
    /// shield cannot regenerate in between.
    pub(crate) fn kill_bob(reg: &mut Registry) -> BlueTime {
        let t0 = crate::pyutil::filetime_now();
        let cycles: Vec<Cycle> = (0..1_000).map(|i| blaster_cycle(t0, i)).collect();
        on_cycles(reg, cycles, t0);
        t0
    }

    /// **A missile volley is a volley** (CP44, for CP45): landed through
    /// [`missile_volley`] it leaves Bob's record exactly where the same
    /// damage through [`fire`] leaves it on a twin world; a volley at a ship
    /// not in space lands nowhere and records nothing.
    #[test]
    fn a_missile_volley_applies_damage_through_the_volley_path() {
        let t0 = crate::pyutil::filetime_now();
        let damage = Damage {
            em: 0.0,
            explosive: 0.0,
            kinetic: 40.0,
            thermal: 0.0,
        };
        let mut reg = alice_has_bob_locked();
        let landed = missile_volley(
            &mut reg,
            MissileVolley {
                shooter: ALICE,
                attacker_ship_id: ALICE_SHIP,
                launcher_item_id: BLASTER,
                missile_type_id: 3640,
                target_ship_id: BOB_SHIP,
                damage,
                at: t0,
            },
            t0,
        );
        assert!(landed);
        let mut twin = alice_has_bob_locked();
        let bob = *twin.roster().get(BOB).expect("Bob in space");
        let shot = Shot {
            at: t0,
            attacker_ship_id: ALICE_SHIP,
            weapon_item_id: BLASTER,
            weapon_type_id: 3640,
            target_ship_id: BOB_SHIP,
            damage,
            hit_quality: HIT_QUALITY,
        };
        fire(&mut twin, shot, ALICE, bob, t0);
        let hit = damage_state(&reg, BOB_SHIP, t0);
        assert_ne!(hit, DamageState::FULL, "the volley did damage");
        assert_eq!(hit, damage_state(&twin, BOB_SHIP, t0));
        assert_eq!(reg.combat().ship(BOB_SHIP), twin.combat().ship(BOB_SHIP));

        let landed = missile_volley(
            &mut reg,
            MissileVolley {
                shooter: ALICE,
                attacker_ship_id: ALICE_SHIP,
                launcher_item_id: BLASTER,
                missile_type_id: 3640,
                target_ship_id: 999_999_999,
                damage,
                at: t0,
            },
            t0,
        );
        assert!(!landed);
        assert!(reg.combat().ship(999_999_999).is_none());
    }

    /// The capsule's `AddBalls2` in the kill batch: `(ball ids, slim dict)`.
    fn capsule_add(out: &[(Vec<i64>, &'static str, Vec<PyValue>)]) -> (Vec<i64>, PyValue) {
        let (_, _, args) = out
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate"
                    && entries(a)
                        .iter()
                        .any(|(n, _)| *n == crate::pyutil::bstr("RemoveBall"))
            })
            .expect("a kill batch");
        let (_, add) = entries(args).pop().expect("three entries");
        let PyValue::Tuple(chunk) = &add[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::Bytes(blob) = &chunk[0] else {
            panic!("blob")
        };
        let park = destiny::Park::decode(blob).expect("decodes");
        let PyValue::List(extra) = &chunk[1] else {
            panic!("extra")
        };
        let PyValue::Tuple(pair) = &extra[0] else {
            panic!("a ship's (slim, damage) pair")
        };
        (
            park.balls.iter().map(|b| b.ball_id).collect(),
            pair[0].clone(),
        )
    }

    /// **A ship's death puts its pilot in their capsule** (§0 steps 10 and
    /// 12, §4.2) [V]:
    ///
    /// - the capsule that sat in the ship at `flagCapsule` is the one that
    ///   appears, as a ball named by its slim, to the whole park;
    /// - `{'shipid': (deadShip, capsule)}` is queued for the victim, the
    ///   shape `ship.Board` uses, and nothing else in it;
    /// - the character now flies the capsule, and the roster has it.
    #[test]
    fn a_ship_death_puts_the_pilot_in_their_capsule() {
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        let out = due(&mut reg);

        let (balls, slim) = capsule_add(&out);
        assert_eq!(balls, vec![BOB_POD], "Bob's own capsule, not a new one");
        let get = |k: &str| crate::pyutil::dict_get(&slim, k.as_bytes()).cloned();
        assert_eq!(get("itemID"), Some(PyValue::Int(BOB_POD)));
        assert_eq!(get("typeID"), Some(PyValue::Int(damage::CAPSULE_TYPE_ID)));
        assert_eq!(get("charID"), Some(PyValue::Int(BOB)));

        let changes = reg.take_session_changes();
        assert_eq!(
            changes,
            vec![(BOB, shipid_change(BOB_SHIP, BOB_POD))],
            "one session change, to Bob, shipid dead ship -> capsule"
        );
        let PyValue::Tuple(pair) = &changes[0].1[0].0 else {
            panic!("(old, new)")
        };
        assert_eq!(pair, &vec![PyValue::Int(BOB_SHIP), PyValue::Int(BOB_POD)]);

        let bob = reg.character(BOB).expect("Bob");
        assert_eq!(
            (bob.ship_id, bob.ship_type_id),
            (BOB_POD, damage::CAPSULE_TYPE_ID)
        );
        assert_eq!(
            reg.roster().by_ship(BOB_POD).map(|p| p.character_id),
            Some(BOB)
        );
        assert!(reg.roster().by_ship(BOB_SHIP).is_none());
    }

    /// **The store after a kill**: the dead ship is gone and nothing is left
    /// inside it, the capsule is out of it, the wreck is an item -- owner
    /// the victim, type the hull's own `wreckTypeID`, in the solar system --
    /// and the pushes the store made are addressed to the victim and on the
    /// clock, none left on `pending` for some other caller's turn.
    #[test]
    fn a_kill_destroys_the_ship_and_its_modules_and_makes_the_wreck_an_item() {
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        assert!(reg.store().get(BOB_SHIP).is_none(), "the hull is gone");
        assert_eq!(reg.store().contents_of(BOB_SHIP).count(), 0);
        assert!(reg.modules().running(BOB_GUN).is_none());
        let pod = reg.store().get(BOB_POD).expect("the capsule survives");
        assert_ne!(pod.location_id, BOB_SHIP, "and is no longer inside it");
        assert!(
            reg.store().get(BLASTER).is_some(),
            "the shooter's gun is not"
        );

        let wreck = reg.combat().wrecks().next().copied().expect("one wreck");
        let item = reg.store().get(wreck.wreck_id).expect("a store item");
        assert_eq!(
            Some(item.type_id),
            wreck_type_in(reg.content(), RIFTER),
            "Rifter's wreck"
        );
        assert_eq!(item.group_id, GROUP_WRECK);
        assert_eq!(item.owner_id, BOB);
        assert_eq!(item.location_id, SYSTEM);
        assert_eq!(wreck.ship_id, BOB_SHIP, "launcherID is the dead ship");
        if let Some(gun) = reg.store().get(BOB_GUN) {
            assert_eq!(gun.location_id, wreck.wreck_id, "dropped, or destroyed");
        }

        assert!(reg.take_pending().is_empty(), "nothing waits for a turn");
        let out = due(&mut reg);
        let items: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "OnItemChange" || *n == "OnInvalidateCacheForLocations")
            .collect();
        assert!(!items.is_empty(), "the store's pushes left by the clock");
        for (to, _, _) in items {
            assert_eq!(to, &vec![BOB], "the victim's items, told to the victim");
        }
    }

    /// **A pilot who undocks after the fight sees the wreck**: it is in
    /// their `SetState`, its ball in the blob and its slim with the wreck's
    /// keys (§6) -- and the dead ship is not.
    #[test]
    fn a_later_bootstrap_shows_the_wreck_and_not_the_dead_ship() {
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        let wreck = reg.combat().wrecks().next().copied().expect("one wreck");
        let state = destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
        assert!(state.every_slim_has_a_ball());
        assert!(state.park.balls.iter().any(|b| b.ball_id == wreck.wreck_id));
        assert!(state.park.balls.iter().all(|b| b.ball_id != BOB_SHIP));
        assert!(state.park.balls.iter().any(|b| b.ball_id == BOB_POD));
        let slim = state
            .wrecks
            .iter()
            .find(|s| s.item_id == wreck.wreck_id)
            .expect("the wreck's slim");
        assert_eq!(slim.launcher_id, BOB_SHIP);
        assert_eq!(slim.type_id, wreck.type_id);
    }

    /// **A wreck goes when its time is up**: out of the store, and out of
    /// every park in the system by `RemoveBalls([id])` -- one positional
    /// argument (`docs/lanes/addballs.md` §3) [V]. The time is the record's
    /// own; how long it is is not this test's business.
    #[test]
    fn an_expired_wreck_leaves_the_store_and_the_park() {
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        let _ = due(&mut reg);
        let wreck = reg.combat().wrecks().next().copied().expect("one wreck");
        tick(&mut reg, wreck.expires_at - 1);
        assert!(reg.store().get(wreck.wreck_id).is_some(), "not yet");
        let _ = due(&mut reg);
        tick(&mut reg, wreck.expires_at);
        assert!(reg.store().get(wreck.wreck_id).is_none());
        assert_eq!(reg.combat().wrecks().count(), 0);
        let out = due(&mut reg);
        let removal = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .flat_map(|(to, _, a)| entries(a).into_iter().map(move |e| (to.clone(), e)))
            .find(|(_, (n, _))| *n == crate::pyutil::bstr("RemoveBalls"))
            .expect("the wreck's ball is removed");
        let (mut to, (_, args)) = removal;
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "everyone in the system");
        assert_eq!(args.len(), 1, "RemoveBalls takes exactly one argument");
        assert_eq!(args[0], PyValue::List(vec![PyValue::Int(wreck.wreck_id)]));
    }

    /// **A capsule dies without a wreck, and its pilot wakes up docked**
    /// (§0 steps 15-18, §5.2) [V]:
    ///
    /// - the terminal removal alone -- `types.wreckTypeID` is absent for 670;
    /// - `OnPodKilled(podID)` (one argument), then `OnPlayerPodDeath()`
    ///   (none), to the pilot, in that order;
    /// - a session change whose `shipid` old value is the pod `OnPodKilled`
    ///   named (`gameui._ParseStateChange:506`) and whose `stationid` docks.
    #[test]
    fn a_capsule_death_leaves_no_wreck_and_revives_the_pilot_docked() {
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        let _ = due(&mut reg);
        let _ = reg.take_session_changes();
        let wrecks_before = reg.combat().wrecks().count();
        reg.locks_mut().add(ALICE_SHIP, BOB_POD);
        let t0 = crate::pyutil::filetime_now();
        let cycles: Vec<Cycle> = (0..1_000)
            .map(|i| Cycle {
                target_id: Some(BOB_POD),
                ..blaster_cycle(t0, i)
            })
            .collect();
        on_cycles(&mut reg, cycles, t0);
        assert!(reg.take_pending().is_empty(), "nothing waits for a turn");
        let out = due(&mut reg);
        let kill: Vec<_> = out
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .map(|(_, _, a)| entries(a))
            .find(|e| {
                e.iter()
                    .any(|(n, _)| *n == crate::pyutil::bstr("RemoveBall"))
            })
            .expect("the pod pops");
        assert_eq!(kill.len(), 1, "RemoveBall alone: no wreck, no capsule");
        assert_eq!(kill[0].1, vec![PyValue::Int(BOB_POD), PyValue::Bool(true)]);
        assert_eq!(reg.combat().wrecks().count(), wrecks_before);

        let at = |name: &str| {
            out.iter()
                .position(|(to, n, _)| *n == name && to == &vec![BOB])
                .unwrap_or_else(|| panic!("{name} to Bob"))
        };
        let (killed, white) = (at("OnPodKilled"), at("OnPlayerPodDeath"));
        assert!(killed < white, "OnPodKilled first");
        assert_eq!(out[killed].2, vec![PyValue::Int(BOB_POD)]);
        assert!(out[white].2.is_empty(), "OnPlayerPodDeath takes nothing");

        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        let (who, change) = &changes[0];
        assert_eq!(*who, BOB);
        let get = |k: &str| {
            change
                .iter()
                .find(|(_, key)| *key == crate::pyutil::bstr(k))
                .map(|(v, _)| v.clone())
        };
        let Some(PyValue::Tuple(ship)) = get("shipid") else {
            panic!("a shipid change")
        };
        assert_eq!(ship[0], PyValue::Int(BOB_POD), "the pod OnPodKilled named");
        let Some(PyValue::Tuple(station)) = get("stationid") else {
            panic!("a stationid change")
        };
        assert_eq!(station[0], PyValue::None);
        assert!(matches!(station[1], PyValue::Int(_)), "docked");
        assert!(reg.roster().get(BOB).is_none(), "out of space");
    }

    /// **K-POD**: a podded pilot's implants die with the capsule. Bob's
    /// head holds one implant when the pod pops; afterwards it is gone from
    /// the store, he is docked in a new capsule, and the capsule's own
    /// report (victim: the capsule type) lists the implant destroyed.
    #[test]
    fn a_pod_kill_destroys_the_implants_and_the_report_lists_them() {
        const IMPLANT_TYPE: i64 = 34; // any type the store knows
        let mut reg = alice_has_bob_locked();
        kill_bob(&mut reg);
        let _ = due(&mut reg);
        let _ = reg.take_session_changes();
        let made = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: IMPLANT_TYPE,
                quantity: 1,
                to: crate::item::Slot::new(BOB, BOB, crate::dogma::FLAG_IMPLANT),
            }])
            .expect("created");
        let _ = reg.take_pending();
        let implant = made
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("an id");
        reg.locks_mut().add(ALICE_SHIP, BOB_POD);
        let t0 = crate::pyutil::filetime_now();
        let cycles: Vec<Cycle> = (0..1_000)
            .map(|i| Cycle {
                target_id: Some(BOB_POD),
                ..blaster_cycle(t0, i)
            })
            .collect();
        on_cycles(&mut reg, cycles, t0);
        let _ = due(&mut reg);
        assert!(reg.store().get(implant).is_none(), "the implant is gone");
        assert!(crate::clones::implants_of(&reg, BOB).is_empty());
        let bob = reg.character(BOB).unwrap();
        assert!(bob.station_id.is_some(), "docked at the clone station");
        assert_eq!(bob.ship_type_id, crate::damage::CAPSULE_TYPE_ID);
        assert_ne!(bob.ship_id, BOB_POD, "a new capsule");
        let mails = crate::killmail::involving(&reg, BOB, None, 10);
        let pod_mail = mails
            .iter()
            .find(|m| m.ship_type == crate::damage::CAPSULE_TYPE_ID)
            .expect("the capsule has its own killmail");
        assert!(
            pod_mail
                .items
                .iter()
                .any(|i| i.type_id == IMPLANT_TYPE && i.destroyed == 1),
            "{:?}",
            pod_mail.items
        );
    }

    /// **IMPL**: implants plugged in through the client's own call die with
    /// the pod, are listed on the capsule's report, and a `nonDestructible`
    /// one stays. Bob plugs two from his ship (the pilot's location, in
    /// space), then the pod pops.
    #[test]
    fn plugged_implants_die_with_the_pod_the_report_lists_them_and_a_keeper_stays() {
        use crate::implants::fixture::{KEEPER, MEMORY_1};
        let content = std::sync::Arc::new(crate::implants::fixture::content());
        let mut reg = locked_on(&[], Some(content));
        kill_bob(&mut reg);
        let _ = due(&mut reg);
        let _ = reg.take_session_changes();
        let mut ids = Vec::new();
        for t in [MEMORY_1, KEEPER] {
            ids.push(create(
                &mut reg,
                t,
                1,
                crate::item::Slot::new(BOB, BOB_POD, crate::fitting::FLAG_CARGO),
            ));
        }
        for &item in &ids {
            reg.dispatch(&crate::Request {
                service: "dogmaIM",
                method: "InjectImplant",
                args: &[PyValue::Int(item)],
                kwargs: &[],
                bind_params: None,
                actor: Some(BOB),
            })
            .expect("plugged");
        }
        assert_eq!(crate::implants::plugged(&reg, BOB).len(), 2);
        let _ = reg.take_pending();
        reg.locks_mut().add(ALICE_SHIP, BOB_POD);
        let t0 = crate::pyutil::filetime_now();
        let cycles: Vec<Cycle> = (0..1_000)
            .map(|i| Cycle {
                target_id: Some(BOB_POD),
                ..blaster_cycle(t0, i)
            })
            .collect();
        on_cycles(&mut reg, cycles, t0);
        let out = due(&mut reg);
        assert!(
            reg.store().get(ids[0]).is_none(),
            "the memory implant is gone"
        );
        assert!(reg.store().get(ids[1]).is_some(), "the keeper stayed");
        assert_eq!(
            crate::implants::head_types(&reg, BOB),
            vec![KEEPER],
            "and is still in the head"
        );
        assert!(
            out.iter()
                .any(|(to, n, _)| *n == "OnServerImplantsChanged" && to == &vec![BOB]),
            "the client is told to re-read its implants"
        );
        let mails = crate::killmail::involving(&reg, BOB, None, 10);
        let pod_mail = mails
            .iter()
            .find(|m| m.ship_type == crate::damage::CAPSULE_TYPE_ID)
            .expect("the capsule has its own killmail");
        let listed: Vec<i64> = pod_mail.items.iter().map(|i| i.type_id).collect();
        assert!(listed.contains(&MEMORY_1), "{listed:?}");
        assert!(
            !listed.contains(&KEEPER),
            "a keeper was not lost: {listed:?}"
        );
    }

    // ---- loot (P2.4) ----

    const TRITANIUM: i64 = 34;
    const EMP_S: i64 = 185;
    const PHASED_PLASMA_S: i64 = 184;
    const CARGO: i64 = crate::fitting::FLAG_CARGO;

    /// Bring `quantity` of `type_id` into existence at `(owner, location,
    /// flag)`, and return its id.
    fn create(reg: &mut Registry, type_id: i64, quantity: i64, to: crate::item::Slot) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity,
                to,
            }])
            .expect("a known type");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("created")
    }

    /// Bob dead, his wreck in space, the kill's pushes drained.
    fn bob_wrecked(reg: &mut Registry) -> Wreck {
        kill_bob(reg);
        let _ = due(reg);
        let _ = reg.take_pending();
        reg.combat().wrecks().next().copied().expect("one wreck")
    }

    /// Put Alice `gap` metres from the wreck's centre, at rest.
    fn alice_at(reg: &mut Registry, wr: &Wreck, gap: f64) {
        let p = wr.position;
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ALICE_SHIP,
                SYSTEM,
                crate::space_state::Motion::arrived([p[0] + gap, p[1], p[2]], [0.0; 3]),
            ));
    }

    /// `(id, type, quantity)` of everything in a container, sorted.
    fn holding(reg: &Registry, container: i64) -> Vec<(i64, i64, i64)> {
        let mut v: Vec<_> = reg
            .store()
            .contents_of(container)
            .map(|i| (i.item_id, i.type_id, i.quantity))
            .collect();
        v.sort_unstable();
        v
    }

    /// Units of `type_id` in a container, however they are stacked.
    fn units_in(reg: &Registry, container: i64, type_id: i64) -> i64 {
        reg.store()
            .contents_of(container)
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    fn kw(name: &str, v: PyValue) -> (PyValue, PyValue) {
        (v, PyValue::Bytes(name.as_bytes().to_vec()))
    }

    /// One call on a bound inventory, as `actor`.
    fn inv(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
        container: i64,
    ) -> Result<PyValue, crate::service::ServiceError> {
        let bind = PyValue::Tuple(vec![PyValue::Int(container), PyValue::Int(0)]);
        reg.dispatch(&crate::service::Request {
            service: "invbroker::inventory",
            method,
            args,
            kwargs,
            bind_params: Some(&bind),
            actor: Some(actor),
        })
    }

    /// The `UserError` key a refusal raised.
    fn raised_key(err: &crate::service::ServiceError) -> String {
        let crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            panic!("not a raised exception: {err:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a header tuple")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's args")
        };
        String::from_utf8_lossy(crate::pyutil::as_bytes(&args[0]).expect("a key")).into_owned()
    }

    /// **Nothing a dead ship held is lost track of**: every item that was
    /// inside it -- a fitted module, a loaded charge, cargo stacks -- is
    /// afterwards either in the wreck, whole and unchanged and still the
    /// victim's, or gone from the store; the wreck holds nothing else; and
    /// the wreck's slim says `isEmpty` exactly when the store says it is.
    /// Which items survive is the drop rule's business, not this test's.
    #[test]
    fn a_kill_puts_every_item_in_the_wreck_or_destroys_it_and_nothing_else() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let charge = create(&mut reg, EMP_S, 40, Slot::new(BOB, BOB_SHIP, 27));
        let mut held = vec![(BOB_GUN, 3640, 1), (charge, EMP_S, 40)];
        for (t, n) in [(TRITANIUM, 1_000), (PHASED_PLASMA_S, 200), (EMP_S, 77)] {
            held.push((
                create(&mut reg, t, n, Slot::new(BOB, BOB_SHIP, CARGO)),
                t,
                n,
            ));
        }
        // A few more, so a test of "some survive and some do not" is not
        // hostage to five rolls.
        for n in 1..=20 {
            held.push((
                create(&mut reg, TRITANIUM, n, Slot::new(BOB, BOB_SHIP, CARGO)),
                TRITANIUM,
                n,
            ));
        }
        held.sort_unstable();
        let _ = reg.take_pending();

        kill_bob(&mut reg);
        let out = due(&mut reg);
        let wreck = reg.combat().wrecks().next().copied().expect("one wreck");
        let dropped = holding(&reg, wreck.wreck_id);
        for d in &dropped {
            assert!(held.contains(d), "{d:?} was not in the ship as it is now");
            let item = reg.store().get(d.0).expect("listed");
            assert_eq!(item.owner_id, BOB, "still the victim's");
        }
        for h in &held {
            assert!(
                dropped.contains(h) || reg.store().get(h.0).is_none(),
                "{h:?} neither dropped nor destroyed"
            );
        }
        assert_eq!(reg.store().contents_of(BOB_SHIP).count(), 0);

        let (_, _, args) = out
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate"
                    && entries(a)
                        .iter()
                        .any(|(n, _)| *n == crate::pyutil::bstr("RemoveBall"))
            })
            .expect("a kill batch");
        let (_, add) = entries(args).remove(0);
        let PyValue::Tuple(chunk) = &add[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::List(extra) = &chunk[1] else {
            panic!("extra")
        };
        assert_eq!(
            crate::pyutil::dict_get(&extra[0], b"isEmpty"),
            Some(&PyValue::Bool(dropped.is_empty())),
            "the slim agrees with the store"
        );
    }

    /// The drop roll is a function of its two ids, nothing else.
    #[test]
    fn the_drop_roll_is_reproducible() {
        for item in 100_000_000..100_000_050 {
            assert_eq!(survives(BOB_SHIP, item), survives(BOB_SHIP, item));
        }
    }

    /// **"Loot All" moves the wreck's contents into the looter's hold**: the
    /// wreck lists its contents to a stranger beside it (`List()`, no flag,
    /// `invControllers.py:140`), `MultiAdd([ids], wreckID, flag=flagCargo)`
    /// on the active ship's inventory (`:686-688`) moves each stack whole,
    /// now Alice's; and the wreck, looted out, turns empty on every screen in
    /// the system by `OnSlimItemChange` (`michelle.py:2006`) [V].
    #[test]
    fn loot_all_moves_everything_into_the_looters_hold_and_empties_the_wreck() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        for i in holding(&reg, wr.wreck_id) {
            let _ = reg.apply_items(&[crate::item::Edit::Destroy { item_id: i.0 }]);
        }
        let a = create(
            &mut reg,
            TRITANIUM,
            500,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        let b = create(
            &mut reg,
            EMP_S,
            60,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        alice_at(&mut reg, &wr, 1_000.0);
        let _ = reg.take_pending();

        let PyValue::List(rows) =
            inv(&mut reg, ALICE, "List", &[], &[], wr.wreck_id).expect("the wreck lists")
        else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 2, "both stacks, though they are Bob's");

        let ids = PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]);
        inv(
            &mut reg,
            ALICE,
            "MultiAdd",
            &[ids, PyValue::Int(wr.wreck_id)],
            &[kw("flag", PyValue::Int(CARGO))],
            ALICE_SHIP,
        )
        .expect("looted");

        assert!(holding(&reg, wr.wreck_id).is_empty(), "left the wreck");
        for (id, t, n) in [(a, TRITANIUM, 500), (b, EMP_S, 60)] {
            let i = reg.store().get(id).expect("still exists");
            assert_eq!(
                (i.owner_id, i.location_id, i.flag_id, i.type_id, i.quantity),
                (ALICE, ALICE_SHIP, CARGO, t, n),
                "arrived in Alice's hold"
            );
        }
        let told: Vec<Vec<i64>> = reg
            .take_pending()
            .iter()
            .filter(|(_, p)| p.to_push().0 == "OnItemChange")
            .map(|(_, p)| {
                let mut to = p.audience().unwrap_or_default();
                to.sort_unstable();
                to
            })
            .collect();
        assert_eq!(told.len(), 2, "one row per stack");
        assert!(told.iter().all(|to| to == &vec![ALICE, BOB]), "{told:?}");

        let out = due(&mut reg);
        let (to, _, args) = out
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate"
                    && entries(a)
                        .iter()
                        .any(|(n, _)| *n == crate::pyutil::bstr("OnSlimItemChange"))
            })
            .expect("the wreck turns empty");
        let mut to = to.clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "everyone in the system");
        let (_, call) = entries(args).remove(0);
        assert_eq!(call[0], PyValue::Int(wr.wreck_id));
        let PyValue::Instance { state, .. } = &call[1] else {
            panic!("a SlimItem")
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"isEmpty"),
            Some(&PyValue::Bool(true))
        );
    }

    /// **A drag of part of a stack** -- `Add(itemID, wreckID, qty=n,
    /// flag=flagCargo)` (`__AddItem`) -- takes `n`, leaves the rest in the
    /// wreck, conserves the units, and says nothing about emptiness.
    #[test]
    fn a_partial_drag_takes_what_was_asked_and_leaves_the_rest() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let stack = create(
            &mut reg,
            TRITANIUM,
            900,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        let in_wreck = units_in(&reg, wr.wreck_id, TRITANIUM);
        alice_at(&mut reg, &wr, 500.0);
        inv(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(stack), PyValue::Int(wr.wreck_id)],
            &[
                kw("qty", PyValue::Int(300)),
                kw("flag", PyValue::Int(CARGO)),
            ],
            ALICE_SHIP,
        )
        .expect("taken");
        assert_eq!(units_in(&reg, ALICE_SHIP, TRITANIUM), 300);
        assert_eq!(units_in(&reg, wr.wreck_id, TRITANIUM), in_wreck - 300);
        let out = due(&mut reg);
        assert!(
            !out.iter().any(|(_, n, a)| *n == "DoDestinyUpdate"
                && entries(a)
                    .iter()
                    .any(|(n, _)| *n == crate::pyutil::bstr("OnSlimItemChange"))),
            "the wreck is not empty"
        );
    }

    /// **Out of reach, nothing moves** (`NotCloseEnoughToLoot`); the range
    /// is the client's own 2,500 m, surface to surface.
    #[test]
    fn a_take_from_out_of_reach_is_refused_and_changes_nothing() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let stack = create(
            &mut reg,
            TRITANIUM,
            10,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        alice_at(&mut reg, &wr, 50_000.0);
        let before = holding(&reg, wr.wreck_id);
        let err = inv(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(stack), PyValue::Int(wr.wreck_id)],
            &[kw("qty", PyValue::Int(10)), kw("flag", PyValue::Int(CARGO))],
            ALICE_SHIP,
        )
        .expect_err("too far");
        assert_eq!(raised_key(&err), "NotCloseEnoughToLoot");
        assert_eq!(holding(&reg, wr.wreck_id), before);
        assert_eq!(units_in(&reg, ALICE_SHIP, TRITANIUM), 0);
    }

    /// **The hold's capacity holds**: of a "Loot All" over two stacks, the one
    /// that fits moves, the one that does not stays in the wreck, and the
    /// refusal is `NotEnoughCargoSpace` -- the key the client's prompt
    /// retries on (`invControllers.py:319-360`) [V].
    #[test]
    fn loot_all_into_a_full_hold_moves_what_fits_and_refuses_the_rest() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let free = crate::fitting::cargo_free_in(&reg, ALICE_SHIP);
        let small = create(
            &mut reg,
            EMP_S,
            10,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        // A Tritanium stack one hundred times bigger than the hold.
        let huge_n = (free / 0.01) as i64 * 100;
        let huge = create(
            &mut reg,
            TRITANIUM,
            huge_n,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        alice_at(&mut reg, &wr, 0.0);
        let ids = PyValue::List(vec![PyValue::Int(small), PyValue::Int(huge)]);
        let err = inv(
            &mut reg,
            ALICE,
            "MultiAdd",
            &[ids, PyValue::Int(wr.wreck_id)],
            &[kw("flag", PyValue::Int(CARGO))],
            ALICE_SHIP,
        )
        .expect_err("the hold is too small");
        assert_eq!(raised_key(&err), "NotEnoughCargoSpace");
        assert_eq!(
            reg.store().get(small).map(|i| i.location_id),
            Some(ALICE_SHIP)
        );
        let h = reg.store().get(huge).expect("still there");
        assert_eq!((h.location_id, h.quantity), (wr.wreck_id, huge_n));
        assert!(
            reg.store().used_volume_in(reg.content(), ALICE_SHIP, CARGO)
                <= crate::fitting::cargo_capacity_in(&reg, ALICE_SHIP)
        );
    }

    /// **Who has loot rights is the client's `HaveLootRight`**
    /// (`michelle.py:1959-1993`) [V] over the slim we send: the owner; a
    /// member of the victim's *player* corporation; not a stranger, and not
    /// a fellow member of an NPC corporation.
    #[test]
    fn loot_rights_follow_the_clients_rule() {
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let npc_corp = 1_000_009;
        let player_corp = 98_000_001;
        let corp_of = |reg: &mut Registry, id: i64, corp: i64| {
            reg.character_mut(id).expect("a pilot").corporation_id = corp;
        };
        // A player's id: this fixture's 1_000_002 is in `IsNPC`'s range, and
        // the client would call any wreck of his an NPC's -- free to all.
        let wr_in = |corp: i64| Wreck {
            owner_id: 90_000_002,
            corp_id: corp,
            ..wr
        };
        assert!(has_loot_right(&reg, ALICE, &wr), "an NPC owner's: anyone's");
        assert!(has_loot_right(&reg, BOB, &wr), "the owner");
        corp_of(&mut reg, ALICE, npc_corp);
        assert!(
            !has_loot_right(&reg, ALICE, &wr_in(npc_corp)),
            "NPC corpmates"
        );
        corp_of(&mut reg, ALICE, player_corp);
        assert!(
            has_loot_right(&reg, ALICE, &wr_in(player_corp)),
            "player corpmates"
        );
        assert!(!has_loot_right(&reg, ALICE, &wr_in(npc_corp)), "a stranger");
    }

    /// Put `joiner` in `host`'s fleet (a fresh one when `host` has none) and
    /// answer its id.
    fn fleet_together(reg: &mut Registry, host: i64, joiner: i64) -> i64 {
        let f = match reg.fleets().fleet_of(host).map(|f| f.fleet_id) {
            Some(f) => f,
            None => {
                let f = reg.fleets_mut().create(host).expect("a fleet");
                reg.fleets_mut()
                    .init(host, f, Some(670), None, Some(SYSTEM))
                    .expect("init");
                f
            }
        };
        reg.fleets_mut()
            .invite(host, f, joiner, None, None, None)
            .expect("invite");
        reg.fleets_mut()
            .accept_invite(joiner, f, Some(670), None, None)
            .expect("accept");
        f
    }

    /// **Fleet loot rights** (GAP-LOOT, `HaveLootRight:1985`): the wreck's
    /// fleet, snapshot at its making, against the taker's fleet now -- in any
    /// system. A fleet joined later, or left, grants nothing; a stranger
    /// has none; `abandoned` gives everyone the right.
    #[test]
    fn a_fleet_shares_loot_rights_and_the_snapshot_holds() {
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let owner = 90_000_002;
        let theirs = Wreck {
            owner_id: owner,
            corp_id: 98_000_001,
            fleet_id: None,
            ..wr
        };
        assert!(!has_loot_right(&reg, ALICE, &theirs), "a stranger");
        let f = fleet_together(&mut reg, BOB, ALICE);
        assert_eq!(fleet_of(&reg, ALICE), Some(f));
        // A can made before the fleet existed carries none.
        assert!(!has_loot_right(&reg, ALICE, &theirs), "joined later");
        let shared = Wreck {
            fleet_id: Some(f),
            ..theirs
        };
        assert!(has_loot_right(&reg, ALICE, &shared), "same fleet");
        // Not limited to the wreck's system.
        let far = Wreck {
            solar_system_id: SYSTEM + 1,
            ..shared
        };
        assert!(has_loot_right(&reg, ALICE, &far), "any system");
        let other = Wreck {
            fleet_id: Some(f + 1),
            ..theirs
        };
        assert!(!has_loot_right(&reg, ALICE, &other), "another fleet");
        let open = Wreck {
            abandoned: true,
            ..theirs
        };
        assert!(has_loot_right(&reg, ALICE, &open), "abandoned");
        // The slim says so.
        let (_, slim) = wreck_parts(&reg, &shared);
        assert_eq!((slim.fleet_id, slim.abandoned), (Some(f), false));
        let (_, slim) = wreck_parts(&reg, &open);
        assert_eq!((slim.fleet_id, slim.abandoned), (None, true));
    }

    /// A player's wreck takes the victim's fleet at death.
    #[test]
    fn a_players_wreck_carries_the_victims_fleet() {
        let mut reg = alice_has_bob_locked();
        let f = fleet_together(&mut reg, BOB, ALICE);
        let wr = bob_wrecked(&mut reg);
        assert_eq!(wr.fleet_id, Some(f));
        assert!(!wr.abandoned);
    }

    /// **`CmdAbandonLoot`**: only the owner, in the wreck's system; the slim
    /// is re-sent with `abandoned` true and then anyone has the right.
    #[test]
    fn only_the_owner_abandons_and_then_anyone_may_take() {
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        let owner = 90_000_002;
        let theirs = Wreck {
            owner_id: owner,
            corp_id: 98_000_001,
            ..wr
        };
        insert_wreck(&mut reg, theirs);
        let _ = due(&mut reg);
        assert_eq!(abandon_loot(&mut reg, ALICE, &[wr.wreck_id]), 0, "not hers");
        assert!(!reg.combat().wreck(wr.wreck_id).expect("kept").abandoned);
        assert!(due(&mut reg).is_empty(), "nothing is pushed");
        // Make Alice the owner.
        insert_wreck(
            &mut reg,
            Wreck {
                owner_id: ALICE,
                ..theirs
            },
        );
        // A wreck in another system is not hers to give up from here.
        insert_wreck(
            &mut reg,
            Wreck {
                wreck_id: wr.wreck_id + 1,
                owner_id: ALICE,
                solar_system_id: SYSTEM + 1,
                ..theirs
            },
        );
        assert_eq!(
            abandon_loot(&mut reg, ALICE, &[wr.wreck_id, wr.wreck_id + 1, 7]),
            1
        );
        assert!(reg.combat().wreck(wr.wreck_id).expect("kept").abandoned);
        assert!(!reg.combat().wreck(wr.wreck_id + 1).expect("kept").abandoned);
        let out = due(&mut reg);
        let (to, _, args) = out
            .iter()
            .find(|(_, n, _)| *n == "DoDestinyUpdate")
            .expect("a slim change");
        assert!(to.contains(&ALICE) && to.contains(&BOB));
        let (_, call) = entries(args).remove(0);
        let PyValue::Instance { state, .. } = &call[1] else {
            panic!("a SlimItem")
        };
        let Some(PyValue::Tuple(rights)) = crate::pyutil::dict_get(state, b"lootRights") else {
            panic!("lootRights")
        };
        assert_eq!(rights[3], PyValue::Bool(true));
        let stranger = 90_000_003;
        let w = *reg.combat().wreck(wr.wreck_id).expect("kept");
        assert!(has_loot_right(&reg, BOB, &w), "now anyone's");
        let _ = stranger;
    }

    /// **An NPC's wreck is the top damager's** (ties to the lower id), with
    /// that character's corp and fleet; with no character damage it stays the
    /// NPC corporation's.
    #[test]
    fn an_npc_wreck_belongs_to_the_top_damager() {
        let mut reg = alice_has_bob_locked();
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let npc = crate::npc::spawn(&mut reg, &crate::npc::ANGEL_HIJACKER, SYSTEM, at)
            .expect("an NPC id");
        assert_eq!(npc_wreck_owner(&reg, npc), None, "nobody hurt it");
        book_npc_damage(&mut reg, ALICE, npc, 400.0);
        book_npc_damage(&mut reg, BOB, npc, 600.0);
        let corp = reg.character(BOB).expect("bob").corporation_id;
        assert_eq!(npc_wreck_owner(&reg, npc), Some((BOB, corp, None)));
        book_npc_damage(&mut reg, ALICE, npc, 200.0);
        book_npc_damage(&mut reg, ALICE, npc, 0.0);
        assert_eq!(
            npc_wreck_owner(&reg, npc).map(|o| o.0),
            Some(ALICE),
            "Alice now leads"
        );
        let f = fleet_together(&mut reg, ALICE, BOB);
        assert_eq!(npc_wreck_owner(&reg, npc).map(|o| o.2), Some(Some(f)));
        // An NPC shooter, or a volley at a character, books nothing.
        book_npc_damage(&mut reg, npc, npc, 5_000.0);
        book_npc_damage(&mut reg, ALICE, BOB, 5_000.0);
        assert_eq!(reg.combat().npc_damage.len(), 1);
    }

    /// The kill itself: the wreck takes the killer's rights; the item stays
    /// the NPC corporation's, so its loot stacks stay consistent.
    #[test]
    fn a_killed_rats_wreck_is_the_killers_and_its_item_the_npcs() {
        let mut reg = alice_has_bob_locked();
        let f = fleet_together(&mut reg, ALICE, BOB);
        let (_, wr) = alice_kills_a_rat(&mut reg, 3_000.0);
        let corp = reg.character(ALICE).expect("alice").corporation_id;
        assert_eq!(
            (wr.owner_id, wr.corp_id, wr.fleet_id),
            (ALICE, corp, Some(f))
        );
        let item = reg.store().get(wr.wreck_id).expect("an item");
        assert_eq!(item.owner_id, crate::npc::ANGEL_HIJACKER.owner_corp_id);
        assert!(reg.combat().npc_damage.is_empty(), "tally dropped");
        // Despawning removes the item whoever owns the slim.
        assert!(remove_wreck(&mut reg, wr.wreck_id));
        assert!(reg.store().get(wr.wreck_id).is_none());
    }

    /// A wreck lists to someone in its system and to nobody elsewhere.
    #[test]
    fn a_wreck_lists_only_to_someone_in_its_system() {
        use crate::item::Slot;
        let mut reg = alice_has_bob_locked();
        let wr = bob_wrecked(&mut reg);
        create(
            &mut reg,
            TRITANIUM,
            5,
            Slot::new(BOB, wr.wreck_id, FLAG_IN_WRECK),
        );
        let rows = |reg: &mut Registry| match inv(reg, ALICE, "List", &[], &[], wr.wreck_id) {
            Ok(PyValue::List(v)) => v.len(),
            other => panic!("a list, got {other:?}"),
        };
        assert!(rows(&mut reg) >= 1);
        reg.roster_mut().leave(ALICE);
        assert_eq!(rows(&mut reg), 0, "Alice docked, or gone");
    }

    /// Kill an NPC `gap` metres from Alice with her civilian blaster and
    /// answer `(npc, its wreck)`. Spawns NPCs until one whose
    /// `npc::loot` is not empty, so there is something to take.
    fn alice_kills_a_rat(reg: &mut Registry, gap: f64) -> (i64, Wreck) {
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let npc = loop {
            let id = crate::npc::spawn(
                reg,
                &crate::npc::ANGEL_HIJACKER,
                SYSTEM,
                [at[0] + gap, at[1], at[2]],
            )
            .expect("an NPC id");
            if !crate::npc::loot(&crate::npc::ANGEL_HIJACKER, id).is_empty() {
                break id;
            }
            reg.roster_mut().leave(id);
            reg.npcs_mut().remove(id);
        };
        // `spawn` tells nobody (`npc::entity_slash` does); the grid's
        // reconcile is what puts the rat in Alice's park, as on the clock.
        crate::grid::reconcile_in(reg, SYSTEM);
        let _ = reg.take_due(Instant::now());
        let t0 = crate::pyutil::filetime_now();
        for _ in 0..200 {
            let Some(victim) = reg.roster().get(npc).copied() else {
                break;
            };
            let shot = Shot {
                at: t0,
                attacker_ship_id: ALICE_SHIP,
                weapon_item_id: BLASTER,
                weapon_type_id: 3640,
                target_ship_id: npc,
                damage: volley_in(reg.content(), 3640).expect("a civilian gun"),
                hit_quality: HIT_QUALITY,
            };
            fire(reg, shot, ALICE, victim, t0);
        }
        assert!(reg.roster().get(npc).is_none(), "the NPC died");
        let wr = reg
            .combat()
            .wrecks()
            .find(|wr| wr.ship_id == npc)
            .copied()
            .expect("its wreck");
        (npc, wr)
    }

    /// **An NPC's wreck is a store item holding what it rolled**, owned by
    /// the NPC corporation -- so the client's `HaveLootRight` gives everyone
    /// the right (`michelle.py:1972`, an NPC owner) [V] -- and the kill
    /// batch's slim says it is not empty.
    #[test]
    fn an_npc_wreck_is_an_item_with_its_loot_in_it() {
        let mut reg = alice_has_bob_locked();
        let (npc, wr) = alice_kills_a_rat(&mut reg, 3_000.0);
        let corp = crate::npc::ANGEL_HIJACKER.owner_corp_id;
        let item = reg.store().get(wr.wreck_id).expect("a store item");
        assert_eq!(
            (
                item.type_id,
                item.group_id,
                item.owner_id,
                item.location_id,
                item.flag_id
            ),
            (26561, GROUP_WRECK, corp, SYSTEM, FLAG_IN_SPACE)
        );
        let mut inside: Vec<(i64, i64)> = reg
            .store()
            .contents_of(wr.wreck_id)
            .map(|i| {
                assert_eq!((i.owner_id, i.flag_id), (corp, FLAG_IN_WRECK));
                (i.type_id, i.quantity)
            })
            .collect();
        let mut want = crate::npc::loot(&crate::npc::ANGEL_HIJACKER, npc);
        inside.sort_unstable();
        want.sort_unstable();
        assert_eq!(inside, want, "exactly what it rolled");
        for who in [ALICE, BOB] {
            assert!(has_loot_right(&reg, who, &wr), "an NPC owner: anyone");
        }
        let (_, slim) = wreck_parts(&reg, &wr);
        assert!(!slim.is_empty);
        // Nothing about the wreck is addressed to the NPC corporation.
        assert!(
            due(&mut reg)
                .iter()
                .all(|(to, _, _)| !to.contains(&corp) && !to.contains(&npc))
        );
    }

    /// **Loot All on an NPC wreck is P2.4's path**: everything into Alice's
    /// hold, hers now, the wreck empty and every screen told so.
    #[test]
    fn an_npc_wreck_is_looted_like_a_players() {
        let mut reg = alice_has_bob_locked();
        let (_, wr) = alice_kills_a_rat(&mut reg, 1_000.0);
        let _ = due(&mut reg);
        let _ = reg.take_pending();
        let ids: Vec<PyValue> = holding(&reg, wr.wreck_id)
            .iter()
            .map(|(id, _, _)| PyValue::Int(*id))
            .collect();
        let taken: Vec<i64> = holding(&reg, wr.wreck_id)
            .iter()
            .map(|(id, _, _)| *id)
            .collect();
        assert!(!ids.is_empty());
        inv(
            &mut reg,
            ALICE,
            "MultiAdd",
            &[PyValue::List(ids), PyValue::Int(wr.wreck_id)],
            &[kw("flag", PyValue::Int(CARGO))],
            ALICE_SHIP,
        )
        .expect("looted");
        assert!(holding(&reg, wr.wreck_id).is_empty(), "left the wreck");
        for id in taken {
            let i = reg.store().get(id).expect("still exists");
            assert_eq!(
                (i.owner_id, i.location_id, i.flag_id),
                (ALICE, ALICE_SHIP, CARGO)
            );
        }
        assert!(
            due(&mut reg).iter().any(|(_, n, a)| *n == "DoDestinyUpdate"
                && entries(a)
                    .iter()
                    .any(|(n, _)| *n == crate::pyutil::bstr("OnSlimItemChange"))),
            "the wreck turns empty"
        );
    }

    /// An expired NPC wreck takes its loot with it.
    #[test]
    fn an_expired_npc_wreck_leaves_nothing_in_the_store() {
        let mut reg = alice_has_bob_locked();
        let (_, wr) = alice_kills_a_rat(&mut reg, 3_000.0);
        let inside: Vec<i64> = holding(&reg, wr.wreck_id)
            .iter()
            .map(|(id, _, _)| *id)
            .collect();
        despawn_wrecks(&mut reg, wr.expires_at);
        assert!(reg.store().get(wr.wreck_id).is_none());
        assert!(inside.iter().all(|id| reg.store().get(*id).is_none()));
        assert!(reg.combat().wreck(wr.wreck_id).is_none());
    }

    // -----------------------------------------------------------------
    // Player launchers (R-MISSILE M4)
    // -----------------------------------------------------------------

    /// Invented ids: a launcher type, its missile, the launcher item.
    const LAUNCHER_TYPE: i64 = 990_460_001;
    const MISSILE_TYPE: i64 = 990_460_002;
    const LAUNCHER: i64 = 100_000_052;
    /// `flagHiSlot1` [V].
    const HI_SLOT_1: i64 = 28;
    /// The invented launcher's cycle (attribute 51) and reload (1795), ms.
    const CYCLE_MS: i64 = 4_000;
    const RELOAD_MS: i64 = 8_000;
    /// An invented guid, so the FX is seen to be the **charge's** effect's.
    const CHARGE_GUID: &str = "effects.FixtureMissileLaunch";

    /// Invented typedata, **not the client's numbers**: `useMissiles`
    /// (101, no guid), the charge's default effect with [`CHARGE_GUID`], a
    /// launcher taking group 99462 one round a cycle, and a missile of
    /// that group that flies (37, 281) and does damage.
    fn launcher_typedata() -> String {
        format!(
            "\
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t
effect\t9\t2\t1\t0\t0\t0\t0\t0\tmissileLaunching\t{CHARGE_GUID}
type\t{LAUNCHER_TYPE}\t99461\t7\t1\t1\t5.0\t0.6\t0.0\t1.0\t101\t12,16,40,101\t51={CYCLE_MS}.0;56=1.0;604=99462.0;1795={RELOAD_MS}.0\tFixture Launcher
type\t{MISSILE_TYPE}\t99462\t8\t1\t100\t0.015\t0.0\t700.0\t300.0\t9\t9\t37=3000.0;70=0.0001;114=10.0;117=20.0;281=5000.0;653=100.0;654=40.0\tFixture Missile
"
        )
    }

    /// [`alice_has_bob_locked`] on [`launcher_typedata`], with the
    /// launcher online in Alice's high slot 1 and `rounds` missiles in it.
    fn alice_has_a_launcher(rounds: i64) -> Registry {
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(&launcher_typedata()).expect("parses"),
            ),
        );
        let mut reg = locked_on(&[], Some(std::sync::Arc::new(content)));
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: LAUNCHER,
            type_id: LAUNCHER_TYPE,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT_1,
            quantity: 1,
            group_id: 99461,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: BOB_POD + 1,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(LAUNCHER, true);
        if rounds > 0 {
            reg.apply_items(&[crate::item::Edit::Create {
                type_id: MISSILE_TYPE,
                quantity: rounds,
                to: crate::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_1),
            }])
            .expect("loaded");
        }
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
        reg
    }

    fn rounds_in_launcher(reg: &Registry) -> i64 {
        crate::modules::loaded(reg.store(), ALICE_SHIP, HI_SLOT_1).map_or(0, |i| i.quantity)
    }

    fn flights(reg: &Registry) -> usize {
        reg.ext::<crate::missile::Flights>().len()
    }

    fn fire_launcher(
        reg: &mut Registry,
        now: i64,
    ) -> Result<PyValue, crate::service::ServiceError> {
        crate::modules::activate(
            reg,
            Some(ALICE),
            LAUNCHER,
            "useMissiles",
            Some(BOB_SHIP),
            1000,
            now,
        )
    }

    /// The `UserError` key `r` raised: `(class, (msg, dict))`.
    fn raised(r: Result<PyValue, crate::service::ServiceError>) -> String {
        let header = match r {
            Err(crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. })) => header,
            other => panic!("a raise, got {other:?}"),
        };
        let PyValue::Tuple(parts) = *header else {
            panic!("the exception header is (class, args)");
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError takes (msg, dict)");
        };
        match &args[0] {
            PyValue::Bytes(key) => String::from_utf8_lossy(key).into_owned(),
            other => panic!("the key is a str, got {other:?}"),
        }
    }

    /// Every `launchModules` value in `v`, however deep. A `Dict` holds
    /// its pairs value first (`PyValue::Dict`).
    fn launch_modules(v: &PyValue, out: &mut Vec<PyValue>) {
        match v {
            PyValue::Dict(pairs) => {
                for (val, k) in pairs {
                    if *k == crate::pyutil::bstr("launchModules") {
                        out.push(val.clone());
                    }
                    launch_modules(val, out);
                }
            }
            PyValue::List(xs) | PyValue::Tuple(xs) => {
                for x in xs {
                    launch_modules(x, out);
                }
            }
            _ => {}
        }
    }

    /// **A launcher cycle launches one missile and spends one round** (M4):
    /// Alice's launcher at Bob flies one missile per cycle -- a ball whose
    /// slim names the launcher in `launchModules` -- and Bob takes no
    /// instant damage; the missile lands it later (`crate::missile::tick`).
    #[test]
    fn a_launcher_cycle_launches_one_missile_and_spends_one_round() {
        let mut reg = alice_has_a_launcher(3);
        let t0 = crate::pyutil::filetime_now();
        fire_launcher(&mut reg, t0).expect("the launcher activates");
        tick(&mut reg, t0);
        assert_eq!(flights(&reg), 1, "one missile in the air");
        assert_eq!(rounds_in_launcher(&reg), 2, "one round spent");
        assert!(reg.combat().ship(BOB_SHIP).is_none(), "no instant volley");
        let mut lists = Vec::new();
        for (_, n, args) in due(&mut reg) {
            if n == "DoDestinyUpdate" {
                for a in &args {
                    launch_modules(a, &mut lists);
                }
            }
        }
        assert_eq!(
            lists,
            vec![PyValue::List(vec![PyValue::Int(LAUNCHER)])],
            "the ball's slim names the launcher"
        );

        // The next cycle, off the clock: one more of each.
        let t1 = t0 + CYCLE_MS * crate::modules::MSEC;
        crate::modules::tick(&mut reg, t1);
        tick(&mut reg, t1);
        assert_eq!(flights(&reg), 2);
        assert_eq!(rounds_in_launcher(&reg), 1);
    }

    // -----------------------------------------------------------------
    // A grouped bank is one unit (BANK-FIRE)
    // -----------------------------------------------------------------

    /// Invented second launcher, and two more guns.
    const LAUNCHER_2: i64 = 990_460_011;
    const GUN_2: i64 = 990_460_012;
    const GUN_3: i64 = 990_460_013;
    const GUN_4: i64 = 990_460_014;

    /// **A 4-gun bank lands one volley**: one `OnDamageMessage` to the
    /// shooter and one to the victim, `isBanked` set, damage the sum of the
    /// four, where four unbanked guns send four lines.
    #[test]
    fn a_four_gun_bank_sends_one_banked_line_with_the_sum() {
        let at = crate::pyutil::filetime_now();
        let lines = |banked: bool| {
            let mut reg = alice_has_bob_locked();
            if banked {
                crate::weapon_bank::set_bank_for_test(
                    &mut reg,
                    ALICE_SHIP,
                    BLASTER,
                    &[GUN_2, GUN_3, GUN_4],
                );
            }
            let cycles: Vec<Cycle> = [BLASTER, GUN_2, GUN_3, GUN_4]
                .into_iter()
                .map(|m| Cycle {
                    module_id: m,
                    ..blaster_cycle(at, 0)
                })
                .collect();
            on_cycles(&mut reg, cycles, at);
            due(&mut reg)
                .into_iter()
                .filter(|(to, n, _)| *n == "OnDamageMessage" && to == &vec![ALICE])
                .map(|(_, _, a)| a[0].clone())
                .collect::<Vec<_>>()
        };
        let get = |d: &PyValue, k: &str| crate::pyutil::dict_get(d, k.as_bytes()).cloned();
        let real = |v: Option<PyValue>| match v {
            Some(PyValue::Real(f)) => f,
            other => panic!("a float, got {other:?}"),
        };
        let apart = lines(false);
        assert_eq!(apart.len(), 4, "four guns, four lines");
        assert!(
            apart
                .iter()
                .all(|d| get(d, "isBanked") == Some(PyValue::Bool(false)))
        );
        let bank = lines(true);
        assert_eq!(bank.len(), 1, "a bank is one line");
        assert_eq!(get(&bank[0], "isBanked"), Some(PyValue::Bool(true)));
        assert_eq!(
            get(&bank[0], "weapon"),
            Some(PyValue::Int(3640)),
            "the master's type"
        );
        let sum: f64 = apart.iter().map(|d| real(get(d, "damage"))).sum();
        assert!((real(get(&bank[0], "damage")) - sum).abs() < 1e-6, "{sum}");
    }

    /// **A launcher bank launches one missile**: two launchers in a bank
    /// fire one ball whose slim names both, carrying twice the volley, and
    /// each launcher spends its own round.
    #[test]
    fn a_launcher_bank_launches_one_missile_carrying_every_members_volley() {
        let mut reg = alice_has_a_launcher(3);
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: LAUNCHER_2,
            type_id: LAUNCHER_TYPE,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT_1 + 1,
            quantity: 1,
            group_id: 99461,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: BOB_POD + 1,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(LAUNCHER_2, true);
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: MISSILE_TYPE,
            quantity: 3,
            to: crate::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_1 + 1),
        }])
        .expect("loaded");
        crate::weapon_bank::set_bank_for_test(&mut reg, ALICE_SHIP, LAUNCHER, &[LAUNCHER_2]);
        let _ = due(&mut reg);
        let t0 = crate::pyutil::filetime_now();
        crate::weapon_bank::activate(
            &mut reg,
            Some(ALICE),
            LAUNCHER,
            "useMissiles",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("the bank activates");
        tick(&mut reg, t0);
        assert_eq!(flights(&reg), 1, "one missile for the bank");
        assert_eq!(rounds_in_launcher(&reg), 2, "the master spent one");
        let second = crate::modules::loaded(reg.store(), ALICE_SHIP, HI_SLOT_1 + 1);
        assert_eq!(second.map(|i| i.quantity), Some(2), "so did the member");
        let mut lists = Vec::new();
        for (_, n, args) in due(&mut reg) {
            if n == "DoDestinyUpdate" {
                for a in &args {
                    launch_modules(a, &mut lists);
                }
            }
        }
        assert_eq!(
            lists,
            vec![PyValue::List(vec![
                PyValue::Int(LAUNCHER),
                PyValue::Int(LAUNCHER_2)
            ])],
            "the slim names both launchers"
        );
    }

    // -----------------------------------------------------------------
    // The range of a projected module (mine-range, `crate::module_range`)
    // -----------------------------------------------------------------

    /// Invented: the web's item, and its type (the fixture row's).
    const WEB: i64 = 100_000_070;
    const WEB_TYPE: i64 = 21860;
    /// `flagMedSlot0` [V].
    const MED_SLOT_0: i64 = 19;

    /// Alice with a webifier online, Bob `gap` metres off her centre, both
    /// at rest.
    fn alice_has_a_web(gap: f64) -> Registry {
        let mut reg = locked_on(&[], None);
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: WEB,
            type_id: WEB_TYPE,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: MED_SLOT_0,
            quantity: 1,
            group_id: 65,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: BOB_POD + 1,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(WEB, true);
        bob_at(&mut reg, gap);
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
        reg
    }

    /// Alice at the origin of a far corner, Bob `gap` along x, both at rest
    /// as of now.
    fn bob_at(reg: &mut Registry, gap: f64) {
        for (who, x) in [(ALICE, 1.0e9), (BOB, 1.0e9 + gap)] {
            let p = reg.roster_mut().get_mut(who).expect("in space");
            p.motion = crate::space_state::Motion::arrived([x, 0.0, 0.0], [0.0; 3]);
            p.since = crate::space_state::now_seconds();
        }
    }

    fn web(reg: &mut Registry, now: i64) -> Result<PyValue, crate::service::ServiceError> {
        crate::modules::activate(
            reg,
            Some(ALICE),
            WEB,
            "remoteWebifierFalloff",
            Some(BOB_SHIP),
            1000,
            now,
        )
    }

    /// **A projected module beyond its reach is refused, with the client's
    /// own key, and nothing starts**; inside it, it runs. The reach is
    /// `maxRange` plus three falloffs from the surface (10 km + 3 x 5 km).
    #[test]
    fn a_web_beyond_its_reach_is_refused_and_inside_it_runs() {
        let t0 = crate::pyutil::filetime_now();
        let mut reg = alice_has_a_web(200_000.0);
        assert_eq!(raised(web(&mut reg, t0)), "TargetTooFar");
        assert!(reg.modules().running(WEB).is_none(), "nothing started");
        assert!(
            due(&mut reg)
                .iter()
                .all(|(_, n, _)| *n != "OnGodmaShipEffect")
        );

        let mut reg = alice_has_a_web(8_000.0);
        web(&mut reg, t0).expect("in range it starts");
        assert!(reg.modules().running(WEB).is_some());

        // Just inside the reach, hulls and all, still runs; just outside
        // does not.
        let mut reg = alice_has_a_web(24_000.0);
        web(&mut reg, t0).expect("a surface gap under 25 km");
        let mut reg = alice_has_a_web(30_000.0);
        assert_eq!(raised(web(&mut reg, t0)), "TargetTooFar");
    }

    /// **A target that moves out of reach mid-cycle stops the module** with
    /// `TargetTooFar` as the reason on the stop push; moving back and
    /// restarting works.
    #[test]
    fn a_web_stops_when_its_target_leaves_its_reach() {
        let t0 = crate::pyutil::filetime_now();
        let mut reg = alice_has_a_web(8_000.0);
        web(&mut reg, t0).expect("in range it starts");
        let _ = due(&mut reg);
        // Bob flies off; the next look from the clock sees it. The web keeps
        // its running cycle and stops at the boundary (range-stop, like
        // GUN-CYCLE), so it cannot be freed and restarted mid-cycle.
        bob_at(&mut reg, 200_000.0);
        crate::modules::tick(&mut reg, t0 + 1_000 * crate::modules::MSEC);
        assert!(reg.modules().running(WEB).is_some(), "still in its cycle");
        crate::modules::tick(&mut reg, t0 + 60_000 * crate::modules::MSEC);
        assert!(reg.modules().running(WEB).is_none(), "the web stopped");
        // The stop pair is posted for the boundary, up to a cycle away.
        let told = format!(
            "{:?}",
            reg.take_due(Instant::now() + Duration::from_secs(60))
                .into_iter()
                .map(|p| p.to_push())
                .collect::<Vec<_>>()
        );
        let key = format!("{:?}", crate::pyutil::bstr("TargetTooFar"));
        assert!(told.contains(&key), "the stop names its reason: {told}");
        bob_at(&mut reg, 8_000.0);
        web(&mut reg, t0 + 61_000 * crate::modules::MSEC).expect("restarts in range");
    }

    /// **A gun is not refused for range**: it fires and misses, as in EVE
    /// (`turret_application_socket`); this pins that the new check leaves a
    /// turret alone.
    #[test]
    fn a_turret_is_not_refused_for_range() {
        let mut reg = alice_has_a_web(8_000.0);
        bob_at(&mut reg, 900_000.0);
        let t0 = crate::pyutil::filetime_now();
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            BLASTER,
            "targetAttack",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("a gun starts at any range");
    }

    /// **A launcher stops at the end of its cycle when its target is gone**
    /// (GUN-CYCLE, was ENGINE-FIX's "at once"): the target leaves space, its
    /// locks go with it, the running cycle runs out, and the effect ends at
    /// the boundary, after which a restart is accepted.
    #[test]
    fn a_launcher_stops_at_the_cycle_end_when_its_target_leaves_space() {
        let mut reg = alice_has_a_launcher(5);
        let t0 = crate::pyutil::filetime_now();
        fire_launcher(&mut reg, t0).expect("the launcher activates");
        assert!(reg.modules().running(LAUNCHER).is_some());
        crate::targeting::left_space(
            &mut reg,
            BOB,
            crate::targeting::Reason::Exploding,
            crate::targeting::Delivery::OnTheClock,
        );
        // A tenth of the cycle on: far from the boundary, still cycling.
        crate::modules::tick(&mut reg, t0 + CYCLE_MS * crate::modules::MSEC / 10);
        assert!(
            reg.modules().running(LAUNCHER).is_some(),
            "the running cycle is not cut short"
        );
        crate::modules::tick(&mut reg, t0 + CYCLE_MS * crate::modules::MSEC);
        assert!(
            reg.modules().running(LAUNCHER).is_none(),
            "the effect ended with its cycle"
        );
    }

    /// **The launch FX is the charge's** (R-MISSILE §1.6): `OnSpecialFX` on
    /// the launcher item and type, the charge's default-effect guid, the
    /// charge as `otherTypeID`, offensive; once at activation and once
    /// more per cycle off the clock.
    #[test]
    fn the_launch_fx_carries_the_charges_guid_and_type() {
        let mut reg = alice_has_a_launcher(3);
        let t0 = crate::pyutil::filetime_now();
        fire_launcher(&mut reg, t0).expect("the launcher activates");
        let is_launch_fx = |args: &[PyValue]| {
            args[1] == PyValue::Int(LAUNCHER)
                && args[2] == PyValue::Int(LAUNCHER_TYPE)
                && args[3] == PyValue::Int(BOB_SHIP)
                && args[4] == PyValue::Int(MISSILE_TYPE)
                && args[5] == PyValue::Bytes(CHARGE_GUID.as_bytes().to_vec())
                && args[6] == PyValue::Bool(true)
        };
        let now: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| *n == "OnSpecialFX")
            .collect();
        assert_eq!(now.len(), 1, "{now:?}");
        assert!(is_launch_fx(&now[0].1), "{now:?}");
        let _ = due(&mut reg);
        crate::modules::tick(&mut reg, t0 + CYCLE_MS * crate::modules::MSEC);
        let later: Vec<_> = due(&mut reg)
            .into_iter()
            .filter(|(_, n, _)| *n == "OnSpecialFX")
            .collect();
        assert_eq!(later.len(), 1, "{later:?}");
        assert!(is_launch_fx(&later[0].2), "{later:?}");
    }

    /// **An empty launcher launches nothing**: the activation is refused
    /// with the client's own `NoCharges` key and dict (`smb:1539`) [V], and
    /// a cycle that fired no charge flies nothing.
    #[test]
    fn an_empty_launcher_launches_nothing() {
        let mut reg = alice_has_a_launcher(0);
        let t0 = crate::pyutil::filetime_now();
        assert_eq!(raised(fire_launcher(&mut reg, t0)), "NoCharges");
        assert!(reg.modules().running(LAUNCHER).is_none());
        on_cycles(
            &mut reg,
            vec![Cycle {
                module_id: LAUNCHER,
                type_id: LAUNCHER_TYPE,
                effect_id: crate::modules::EFFECT_USE_MISSILES,
                charge_type_id: None,
                ..blaster_cycle(t0, 0)
            }],
            t0,
        );
        assert_eq!(flights(&reg), 0);
        assert!(reg.combat().ship(BOB_SHIP).is_none());
    }

    /// **The last round, then a reload**: a launcher with one missile
    /// launches it, stops at the next boundary with nothing loaded, takes
    /// a reload from the cargo hold, refuses to fire while it lasts
    /// (`CantActivateReloadingModule`) and launches again after it.
    #[test]
    fn a_launcher_reloads_from_the_hold_and_launches_again() {
        let mut reg = alice_has_a_launcher(1);
        let t0 = crate::pyutil::filetime_now();
        fire_launcher(&mut reg, t0).expect("the launcher activates");
        tick(&mut reg, t0);
        assert_eq!((flights(&reg), rounds_in_launcher(&reg)), (1, 0));
        let t1 = t0 + CYCLE_MS * crate::modules::MSEC;
        crate::modules::tick(&mut reg, t1);
        tick(&mut reg, t1);
        assert!(reg.modules().running(LAUNCHER).is_none(), "out of rounds");
        assert_eq!(flights(&reg), 1, "nothing more launched");

        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: MISSILE_TYPE,
                quantity: 10,
                to: crate::item::Slot::new(ALICE, ALICE_SHIP, crate::fitting::FLAG_CARGO),
            }])
            .expect("spawned");
        let stack = changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a stack");
        crate::modules::load_ammo(&mut reg, Some(ALICE), &[LAUNCHER], &[stack], ALICE_SHIP, t1)
            .expect("reloaded");
        assert_eq!(rounds_in_launcher(&reg), 10);
        assert_eq!(
            raised(fire_launcher(&mut reg, t1)),
            "CantActivateReloadingModule"
        );
        let t2 = t1 + RELOAD_MS * crate::modules::MSEC;
        fire_launcher(&mut reg, t2).expect("reloaded, it fires");
        tick(&mut reg, t2);
        assert_eq!((flights(&reg), rounds_in_launcher(&reg)), (2, 9));
    }
}
