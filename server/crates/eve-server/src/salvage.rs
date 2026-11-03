//! **Salvaging and the tractor beam** (X-SALV, `docs/lanes/x-salv.md`).
//!
//! A wreck is a ball in space, lockable and in range of a salvager; a
//! successful cycle of the module turns it into salvage in the pilot's hold
//! and the wreck goes. Salvage is what rigs are built from (every published
//! rig blueprint takes group 754 [V-data]), so the loop closes through the
//! existing industry path with nothing added there.
//!
//! ## What the client says [V]
//!
//! The salvager's effect is `salvaging` (2757 [V-data]: category 2 "target",
//! duration 73, range 54, guid `effects.Salvaging`); the client's messages
//! (`findings/user-errors.tsv`, the texts through `tools/loctable.py`) are
//! the rules we keep:
//!
//! - `SalvagingPointlessLoot`: the equipment "cannot be engaged until all
//!   loot has been removed" from the wreck -- **salvage comes after
//!   looting**;
//! - `SalvagingPointlessOverload`: refused without the cargo space "to store
//!   all the resources which might be recovered under ideal circumstances",
//!   naming the m3 to clear;
//! - `SalvagingSuccess`, `SalvagingSuccessOverloaded` (some of it destroyed
//!   for lack of room), `SalvagingSuccessHollow` (nothing of value),
//!   `SalvagingFailure`, `SalvagingTooLate` (already salvaged).
//!
//! Each outcome ends the cycle's run with its key on the module's stop
//! (`OnGodmaShipEffect` `error`, the one channel we have for "why it
//! stopped"); a failed attempt stops the module too, so the pilot sees it
//! and the bot tries again.
//!
//! ## What is ours [A]
//!
//! - **The chance** ([`chance`]): `BASE_CHANCE_PCT` plus the module's
//!   `accessDifficultyBonus` (902 [V-data]: Salvager I 5, II 7) plus the
//!   Salvaging skill's level times its `accessDifficultyBonusAbsolutePercent`
//!   (1772 [V-data], 1 per level), as percent points, kept inside
//!   [`MIN_CHANCE`, `MAX_CHANCE`]. The attributes are the client's; adding
//!   them to a base is ours. Falsified by a success rate on TQ that the
//!   attributes do not explain.
//! - **The yield**: a wreck's size and race ([`Table`], from
//!   `salvage/wrecks.tsv`) pick the rows of `salvage/yields.tsv`; a success
//!   makes `DRAWS` draws of those rows. The client has no such table. A
//!   wreck type the table does not know is size [`DEFAULT_SIZE`], any race.
//! - **The wreck goes on success**, with anything left in it (nothing: the
//!   loot had to go first), and a failure leaves it.
//! - **Theft**: salvaging a wreck you have no right to is theft
//!   (`combat::has_loot_right`, `crimewatch::theft`), as taking from it is.
//!
//! ## The tractor beam [A]
//!
//! `tractorBeamCan` (2255 [V-data]) pulls a locked wreck toward the ship.
//! The real beam drags the ball at `maxTractorVelocity` (1045); the ball is
//! not a free one here, so each **completed cycle** moves the wreck
//! `velocity x cycle seconds` toward the ship in one `SetBallPosition` (the
//! call `/tp` and warp arrival use: nothing resets on the client). The wreck
//! stops [`TRACTOR_STOP_GAP`] short of the hull. Falsified by a client that
//! draws the wreck where it was.
//!
//! The pure parts ([`chance`], [`draws`], [`roll`], [`tractor_step`]) are
//! tested on invented rows; the world tests drive `modules::activate` and
//! the clock.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::modules::{self, Cycle};
use crate::notify::Addressed;
use crate::seed::{self, Stream};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "salvage",
    load: load_table,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// The bundle's files; with no bundle an **empty** table (a salvage is
/// hollow): the fixture's invented types are no client's.
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => {
            let mut t = Table::from_bundle(b)?;
            t.merge(crate::wormhole_sites::salvage_overlay(src)?);
            t
        }
        None => Table::default(),
    }))
}

/// The bundle files and their header lines, copied from `schema.md`.
pub const WRECKS: (&str, &str) = ("salvage/wrecks.tsv", "#wreck_type_id\tsize\trace_id");
pub const YIELDS: (&str, &str) = (
    "salvage/yields.tsv",
    "#size\trace_id\ttype_id\tweight\tqty_min\tqty_max\tvalue",
);

// ---------------------------------------------------------------------------
// Ids and attributes [V-data]
// ---------------------------------------------------------------------------

/// `salvaging` (`dogmaeffects[2757]`) **[V-data]**: the default effect of
/// every Salvager (group 1122).
pub const EFFECT_SALVAGING: i64 = 2757;
/// `tractorBeamCan` (`dogmaeffects[2255]`) **[V-data]**: the default effect
/// of every Tractor Beam (group 650).
pub const EFFECT_TRACTOR_BEAM: i64 = 2255;
/// Salvaging, the skill **[V-data]** (type 25863, group 1218).
pub const SKILL_SALVAGING: i64 = 25_863;
/// `accessDifficultyBonus` **[V-data]**: a salvager's percent points.
pub const ATTR_ACCESS_DIFFICULTY_BONUS: i64 = 902;
/// `accessDifficultyBonusAbsolutePercent` **[V-data]**: the skill's percent
/// points per level (1.0 on Salvaging).
pub const ATTR_SKILL_PER_LEVEL: i64 = 1772;
/// `maxRange` **[V]**.
pub const ATTR_MAX_RANGE: i64 = 54;
/// `maxTractorVelocity` **[V-data]**, m/s.
pub const ATTR_MAX_TRACTOR_VELOCITY: i64 = 1045;
/// `groupSalvagedMaterials` **[V]**.
pub const GROUP_SALVAGE: i64 = 754;

/// The skill's percent points per level when the type carries no 1772.
/// **[A]**, the shipped Salvaging's value.
pub const SKILL_PCT_FALLBACK: f64 = 1.0;
/// The percent chance of a salvager with no bonus and no skill. **[A]**:
/// the client has the bonuses and not the base. Falsified by TQ's observed
/// success rate at Salvaging 0 (and by a bot that never gets a success, or
/// never fails).
pub const BASE_CHANCE_PCT: f64 = 50.0;
/// The chance's floor and ceiling, as fractions. **[A]**.
pub const MIN_CHANCE: f64 = 0.05;
pub const MAX_CHANCE: f64 = 0.95;
/// A wreck type the table does not know. **[A]**.
pub const DEFAULT_SIZE: u8 = 2;
/// Draws a success makes, by size (1, 2, 3): `(min, max)` inclusive.
/// **[A]**. Falsified by a flood of salvage on the book, or none.
pub const DRAWS: [(u64, u64); 3] = [(1, 2), (2, 4), (3, 6)];
/// How far the beam leaves a wreck from the hull, surface to surface, in
/// metres. **[A]**.
pub const TRACTOR_STOP_GAP: f64 = 100.0;

/// A domain tag in the salvage stream's parts, so a wreck's draw never meets
/// an NPC's or a can's.
const STREAM_TAG: u64 = 0x0053_4c56;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One row of `salvage/yields.tsv`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Yield {
    pub type_id: i64,
    pub weight: u32,
    pub qty: (i64, i64),
    pub value: f64,
}

/// `salvage/wrecks.tsv` and `salvage/yields.tsv`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// `wreck type -> (size, race)`.
    wrecks: BTreeMap<i64, (u8, i64)>,
    /// `(size, race) -> rows`; race 0 is any.
    yields: BTreeMap<(u8, i64), Vec<Yield>>,
}

