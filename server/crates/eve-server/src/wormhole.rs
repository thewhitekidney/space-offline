//! **Wormhole statics and signatures**: which wormhole each system holds,
//! where it leads, and how it shows up in space.
//!
//! W16 packet CP54a (`docs/content/plan.md` §6.2; spec
//! `docs/content/R-WH.md`, primary, and C3 §4 archetype 38). Lane note:
//! `docs/lanes/cp54a.md`. CP54b (`wormhole_jump.rs`) builds the jump on
//! [`hole`], [`pair`] and [`debit`].
//!
//! # The rule (R-WH §6.2, G-WH)
//!
//! - A system of effective class `c` ([`effective_class`]) has
//!   [`statics_per_system`]`(c)` static slots `k` (a fraction is a seeded
//!   per-system chance, as in `population.tsv`), at most [`MAX_STATICS`].
//! - Slot `k` at generation `g` draws one `content/wormholes/statics.tsv`
//!   row of class `c` ([`Prepared::draw`]); two slots of one system draw from
//!   disjoint parts of the class's rows, so they never hold the same type.
//!   The far end is a system of the row's `dest_class`, never the home.
//! - A slot's generations follow each other with no gap: generation 0 is
//!   part-way through its life at the world's birth ([`FIRST_LIFE`]), and
//!   each next one is born when the last expires, after the type's own
//!   `wormholeMaxStableTime` minutes [V attribute, I unit].
//!
//! Nothing generated is stored. The one persisted state is the `wh` delta
//! ([`KIND`]), `system_id static k -> dest_system expires_at mass_left
//! generation`, written when a jump debits mass ([`debit`]). It holds while
//! `now < expires_at`; past that the slot is the pure timeline again, and the
//! rollover ([`on_rollover`]) drops the row.
//!
//! # K162 (R-WH §4.1)
//!
//! **The far end exists iff the pair's `mass_left` is below its maximum**:
//! before the first jump only the static is in space; the first debit
//! writes the delta, and the K162 (type [`K162`], which has no dogma of its
//! own [V]) appears in the far system, taking every number from its static.
//!
//! # In space
//!
//! A wormhole is a C3 site whose dungeon is **fixed by its type**: CCP's own
//! archetype-38 dungeons hold one wormhole object plus one Deadspace
//! Signature each (R-WH §5 [V-data]; [`fixed_dungeons`]). So:
//!
//! - its signature goes to the signal tracker as archetype [`ARCHETYPE`]
//!   with scan attribute [`SCAN_ATTRIBUTE`] ([`tick`], the population path,
//!   `signal_tracker::add_site`);
//! - its ball is the dungeon's room laid out by `site::instantiate` when a
//!   pilot arrives, so it is a **local** site object on the grid
//!   (`grid::local_balls` lists `site::balls_in`) and goes out in the site
//!   path's `AddBalls2`, which declares its grid role;
//! - its slim carries the six wormhole keys of R-WH §3 ([`slim_extras`],
//!   seam S9).
//!
//! `population.tsv` gets **no** archetype-38 row (R-WH §5's recommendation,
//! C3 §4.1): a population row draws a random dungeon of its archetypes, and
//! a wormhole's dungeon is fixed by its type.

use crate::celestial::{Band, Catalogue, Universe};
use crate::content::{Content, Sources, TableSpec};
use crate::damage::BlueTime;
use crate::delta::Rows;
use crate::destiny::SlimExtras;
use crate::dungeon_data::{self, ENTRY_SIGNATURE};
use crate::population::{self, SITE_BASE, SITE_END, SLOTS_PER_SYSTEM};
use crate::seed::{self, Kind, Stream};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "wormholes/statics.tsv";

/// Its header, frozen in `docs/content/schema.md` §4.
pub const HEADER: &str = "class\tstatic_type_id\tdest_class\twhy";

/// The shipped copy (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/wormholes/statics.tsv");

/// The registration row in `content_tables::TABLES` (coordinator-only).
pub const TABLE: TableSpec = TableSpec {
    name: "wormhole",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// The registration row in `world_ext::SLOTS` (coordinator-only). Its
/// `make` reads the content tables and typedata, never deltas (CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "wormhole",
    make,
};

/// The delta kind this module owns (`delta_kinds::KINDS`, `("wh",
/// "wormhole")`); `population::settle` drops it with `site` when
/// `population::GEN_VERSION` moves.
pub const KIND: &str = population::WH_KIND;

/// The `key` field of every `wh` delta key (`system_id key k`). A static is
/// not a `population.tsv` row (see the module header), so the key is this
/// module's. **Never rename it**: a rename drops every live `wh` row.
pub const KEY: &str = "static";

/// `dunArchetypeWormhole` (`appConst.pyc:574`) [V].
pub const ARCHETYPE: i64 = 38;
/// `groupWormhole` (`inventorycommon/const`) [V].
pub const GROUP_WORMHOLE: i64 = 988;
/// The K162 type [V] (R-WH §1.1): the far end of every pair.
pub const K162: i64 = 30831;
/// `scanWormholeStrength`, the `siteTypeWormhole` attribute
/// (`probescanning/explorationSites.pyc:20`) [V].
pub const SCAN_ATTRIBUTE: i64 = 1908;

/// `wormholeTargetSystemClass` [V] (R-WH §1.2).
pub const ATTR_TARGET_CLASS: i64 = 1381;
/// `wormholeMaxStableTime`, minutes [V attribute, I unit] (R-WH §1.2).
pub const ATTR_MAX_STABLE_TIME: i64 = 1382;
/// `wormholeMaxStableMass`, kg [V].
pub const ATTR_MAX_STABLE_MASS: i64 = 1383;
/// `wormholeMaxJumpMass`, kg [V].
pub const ATTR_MAX_JUMP_MASS: i64 = 1385;

/// The k-space class constants `HIGH_SEC`, `LOW_SEC`, `ZERO_SEC`
/// (`appConst.pyc:1896-1900`) [V].
pub const CLASS_HIGH: i64 = 7;
/// See [`CLASS_HIGH`].
pub const CLASS_LOW: i64 = 8;
/// See [`CLASS_HIGH`].
pub const CLASS_NULL: i64 = 9;

/// The most statics one system holds; also how many site-id slots the
/// module reserves per system ([`static_slot`], [`k162_slot`]).
pub const MAX_STATICS: u32 = 2;

/// **[A] The chance a k-space system holds a static** (Y-WH raised it from
/// 0.02). A static draws uniformly among its class's nine rows, so two in
/// three lead into C1-C6: at 0.12 about 8% of k-space systems hold a hole
/// into J-space (about 430 of 5,485), enough that a bot ten jumps from home
/// sees one. With 0.02 a bot saw one in about six trips. Falsified by humans
/// calling J-space unreachable (raise) or every k-space system riddled with
/// holes (lower).
pub const KSPACE_HOLE_CHANCE: f64 = 0.12;

/// **[A] How many statics a system of `class` holds** (R-WH §6.2 step 1):
/// one for C1-C3, C5 and C6, two for C4, a per-system chance of
/// [`KSPACE_HOLE_CHANCE`] in high, low and null, none elsewhere (Thera, shattered, Drifter,
/// Pochven, void). Falsified by humans calling J-space unreachable (raise
/// k-space) or a maze (lower it).
pub fn statics_per_system(class: i64) -> f64 {
    match class {
        1 | 2 | 3 | 5 | 6 => 1.0,
        4 => 2.0,
        CLASS_HIGH | CLASS_LOW | CLASS_NULL => KSPACE_HOLE_CHANCE,
        _ => 0.0,
    }
}

