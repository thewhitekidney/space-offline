//! System-wide effects: which effect beacon a system carries (SYS-1).
//!
//! A wormhole system's `secondarySun` names an **effect beacon**, a group 920
//! type whose category 7 dogma effects (in `types/typedata.tsv`) are the
//! system's magnetar, black hole, red giant and so on **[V-data]**
//! (`handback/r-wormhole` §3). `tools/content.py` writes one row per such
//! system to `universe/system-effects.tsv`; this module loads it as [`TABLE`].
//! The beacon type is read from the row, never computed from the system's
//! class: C13, the Drifter systems and some C4 systems do not match.
//!
//! SYS-1 is the data seam. Applying the effects to a ship's sheet is SYS-2
//! (`dogma::Inputs::beacon`, `dogma::Eval::add_beacon`); telling the client is
//! SYS-3 (the second half of this file). A bundle without the file serves no
//! effect, and a system without a row has none.

use crate::damage::BlueTime;
use crate::notify::Push;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The bundle file and its header (`docs/content/schema.md`).
pub const FILE: &str = "universe/system-effects.tsv";
pub const HEADER: &str = "#system_id\tbeacon_type_id\tsun_type_id\tsun_item_id\tx\ty\tz";

/// One system's beacon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub system_id: i64,
    /// The group 920 type: what the client's tooltip subscripts, so what the
    /// server sends as `sourceTypeID`.
    pub beacon_type_id: i64,
    /// The sun type the beacon accompanies (not a beacon).
    pub sun_type_id: Option<i64>,
    /// The sun's item id.
    pub sun_item_id: Option<i64>,
    /// `secondarySun.position`, metres in the system frame.
    pub position: [f64; 3],
}

/// Every system's beacon, by system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects(BTreeMap<i64, Row>);

impl Effects {
    /// Build from the bundle's data lines. A short or unreadable line is an
    /// error naming it; a system twice is too.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Effects, String> {
        let mut out = BTreeMap::new();
        for (n, l) in lines.iter().enumerate() {
            let bad = || format!("{FILE} data line {}: {:?}", n + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            if f.len() != 7 {
                return Err(bad());
            }
            let int = |i: usize| f[i].parse::<i64>().map_err(|_| bad());
            let opt = |i: usize| -> Result<Option<i64>, String> {
                if f[i].is_empty() {
                    Ok(None)
                } else {
                    int(i).map(Some)
                }
            };
            let num = |i: usize| {
                f[i].parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(bad)
            };
            let row = Row {
                system_id: int(0)?,
                beacon_type_id: int(1)?,
                sun_type_id: opt(2)?,
                sun_item_id: opt(3)?,
                position: [num(4)?, num(5)?, num(6)?],
            };
            if out.insert(row.system_id, row).is_some() {
                return Err(format!("{FILE}: system {} twice", row.system_id));
            }
        }
        Ok(Effects(out))
    }

    /// How many systems carry a beacon.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "system effects",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Effects::default()));
    };
    let lines = b.data_lines(FILE, HEADER)?;
    if lines.is_empty() {
        src.note(crate::content::Origin::Fixture);
    }
    Ok(Box::new(Effects::parse(&lines)?))
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Effects::parse(&fixture::rows()).expect("the fixture rows are well formed"))
}

/// The beacon of `system` in `content`: `None` for a system without one and
/// for a bundle without `universe/system-effects.tsv`.
pub fn beacon_of(content: &crate::content::Content, system: i64) -> Option<Row> {
    content.try_get::<Effects>()?.0.get(&system).copied()
}

// ---------------------------------------------------------------------------
// SYS-3: telling the client
// ---------------------------------------------------------------------------

/// `effectCategory` 7: the system-wide effects of a beacon.
const CATEGORY_SYSTEM: i64 = 7;

/// The category 7 effect ids of a beacon type, ascending.
pub fn effects_of(reg: &Registry, beacon_type: i64) -> Vec<i64> {
    let table = crate::typedata::installed_in(reg.content()).table();
    let Some(row) = table.get(beacon_type) else {
        return Vec::new();
    };
    let mut ids: Vec<i64> = row
        .effects
        .iter()
        .copied()
        .filter(|e| {
            table
                .effect(*e)
                .is_some_and(|e| e.category == CATEGORY_SYSTEM)
        })
        .collect();
    ids.sort_unstable();
    ids
}