impl Table {
    /// The bundle's files; an absent one is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let (w, wh) = WRECKS;
        let (y, yh) = YIELDS;
        Table::parse(&b.data_lines(w, wh)?, &b.data_lines(y, yh)?)
    }

    /// The data lines of both files. A bad row is an error naming its line.
    pub fn parse<S: AsRef<str>>(wrecks: &[S], yields: &[S]) -> Result<Table, String> {
        let mut t = Table::default();
        for (i, line) in wrecks.iter().enumerate() {
            let bad = || format!("{}: data line {}: {:?}", WRECKS.0, i + 1, line.as_ref());
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [wreck, size, race] = f[..] else {
                return Err(bad());
            };
            let size: u8 = size.parse().map_err(|_| bad())?;
            if !(1..=3).contains(&size) {
                return Err(bad());
            }
            t.wrecks.insert(
                wreck.parse().map_err(|_| bad())?,
                (size, race.parse().map_err(|_| bad())?),
            );
        }
        for (i, line) in yields.iter().enumerate() {
            let bad = || format!("{}: data line {}: {:?}", YIELDS.0, i + 1, line.as_ref());
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [size, race, type_id, weight, lo, hi, value] = f[..] else {
                return Err(bad());
            };
            let size: u8 = size.parse().map_err(|_| bad())?;
            let qty: (i64, i64) = (
                lo.parse().map_err(|_| bad())?,
                hi.parse().map_err(|_| bad())?,
            );
            let weight: u32 = weight.parse().map_err(|_| bad())?;
            if !(1..=3).contains(&size) || qty.0 < 1 || qty.1 < qty.0 || weight < 1 {
                return Err(bad());
            }
            t.yields
                .entry((size, race.parse().map_err(|_| bad())?))
                .or_default()
                .push(Yield {
                    type_id: type_id.parse().map_err(|_| bad())?,
                    weight,
                    qty,
                    value: value.parse().map_err(|_| bad())?,
                });
        }
        Ok(t)
    }

    /// Y-WH: add `other`'s wrecks and yield rows (the Sleeper wrecks,
    /// `wormhole_sites::salvage_overlay`) to this table's; a wreck or a
    /// `(size, race)` the table already has keeps its own rows.
    pub fn merge(&mut self, other: Table) {
        for (w, c) in other.wrecks {
            self.wrecks.entry(w).or_insert(c);
        }
        for (k, rows) in other.yields {
            self.yields.entry(k).or_insert(rows);
        }
    }

    /// A wreck type's `(size, race)`; [`DEFAULT_SIZE`] and any race when the
    /// table does not know it.
    pub fn class_of(&self, wreck_type: i64) -> (u8, i64) {
        self.wrecks
            .get(&wreck_type)
            .copied()
            .unwrap_or((DEFAULT_SIZE, 0))
    }

    /// The rows a `(size, race)` draws from: its own, else the race-0 rows.
    pub fn yields_for(&self, size: u8, race: i64) -> &[Yield] {
        self.yields
            .get(&(size, race))
            .or_else(|| self.yields.get(&(size, 0)))
            .map_or(&[], Vec::as_slice)
    }
}

// ---------------------------------------------------------------------------
// The pure parts
// ---------------------------------------------------------------------------

/// The chance a cycle salvages, as a fraction: [`BASE_CHANCE_PCT`] plus
/// `module_pct` (the module's 902) plus `level` times `per_level_pct` (the
/// skill's 1772), kept inside [`MIN_CHANCE`], [`MAX_CHANCE`].
pub fn chance(module_pct: f64, level: i64, per_level_pct: f64) -> f64 {
    let pct = BASE_CHANCE_PCT + module_pct + level.clamp(0, 5) as f64 * per_level_pct;
    if pct.is_nan() {
        return MIN_CHANCE;
    }
    (pct / 100.0).clamp(MIN_CHANCE, MAX_CHANCE)
}

/// How many draws a success of a wreck of `size` makes.
pub fn draws(size: u8, s: &mut Stream) -> u64 {
    let (lo, hi) = DRAWS[usize::from(size.clamp(1, 3)) - 1];
    s.range(lo, hi + 1)
}

/// The most draws any size makes.
pub fn most_draws(size: u8) -> u64 {
    DRAWS[usize::from(size.clamp(1, 3)) - 1].1
}

/// `n` draws from `rows`, each by weight and a quantity in its range,
/// merged by type: `(type, quantity)` ascending by type. `mintable` drops
/// the types the store cannot make before any draw.
pub fn roll(
    rows: &[Yield],
    n: u64,
    s: &mut Stream,
    mintable: impl Fn(i64) -> bool,
) -> Vec<(i64, i64)> {
    let rows: Vec<&Yield> = rows.iter().filter(|r| mintable(r.type_id)).collect();
    let total: u64 = rows.iter().map(|r| u64::from(r.weight)).sum();
    let mut got: BTreeMap<i64, i64> = BTreeMap::new();
    if total == 0 {
        return Vec::new();
    }
    for _ in 0..n {
        let mut at = s.range(0, total);
        let Some(row) = rows.iter().find(|r| {
            let w = u64::from(r.weight);
            if at < w {
                true
            } else {
                at -= w;
                false
            }
        }) else {
            continue;
        };
        let q = s.range(row.qty.0 as u64, row.qty.1 as u64 + 1) as i64;
        *got.entry(row.type_id).or_default() += q;
    }
    got.into_iter().collect()
}

/// The cargo a salvage could fill at most: `max_draws` draws of the
/// roomiest row, in m3 (`volume_of` a type's unit volume). The room the
/// client's "ideal circumstances" asks for.
pub fn worst_case_m3(
    rows: &[Yield],
    max_draws: u64,
    volume_of: impl Fn(i64) -> Option<f64>,
) -> f64 {
    let one = rows
        .iter()
        .filter_map(|r| volume_of(r.type_id).map(|v| v * r.qty.1 as f64))
        .fold(0.0, f64::max);
    one * max_draws as f64
}

/// How far a tractor beam moves a wreck this cycle: `velocity x seconds`,
/// never past `stop` from the ship. `gap` is the centre-to-centre distance
/// now, `stop` the centre-to-centre distance to stop at.
pub fn tractor_step(velocity: f64, seconds: f64, gap: f64, stop: f64) -> f64 {
    if !(velocity > 0.0 && seconds > 0.0 && gap.is_finite()) {
        return 0.0;
    }
    (velocity * seconds).min((gap - stop).max(0.0))
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

/// Is `id` a wreck a pilot in `system` may lock? (`targeting::lock`.)
pub fn is_lockable(w: &Registry, id: i64, system: i64) -> bool {
    // An anchorable unit is a target for weapons, not for a salvager
    // (`crate::anchor::is_lockable`).
    !crate::anchor::is_unit(w, id)
        && w.combat()
            .wreck(id)
            .is_some_and(|wr| wr.solar_system_id == system)
}

/// A wreck's `(system, position)`, for the lock sweep and the lock bar.
pub fn place(w: &Registry, id: i64) -> Option<(i64, [f64; 3])> {
    w.combat()
        .wreck(id)
        .map(|wr| (wr.solar_system_id, wr.position))
}

/// Is `effect_id` one of this module's?
pub fn is_salvage_effect(effect_id: i64) -> bool {
    effect_id == EFFECT_SALVAGING || effect_id == EFFECT_TRACTOR_BEAM
}

fn module_error(key: &str, args: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr(key),
        PyValue::Dict(crate::pyutil::pypairs(args)),
    ])
}