/// **[A] A hole's lifetime, minutes, when typedata lacks its type** (no
/// `--typedata`, an extract older than FX7's group 988, or K162, which
/// carries no wormhole attributes [V]). Falsified by a hole that outlives
/// or undercuts the info window's age text by hours.
pub const DEFAULT_LIFE_MIN: f64 = 960.0;
/// **[A] A hole's total mass, kg, when typedata lacks its type.** Falsified
/// by a pair that collapses after far fewer or far more jumps than TQ's.
pub const DEFAULT_MAX_MASS: f64 = 2.0e9;
/// **[A] A hole's per-jump mass, kg, when typedata lacks its type.**
/// Falsified by a battleship passing a hole that should refuse it.
pub const DEFAULT_JUMP_MASS: f64 = 3.0e8;
/// **[A] The shortest life any generation is given, minutes**, so a type
/// with a zero lifetime cannot spin the timeline. Falsified by nothing a
/// pilot sees: no shipped type is below it.
pub const MIN_LIFE_MIN: f64 = 60.0;
/// **[A] Generation 0 is born with this fraction of its life left**, drawn
/// uniformly per slot, so a fresh world's holes do not all expire at once.
/// Falsified by a fresh world whose holes all read "end of life".
pub const FIRST_LIFE: (f64, f64) = (0.1, 1.0);

/// [A] `wormholeSize` buckets: `1.0` while more than this fraction of the
/// mass is left, ...
pub const SIZE_REDUCED: f64 = 0.5;
/// ... `0.7` while more than this, else `0.4`. The values are the keys of
/// `WormholeSetup`'s size dict [V bytecode, `clientControllerRegistry`
/// 164-166: 1.0, 0.7, 0.4; any other key is a `KeyError`]; the info window's
/// "reduced" (`< 1`) and "critical" (`< 0.5`) texts then read the buckets
/// 0.7 and 0.4. Falsified by the model's shrink appearing at a different
/// point than TQ's "reduced" and "critical" texts.
pub const SIZE_CRITICAL: f64 = 0.1;
/// [A] `wormholeAge` buckets, hours left (R-WH §3): above the first is 0,
/// above the second 1, above the third 2, else 3. Falsified by an info
/// window that says "end of life" on a hole with a day left.
pub const AGE_HOURS: [f64; 3] = [24.0, 4.0, 1.0];
/// [I] `maxShipJumpMass` cut points, kg (R-WH §3): at most the first is 1
/// (`WH_SLIM_MAX_SHIP_MASS_SMALL`), the second 2, the third 3, else 4 [V
/// constants]. Falsified by a hole whose info text names a ship class its
/// jump mass refuses.
pub const JUMP_MASS_CUTS: [f64; 3] = [5.0e6, 6.2e7, 4.1e8];

const BLUE_PER_MINUTE: f64 = 600_000_000.0;

const TAG_PRESENCE: u64 = 1;
const TAG_PICK: u64 = 2;
const TAG_DEST: u64 = 3;
const TAG_ANCHOR: u64 = 4;
const TAG_TARGET: u64 = 5;
const TAG_FIRST: u64 = 6;
const TAG_K162: u64 = 7;

fn boxed_load(src: &Sources) -> Result<crate::content::Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_row);
    Ok(Box::new(
        Table::new(rows).map_err(|e| format!("{FILE}: {e}"))?,
    ))
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

/// One `statics.tsv` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub class: i64,
    pub type_id: i64,
    pub dest_class: i64,
}

fn parse_row(r: &crate::authored::Row) -> Result<Row, String> {
    Ok(Row {
        class: r.i64(0)?,
        type_id: r.i64(1)?,
        dest_class: r.i64(2)?,
    })
}

/// Every row, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    rows: Vec<Row>,
}

impl Table {
    /// # Errors
    ///
    /// A `(class, static_type_id)` listed twice (the file's key).
    pub fn new(rows: Vec<Row>) -> Result<Table, String> {
        let mut seen = BTreeSet::new();
        for r in &rows {
            if !seen.insert((r.class, r.type_id)) {
                return Err(format!(
                    "class {} type {} is listed twice",
                    r.class, r.type_id
                ));
            }
        }
        Ok(Table { rows })
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Invented rows: invented types (990_400 up) and the universe
    /// fixture's invented class 990_001, plus the [V] class constants.
    pub fn fixture() -> Table {
        let text = format!(
            "{HEADER}\n\
             990001\t990400\t{CLASS_HIGH}\tx\n\
             990001\t990401\t{CLASS_LOW}\tx\n\
             {CLASS_NULL}\t990402\t990001\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, HEADER).expect("the fixture header");
        let rows = raw.iter().map(|r| parse_row(r).expect("a fixture row"));
        Table::new(rows.collect()).expect("the fixture is well-formed")
    }
}

/// What typedata says about one type: its group and its wormhole
/// attributes, each `None` when the type does not carry it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Facts {
    pub group_id: i64,
    pub target_class: Option<f64>,
    pub life_min: Option<f64>,
    pub max_mass: Option<f64>,
    pub jump_mass: Option<f64>,
}

/// `content`'s typedata facts for a type; `None` when the content holds no
/// typedata or it lacks the type.
pub fn typedata_facts(content: &Content, type_id: i64) -> Option<Facts> {
    let row = content
        .try_get::<crate::typedata::Installed>()?
        .table()
        .get(type_id)?;
    Some(Facts {
        group_id: row.group_id,
        target_class: row.attribute(ATTR_TARGET_CLASS),
        life_min: row.attribute(ATTR_MAX_STABLE_TIME),
        max_mass: row.attribute(ATTR_MAX_STABLE_MASS),
        jump_mass: row.attribute(ATTR_MAX_JUMP_MASS),
    })
}

/// **The loader's check** (R-WH §6.3): a row whose type typedata knows must
/// be a wormhole (group 988) whose `wormholeTargetSystemClass` is the row's
/// `dest_class` [V rule]. A type typedata does not know is kept [A] (no
/// `--typedata`, or an extract older than FX7's group 988). Returns the
/// kept rows and one line per dropped row.
pub fn validate(rows: &[Row], facts: &dyn Fn(i64) -> Option<Facts>) -> (Vec<Row>, Vec<String>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for r in rows {
        match facts(r.type_id) {
            Some(f) if f.group_id != GROUP_WORMHOLE => dropped.push(format!(
                "class {} type {}: group {}, not a wormhole",
                r.class, r.type_id, f.group_id
            )),
            Some(f) if f.target_class != Some(r.dest_class as f64) => dropped.push(format!(
                "class {} type {}: its target class is {:?}, the row says {}",
                r.class, r.type_id, f.target_class, r.dest_class
            )),
            _ => kept.push(*r),
        }
    }
    (kept, dropped)
}

/// A type's lifetime and masses, with the [A] defaults filled in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hole {
    pub life_min: f64,
    pub max_mass: f64,
    pub jump_mass: f64,
}

impl Hole {
    fn of(f: Option<Facts>) -> Hole {
        let f = f.unwrap_or_default();
        Hole {
            life_min: f.life_min.unwrap_or(DEFAULT_LIFE_MIN).max(MIN_LIFE_MIN),
            max_mass: f.max_mass.filter(|m| *m > 0.0).unwrap_or(DEFAULT_MAX_MASS),
            jump_mass: f
                .jump_mass
                .filter(|m| *m > 0.0)
                .unwrap_or(DEFAULT_JUMP_MASS),
        }
    }

    fn life(&self) -> BlueTime {
        (self.life_min * BLUE_PER_MINUTE) as BlueTime
    }
}