/// Whether `beacon_type` carries a category 7 modifier on the ship's own
/// `maxVelocity`: a Black Hole does, a Red Giant (modules and charges only)
/// does not. `/whjump fx` picks such a system, so a scenario can see a ship
/// attribute move.
pub fn slows_ships(content: &crate::content::Content, beacon_type: i64) -> bool {
    let table = crate::typedata::installed_in(content).table();
    let Some(row) = table.get(beacon_type) else {
        return false;
    };
    row.effects
        .iter()
        .filter(|e| {
            table
                .effect(**e)
                .is_some_and(|e| e.category == CATEGORY_SYSTEM)
        })
        .flat_map(|e| table.modifiers(*e))
        .any(|m| {
            matches!(m.domain, crate::typedata::Domain::Ship)
                && m.modified_attribute == Some(crate::dogma::attr::MAX_VELOCITY)
        })
}

/// Whether `beacon_type` carries a category 7 modifier that aims at the ship's
/// fitted modules (a group, skill or whole-location aim on `shipID`): a Red
/// Giant or a Magnetar does, whatever it does to the hull. `/whjump fxmod`
/// picks such a system (SYS-4), so a scenario can see a module attribute move.
pub fn moves_modules(content: &crate::content::Content, beacon_type: i64) -> bool {
    use crate::typedata::{Domain, Func};
    let table = crate::typedata::installed_in(content).table();
    let Some(row) = table.get(beacon_type) else {
        return false;
    };
    row.effects
        .iter()
        .filter(|e| {
            table
                .effect(**e)
                .is_some_and(|e| e.category == CATEGORY_SYSTEM)
        })
        .flat_map(|e| table.modifiers(*e))
        .any(|m| {
            matches!(m.domain, Domain::Ship)
                && matches!(
                    m.func,
                    Func::LocationModifier
                        | Func::LocationGroupModifier
                        | Func::LocationRequiredSkillModifier
                )
        })
}

/// The `sourceItemID` the client keys a beacon by: the secondary sun's item
/// id [A: `godma` only keys on it; falsifier: a retail capture of
/// `systemWideEffectsOnShip` naming another id], the beacon type when the row
/// has none.
fn source_item(row: &Row) -> i64 {
    row.sun_item_id.unwrap_or(row.beacon_type_id)
}

/// What `GetAllInfo.systemWideEffectsOnShip` says for `character`: the
/// client's `{(sourceItemID, sourceTypeID): set(effectIDs)}`, `sourceTypeID`
/// being the **beacon** type (the tooltip subscripts it). `None` unless the
/// pilot is in space in a system with a beacon, which is the only time the
/// ship carries the effects (docked: [A], falsifier: the fitting window in a
/// docked citadel showing them). An undock or a login that has not yet been
/// seated gets its effects from [`tick`].
pub fn info(reg: &Registry, character_id: i64) -> PyValue {
    let Some(p) = reg.roster().get(character_id) else {
        return PyValue::None;
    };
    let Some(row) = beacon_of(reg.content(), p.solar_system_id) else {
        return PyValue::None;
    };
    let effects = effects_of(reg, row.beacon_type_id);
    if effects.is_empty() {
        return PyValue::None;
    }
    PyValue::Dict(vec![(
        crate::fleet_finder::py_set(&effects.into_iter().collect::<BTreeSet<i64>>()),
        PyValue::Tuple(vec![
            PyValue::Int(source_item(&row)),
            PyValue::Int(row.beacon_type_id),
        ]),
    )])
}

/// What one pilot has been told.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shown {
    system: i64,
    beacon: i64,
    item: i64,
    ship: i64,
}

#[derive(Debug, Default)]
pub struct State {
    shown: BTreeMap<i64, Shown>,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "system_effects",
    make: |_| Box::new(State::default()) as Boxed,
};

/// `OnSystemWideEffectStart` / `OnSystemWideEffectStop`
/// `(effectID, sourceItemID, sourceTypeID)` **[V]**
/// (`systemWideEffectSvc.__notifyevents__`).
#[derive(Debug, Clone, PartialEq)]
pub struct EffectPush {
    pub character: i64,
    pub start: bool,
    pub effect: i64,
    pub item: i64,
    pub beacon: i64,
}