fn type_error(key: &str, wreck_type: i64) -> PyValue {
    module_error(key, vec![("type", modules::type_arg(wreck_type))])
}

/// The surface-to-surface gap from a ship at `ship` to a wreck at `wreck`.
fn gap_between(
    content: &crate::content::Content,
    ship_type: i64,
    ship: [f64; 3],
    wreck: [f64; 3],
) -> f64 {
    let hull = crate::destiny::hull::of_in(content, ship_type);
    crate::space_state::distance(ship, wreck)
        - f64::from(hull.radius)
        - f64::from(crate::destiny::WRECK_RADIUS)
}

/// A fitted module's value for `attribute`: the sheet's, else the type's.
fn module_value(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    attribute: i64,
) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, attribute)
        .filter(|v| *v != 0.0)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, attribute))
        .or_else(|| modules::find_in(w.content(), type_id).and_then(|t| t.attribute(attribute)))
}

/// `character`'s trained level of Salvaging.
fn salvaging_level(w: &Registry, character_id: i64) -> i64 {
    w.skills_for(Some(character_id))
        .and_then(|s| s.get(SKILL_SALVAGING))
        .map_or(0, |e| e.level.clamp(0, 5))
}

/// The chance this module has for this pilot.
pub fn chance_of(
    w: &Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
) -> f64 {
    let module_pct =
        module_value(w, ship_id, module_id, type_id, ATTR_ACCESS_DIFFICULTY_BONUS).unwrap_or(0.0);
    let per_level =
        crate::typedata::attribute_in(w.content(), SKILL_SALVAGING, ATTR_SKILL_PER_LEVEL)
            .unwrap_or(SKILL_PCT_FALLBACK);
    chance(module_pct, salvaging_level(w, character_id), per_level)
}

/// What `modules::activate` asks before a salvager or a tractor beam
/// starts. The refusals are CCP's keys; which key for which case is **[A]**
/// where no client code raises it:
///
/// - not a wreck in this system -> `DeniedActivateTargetModuleDisallowed`;
/// - beyond `maxRange` -> `TargetTooFar`;
/// - (salvager) loot still in the wreck -> `SalvagingPointlessLoot` [V text];
/// - (salvager) less free cargo than the worst-case yield ->
///   `SalvagingPointlessOverload` [V text], naming the m3.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    ship_type_id: i64,
    module: &modules::ModuleType,
    module_id: i64,
    target_id: Option<i64>,
) -> Result<(), ServiceError> {
    let disallowed = || {
        modules::user_error(
            "DeniedActivateTargetModuleDisallowed",
            vec![("moduleTypeID", modules::type_arg(module.type_id))],
        )
    };
    let here = w.roster().get(character_id).copied();
    let wreck = target_id
        .and_then(|t| w.combat().wreck(t).copied())
        // An anchorable unit is not salvage (NW-2): it is shot, not worked.
        .filter(|wr| !crate::anchor::is_unit(w, wr.wreck_id))
        .filter(|wr| here.is_some_and(|p| p.solar_system_id == wr.solar_system_id));
    let Some(wreck) = wreck else {
        return Err(disallowed());
    };
    w.advance_character(character_id);
    let position = w
        .roster()
        .get(character_id)
        .map_or([0.0; 3], |p| p.motion.position);
    let gap = gap_between(w.content(), ship_type_id, position, wreck.position);
    let range = module_value(w, ship_id, module_id, module.type_id, ATTR_MAX_RANGE).unwrap_or(0.0);
    if gap > range {
        let name = crate::item::find_in(w.content(), wreck.type_id).map_or("wreck", |t| t.name);
        return Err(modules::user_error(
            "TargetTooFar",
            crate::module_range::too_far_pairs(module.type_id, gap, name),
        ));
    }
    if module
        .effect
        .is_none_or(|e| e.effect_id != EFFECT_SALVAGING)
    {
        return Ok(());
    }
    if w.store().contents_of(wreck.wreck_id).next().is_some() {
        return Err(modules::user_error(
            "SalvagingPointlessLoot",
            vec![("type", modules::type_arg(wreck.type_id))],
        ));
    }
    let (size, race) = salvage_table(w).class_of(wreck.type_id);
    let need = worst_case_m3(
        salvage_table(w).yields_for(size, race),
        most_draws(size),
        |t| crate::item::find_in(w.content(), t).map(|i| i.volume),
    );
    let free = crate::mining::cargo_free(w, ship_id, ship_type_id);
    if free < need {
        return Err(modules::user_error(
            "SalvagingPointlessOverload",
            vec![
                ("units", PyValue::Real(need - free)),
                ("type", modules::type_arg(wreck.type_id)),
            ],
        ));
    }
    Ok(())
}

fn salvage_table(w: &Registry) -> &Table {
    static EMPTY: Table = Table {
        wrecks: BTreeMap::new(),
        yields: BTreeMap::new(),
    };
    w.content().try_get::<Table>().unwrap_or(&EMPTY)
}

/// **The cycle-completion hook** (`mining::tick` hands every completed
/// cycle here beside hacking's): a salvager's or a tractor beam's ended
/// cycle acts; any other cycle is not this module's.
pub fn on_completed(w: &mut Registry, done: &[Cycle]) {
    for c in done {
        match c.effect_id {
            EFFECT_SALVAGING => salvage_cycle(w, c),
            EFFECT_TRACTOR_BEAM => tractor_cycle(w, c),
            _ => {}
        }
    }
}

/// Run `f` and send what it put on `pending` from the clock instead
/// (`mining::on_clock`'s trade).
fn on_clock<R>(w: &mut Registry, f: impl FnOnce(&mut Registry) -> R) -> R {
    let before = w.take_pending();
    let out = f(w);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        w.schedule(Duration::ZERO, Addressed { to: None, inner: p });
    }
    out
}

/// One ended salvager cycle.
fn salvage_cycle(w: &mut Registry, c: &Cycle) {
    let Some(wreck_id) = c.target_id else { return };
    let Some(here) = w.roster().get(c.character_id).copied() else {
        return;
    };
    if here.ship_id != c.ship_id {
        return;
    }
    let Some(wreck) = w.combat().wreck(wreck_id).copied() else {
        // Salvaged by someone else in this batch, or gone with its clock.
        modules::stop(w, c.module_id, c.at, None);
        return;
    };
    if wreck.solar_system_id != here.solar_system_id {
        return;
    }
    let Some(ship_type_id) = w.character(c.character_id).map(|ch| ch.ship_type_id) else {
        return;
    };
    w.advance_character(c.character_id);
    let position = w
        .roster()
        .get(c.character_id)
        .map_or(here.motion.position, |p| p.motion.position);
    let gap = gap_between(w.content(), ship_type_id, position, wreck.position);
    let range = module_value(w, c.ship_id, c.module_id, c.type_id, ATTR_MAX_RANGE).unwrap_or(0.0);
    if gap > range {
        modules::stop(
            w,
            c.module_id,
            c.at,
            Some(module_error(
                "TargetTooFar",
                crate::module_range::too_far_pairs(
                    c.type_id,
                    gap,
                    crate::item::find_in(w.content(), wreck.type_id).map_or("wreck", |t| t.name),
                ),
            )),
        );
        return;
    }
    if w.store().contents_of(wreck_id).next().is_some() {
        modules::stop(
            w,
            c.module_id,
            c.at,
            Some(type_error("SalvagingPointlessLoot", wreck.type_id)),
        );
        return;
    }
    let mut s = seed::stream(
        w.world().seed,
        seed::Kind::Loot,
        &[STREAM_TAG, wreck_id as u64, c.module_id as u64, c.at as u64],
    );
    let p = chance_of(w, c.character_id, c.ship_id, c.module_id, c.type_id);
    if !s.chance(p) {
        modules::stop(
            w,
            c.module_id,
            c.at,
            Some(module_error("SalvagingFailure", vec![])),
        );
        return;
    }
    let (size, race) = salvage_table(w).class_of(wreck.type_id);
    let n = draws(size, &mut s);
    let stacks = {
        let content = w.content();
        roll(salvage_table(w).yields_for(size, race), n, &mut s, |t| {
            crate::item::find_in(content, t).is_some()
        })
    };
    let (kept, lost) = into_cargo(w, c.character_id, c.ship_id, ship_type_id, &stacks);
    // Theft before the wreck goes: its owner is on the record.
    if !crate::combat::has_loot_right(w, c.character_id, &wreck) {
        crate::crimewatch::theft_from(
            w,
            c.character_id,
            Some(wreck.owner_id),
            wreck.solar_system_id,
            crate::pyutil::filetime_now(),
        );
    }
    crate::outln!(
        "[salvage] {} salvaged wreck {wreck_id}: {kept} units kept, {lost} lost",
        c.character_id
    );
    remove(w, wreck_id, c.at, Some(c.module_id));
    let key = if stacks.is_empty() {
        "SalvagingSuccessHollow"
    } else if lost > 0 {
        "SalvagingSuccessOverloaded"
    } else {
        "SalvagingSuccess"
    };
    modules::stop(w, c.module_id, c.at, Some(type_error(key, wreck.type_id)));
}