/// **Each wormhole type's fixed dungeon** (R-WH §5): an archetype-38
/// dungeon of exactly two objects, one a Deadspace Signature, holds the
/// other object's type; the lowest such dungeon id wins [A choice].
pub fn fixed_dungeons(d: &dungeon_data::Table) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    for (id, dun) in &d.dungeons {
        if dun.archetype != ARCHETYPE || dun.objects.len() != 2 {
            continue;
        }
        let sigs = dun
            .objects
            .iter()
            .filter(|o| o.type_id == ENTRY_SIGNATURE)
            .count();
        if sigs != 1 {
            continue;
        }
        if let Some(o) = dun.objects.iter().find(|o| o.type_id != ENTRY_SIGNATURE) {
            out.entry(o.type_id).or_insert(*id);
        }
    }
    out
}

/// **A system's effective wormhole class** (R-WH §6.1): its own class (or
/// its constellation's or region's, `Universe::wormhole_class`), else its
/// band's k-space class [A] (high 7, low 8, null 9). Void and unknown
/// systems have none. Falsified by a k-space system whose map says another
/// class.
pub fn effective_class(u: &Universe, system: i64) -> Option<i64> {
    u.wormhole_class(system).or_else(|| match u.band(system)? {
        Band::High => Some(CLASS_HIGH),
        Band::Low => Some(CLASS_LOW),
        Band::Null => Some(CLASS_NULL),
        Band::Wormhole | Band::Void => None,
    })
}

/// The site-id slot of static `k`: the top of the system's 64 (plan §3.4),
/// clear of population's slots, which count up from 0. **[A]** population
/// never fills more than `SLOTS_PER_SYSTEM - 2 * MAX_STATICS` slots in one
/// system (that needs about 175 pilots in it). Falsified by a site id two
/// sites share.
pub fn static_slot(k: u32) -> usize {
    SLOTS_PER_SYSTEM - 1 - 2 * k as usize
}

/// The site-id slot of static `k`'s K162, numbered in the **home** system
/// (so ids stay unique however many K162s land in one system).
pub fn k162_slot(k: u32) -> usize {
    SLOTS_PER_SYSTEM - 2 - 2 * k as usize
}

/// Whether a site id is one of this module's ([`static_slot`] or
/// [`k162_slot`]). `site.rs` leaves these instances alone.
pub fn is_wormhole_site(site_id: i64) -> bool {
    if !(SITE_BASE..SITE_END).contains(&site_id) {
        return false;
    }
    let cell = (site_id - SITE_BASE) / i64::from(population::GENERATIONS);
    let slot = (cell % SLOTS_PER_SYSTEM as i64) as usize;
    slot >= SLOTS_PER_SYSTEM - 2 * MAX_STATICS as usize
}

/// One slot's draw at one generation: the row and the far system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Draw {
    pub row: Row,
    pub dest: i64,
}

/// One live pair: a static slot's current generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub home: i64,
    pub k: u32,
    pub generation: u32,
    pub row: Row,
    pub dest: i64,
    pub expires_at: BlueTime,
    pub mass_left: f64,
    pub hole: Hole,
}

impl Pair {
    /// R-WH §4.1: the K162 is in space iff mass has been taken.
    pub fn has_k162(&self) -> bool {
        self.mass_left < self.hole.max_mass
    }

    fn value(&self) -> Vec<String> {
        vec![
            self.dest.to_string(),
            self.expires_at.to_string(),
            self.mass_left.to_string(),
            self.generation.to_string(),
        ]
    }
}

/// The `wh` delta key of `(home, k)`.
pub fn delta_key(home: i64, k: u32) -> Vec<String> {
    vec![home.to_string(), KEY.to_owned(), k.to_string()]
}

/// `dest_system expires_at mass_left generation`.
fn parse_value(v: &[String]) -> Option<(i64, BlueTime, f64, u32)> {
    let [d, e, m, g] = v else { return None };
    Some((
        d.parse().ok()?,
        e.parse().ok()?,
        m.parse().ok()?,
        g.parse().ok()?,
    ))
}

/// The table joined with the universe, the dungeon catalogue and typedata
/// once.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    rows: Vec<Row>,
    holes: BTreeMap<i64, Hole>,
    dungeon: BTreeMap<i64, i64>,
    class_of: BTreeMap<i64, i64>,
    /// A system's region's `nebulaID` (a graphic id), where the bundle has it.
    nebula_of: BTreeMap<i64, i64>,
    rank: BTreeMap<i64, usize>,
    by_class: BTreeMap<i64, Vec<i64>>,
    per: BTreeMap<i64, f64>,
}

impl Prepared {
    /// `facts` is a type's typedata ([`typedata_facts`] on the content, in
    /// the server);
    /// `per_system` is [`statics_per_system`] in the server. Rows the check
    /// drops ([`validate`]) and rows whose type has no fixed dungeon are
    /// logged once here.
    pub fn new(
        table: &Table,
        universe: &Universe,
        dungeons: &dungeon_data::Table,
        facts: &dyn Fn(i64) -> Option<Facts>,
        per_system: &dyn Fn(i64) -> f64,
    ) -> Prepared {
        let (rows, dropped) = validate(&table.rows, facts);
        for line in dropped {
            crate::outln!("[wormhole] {FILE}: dropped {line}");
        }
        let dungeon = fixed_dungeons(dungeons);
        let mut p = Prepared {
            rows,
            dungeon,
            ..Prepared::default()
        };
        for r in &p.rows {
            p.holes.insert(r.type_id, Hole::of(facts(r.type_id)));
            if !p.dungeon.contains_key(&r.type_id) && !dungeons.dungeons.is_empty() {
                crate::outln!(
                    "[wormhole] type {} has no archetype-{ARCHETYPE} dungeon: its signature is never shown",
                    r.type_id
                );
            }
        }
        for (rank, s) in universe.systems().enumerate() {
            p.rank.insert(s.id, rank);
            if let Some(n) = universe.nebula_of(s.id) {
                p.nebula_of.insert(s.id, n);
            }
            if let Some(c) = effective_class(universe, s.id) {
                p.class_of.insert(s.id, c);
                p.by_class.entry(c).or_default().push(s.id);
                p.per.entry(c).or_insert_with(|| per_system(c));
            }
        }
        p
    }

    /// The effective class [`Prepared::new`] found for a system.
    pub fn class_of(&self, system: i64) -> Option<i64> {
        self.class_of.get(&system).copied()
    }

    /// The `nebulaID` of the region a system is in, `None` when the bundle
    /// has no such column (a stale one fails its header check first).
    pub fn nebula_of(&self, system: i64) -> Option<i64> {
        self.nebula_of.get(&system).copied()
    }

    /// The fixed dungeon of a type ([`fixed_dungeons`]).
    pub fn dungeon_of(&self, type_id: i64) -> Option<i64> {
        self.dungeon.get(&type_id).copied()
    }

    /// The static slots of `home`: `0..n`, `n` the class's count (a
    /// fraction is a seeded chance), at most [`MAX_STATICS`].
    pub fn slots(&self, seed: u64, home: i64) -> Vec<u32> {
        let Some(per) = self.class_of(home).and_then(|c| self.per.get(&c)) else {
            return Vec::new();
        };
        let whole = per.floor();
        let frac = per - whole;
        let mut r = seed::stream(seed, Kind::Wormhole, &[home as u64, TAG_PRESENCE]);
        let n = whole as u32 + u32::from(frac > 0.0 && r.chance(frac));
        (0..n.min(MAX_STATICS)).collect()
    }

    fn stream(seed: u64, home: i64, k: u32, g: u32, tag: u64) -> Stream {
        seed::stream(
            seed,
            Kind::Wormhole,
            &[home as u64, u64::from(k), u64::from(g), tag],
        )
    }