impl Push for EffectPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.start {
                "OnSystemWideEffectStart"
            } else {
                "OnSystemWideEffectStop"
            },
            vec![
                PyValue::Int(self.effect),
                PyValue::Int(self.item),
                PyValue::Int(self.beacon),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// **The world clock's stage** (SYS-3): bring what each pilot has been told
/// in line with where they are. One place that sees every way in and out of a
/// system (gate, wormhole, jump drive, `/whjump`, `/tr`, undock, dock, death,
/// login) because it reads the roster, which all of them write. A pilot who
/// arrived is sent a Start per effect and the attribute pushes the beacon
/// moved; one who left or died, the Stop and the revert. A pilot already told
/// costs one map lookup. Runs after the bind, because the roster takes the
/// pilot at the bind [A: falsifier, an empty effects container on arrival].
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    let nobody_told = w.ext::<State>().shown.is_empty();
    if nobody_told
        && w.content()
            .try_get::<Effects>()
            .is_none_or(|e| e.is_empty())
    {
        return;
    }
    // Who is in space in a beacon system, and where.
    let mut here: BTreeMap<i64, Shown> = BTreeMap::new();
    for (system, ids) in w.roster().systems() {
        let Some(row) = beacon_of(w.content(), system) else {
            continue;
        };
        // The ids first: NPCs and drones share the roster and are dropped
        // by the character test before any presence is read.
        for id in ids {
            if w.character(*id).is_none() {
                continue;
            }
            let Some(p) = w.roster().get(*id) else {
                continue;
            };
            here.insert(
                *id,
                Shown {
                    system,
                    beacon: row.beacon_type_id,
                    item: source_item(&row),
                    ship: p.ship_id,
                },
            );
        }
    }
    let told: Vec<(i64, Shown)> = w
        .ext::<State>()
        .shown
        .iter()
        .map(|(c, s)| (*c, *s))
        .collect();
    for (character, was) in told {
        if here.get(&character) == Some(&was) {
            continue;
        }
        w.ext_mut::<State>().shown.remove(&character);
        send(w, character, was, false, now);
    }
    for (character, is) in here {
        if w.ext::<State>().shown.get(&character) == Some(&is) {
            continue;
        }
        w.ext_mut::<State>().shown.insert(character, is);
        send(w, character, is, true, now);
    }
}

/// Start or stop every effect of `at` for `character`, then the attributes
/// the beacon moved on the ship (`boosters::after_change`'s pushes).
fn send(w: &mut Registry, character: i64, at: Shown, start: bool, now: BlueTime) {
    for effect in effects_of(w, at.beacon) {
        w.schedule(
            Duration::ZERO,
            EffectPush {
                character,
                start,
                effect,
                item: at.item,
                beacon: at.beacon,
            },
        );
    }
    // The ship's attributes as the sheet reads them now, against the same
    // inputs with the beacon on or off. A ship that is gone has no sheet.
    let ship = if start {
        at.ship
    } else {
        match w.character(character) {
            Some(c) if c.ship_id == at.ship => at.ship,
            _ => return,
        }
    };
    let Some(current) = crate::dogma::inputs(w, ship) else {
        return;
    };
    let table = crate::typedata::installed_in(w.content()).table();
    if table
        .get(current.ship_type)
        .is_none_or(|t| t.category_id != crate::typedata::CATEGORY_SHIP)
    {
        return;
    }
    let (old, new) = if start {
        (
            crate::dogma::Inputs {
                beacon: None,
                ..current.clone()
            },
            current,
        )
    } else {
        (
            crate::dogma::Inputs {
                beacon: Some(at.beacon),
                ..current.clone()
            },
            current,
        )
    };
    let buffs = crate::dbuff::applied(w, ship);
    let changes = crate::dbuff::attribute_changes_inputs(table, ship, &old, &new, &buffs);
    let in_space = w.roster().get(character).is_some_and(|p| p.ship_id == ship);
    if in_space {
        let speed = changes
            .iter()
            .any(|c| c.item == ship && c.attribute == crate::dogma::attr::MAX_VELOCITY);
        if speed && let Some(h) = crate::dogma::hull(w, ship) {
            crate::dbuff::speed_push(w, character, ship, f64::from(h.max_velocity));
        }
    }
    // A pilot who left by a jump is out of the roster for the tunnel, yet the
    // combat record is kept for them (`combat`), so the beacon's armour and
    // shield profile must come off it too.
    crate::dbuff::refresh_record(w, ship, character, now);
    for c in &changes {
        crate::dbuff::attribute_push(w, character, c, now);
    }
}