/// Put `stacks` in the cargo hold, as much as it takes: `(kept, lost)` in
/// units. A stack is cut to the room that is left, whole units, in the
/// order of `stacks`.
fn into_cargo(
    w: &mut Registry,
    owner: i64,
    ship_id: i64,
    ship_type_id: i64,
    stacks: &[(i64, i64)],
) -> (i64, i64) {
    let mut free = crate::mining::cargo_free(w, ship_id, ship_type_id);
    let slot = crate::item::Slot::new(owner, ship_id, crate::fitting::FLAG_CARGO);
    let (mut kept, mut lost) = (0, 0);
    let mut edits = Vec::new();
    for &(type_id, quantity) in stacks {
        let volume = crate::item::find_in(w.content(), type_id).map_or(0.0, |t| t.volume);
        let fits = if volume > 0.0 {
            crate::mining::units_in(free, volume).min(quantity)
        } else {
            quantity
        };
        lost += quantity - fits;
        if fits < 1 {
            continue;
        }
        free -= volume * fits as f64;
        kept += fits;
        let stack = w
            .store()
            .at(slot)
            .filter(|i| i.type_id == type_id && !i.singleton)
            .map(|i| i.item_id)
            .min();
        edits.push(match stack {
            Some(item_id) => crate::item::Edit::Grow {
                item_id,
                quantity: fits,
            },
            None => crate::item::Edit::Create {
                type_id,
                quantity: fits,
                to: slot,
            },
        });
    }
    if edits.is_empty() {
        return (0, lost + kept);
    }
    if let Err(e) = on_clock(w, |w| w.apply_items(&edits)) {
        crate::outln!("[salvage] the store refused the salvage: {e}");
        return (0, lost + kept);
    }
    (kept, lost)
}

/// A wreck gone: stop every salvager and beam still on it (but `except`),
/// drop it from every park, release every lock on it.
fn remove(w: &mut Registry, wreck_id: i64, at: i64, except: Option<i64>) {
    let type_id = w.combat().wreck(wreck_id).map_or(0, |wr| wr.type_id);
    for r in w.modules().running_against(wreck_id) {
        if is_salvage_effect(r.effect.effect_id) && Some(r.module_id) != except {
            modules::stop(
                w,
                r.module_id,
                at,
                Some(type_error("SalvagingTooLate", type_id)),
            );
        }
    }
    crate::combat::remove_wreck(w, wreck_id);
    for locker_ship in w.locks().targeters_of(wreck_id) {
        let Some(locker) = w.roster().by_ship(locker_ship).map(|p| p.character_id) else {
            w.locks_mut().remove(locker_ship, wreck_id);
            continue;
        };
        on_clock(w, |w| {
            crate::targeting::unlock(w, locker, wreck_id, crate::targeting::Reason::Exploding)
        });
    }
}

/// One ended tractor-beam cycle: the wreck moves toward the ship.
fn tractor_cycle(w: &mut Registry, c: &Cycle) {
    let Some(wreck_id) = c.target_id else { return };
    let Some(here) = w.roster().get(c.character_id).copied() else {
        return;
    };
    if here.ship_id != c.ship_id {
        return;
    }
    let Some(wreck) = w.combat().wreck(wreck_id).copied() else {
        modules::stop(w, c.module_id, c.at, None);
        return;
    };
    if wreck.solar_system_id != here.solar_system_id {
        return;
    }
    let ship_type_id = w.character(c.character_id).map_or(0, |ch| ch.ship_type_id);
    w.advance_character(c.character_id);
    let ship = w
        .roster()
        .get(c.character_id)
        .map_or(here.motion.position, |p| p.motion.position);
    let hull = crate::destiny::hull::of_in(w.content(), ship_type_id);
    let stop = f64::from(hull.radius) + f64::from(crate::destiny::WRECK_RADIUS) + TRACTOR_STOP_GAP;
    let dist = crate::space_state::distance(ship, wreck.position);
    let velocity = module_value(
        w,
        c.ship_id,
        c.module_id,
        c.type_id,
        ATTR_MAX_TRACTOR_VELOCITY,
    )
    .unwrap_or(0.0);
    let seconds =
        modules::find_in(w.content(), c.type_id).map_or(0.0, |t| t.duration_ms() as f64 / 1000.0);
    let step = tractor_step(velocity, seconds, dist, stop);
    if step <= 0.0 {
        // Already home (or a beam with no speed): nothing left to pull.
        modules::stop(w, c.module_id, c.at, None);
        return;
    }
    let Some(dir) = crate::space_state::unit([
        ship[0] - wreck.position[0],
        ship[1] - wreck.position[1],
        ship[2] - wreck.position[2],
    ]) else {
        return;
    };
    let point = [
        wreck.position[0] + dir[0] * step,
        wreck.position[1] + dir[1] * step,
        wreck.position[2] + dir[2] * step,
    ];
    if !crate::combat::set_wreck_position(w, wreck_id, point) {
        return;
    }
    let mut to = vec![c.character_id];
    to.extend(
        w.roster()
            .others_in_system(here.solar_system_id, c.character_id),
    );
    to.retain(|id| w.character(*id).is_some());
    let stamp = w.next_destiny_stamp();
    if let Some(u) = crate::destiny::ActionUpdate::new(
        to,
        stamp,
        vec![crate::destiny::Action::SetBallPosition {
            ball: wreck_id,
            point,
        }],
    ) {
        w.schedule(Duration::ZERO, u);
    }
    if step >= dist - stop {
        // Home: the beam has done its work.
        modules::stop(w, c.module_id, c.at, None);
    }
}