    /// **R-WH §6.2 steps 2-3** for `(home, k, generation)`. Slot `k` of `n`
    /// draws among the class's rows whose index is `k` modulo `n` (all of
    /// them when that part is empty), so two slots of one system never hold
    /// one type [A: the partition]; then a system of the row's `dest_class`
    /// other than `home`, by id order. `None` when no row or no system fits.
    pub fn draw(&self, seed: u64, home: i64, k: u32, generation: u32) -> Option<Draw> {
        let class = self.class_of(home)?;
        let n = self.slots(seed, home).len().max(1);
        let all: Vec<&Row> = self.rows.iter().filter(|r| r.class == class).collect();
        let part: Vec<&Row> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| i % n == k as usize % n)
            .map(|(_, r)| *r)
            .collect();
        let list = if part.is_empty() { all } else { part };
        if list.is_empty() {
            return None;
        }
        let pick = Self::stream(seed, home, k, generation, TAG_PICK).next() % list.len() as u64;
        let row = *list[pick as usize];
        let dests: Vec<i64> = self
            .by_class
            .get(&row.dest_class)?
            .iter()
            .copied()
            .filter(|s| *s != home)
            .collect();
        if dests.is_empty() {
            return None;
        }
        let i = Self::stream(seed, home, k, generation, TAG_DEST).next() % dests.len() as u64;
        Some(Draw {
            row,
            dest: dests[i as usize],
        })
    }

    fn hole_of(&self, type_id: i64) -> Hole {
        self.holes.get(&type_id).copied().unwrap_or(Hole::of(None))
    }

    /// **The pure timeline**: the generation of `(home, k)` alive at `now`,
    /// with no delta. `None` for a slot that draws nothing.
    pub fn pure(
        &self,
        seed: u64,
        birth: BlueTime,
        home: i64,
        k: u32,
        now: BlueTime,
    ) -> Option<Pair> {
        let mut g = 0;
        let mut d = self.draw(seed, home, k, g)?;
        let mut hole = self.hole_of(d.row.type_id);
        let mut r = Self::stream(seed, home, k, 0, TAG_FIRST);
        let left = FIRST_LIFE.0 + (FIRST_LIFE.1 - FIRST_LIFE.0) * r.unit();
        let mut expires = birth + (hole.life() as f64 * left) as BlueTime;
        while now >= expires {
            g = g.checked_add(1)?;
            d = self.draw(seed, home, k, g)?;
            hole = self.hole_of(d.row.type_id);
            expires += hole.life();
        }
        Some(Pair {
            home,
            k,
            generation: g,
            row: d.row,
            dest: d.dest,
            expires_at: expires,
            mass_left: hole.max_mass,
            hole,
        })
    }

    /// **The pair of `(home, k)` at `now`**, its `wh` delta applied. The
    /// delta holds while `now < expires_at` and its `dest_system` is still
    /// its generation's draw; the flag says it was set and does not hold
    /// (the rollover drops such rows).
    pub fn pair(
        &self,
        seed: u64,
        birth: BlueTime,
        deltas: &Rows,
        home: i64,
        k: u32,
        now: BlueTime,
    ) -> (Option<Pair>, bool) {
        let Some(v) = deltas.get(&delta_key(home, k)) else {
            return (self.pure(seed, birth, home, k, now), false);
        };
        let held = parse_value(v).and_then(|(dest, expires_at, mass_left, g)| {
            let d = self.draw(seed, home, k, g)?;
            (d.dest == dest && now < expires_at).then(|| Pair {
                home,
                k,
                generation: g,
                row: d.row,
                dest,
                expires_at,
                mass_left,
                hole: self.hole_of(d.row.type_id),
            })
        });
        match held {
            Some(p) => (Some(p), false),
            None => (self.pure(seed, birth, home, k, now), true),
        }
    }

    /// The site id of a static or a K162 (plan §3.4's formula, in the home
    /// system's rank).
    fn site_id(&self, p: &Pair, k162: bool) -> Option<i64> {
        let slot = if k162 {
            k162_slot(p.k)
        } else {
            static_slot(p.k)
        };
        population::site_id(*self.rank.get(&p.home)?, slot, p.generation)
    }

    /// The static's and (when it is open) the K162's site records, from one
    /// pair. A hole whose type has no fixed dungeon has no record.
    pub fn sigs(&self, seed: u64, cat: &Catalogue, p: &Pair, now: BlueTime) -> Vec<Sig> {
        let mut out = Vec::new();
        let ends = [(false, p.home, p.row.type_id), (true, p.dest, K162)];
        for (k162, system, type_id) in ends {
            if k162 && !p.has_k162() {
                continue;
            }
            let (Some(dungeon_id), Some(site_id)) =
                (self.dungeon_of(type_id), self.site_id(p, k162))
            else {
                continue;
            };
            let tag = if k162 { TAG_K162 } else { 0 };
            let mut r = Self::stream(seed, p.home, p.k, p.generation, TAG_ANCHOR + tag);
            let anchor = population::anchor(&mut r, cat, system);
            let mut t = Self::stream(seed, p.home, p.k, p.generation, TAG_TARGET + tag);
            let other = if k162 { p.home } else { p.dest };
            out.push(Sig {
                site_id,
                system,
                dungeon_id,
                type_id,
                anchor,
                target_id: population::target_id(&mut t),
                k162,
                other_class: self.class_of(other).unwrap_or(0),
                other_nebula: self.nebula_of(other),
                pair: *p,
                at: now,
            });
        }
        out
    }
}

/// One wormhole in space: what the tracker and the site path are handed,
/// and what the slim and CP54b's jump read.
#[derive(Debug, Clone, PartialEq)]
pub struct Sig {
    pub site_id: i64,
    /// The system the hole is in (the far system for a K162).
    pub system: i64,
    pub dungeon_id: i64,
    /// The hole's type: the static's, or [`K162`].
    pub type_id: i64,
    pub anchor: [f64; 3],
    pub target_id: String,
    pub k162: bool,
    /// The effective class of the system on the other side.
    pub other_class: i64,
    /// The far region's `nebulaID`, sent as the slim's `nebulaType`.
    pub other_nebula: Option<i64>,
    pub pair: Pair,
    /// When this record was resolved.
    pub at: BlueTime,
}

impl Sig {
    /// The record as the site path takes it (`site::instantiate`).
    pub fn as_site(&self) -> population::Site {
        population::Site {
            system: self.system,
            row: 0,
            k: self.pair.k,
            slot: if self.k162 {
                k162_slot(self.pair.k)
            } else {
                static_slot(self.pair.k)
            },
            generation: self.pair.generation,
            site_id: self.site_id,
            dungeon_id: self.dungeon_id,
            archetype: ARCHETYPE,
            entry: dungeon_data::Entry::Signature,
            anchor: self.anchor,
            target_id: self.target_id.clone(),
            state: population::State::Live,
            at: self.at,
        }
    }

    /// The signal tracker's record (C2 §2.4), as population's: a signature
    /// with the wormhole scan attribute, and the population's [A] radius and
    /// strength (R-WH §5: the types carry neither).
    pub fn tracker_site(&self) -> crate::signal_tracker::Site {
        crate::signal_tracker::Site {
            site_id: self.site_id,
            instance_id: self.site_id,
            dungeon_id: self.dungeon_id,
            solar_system_id: self.system,
            position: self.anchor,
            group_id: crate::signal_tracker::GROUP_SIGNATURE,
            is_scannable: true,
            scan_strength_attribute: SCAN_ATTRIBUTE,
            scan_strength_value: population::SIGNATURE_STRENGTH,
            signature_radius: population::SIGNATURE_RADIUS,
            target_id: self.target_id.clone(),
            allowed_types: Vec::new(),
        }
    }