/// Invented rows for tests: round numbers, not the client's.
pub mod fixture {
    pub fn rows() -> Vec<String> {
        [
            "990001\t990101\t990201\t990301\t1000.0\t-2000.0\t3000.5",
            // A beacon with no sun columns and the origin.
            "990002\t990102\t\t\t0.0\t0.0\t0.0",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_parse_with_optional_sun_columns() {
        let e = Effects::parse(&fixture::rows()).unwrap();
        assert_eq!(e.len(), 2);
        let a = e.0[&990001];
        assert_eq!(a.beacon_type_id, 990101);
        assert_eq!(a.sun_type_id, Some(990201));
        assert_eq!(a.sun_item_id, Some(990301));
        assert_eq!(a.position, [1000.0, -2000.0, 3000.5]);
        let b = e.0[&990002];
        assert_eq!((b.sun_type_id, b.sun_item_id), (None, None));
    }

    #[test]
    fn a_short_row_a_bad_number_and_a_repeat_are_errors() {
        let ok = fixture::rows();
        assert!(Effects::parse(&["990001\t990101".to_string()]).is_err());
        let nan = ok[0].replace("1000.0", "far");
        assert!(Effects::parse(&[nan]).is_err());
        let twice = vec![ok[0].clone(), ok[0].clone()];
        assert!(Effects::parse(&twice).unwrap_err().contains("twice"));
        assert!(Effects::parse::<String>(&[]).unwrap().is_empty());
    }

    #[test]
    fn beacon_of_reads_the_row_and_says_none_for_other_systems() {
        let c = crate::content::Content::fixture();
        assert_eq!(
            beacon_of(&c, 990001).map(|r| r.beacon_type_id),
            Some(990101)
        );
        assert_eq!(beacon_of(&c, 990999), None);
    }

    #[test]
    fn the_header_is_the_one_the_writer_emits() {
        // schema.md prints the header line; the writer's columns are these.
        assert_eq!(HEADER.matches('\t').count(), 6);
        assert!(HEADER.starts_with("#system_id\tbeacon_type_id"));
    }

    // ---- SYS-3: telling the client. Invented ids; the beacon 990101 and
    // the sun item 990301 are the fixture rows' for the system 990001. ----

    use std::sync::Arc;

    const HULL: i64 = 990_401;
    const SHIP: i64 = 9_100_911;
    const EFFECT: i64 = 99_310;
    const IN_SYSTEM: i64 = 990_001;
    const ELSEWHERE: i64 = 990_999;

    /// A world with a one-hull table and a beacon of one category 7 effect
    /// (x0.5 on max velocity) and one that is not category 7, with the
    /// first character's ship assembled in its hangar.
    fn world() -> (Registry, i64) {
        let text = format!(
            "attribute\t37\t0.0\tmaxVelocity\n\
             attribute\t99411\t0.0\tinvented\n\
             type\t{HULL}\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t\t37=400.0\tHull\n\
             type\t990101\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{EFFECT},99311\t99411=0.5\tBeacon\n\
             effect\t{EFFECT}\t7\t0\t0\t0\t0\t0\t0\tbeaconEffect\t\n\
             effect\t99311\t0\t0\t0\t0\t0\t0\t0\tpassiveEffect\t\n\
             modifier\t{EFFECT}\tshipID\tItemModifier\t37\t99411\t4\t\t\n\
             modifier\t{EFFECT}\tshipID\tLocationGroupModifier\t64\t99411\t4\t\t55\n"
        );
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(&text).expect("parses"),
            ),
        );
        let mut reg =
            Registry::new(crate::model::SandboxConfig::default()).with_content(Arc::new(content));
        let c = reg.config().characters[0].clone();
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: SHIP,
            type_id: HULL,
            owner_id: c.character_id,
            location_id: c.station_id.expect("docked"),
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: 25,
            category_id: crate::typedata::CATEGORY_SHIP,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: SHIP + 1,
            ..crate::store::Snapshot::default()
        });
        (reg, c.character_id)
    }

    fn fly(reg: &mut Registry, pilot: i64, system: i64) {
        use crate::space_state::{Motion, Presence};
        reg.roster_mut().arrive(Presence::arriving(
            pilot,
            SHIP,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(5))
            .iter()
            .map(|p| p.to_push())
            .collect()
    }

    #[test]
    fn only_the_category_seven_effects_of_the_beacon_are_its_system_effects() {
        let (reg, _) = world();
        assert_eq!(effects_of(&reg, 990_101), vec![EFFECT]);
        assert!(slows_ships(reg.content(), 990_101));
        // SYS-4: the same beacon has a group aim and a skill aim on the ship's
        // modules; a hull with no effects has none.
        assert!(moves_modules(reg.content(), 990_101));
        assert!(!moves_modules(reg.content(), HULL));
        assert!(!moves_modules(reg.content(), 990_998));
        assert!(!slows_ships(reg.content(), HULL));
        assert!(!slows_ships(reg.content(), 990_998));
        assert!(effects_of(&reg, HULL).is_empty());
        assert!(effects_of(&reg, 990_998).is_empty());
    }

    #[test]
    fn get_all_info_names_the_beacon_type_and_a_set_only_in_space_in_its_system() {
        let (mut reg, pilot) = world();
        fly(&mut reg, pilot, ELSEWHERE);
        assert_eq!(info(&reg, pilot), PyValue::None, "no beacon here");
        fly(&mut reg, pilot, IN_SYSTEM);
        let PyValue::Dict(entries) = info(&reg, pilot) else {
            panic!("a dict in a beacon system");
        };
        assert_eq!(entries.len(), 1);
        let (value, key) = &entries[0];
        let item = source_item(&beacon_of(reg.content(), IN_SYSTEM).unwrap());
        assert_eq!(
            key,
            &PyValue::Tuple(vec![PyValue::Int(item), PyValue::Int(990_101)])
        );
        assert_eq!(
            value,
            &crate::fleet_finder::py_set(&[EFFECT].into_iter().collect())
        );
    }

    #[test]
    fn arriving_starts_the_effect_and_moves_the_speed_and_leaving_reverts_it() {
        let (mut reg, pilot) = world();
        let t = 133_000_000_000_000_000;
        tick(&mut reg, Instant::now(), t);
        assert!(pushes(&mut reg).is_empty(), "nobody in a beacon system");

        fly(&mut reg, pilot, IN_SYSTEM);
        tick(&mut reg, Instant::now(), t);
        let sent = pushes(&mut reg);
        let start = sent
            .iter()
            .find(|(n, _)| *n == "OnSystemWideEffectStart")
            .expect("a Start");
        assert_eq!(
            start.1,
            vec![
                PyValue::Int(EFFECT),
                PyValue::Int(source_item(&beacon_of(reg.content(), IN_SYSTEM).unwrap())),
                PyValue::Int(990_101)
            ]
        );
        assert_eq!(
            sent.iter()
                .filter(|(n, _)| *n == "OnSystemWideEffectStart")
                .count(),
            1,
            "one per category 7 effect"
        );
        let speed = sent
            .iter()
            .find(|(n, a)| *n == "OnModuleAttributeChange" && a.contains(&PyValue::Int(37)))
            .expect("the max velocity attribute is pushed");
        assert!(speed.1.contains(&PyValue::Real(200.0)));
        assert!(speed.1.contains(&PyValue::Real(400.0)));

        tick(&mut reg, Instant::now(), t);
        assert!(pushes(&mut reg).is_empty(), "told once");

        // A jump out: the roster says the pilot is now elsewhere.
        fly(&mut reg, pilot, ELSEWHERE);
        tick(&mut reg, Instant::now(), t);
        let sent = pushes(&mut reg);
        assert_eq!(
            sent.iter()
                .filter(|(n, _)| *n == "OnSystemWideEffectStop")
                .count(),
            1
        );
        assert!(!sent.iter().any(|(n, _)| *n == "OnSystemWideEffectStart"));

        // Back in, then out of space altogether.
        fly(&mut reg, pilot, IN_SYSTEM);
        tick(&mut reg, Instant::now(), t);
        let _ = pushes(&mut reg);
        reg.roster_mut().leave(pilot);
        tick(&mut reg, Instant::now(), t);
        assert!(
            pushes(&mut reg)
                .iter()
                .any(|(n, _)| *n == "OnSystemWideEffectStop")
        );
        tick(&mut reg, Instant::now(), t);
        assert!(pushes(&mut reg).is_empty());
    }

    /// A pilot who jumped out is not in the roster, but a hit ship's record
    /// stays for the tunnel: the Stop must take the beacon off it too.
    #[test]
    fn leaving_by_a_jump_reverts_the_combat_record_of_a_hit_ship() {
        let (mut reg, pilot) = world();
        let t = 133_000_000_000_000_000;
        reg.character_mut(pilot).expect("pilot").ship_id = SHIP;
        fly(&mut reg, pilot, IN_SYSTEM);
        tick(&mut reg, Instant::now(), t);
        let _ = pushes(&mut reg);
        let honest = crate::dogma::profile(&reg, SHIP).expect("a profile");
        // A record that still carries something else, as the beacon's
        // numbers would be.
        let stale = crate::damage::Profile {
            armor_hp: honest.armor_hp + 1000.0,
            ..honest
        };
        reg.combat_mut()
            .set_ship(SHIP, crate::damage::ShipDamage::full(stale, t));
        reg.roster_mut().leave(pilot);
        tick(&mut reg, Instant::now(), t);
        let record = *reg.combat().ship(SHIP).expect("the record is kept");
        assert_eq!(*record.profile(), honest);
    }
}