// ---------------------------------------------------------------------------
// Tests: invented ids and rows; no [A] value appears in one.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids (the 99xxxx band).
    const SALV_A: i64 = 990_501;
    const SALV_B: i64 = 990_502;
    const SALV_C: i64 = 990_503;
    const GHOST: i64 = 990_599;
    const WRECK_SMALL: i64 = 990_511;
    const WRECK_BIG: i64 = 990_512;
    const WRECK_ODD: i64 = 990_513;
    const RACE_X: i64 = 990_521;
    const RACE_Y: i64 = 990_522;

    fn rows() -> Vec<Yield> {
        vec![
            Yield {
                type_id: SALV_A,
                weight: 10,
                qty: (1, 4),
                value: 1.0,
            },
            Yield {
                type_id: SALV_B,
                weight: 30,
                qty: (2, 2),
                value: 2.0,
            },
            Yield {
                type_id: GHOST,
                weight: 1000,
                qty: (1, 1),
                value: 3.0,
            },
        ]
    }

    fn table() -> Table {
        Table::parse(
            &[
                format!("{WRECK_SMALL}\t1\t{RACE_X}"),
                format!("{WRECK_BIG}\t3\t0"),
            ],
            &[
                format!("1\t0\t{SALV_A}\t5\t1\t2\t1.0"),
                format!("1\t{RACE_X}\t{SALV_B}\t7\t1\t3\t2.0"),
                format!("3\t0\t{SALV_C}\t9\t4\t5\t3.0"),
            ],
        )
        .expect("the test rows parse")
    }

    // --- the table -----------------------------------------------------

    #[test]
    fn the_table_reads_both_files() {
        let t = table();
        assert_eq!(t.class_of(WRECK_SMALL), (1, RACE_X));
        assert_eq!(t.class_of(WRECK_BIG), (3, 0));
        assert_eq!(t.yields_for(3, 0).len(), 1);
        assert_eq!(t.yields_for(1, RACE_X)[0].type_id, SALV_B);
        assert_eq!(t.yields_for(1, RACE_X)[0].qty, (1, 3));
    }

    #[test]
    fn an_unknown_wreck_is_the_default_size_and_a_race_without_rows_falls_back_to_any() {
        let t = table();
        assert_eq!(t.class_of(WRECK_ODD), (DEFAULT_SIZE, 0));
        assert_eq!(t.yields_for(1, RACE_Y)[0].type_id, SALV_A);
        assert!(
            t.yields_for(2, 0).is_empty(),
            "no rows for a size is hollow"
        );
        assert!(Table::default().yields_for(1, 0).is_empty());
    }

    #[test]
    fn a_bad_row_is_an_error_naming_its_line() {
        let ok = format!("1\t0\t{SALV_A}\t5\t1\t2\t1.0");
        for bad in [
            "9\t0\t1\t5\t1\t2\t1.0",
            "1\t0\t1\t0\t1\t2\t1.0",
            "1\t0\t1\t5\t0\t2\t1.0",
            "1\t0\t1\t5\t3\t2\t1.0",
            "1\t0\t1\t5\t1\t2",
            "1\t0\tx\t5\t1\t2\t1.0",
        ] {
            let e = Table::parse::<&str>(&[], &[ok.as_str(), bad]).expect_err(bad);
            assert!(e.contains("data line 2"), "{e}");
        }
        for bad in ["1\t2", "7\t9\t0", "x\t1\t0"] {
            assert!(Table::parse::<&str>(&[bad], &[]).is_err(), "{bad}");
        }
    }

    // --- the chance ------------------------------------------------------

    #[test]
    fn the_chance_grows_with_the_module_and_the_skill_and_stays_inside_its_bounds() {
        let per = 2.0;
        assert!(chance(5.0, 0, per) > chance(0.0, 0, per));
        assert!(chance(0.0, 3, per) > chance(0.0, 1, per));
        assert_eq!(chance(0.0, 0, per), BASE_CHANCE_PCT / 100.0);
        assert_eq!(chance(1.0e6, 5, per), MAX_CHANCE);
        assert_eq!(chance(-1.0e6, 0, per), MIN_CHANCE);
        assert_eq!(chance(f64::NAN, 0, per), MIN_CHANCE);
        assert_eq!(chance(0.0, 99, per), chance(0.0, 5, per), "levels cap at V");
        assert_eq!(chance(0.0, -3, per), chance(0.0, 0, per));
    }

    // --- the roll ------------------------------------------------------------

    #[test]
    fn a_roll_draws_only_mintable_rows_and_merges_by_type() {
        let mut s = Stream::new(7);
        let got = roll(&rows(), 200, &mut s, |t| t != GHOST);
        assert!(got.iter().all(|(t, _)| *t == SALV_A || *t == SALV_B));
        assert_eq!(got.len(), 2, "both rows appear in 200 draws");
        let total: i64 = got.iter().map(|(_, q)| q).sum();
        assert!((200..=200 * 4).contains(&total));
        let a = got.iter().find(|(t, _)| *t == SALV_A).unwrap().1;
        let b = got.iter().find(|(t, _)| *t == SALV_B).unwrap().1;
        assert!(b > a, "the heavier row is drawn more: {a} {b}");
        assert!(got.windows(2).all(|w| w[0].0 < w[1].0), "ascending by type");
    }

    #[test]
    fn a_roll_respects_each_rows_quantity_range() {
        let mut s = Stream::new(11);
        let one = [Yield {
            type_id: SALV_A,
            weight: 1,
            qty: (3, 5),
            value: 0.0,
        }];
        for _ in 0..50 {
            let got = roll(&one, 1, &mut s, |_| true);
            assert!((3..=5).contains(&got[0].1), "{got:?}");
        }
    }

    #[test]
    fn nothing_to_draw_is_hollow() {
        let mut s = Stream::new(1);
        assert!(roll(&[], 5, &mut s, |_| true).is_empty());
        assert!(roll(&rows(), 5, &mut s, |_| false).is_empty());
        assert!(roll(&rows(), 0, &mut s, |_| true).is_empty());
    }

    #[test]
    fn a_roll_is_a_function_of_its_stream() {
        let a = roll(&rows(), 9, &mut Stream::new(3), |_| true);
        let b = roll(&rows(), 9, &mut Stream::new(3), |_| true);
        assert_eq!(a, b);
    }

    #[test]
    fn draws_stay_inside_the_sizes_range() {
        let mut s = Stream::new(5);
        for size in 1..=3u8 {
            let (lo, hi) = DRAWS[usize::from(size) - 1];
            let seen: Vec<u64> = (0..200).map(|_| draws(size, &mut s)).collect();
            assert!(
                seen.iter().all(|n| (lo..=hi).contains(n)),
                "{size}: {seen:?}"
            );
            assert_eq!(most_draws(size), hi);
        }
        // Out-of-range sizes clamp rather than index out of the table.
        assert_eq!(most_draws(0), DRAWS[0].1);
        assert_eq!(most_draws(9), DRAWS[2].1);
    }

    #[test]
    fn the_worst_case_is_the_roomiest_row_at_most_draws() {
        let vol = |t: i64| match t {
            SALV_A => Some(0.5),
            SALV_B => Some(0.1),
            _ => None,
        };
        // A: 4 x 0.5 = 2.0; B: 2 x 0.1; GHOST has no volume.
        assert_eq!(worst_case_m3(&rows(), 3, vol), 6.0);
        assert_eq!(worst_case_m3(&[], 3, vol), 0.0);
    }

    // --- the tractor -----------------------------------------------------------

    #[test]
    fn a_tractor_step_is_speed_times_time_and_never_passes_the_stop() {
        assert_eq!(tractor_step(100.0, 5.0, 10_000.0, 150.0), 500.0);
        assert_eq!(tractor_step(100.0, 5.0, 400.0, 150.0), 250.0);
        assert_eq!(
            tractor_step(100.0, 5.0, 100.0, 150.0),
            0.0,
            "already inside"
        );
        assert_eq!(tractor_step(0.0, 5.0, 10_000.0, 150.0), 0.0);
        assert_eq!(tractor_step(100.0, 0.0, 10_000.0, 150.0), 0.0);
        assert_eq!(tractor_step(100.0, 5.0, f64::NAN, 150.0), 0.0);
    }

    // --- the world: a ship, a wreck, a salvager ---------------------------------

    const ALICE: i64 = 1_000_001;
    const MERLIN: i64 = 100_000_070;
    const MODULE: i64 = 100_000_071;
    const HI_SLOT_0: i64 = 27;
    const SALVAGER_TYPE: i64 = 990_531;
    const WEAK_SALVAGER_TYPE: i64 = 990_532;
    const BEAM_TYPE: i64 = 990_533;
    /// The store's own first wreck type (a hand row, protocol not data): an
    /// invented one would be a type the store cannot create.
    const WRECK_TYPE: i64 = crate::item::WRECK_TYPES[0].type_id;
    /// Not in the NPC band, so the wreck is a player's and salvaging it is
    /// theft.
    const PLAYER_OWNER: i64 = 90_000_555;
    /// Blue ticks; any fixed instant.
    const T0: i64 = 133_000_000_000_000_000;
    /// The invented salvagers' own numbers.
    const SALVAGER_RANGE: f64 = 5_000.0;
    const SALVAGER_CYCLE_MS: i64 = 10_000;
    const BEAM_RANGE: f64 = 24_000.0;
    const BEAM_VELOCITY: f64 = 600.0;
    const BEAM_CYCLE_MS: i64 = 5_000;

    fn world_content(table: Table) -> crate::content::Content {
        let rows = format!(
            "type\t{SALV_A}\t{GROUP_SALVAGE}\t17\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Salvage A\n\
             type\t{SALV_B}\t{GROUP_SALVAGE}\t17\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Salvage B\n\
             type\t{SALV_C}\t{GROUP_SALVAGE}\t17\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Salvage C\n\
             type\t{SALVAGER_TYPE}\t1122\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_SALVAGING}\t12,16,{EFFECT_SALVAGING}\t6=0.0;54={SALVAGER_RANGE};73={SALVAGER_CYCLE_MS}.0;902=100.0\tFixture Salvager\n\
             type\t{WEAK_SALVAGER_TYPE}\t1122\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_SALVAGING}\t12,16,{EFFECT_SALVAGING}\t6=0.0;54={SALVAGER_RANGE};73={SALVAGER_CYCLE_MS}.0;902=-100.0\tFixture Weak Salvager\n\
             type\t{BEAM_TYPE}\t650\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{EFFECT_TRACTOR_BEAM}\t12,16,{EFFECT_TRACTOR_BEAM}\t6=0.0;54={BEAM_RANGE};73={BEAM_CYCLE_MS}.0;1045={BEAM_VELOCITY}\tFixture Tractor Beam\n\
             effect\t{EFFECT_SALVAGING}\t2\t0\t73\t6\t54\t0\t0\tsalvaging\teffects.Salvaging\n\
             effect\t{EFFECT_TRACTOR_BEAM}\t2\t0\t73\t6\t54\t0\t0\ttractorBeamCan\teffects.TractorBeam\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let td = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(td));
        c.insert("salvage", table);
        c
    }

    fn world_table() -> Table {
        Table::parse(
            &[format!("{WRECK_TYPE}\t1\t0")],
            &[
                format!("1\t0\t{SALV_A}\t3\t1\t3\t1.0"),
                format!("1\t0\t{SALV_B}\t1\t2\t2\t2.0"),
            ],
        )
        .expect("the world's rows parse")
    }

    struct World {
        reg: Registry,
        wreck: i64,
        system: i64,
    }

    fn park(reg: &mut Registry, who: i64, at: [f64; 3]) {
        let p = reg.roster_mut().get_mut(who).expect("in space");
        p.motion = crate::space_state::Motion::arrived(at, [0.0; 3]);
        p.since = crate::space_state::now_seconds();
    }

    /// Alice in a Merlin with `module_type` fitted, 2 km from a wreck of
    /// [`WRECK_TYPE`] owned by `owner`; the wreck locked.
    fn world_with(table: Table, module_type: i64, owner: i64) -> World {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(world_content(table)));
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let hull = *crate::item::find_in(reg.content(), 603).expect("the hull");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: MERLIN,
            type_id: 603,
            owner_id: ALICE,
            location_id: station,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: hull.group_id,
            category_id: hull.category_id,
            singleton: true,
        });
        let t = crate::item::find_in(reg.content(), module_type).expect("the module type");
        items.push(crate::item::Item {
            item_id: MODULE,
            type_id: module_type,
            owner_id: ALICE,
            location_id: MERLIN,
            flag_id: HI_SLOT_0,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: MERLIN + 1_000,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = MERLIN;
        c.ship_type_id = 603;
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let here = reg.roster().get(ALICE).copied().expect("in space");
        let system = here.solar_system_id;
        let wreck = new_wreck(&mut reg, system, owner, [2_000.0, 0.0, 0.0]);
        // The wreck is on Alice's grid, as a kill would have put it.
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        park(&mut reg, ALICE, [0.0; 3]);
        reg.locks_mut().add(MERLIN, wreck);
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        World { reg, wreck, system }
    }

    fn world() -> World {
        world_with(world_table(), SALVAGER_TYPE, PLAYER_OWNER)
    }

    /// A wreck item in `system` and its record at `position`.
    fn new_wreck(reg: &mut Registry, system: i64, owner: i64, position: [f64; 3]) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: WRECK_TYPE,
                quantity: 1,
                to: crate::item::Slot::new(owner, system, crate::combat::FLAG_IN_SPACE),
            }])
            .expect("the wreck item");
        let wreck_id = changes
            .iter()
            .find_map(|ch| match ch {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("created");
        crate::combat::insert_wreck(
            reg,
            crate::combat::Wreck {
                wreck_id,
                type_id: WRECK_TYPE,
                solar_system_id: system,
                position,
                ship_id: 0,
                owner_id: owner,
                corp_id: owner,
                fleet_id: None,
                abandoned: false,
                expires_at: crate::damage::BlueTime::MAX,
            },
        );
        wreck_id
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    fn period() -> i64 {
        SALVAGER_CYCLE_MS * modules::MSEC
    }

    fn activate(w: &mut World) -> Result<PyValue, ServiceError> {
        modules::activate(
            &mut w.reg,
            Some(ALICE),
            MODULE,
            "salvaging",
            Some(w.wreck),
            1000,
            T0,
        )
    }

    /// The world tick, as `push::spawn_world_ticker` runs it, at `at`.
    fn clock(reg: &mut Registry, at: i64) {
        modules::tick(reg, at);
        crate::combat::tick(reg, at);
        crate::mining::tick(reg, at);
    }

    fn cargo(reg: &Registry) -> Vec<crate::item::Item> {
        reg.store()
            .at(crate::item::Slot::new(
                ALICE,
                MERLIN,
                crate::fitting::FLAG_CARGO,
            ))
            .cloned()
            .collect()
    }

    fn salvage_in_cargo(reg: &Registry) -> i64 {
        cargo(reg)
            .iter()
            .filter(|i| [SALV_A, SALV_B, SALV_C].contains(&i.type_id))
            .map(|i| i.quantity)
            .sum()
    }

    /// Everything sent since the last call, as debug text with its event
    /// name: enough to ask "did this key go out".
    fn sent_text(reg: &mut Registry) -> Vec<String> {
        reg.take_due(far())
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                readable(&format!("{name} {args:?}"))
            })
            .collect()
    }

    fn has_key(sent: &[String], key: &str) -> bool {
        sent.iter().any(|s| s.contains(key))
    }

    /// `Debug` text with every `Bytes([..])` shown as its string, so a key
    /// can be looked for in an error or a push.
    fn readable(debug: &str) -> String {
        let mut out = String::new();
        let mut rest = debug;
        while let Some(at) = rest.find("Bytes([") {
            out.push_str(&rest[..at]);
            let tail = &rest[at + "Bytes([".len()..];
            let end = tail.find("])").unwrap_or(tail.len());
            let bytes: Vec<u8> = tail[..end]
                .split(',')
                .filter_map(|n| n.trim().parse().ok())
                .collect();
            out.push_str(&String::from_utf8_lossy(&bytes));
            rest = &tail[(end + 2).min(tail.len())..];
        }
        out.push_str(rest);
        out
    }

    fn refused(e: &ServiceError) -> String {
        readable(&format!("{e:?}"))
    }

    // --- activation -------------------------------------------------------------

    #[test]
    fn a_salvager_starts_on_a_locked_empty_wreck_in_range() {
        let mut w = world();
        activate(&mut w).expect("the salvager starts");
        assert!(w.reg.modules().running(MODULE).is_some());
    }

    #[test]
    fn a_salvager_refuses_what_is_not_a_wreck() {
        let mut w = world();
        let other = w.wreck + 5_000;
        w.reg.locks_mut().add(MERLIN, other);
        let e = modules::activate(
            &mut w.reg,
            Some(ALICE),
            MODULE,
            "salvaging",
            Some(other),
            1000,
            T0,
        )
        .expect_err("not a wreck");
        assert!(
            refused(&e).contains("DeniedActivateTargetModuleDisallowed"),
            "{e:?}"
        );
        assert!(w.reg.modules().running(MODULE).is_none());
    }

    #[test]
    fn a_salvager_refuses_an_anchorable_unit() {
        let mut w = world();
        assert!(activate(&mut w).is_ok(), "a wreck takes it");
        let mut w = world();
        // The wreck record is also an anchorable unit's (NW-2): it is shot,
        // never salvaged.
        let key = vec![w.wreck.to_string()];
        let row = vec!["free".to_owned(), "0".to_owned(), "0.0".to_owned()];
        w.reg.deltas_mut().put(crate::anchor::KIND, key, row);
        let e = activate(&mut w).expect_err("a unit is not salvage");
        assert!(
            refused(&e).contains("DeniedActivateTargetModuleDisallowed"),
            "{e:?}"
        );
        assert!(w.reg.modules().running(MODULE).is_none());
    }

    #[test]
    fn a_salvager_refuses_a_wreck_out_of_range() {
        let mut w = world();
        park(&mut w.reg, ALICE, [SALVAGER_RANGE + 50_000.0, 0.0, 0.0]);
        let e = activate(&mut w).expect_err("too far");
        assert!(refused(&e).contains("TargetTooFar"), "{e:?}");
    }

    #[test]
    fn a_salvager_waits_for_the_loot_to_go() {
        let mut w = world();
        w.reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: SALV_A,
                quantity: 1,
                to: crate::item::Slot::new(PLAYER_OWNER, w.wreck, crate::combat::FLAG_IN_WRECK),
            }])
            .expect("loot in the wreck");
        let e = activate(&mut w).expect_err("loot first");
        assert!(refused(&e).contains("SalvagingPointlessLoot"), "{e:?}");
    }

    #[test]
    fn a_salvager_wants_room_for_the_worst_case() {
        let mut w = world();
        let capacity = crate::item::find_in(w.reg.content(), 603)
            .expect("hull")
            .capacity;
        // A cargo of invented ballast: salvage-sized volume to the brim.
        let volume = crate::item::find_in(w.reg.content(), SALV_A)
            .expect("salvage")
            .volume;
        w.reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: SALV_C,
                quantity: crate::mining::units_in(capacity, volume),
                to: crate::item::Slot::new(ALICE, MERLIN, crate::fitting::FLAG_CARGO),
            }])
            .expect("a full hold");
        let e = activate(&mut w).expect_err("no room");
        assert!(refused(&e).contains("SalvagingPointlessOverload"), "{e:?}");
    }

    // --- cycles ---------------------------------------------------------------------

    /// Run `attempts` activations one after the other, each a full cycle
    /// later, until the wreck is gone. Returns the attempts it took.
    fn salvage_until_gone(w: &mut World, attempts: i64) -> Option<i64> {
        for n in 0..attempts {
            let at = T0 + n * (period() + 1_000);
            if w.reg.modules().running(MODULE).is_none() {
                modules::activate(
                    &mut w.reg,
                    Some(ALICE),
                    MODULE,
                    "salvaging",
                    Some(w.wreck),
                    1000,
                    at,
                )
                .expect("starts");
            }
            clock(&mut w.reg, at + period() + 100);
            if w.reg.combat().wreck(w.wreck).is_none() {
                return Some(n + 1);
            }
        }
        None
    }

    #[test]
    fn a_successful_cycle_puts_salvage_in_the_hold_and_the_wreck_goes() {
        let mut w = world();
        let took =
            salvage_until_gone(&mut w, 60).expect("a salvager at the chance's ceiling succeeds");
        assert!(took >= 1);
        assert!(salvage_in_cargo(&w.reg) > 0, "salvage arrived");
        for i in cargo(&w.reg) {
            assert!(
                [SALV_A, SALV_B].contains(&i.type_id),
                "only the table's rows: {i:?}"
            );
        }
        assert!(w.reg.combat().wreck(w.wreck).is_none());
        assert!(
            w.reg.store().get(w.wreck).is_none(),
            "the item is gone with it"
        );
        assert!(
            w.reg.locks().targets_of(MERLIN).is_empty(),
            "the lock was released"
        );
        assert!(
            w.reg.modules().running(MODULE).is_none(),
            "the module stopped"
        );
        let sent = sent_text(&mut w.reg);
        assert!(has_key(&sent, "SalvagingSuccess"), "{sent:?}");
        assert!(
            sent.iter()
                .any(|s| s.starts_with("DoDestinyUpdate") && s.contains("RemoveBalls")),
            "the wreck left every park: {sent:?}"
        );
    }

    #[test]
    fn the_quantities_follow_the_table() {
        let mut w = world();
        salvage_until_gone(&mut w, 60).expect("salvaged");
        let (size, _) = salvage_table(&w.reg).class_of(WRECK_TYPE);
        let most = most_draws(size) as i64;
        let a: i64 = cargo(&w.reg)
            .iter()
            .filter(|i| i.type_id == SALV_A)
            .map(|i| i.quantity)
            .sum();
        let b: i64 = cargo(&w.reg)
            .iter()
            .filter(|i| i.type_id == SALV_B)
            .map(|i| i.quantity)
            .sum();
        // Row A draws 1..=3 a time, row B exactly 2.
        assert!(
            a <= 3 * most && b <= 2 * most,
            "a {a}, b {b}, draws at most {most}"
        );
        assert_eq!(b % 2, 0);
        assert!(a + b >= 1);
    }

    /// A salvager at the chance's floor: over twenty fresh wrecks, each
    /// attempt at its own time, some fail -- and a failure leaves the wreck,
    /// the hold empty and the module stopped with the client's key.
    #[test]
    fn a_failed_cycle_leaves_the_wreck_and_says_so() {
        let mut failures = 0;
        for n in 0..20 {
            let mut w = world_with(world_table(), WEAK_SALVAGER_TYPE, PLAYER_OWNER);
            let at = T0 + n * (period() + 1_000);
            modules::activate(
                &mut w.reg,
                Some(ALICE),
                MODULE,
                "salvaging",
                Some(w.wreck),
                1000,
                at,
            )
            .expect("starts");
            clock(&mut w.reg, at + period() + 100);
            if w.reg.combat().wreck(w.wreck).is_none() {
                continue;
            }
            failures += 1;
            let sent = sent_text(&mut w.reg);
            assert!(has_key(&sent, "SalvagingFailure"), "{sent:?}");
            assert!(w.reg.modules().running(MODULE).is_none());
            assert_eq!(salvage_in_cargo(&w.reg), 0);
            assert!(w.reg.store().get(w.wreck).is_some(), "the wreck item stays");
        }
        assert!(
            failures > 0,
            "a salvager at the floor fails some of twenty tries"
        );
    }

    #[test]
    fn the_same_wreck_and_module_roll_differently_each_attempt() {
        let seen: std::collections::BTreeSet<bool> = (0..64)
            .map(|n| {
                let mut s = seed::stream(
                    5,
                    seed::Kind::Loot,
                    &[STREAM_TAG, 1, 2, (T0 + n * period()) as u64],
                );
                s.chance(MIN_CHANCE.max(0.5))
            })
            .collect();
        assert_eq!(seen.len(), 2, "attempts differ by time");
    }

    #[test]
    fn a_wreck_with_no_rows_is_hollow() {
        let mut w = world_with(
            Table::parse(&[format!("{WRECK_TYPE}\t1\t0")], &[] as &[String]).unwrap(),
            SALVAGER_TYPE,
            PLAYER_OWNER,
        );
        salvage_until_gone(&mut w, 60).expect("salvaged");
        assert_eq!(salvage_in_cargo(&w.reg), 0);
        assert!(has_key(&sent_text(&mut w.reg), "SalvagingSuccessHollow"));
    }

    #[test]
    fn salvaging_a_wreck_of_someone_else_is_theft_and_a_free_wreck_is_not() {
        // The fixture system's flags decide whether theft shows at all, so
        // the test asks the rule rather than the flag: the owner is the
        // pilot's own corporation-less self for the second wreck.
        let mut mine = world_with(world_table(), SALVAGER_TYPE, ALICE);
        let wr = mine.reg.combat().wreck(mine.wreck).copied().unwrap();
        assert!(crate::combat::has_loot_right(&mine.reg, ALICE, &wr));
        let theirs = world();
        let wr = theirs.reg.combat().wreck(theirs.wreck).copied().unwrap();
        assert!(!crate::combat::has_loot_right(&theirs.reg, ALICE, &wr));
        // Either way the salvage itself works.
        salvage_until_gone(&mut mine, 60).expect("own wreck salvaged");
    }

    #[test]
    fn a_second_salvager_on_a_gone_wreck_is_too_late() {
        let mut w = world();
        activate(&mut w).expect("starts");
        // The wreck leaves (someone else salvaged it) mid-cycle.
        remove(&mut w.reg, w.wreck, T0, None);
        let _ = w.reg.take_due(far());
        clock(&mut w.reg, T0 + period() + 100);
        assert!(
            w.reg.modules().running(MODULE).is_none(),
            "the module stopped"
        );
        assert_eq!(salvage_in_cargo(&w.reg), 0);
    }

    #[test]
    fn removing_a_wreck_stops_the_other_salvagers_on_it() {
        let mut w = world();
        activate(&mut w).expect("starts");
        remove(&mut w.reg, w.wreck, T0, None);
        assert!(w.reg.modules().running(MODULE).is_none());
        let sent = sent_text(&mut w.reg);
        assert!(has_key(&sent, "SalvagingTooLate"), "{sent:?}");
    }

    // --- locking ----------------------------------------------------------------------

    #[test]
    fn a_wreck_in_the_system_can_be_locked_and_one_elsewhere_cannot() {
        let mut w = world();
        w.reg.locks_mut().remove(MERLIN, w.wreck);
        assert!(is_lockable(&w.reg, w.wreck, w.system));
        assert!(!is_lockable(&w.reg, w.wreck, w.system + 1));
        assert!(!is_lockable(&w.reg, 1, w.system));
        let answer = crate::targeting::lock(&mut w.reg, ALICE, w.wreck);
        assert!(answer.is_ok(), "{answer:?}");
        assert!(w.reg.locks().targets_of(MERLIN).contains(&w.wreck));
        assert_eq!(
            place(&w.reg, w.wreck),
            Some((w.system, [2_000.0, 0.0, 0.0]))
        );
    }

    // --- the tractor beam -----------------------------------------------------------------

    fn beam_world(wreck_at: [f64; 3]) -> World {
        let mut w = world_with(world_table(), BEAM_TYPE, PLAYER_OWNER);
        crate::combat::set_wreck_position(&mut w.reg, w.wreck, wreck_at);
        w
    }

    fn beam(w: &mut World, at: i64) {
        modules::activate(
            &mut w.reg,
            Some(ALICE),
            MODULE,
            "tractorBeamCan",
            Some(w.wreck),
            1000,
            at,
        )
        .expect("the beam starts");
    }

    #[test]
    fn a_tractor_cycle_pulls_the_wreck_toward_the_ship() {
        let mut w = beam_world([10_000.0, 0.0, 0.0]);
        beam(&mut w, T0);
        clock(&mut w.reg, T0 + BEAM_CYCLE_MS * modules::MSEC + 100);
        let (_, p) = place(&w.reg, w.wreck).expect("still there");
        let moved = 10_000.0 - p[0];
        let expect = BEAM_VELOCITY * (BEAM_CYCLE_MS as f64 / 1000.0);
        assert!(
            (moved - expect).abs() < 1.0,
            "moved {moved}, expected {expect}"
        );
        assert!(
            p[1].abs() < 1e-6 && p[2].abs() < 1e-6,
            "straight at the ship"
        );
        let sent = sent_text(&mut w.reg);
        assert!(
            sent.iter()
                .any(|s| s.starts_with("DoDestinyUpdate") && s.contains("SetBallPosition")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_tractor_beam_stops_short_of_the_hull_and_the_module_with_it() {
        let mut w = beam_world([400.0, 0.0, 0.0]);
        beam(&mut w, T0);
        clock(&mut w.reg, T0 + BEAM_CYCLE_MS * modules::MSEC + 100);
        let (_, p) = place(&w.reg, w.wreck).expect("still there");
        let hull = crate::destiny::hull::of_in(w.reg.content(), 603).radius;
        let stop = f64::from(hull) + f64::from(crate::destiny::WRECK_RADIUS) + TRACTOR_STOP_GAP;
        assert!((p[0] - stop).abs() < 1.0, "stops at {stop}, is at {}", p[0]);
        assert!(
            w.reg.modules().running(MODULE).is_none(),
            "nothing left to pull"
        );
    }

    #[test]
    fn a_tractor_beam_refuses_what_is_not_a_wreck() {
        let mut w = beam_world([10_000.0, 0.0, 0.0]);
        let other = w.wreck + 5_000;
        w.reg.locks_mut().add(MERLIN, other);
        let e = modules::activate(
            &mut w.reg,
            Some(ALICE),
            MODULE,
            "tractorBeamCan",
            Some(other),
            1000,
            T0,
        )
        .expect_err("not a wreck");
        assert!(
            refused(&e).contains("DeniedActivateTargetModuleDisallowed"),
            "{e:?}"
        );
    }

    // --- the table through the content layer -----------------------------------------------

    #[test]
    fn the_registered_table_is_empty_without_a_bundle() {
        let c = crate::content::Content::fixture();
        assert!(c.try_get::<Table>().is_none() || c.get::<Table>().yields_for(1, 0).is_empty());
    }
}