    /// The six slim keys of R-WH §3, at the record's time.
    pub fn slim_fields(&self) -> Vec<(&'static str, PyValue)> {
        let p = &self.pair;
        vec![
            (
                "wormholeSize",
                PyValue::Real(size_bucket(p.mass_left / p.hole.max_mass)),
            ),
            (
                "wormholeAge",
                PyValue::Int(age_bucket(p.expires_at - self.at)),
            ),
            (
                "maxShipJumpMass",
                PyValue::Int(jump_mass_bucket(p.hole.jump_mass)),
            ),
            // The far region's `nebulaID` (R-WH §3, `regions.static`). The
            // client's `Wormhole.Assemble` loads it as a graphic id, and a
            // None made `GetNebulaTextureForType` raise and the hole fail to
            // assemble [V regress run bb0de23]. `None` only when the bundle
            // names no nebula for that region.
            (
                "nebulaType",
                self.other_nebula.map_or(PyValue::None, PyValue::Int),
            ),
            ("otherSolarSystemClass", PyValue::Int(self.other_class)),
            ("isDestTriglavian", PyValue::Bool(false)),
        ]
    }
}

/// `wormholeSize` [V domain {1.0, 0.7, 0.4}] from the fraction of mass left.
pub fn size_bucket(left: f64) -> f64 {
    if left > SIZE_REDUCED {
        1.0
    } else if left > SIZE_CRITICAL {
        0.7
    } else {
        0.4
    }
}

/// `wormholeAge` [V domain 0..4; 4 never sent] from the time left.
pub fn age_bucket(left: BlueTime) -> i64 {
    let hours = left as f64 / (60.0 * BLUE_PER_MINUTE);
    AGE_HOURS
        .iter()
        .position(|h| hours > *h)
        .map_or(AGE_HOURS.len() as i64, |i| i as i64)
}

/// `maxShipJumpMass` [V domain 1..4] from the per-jump mass.
pub fn jump_mass_bucket(jump_mass: f64) -> i64 {
    JUMP_MASS_CUTS
        .iter()
        .position(|c| jump_mass <= *c)
        .map_or(JUMP_MASS_CUTS.len() as i64 + 1, |i| i as i64 + 1)
}

// ---- the world slot and the Registry side ----

/// The world slot: the prepared table, and what the tracker and the site
/// path were handed.
#[derive(Debug, Default)]
pub struct Wormholes {
    prep: Prepared,
    /// Per live system, the records the tracker holds, by site id.
    shown: BTreeMap<i64, BTreeMap<i64, Sig>>,
    /// Per live system, when its records are next re-read.
    due: BTreeMap<i64, BlueTime>,
    /// Per hot system, when its arrivals are next checked.
    check: BTreeMap<i64, BlueTime>,
}

impl Wormholes {
    pub fn new(prep: Prepared) -> Wormholes {
        Wormholes {
            prep,
            ..Wormholes::default()
        }
    }

    pub fn prepared(&self) -> &Prepared {
        &self.prep
    }
}

fn make(c: &Content) -> Boxed {
    let prep = match (
        c.try_get::<Table>(),
        c.try_get::<Universe>(),
        c.try_get::<dungeon_data::Table>(),
    ) {
        (Some(t), Some(u), Some(d)) => {
            // Y-WH: a test may pin the k-space chance (`Sites::with_hole_chance`).
            let pinned = c
                .try_get::<crate::wormhole_sites::Sites>()
                .and_then(crate::wormhole_sites::Sites::hole_chance);
            let per_system = move |class: i64| match (pinned, class) {
                (Some(p), CLASS_HIGH | CLASS_LOW | CLASS_NULL) => p,
                _ => statics_per_system(class),
            };
            Prepared::new(t, u, d, &|type_id| typedata_facts(c, type_id), &per_system)
        }
        _ => Prepared::default(),
    };
    Box::new(Wormholes::new(prep))
}

/// [`Prepared::pair`] on the registry's world.
pub fn pair(reg: &Registry, home: i64, k: u32, now: BlueTime) -> Option<Pair> {
    let w = reg.world();
    let deltas = reg.deltas().of(KIND);
    reg.ext::<Wormholes>()
        .prep
        .pair(w.seed, w.birth, deltas, home, k, now)
        .0
}

/// **Every wormhole in `system` at `now`**: its statics, and the K162s of
/// the pairs that lead into it and are open (those all have a `wh` delta).
pub fn sigs(reg: &Registry, system: i64, now: BlueTime) -> Vec<Sig> {
    let (seed, cat) = (reg.world().seed, reg.celestials());
    let prep = &reg.ext::<Wormholes>().prep;
    let mut out = Vec::new();
    for k in prep.slots(seed, system) {
        // A spent pair (W17 CP54b) is in space at neither end until its
        // generation ends.
        if let Some(p) = pair(reg, system, k, now).filter(|p| p.mass_left > 0.0) {
            out.extend(
                prep.sigs(seed, cat, &p, now)
                    .into_iter()
                    .filter(|s| !s.k162),
            );
        }
    }
    let homes: BTreeSet<(i64, u32)> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|key| match key.as_slice() {
            [h, _, k] => Some((h.parse().ok()?, k.parse().ok()?)),
            _ => None,
        })
        .collect();
    for (home, k) in homes {
        let Some(p) = pair(reg, home, k, now)
            .filter(|p| p.dest == system && p.has_k162() && p.mass_left > 0.0)
        else {
            continue;
        };
        out.extend(prep.sigs(seed, cat, &p, now).into_iter().filter(|s| s.k162));
    }
    out
}

/// **The wormhole a site id names**, as the last sync saw it: for CP54b,
/// from a ball's instance (`site::Sites::of_item(ball).site_id`).
pub fn hole(reg: &Registry, site_id: i64) -> Option<&Sig> {
    reg.ext::<Wormholes>()
        .shown
        .values()
        .find_map(|m| m.get(&site_id))
}

/// **Take `mass` kg from the pair `(home, k)`** at `now` (CP54b's jump,
/// R-WH §4.3 step 4) and persist it as the `wh` delta. The first debit
/// opens the K162. Returns the mass left (at or below 0: the pair is spent,
/// and collapsing it is CP54b's); `None` for a slot with no pair.
pub fn debit(reg: &mut Registry, home: i64, k: u32, mass: f64, now: BlueTime) -> Option<f64> {
    let mut p = pair(reg, home, k, now)?;
    p.mass_left -= mass.max(0.0);
    population::stamp(reg);
    reg.deltas_mut().put(KIND, delta_key(home, k), p.value());
    let w = reg.ext_mut::<Wormholes>();
    for s in [home, p.dest] {
        w.due.insert(s, BlueTime::MIN);
        w.check.insert(s, BlueTime::MIN);
    }
    Some(p.mass_left)
}

/// **Re-read `system` on the next tick** (W17 CP54b's collapse): its
/// records go to the tracker again and its arrivals are checked again, so a
/// hole that collapsed is neither shown nor laid out a second time.
pub fn resync(reg: &mut Registry, system: i64) {
    let w = reg.ext_mut::<Wormholes>();
    w.due.insert(system, BlueTime::MIN);
    w.check.insert(system, BlueTime::MIN);
}

/// **The rollover step**: drop every `wh` row that no longer holds (its
/// pair expired, or its key or value matches no slot). Returns how many.
pub fn on_rollover(reg: &mut Registry, day: u32) -> usize {
    population::settle(reg);
    // Lane clock: the rollover's own time at a `--time-scale` above 1.
    let t = crate::game_clock::day_start(reg, day);
    let (seed, birth) = (reg.world().seed, reg.world().birth);
    let prep = &reg.ext::<Wormholes>().prep;
    let deltas = reg.deltas().of(KIND);
    let stale: Vec<Vec<String>> = deltas
        .keys()
        .filter(|key| {
            let slot = match key.as_slice() {
                [h, key, k] if key == KEY => h.parse::<i64>().ok().zip(k.parse::<u32>().ok()),
                _ => None,
            };
            match slot {
                Some((h, k)) if prep.slots(seed, h).contains(&k) => {
                    prep.pair(seed, birth, deltas, h, k, t).1
                }
                _ => true,
            }
        })
        .cloned()
        .collect();
    for key in &stale {
        reg.deltas_mut().del(KIND, key);
    }
    stale.len()
}

/// **The stage** (after `population`, before `site`): every live system's
/// wormholes go to the signal tracker, re-read every
/// `population::SYNC_EVERY`; in a hot system, a pilot within
/// `site::ARRIVAL_RANGE` of one gets its ball through `site::instantiate`
/// every `site::CHECK_EVERY`, and an instance whose hole has gone is
/// vacated. A system gone cold takes its records with it.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    let (live, hot): (BTreeSet<i64>, BTreeSet<i64>) = {
        let l = reg.ext::<crate::liveness::Liveness>();
        (l.iter().map(|(s, _)| s).collect(), l.hot().collect())
    };
    let w = reg.ext::<Wormholes>();
    let gone: Vec<i64> = w
        .shown
        .keys()
        .filter(|s| !live.contains(s))
        .copied()
        .collect();
    let due: Vec<i64> = live
        .iter()
        .filter(|s| w.due.get(s).is_none_or(|t| ft >= *t))
        .copied()
        .collect();
    let check: Vec<i64> = hot
        .iter()
        .filter(|s| w.check.get(s).is_none_or(|t| ft >= *t))
        .copied()
        .collect();
    for system in gone {
        let w = reg.ext_mut::<Wormholes>();
        w.due.remove(&system);
        w.check.remove(&system);
        for id in w.shown.remove(&system).unwrap_or_default().into_keys() {
            crate::signal_tracker::remove_site(reg, system, id);
        }
    }
    for system in due {
        sync(reg, system, ft);
    }
    for system in check {
        if crate::site::nothing_to_check(reg, system) {
            reg.ext_mut::<Wormholes>()
                .check
                .insert(system, ft + crate::site::CHECK_EVERY);
            continue;
        }
        arrivals(reg, system, ft);
    }
}

/// Re-read one system's wormholes and hand the tracker the difference.
fn sync(reg: &mut Registry, system: i64, now: BlueTime) {
    let want: BTreeMap<i64, Sig> = sigs(reg, system, now)
        .into_iter()
        .map(|s| (s.site_id, s))
        .collect();
    let have = reg
        .ext_mut::<Wormholes>()
        .shown
        .remove(&system)
        .unwrap_or_default();
    for id in have.keys().filter(|id| !want.contains_key(id)) {
        crate::signal_tracker::remove_site(reg, system, *id);
    }
    for (id, s) in &want {
        if !have.contains_key(id) {
            crate::signal_tracker::add_site(reg, s.tracker_site());
        }
    }
    let w = reg.ext_mut::<Wormholes>();
    w.shown.insert(system, want);
    w.due.insert(system, now + population::SYNC_EVERY);
}

/// One hot system: vacate the instances of holes that went, instantiate
/// the holes a pilot has reached.
fn arrivals(reg: &mut Registry, system: i64, ft: BlueTime) {
    reg.ext_mut::<Wormholes>()
        .check
        .insert(system, ft + crate::site::CHECK_EVERY);
    reg.advance_system(system);
    let shown: Vec<population::Site> = reg
        .ext::<Wormholes>()
        .shown
        .get(&system)
        .map(|m| m.values().map(Sig::as_site).collect())
        .unwrap_or_default();
    let live: BTreeSet<i64> = shown.iter().map(|s| s.site_id).collect();
    let have: BTreeSet<i64> = reg
        .ext::<crate::site::Sites>()
        .iter()
        .filter(|i| i.system == system && is_wormhole_site(i.site_id))
        .map(|i| i.site_id)
        .collect();
    for id in have.iter().filter(|id| !live.contains(id)) {
        crate::site::vacate(reg, *id);
    }
    let pilots: Vec<[f64; 3]> = reg
        .roster()
        .in_system(system)
        .into_iter()
        .filter(|p| reg.character(p.character_id).is_some())
        .map(|p| p.motion.position)
        .collect();
    let reached: Vec<population::Site> = crate::site::arrivals(&shown, &have, &pilots)
        .into_iter()
        .cloned()
        .collect();
    for s in &reached {
        crate::site::instantiate(reg, s);
    }
}

/// **The S9 source** (`slim_extra::SOURCES`): the six wormhole keys on
/// every placed wormhole object among `ids` (the instance's object of the
/// hole's own type; its Deadspace Signature gets none).
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> SlimExtras {
    let sites = reg.ext::<crate::site::Sites>();
    ids.iter()
        .filter_map(|id| {
            let inst = sites.of_item(*id)?;
            if !is_wormhole_site(inst.site_id) {
                return None;
            }
            let sig = hole(reg, inst.site_id)?;
            let placed = inst.objects.iter().find(|p| p.item_id == *id)?;
            (placed.type_id == sig.type_id).then(|| (*id, sig.slim_fields()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Dungeon, Object, Room};
    use crate::model::SandboxConfig;
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const SEED: u64 = 0x5eed_c54a;
    /// The universe fixture's systems by effective class.
    const HIGH: i64 = 39_000_001;
    const NULL: i64 = 39_000_003;
    const WH: i64 = 39_000_006;
    const WH_CLASS: i64 = 990_001;
    /// A character the default sandbox config knows, and its ship.
    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;

    /// Invented per-class counts: two statics in the invented class, one in
    /// null, none elsewhere.
    fn per(c: i64) -> f64 {
        match c {
            WH_CLASS => 2.0,
            CLASS_NULL => 1.0,
            _ => 0.0,
        }
    }

    /// One invented archetype-38 dungeon per fixture type and one for the
    /// K162: a room holding the hole and a Deadspace Signature.
    fn dungeons() -> dungeon_data::Table {
        let types = [990_400, 990_401, 990_402, K162];
        let list = types.iter().enumerate().map(|(i, t)| {
            let object = |id, type_id| Object {
                room: 1,
                id,
                type_id,
                ..Object::default()
            };
            Dungeon {
                id: 999_380 + i as i64,
                archetype: ARCHETYPE,
                entry_type: Some(ENTRY_SIGNATURE),
                rooms: vec![Room {
                    id: 1,
                    ..Room::default()
                }],
                objects: vec![object(1, *t), object(2, ENTRY_SIGNATURE)],
                ..Dungeon::default()
            }
        });
        dungeon_data::Table {
            dungeons: list.map(|d| (d.id, d)).collect(),
            ..dungeon_data::Table::default()
        }
    }

    fn prep() -> Prepared {
        Prepared::new(
            &Table::fixture(),
            &Universe::fixture(),
            &dungeons(),
            &|_| None,
            &per,
        )
    }

    fn birth() -> BlueTime {
        crate::delta::World::FIXTURE.birth
    }

    /// **The plan's test**: a fixture class-N system's static draws a row
    /// of class N, and its far end is a system of that row's `dest_class`,
    /// never the home; over many generations and every slot.
    #[test]
    fn a_static_draws_a_destination_of_its_rows_class() {
        let (p, u) = (prep(), Universe::fixture());
        let mut seen = 0;
        for home in [WH, NULL] {
            let class = p.class_of(home).unwrap();
            for k in p.slots(SEED, home) {
                for g in 0..32 {
                    let d = p.draw(SEED, home, k, g).expect("a fixture row fits");
                    assert_eq!(d.row.class, class);
                    assert_ne!(d.dest, home);
                    assert_eq!(effective_class(&u, d.dest), Some(d.row.dest_class));
                    seen += 1;
                }
            }
        }
        assert!(seen > 0);
    }

    /// The same inputs give the same pairs on a fresh `Prepared`.
    #[test]
    fn the_draw_is_deterministic() {
        let (a, b) = (prep(), prep());
        for home in [WH, NULL] {
            for k in a.slots(SEED, home) {
                for g in 0..8 {
                    assert_eq!(a.draw(SEED, home, k, g), b.draw(SEED, home, k, g));
                }
                let now = birth() + 7 * 24 * 60 * BLUE_PER_MINUTE as BlueTime;
                assert_eq!(
                    a.pure(SEED, birth(), home, k, now),
                    b.pure(SEED, birth(), home, k, now)
                );
            }
        }
    }

    /// Two statics of one system never hold the same row, at any pair of
    /// generations; the count follows the class's rule.
    #[test]
    fn two_statics_of_one_system_never_share_a_row() {
        let p = prep();
        assert_eq!(p.slots(SEED, WH).len(), per(WH_CLASS) as usize);
        assert_eq!(p.slots(SEED, HIGH), Vec::<u32>::new());
        for g0 in 0..16 {
            for g1 in 0..16 {
                let a = p.draw(SEED, WH, 0, g0).unwrap().row;
                let b = p.draw(SEED, WH, 1, g1).unwrap().row;
                assert_ne!(a, b);
            }
        }
    }

    /// The timeline: a slot's generation is alive until its expiry, and the
    /// next one starts at that instant.
    #[test]
    fn generations_follow_each_other_without_a_gap() {
        let p = prep();
        let first = p.pure(SEED, birth(), NULL, 0, birth()).unwrap();
        assert_eq!(first.generation, 0);
        assert!(first.expires_at > birth());
        let last = p
            .pure(SEED, birth(), NULL, 0, first.expires_at - 1)
            .unwrap();
        assert_eq!(last.generation, 0);
        let next = p.pure(SEED, birth(), NULL, 0, first.expires_at).unwrap();
        assert_eq!(next.generation, 1);
        assert_eq!(next.expires_at, first.expires_at + next.hole.life());
    }

    /// The loader's check: a type typedata knows must be group 988 with
    /// the row's target class; an unknown type is kept.
    #[test]
    fn the_check_drops_a_non_wormhole_and_a_wrong_target_class() {
        let rows = Table::fixture().rows().to_vec();
        let facts = |t: i64| match t {
            990_400 => Some(Facts {
                group_id: GROUP_WORMHOLE + 1,
                target_class: Some(CLASS_HIGH as f64),
                ..Facts::default()
            }),
            990_401 => Some(Facts {
                group_id: GROUP_WORMHOLE,
                target_class: Some(CLASS_HIGH as f64),
                ..Facts::default()
            }),
            _ => None,
        };
        let (kept, dropped) = validate(&rows, &facts);
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        assert_eq!(
            kept.iter().map(|r| r.type_id).collect::<Vec<_>>(),
            [990_402]
        );
        let right = |t: i64| {
            let r = rows.iter().find(|r| r.type_id == t)?;
            Some(Facts {
                group_id: GROUP_WORMHOLE,
                target_class: Some(r.dest_class as f64),
                ..Facts::default()
            })
        };
        assert_eq!(validate(&rows, &right).0, rows);
    }

    /// A type's fixed dungeon is the lowest archetype-38 two-object
    /// dungeon holding it with a Deadspace Signature; other shapes hold
    /// nothing.
    #[test]
    fn a_type_has_the_lowest_two_object_wormhole_dungeon() {
        let mut d = dungeons();
        let mut later = d.dungeons[&999_380].clone();
        later.id = 999_390;
        d.dungeons.insert(later.id, later);
        let mut wrong = d.dungeons[&999_381].clone();
        wrong.id = 999_370;
        wrong.archetype = ARCHETYPE + 1;
        d.dungeons.insert(wrong.id, wrong);
        let f = fixed_dungeons(&d);
        assert_eq!(f[&990_400], 999_380);
        assert_eq!(f[&990_401], 999_381);
        assert!(!f.contains_key(&ENTRY_SIGNATURE));
    }

    /// The slim buckets stay in the client's domains and are monotone.
    #[test]
    fn the_slim_buckets_are_in_domain_and_monotone() {
        let mut last = 1.0;
        for i in (0..=100).rev() {
            let s = size_bucket(f64::from(i) / 100.0);
            assert!([1.0, 0.7, 0.4].contains(&s));
            assert!(s <= last);
            last = s;
        }
        let hour = (60.0 * BLUE_PER_MINUTE) as BlueTime;
        let mut last = 0;
        for h in (0..72).rev() {
            let a = age_bucket(h * hour);
            assert!((0..=3).contains(&a));
            assert!(a >= last);
            last = a;
        }
        let mut last = 0;
        for m in [1.0e5, 1.0e6, 1.0e7, 1.0e8, 1.0e9, 1.0e10] {
            let b = jump_mass_bucket(m);
            assert!((1..=4).contains(&b));
            assert!(b >= last);
            last = b;
        }
    }

    /// Wormhole site ids sit in the site band's top slots, distinct per
    /// end, and population's low slots are never taken for one.
    #[test]
    fn wormhole_site_ids_are_the_top_slots() {
        for k in 0..MAX_STATICS {
            for (slot, g) in [(static_slot(k), 0), (k162_slot(k), 5)] {
                let id = population::site_id(3, slot, g).unwrap();
                assert!(is_wormhole_site(id));
            }
            assert_ne!(static_slot(k), k162_slot(k));
        }
        for slot in 0..SLOTS_PER_SYSTEM - 2 * MAX_STATICS as usize {
            assert!(!is_wormhole_site(population::site_id(3, slot, 7).unwrap()));
        }
        assert!(!is_wormhole_site(SITE_END));
    }

    /// The shipped file parses whole. (No value in it is asserted.)
    #[test]
    fn the_shipped_file_loads() {
        let dir = crate::authored::Dir::compiled_only();
        let file = dir.load(FILE, SHIPPED, HEADER).expect("the header matches");
        let (rows, skipped) = file.parse_rows_logged(parse_row);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(Table::new(rows).is_ok());
    }

    /// The schema's invented example row parses.
    #[test]
    fn the_schema_example_row_parses() {
        let text = format!("{HEADER}\n1\t990400\t7\t[A] invented example row\n");
        let (raw, _) = crate::authored::parse(&text, HEADER).unwrap();
        assert!(parse_row(&raw[0]).is_ok());
    }

    // ---- the Registry side ----

    fn cat() -> Catalogue {
        let systems: Vec<i64> = Universe::fixture().systems().map(|s| s.id).collect();
        let mut text = String::new();
        for (n, s) in systems.into_iter().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\n",
                40_000_000 + n
            );
        }
        Catalogue::parse(&text)
    }

    fn world() -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeons", dungeons());
        c.insert("wormhole", Table::fixture());
        let mut reg = Registry::new(SandboxConfig {
            celestials: cat(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        *reg.ext_mut::<Wormholes>() = Wormholes::new(prep());
        reg.deltas_mut().set_world(crate::delta::World {
            seed: SEED,
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        reg
    }

    fn pilot_at(reg: &mut Registry, system: i64, at: [f64; 3]) {
        reg.roster_mut().leave(ALICE);
        let p = Presence::arriving(ALICE, ALICE_SHIP, system, Motion::arrived(at, [0.0; 3]));
        reg.roster_mut().arrive(p);
        let now = reg.world().birth;
        crate::liveness::refresh(reg, Instant::now(), now);
    }

    /// The signature ids the tracker's full state names for Alice's system.
    fn tracked(reg: &Registry) -> Vec<i64> {
        let PyValue::Tuple(parts) = crate::signal_tracker::full_state(reg, Some(ALICE)) else {
            panic!("the full state is a tuple");
        };
        parts[1]
            .dict_as_kv()
            .expect("a dict")
            .into_iter()
            .filter_map(|(k, _)| match k {
                PyValue::Int(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// **The plan's test**: a wormhole signature reaches the signal tracker
    /// through the population path as archetype 38 [V], with the wormhole
    /// scan attribute and its type's fixed dungeon, in its home system only.
    #[test]
    fn a_static_signature_reaches_the_tracker_as_archetype_38() {
        let mut reg = world();
        let now = reg.world().birth;
        pilot_at(&mut reg, NULL, [0.0; 3]);
        tick(&mut reg, Instant::now(), now);
        let want = sigs(&reg, NULL, now);
        assert_eq!(want.len(), 1);
        let s = &want[0];
        let d = reg.content().get::<dungeon_data::Table>();
        assert_eq!(d.dungeons[&s.dungeon_id].archetype, ARCHETYPE);
        assert_eq!(
            Some(s.dungeon_id),
            reg.ext::<Wormholes>().prep.dungeon_of(s.type_id)
        );
        assert_eq!(tracked(&reg), [s.site_id]);
        let site = s.tracker_site();
        assert_eq!(site.scan_strength_attribute, SCAN_ATTRIBUTE);
        assert_eq!(site.group_id, crate::signal_tracker::GROUP_SIGNATURE);
        pilot_at(&mut reg, s.pair.dest, [0.0; 3]);
        tick(&mut reg, Instant::now(), now);
        let far = tracked(&reg);
        assert!(
            !far.contains(&s.site_id),
            "the static is in its home system only"
        );
        assert!(sigs(&reg, s.pair.dest, now).iter().all(|x| !x.k162));
    }

    /// **The plan's test**: before any jump the far system holds nothing;
    /// after the first debit it holds the K162, of the K162's own dungeon,
    /// and the pair shows the mass taken.
    /// The slim's `nebulaType` is the far region's `nebulaID` as an int: a
    /// `None` made the client's `Wormhole.Assemble` raise in
    /// `GetNebulaTextureForType` and the hole never assembled.
    #[test]
    fn the_slim_names_the_far_regions_nebula() {
        let reg = world();
        let now = reg.world().birth;
        let p = pair(&reg, NULL, 0, now).unwrap();
        let s = &sigs(&reg, NULL, now)[0];
        let want = Universe::fixture()
            .nebula_of(p.dest)
            .expect("a fixture nebula");
        let fields = s.slim_fields();
        let (_, v) = fields
            .iter()
            .find(|(k, _)| *k == "nebulaType")
            .expect("nebulaType");
        assert_eq!(*v, PyValue::Int(want));
    }

    #[test]
    fn the_k162_exists_after_the_first_jump() {
        let mut reg = world();
        let now = reg.world().birth;
        let p = pair(&reg, NULL, 0, now).unwrap();
        assert!(!p.has_k162());
        assert!(sigs(&reg, p.dest, now).iter().all(|s| !s.k162));
        let left = debit(&mut reg, NULL, 0, p.hole.jump_mass / 2.0, now).unwrap();
        assert!(left < p.hole.max_mass);
        let far: Vec<Sig> = sigs(&reg, p.dest, now)
            .into_iter()
            .filter(|s| s.k162)
            .collect();
        assert_eq!(far.len(), 1);
        assert!(far[0].k162);
        assert_eq!(far[0].type_id, K162);
        assert_eq!(
            Some(far[0].dungeon_id),
            reg.ext::<Wormholes>().prep.dungeon_of(K162)
        );
        assert_eq!(far[0].other_class, CLASS_NULL);
        let home = sigs(&reg, NULL, now);
        assert_eq!(home.len(), 1, "the K162 is not in the home system");
        assert_ne!(home[0].site_id, far[0].site_id);
    }

    /// **The plan's test**: the `wh` delta round-trips (a fresh registry
    /// on the same rows resolves the same pair) and expires: past
    /// `expires_at` the pair is the next generation with full mass and no
    /// K162, and the rollover drops the row.
    #[test]
    fn the_wh_delta_round_trips_and_expires() {
        let mut reg = world();
        let now = reg.world().birth;
        let before = pair(&reg, NULL, 0, now).unwrap();
        debit(&mut reg, NULL, 0, 1.0, now).unwrap();
        let rows = reg.deltas().of(KIND).clone();
        assert_eq!(rows.len(), 1);
        let mut again = world();
        for (k, v) in &rows {
            again.deltas_mut().put(KIND, k.clone(), v.clone());
        }
        let a = pair(&reg, NULL, 0, now).unwrap();
        assert_eq!(pair(&again, NULL, 0, now), Some(a));
        assert_eq!(a.mass_left, before.mass_left - 1.0);
        assert_eq!((a.generation, a.dest), (before.generation, before.dest));

        let later = pair(&reg, NULL, 0, a.expires_at).unwrap();
        assert_eq!(later.generation, a.generation + 1);
        assert!(!later.has_k162());
        assert!(sigs(&reg, a.dest, a.expires_at).iter().all(|s| !s.k162));

        let day = (1..)
            .find(|d| seed::day_start(reg.world().birth, *d) >= a.expires_at)
            .unwrap();
        assert_eq!(on_rollover(&mut reg, day - 1), 0, "still live");
        assert_eq!(on_rollover(&mut reg, day), 1);
        assert!(reg.deltas().of(KIND).is_empty());
    }

    /// A wormhole is a **local** ball: a pilot who reaches it gets its room
    /// through `site::instantiate`, the hole is in `grid::local_balls`, its
    /// slim carries the six R-WH §3 keys, and the signature object none.
    #[test]
    fn a_reached_hole_is_a_local_ball_with_the_wormhole_slim() {
        let mut reg = world();
        let now = reg.world().birth;
        let s = sigs(&reg, NULL, now).remove(0);
        pilot_at(&mut reg, NULL, s.anchor);
        tick(&mut reg, Instant::now(), now);
        let inst = reg
            .ext::<crate::site::Sites>()
            .get(s.site_id)
            .expect("instantiated")
            .clone();
        let ball = inst
            .objects
            .iter()
            .find(|p| p.type_id == s.type_id)
            .expect("the hole object");
        let local = crate::grid::local_balls(&reg, NULL);
        assert!(local.iter().any(|(id, _)| *id == ball.item_id));
        let ids: Vec<i64> = inst.objects.iter().map(|p| p.item_id).collect();
        let extras = slim_extras(&reg, &ids);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, ball.item_id);
        let keys: Vec<&str> = extras[0].1.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "wormholeSize",
                "wormholeAge",
                "maxShipJumpMass",
                "nebulaType",
                "otherSolarSystemClass",
                "isDestTriglavian"
            ]
        );
        assert_eq!(hole(&reg, s.site_id).map(|h| h.pair.home), Some(NULL));
    }
}
