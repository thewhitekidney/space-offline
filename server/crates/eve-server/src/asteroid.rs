//! Rocks in asteroid belts: where they are, what they are, and their balls.
//!
//! Spec: `docs/lanes/asteroids.md`. Owned by the P3.1 lane.
//!
//! **A belt with no rocks in it is a belt the client calls "fully depleted"**,
//! because it is: the client reads the belt's composition out of its own
//! `asteroidcountbytypeidbeltid.static` and compares it with the harvestables
//! *in the ballpark* (`mining_util.get_depletion_status`) **[V]**. Until this
//! module a pilot could warp to a belt, see the brackets of four belts, and
//! find nothing to lock, nothing to mine, and a banner calling a fresh belt
//! empty.
//!
//! ## What is the client's, and what is ours
//!
//! - **Which rocks, and how many of each ore** -- the client's, exactly
//!   **[V-data]**. `tools/asteroids.py` copies the count table into
//!   `out/belt-rocks-<system>.tsv` (no client data in the repository; the
//!   same arrangement as `crate::celestial`). A belt holds 1..186 rocks.
//! - **How big a rock is for how much ore** -- the client's own law,
//!   `ballparkCommon/ballRadius.py:AsteroidQuantityToRadius` **[V]**, with
//!   each ore type's own three attributes, which the extract carries per row
//!   because they differ by ore (unit size 90 m for Veldspar, 600 m for
//!   others) **[V-data]**. See [`Law`].
//! - **Where each rock sits** -- ours, and **[A]** throughout. The client has
//!   counts, not positions. [`rocks_for_belt`] is a deterministic layout in a
//!   flattened ellipsoid whose centre is offset from the belt point (the
//!   ore-anomaly rooms' shape), clear of where a warp to the belt lands.
//!   How much ore a rock holds is drawn from the quantities CCP authored for
//!   its ore group in the ore-anomaly rooms ([`Quantities`]).
//! - **Item ids** -- the client's own asteroid range, `minUniverseAsteroid`
//!   ..`maxUniverseAsteroid` = 70,000,000..79,999,999 **[V]**
//!   (`inventorycommon/const.py:73-74`), whose only reader
//!   (`idCheckers.IsUniverseAsteroid`) has no caller. The celestial range is
//!   the one to avoid: `ProcessBallAdd` primes any id in 40M..50M through
//!   `cfg.evelocations`, a round-trip for an id with no location row.
//!
//! ## Why the client will draw them
//!
//! `Asteroid._GetGraphicID` raises `RuntimeError` for an ore with neither
//! group graphics nor a type graphic, and every ore type has no type graphic
//! **[V-data]**. So the rock renders only if `groupgraphics.fsdbinary` has
//! graphics for its group. **It does, for all 36 ore types in the count
//! table** **[V-data]** (`out/asteroids/gg_ore_probe.py`): each group's row
//! carries four `graphicIDs`, every one a key of `graphicids.fsdbinary`, and
//! a `categoryAttribute` of `'radius'` with Small/Medium/Large bands (for
//! Veldspar 0-600 m, 600-3,537 m and above). Which record slot is which field
//! is **[I]** -- the native loader is undocumented -- but the names, the four
//! ids and the bands all line up.
//!
//! ## What the rest of the server needs from here
//!
//! A [`Field`] held beside the celestial catalogue, read by three callers:
//! `destiny::bootstrap_for` (a ball and a slim per rock in the system),
//! `targeting::lock` (a rock is lockable), and `Registry::ball_target` (so
//! approach, orbit and warp resolve a rock; [`Rock::target`]).
//!
//! And written by one: `crate::mining`, through [`Field::mine`]. A cycle
//! takes ore out, the rock's radius follows its remaining quantity by its own
//! [`Law`], and a rock with nothing left leaves the field.
//!
//! ## Lazy rocks and regrowth (CP14', C7 L4, C3 §7.3)
//!
//! - **A served field is an offset index, not rocks.** [`Index::build`]
//!   reads the extract once at boot and keeps, per system, the byte ranges
//!   of its rows, plus each belt's centre and static count. A system's rocks
//!   are laid out only while it is warm or hot ([`sync`], a world-clock stage
//!   right after `liveness`), and dropped when it goes cold. **A cold system
//!   holds no rock**, so the universe costs its index, not 2.1M rocks.
//! - **The `Field` read API is unchanged**: `get`, `in_system`, `iter`,
//!   `mine`. Every reader sees the materialised systems, which are the only
//!   ones a pilot can be in. A field built by [`Field::generate`] or
//!   [`Field::from_rocks`] is eager, as before, and never syncs.
//! - **Depletion is a `rock` delta** ([`KIND`], ruling 4): key `(belt itemID,
//!   n)`, the rock's ordinal in that belt, and value `type_id x y z
//!   remaining`, with `0` for a gone rock. [`sync`] captures every bite into
//!   it, and a materialisation applies it, so a restart keeps a gone rock
//!   gone. A delta whose type or position disagrees with the generated rock
//!   is **dropped**, and the rock comes back whole.
//! - **Downtime reset** ([`on_rollover`], owner decision 2026-11-03): at the
//!   daily rollover every belt's rocks come back whole, at their original id
//!   and position. Every `rock` delta is dropped, the mined-out and the
//!   partly mined alike, and a laid-out system's rocks are put back whole.
//!   This replaces the old 25%-of-gone-rocks rule (C3 rule R5).

use crate::delta::{Deltas, Rows};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The delta kind this module owns (`delta_kinds::KINDS`): `belt_item_id n
/// -> type_id x y z remaining` (ruling 4).
pub const KIND: &str = "rock";

/// How far, in metres, a delta's stored position may sit from the generated
/// rock's before the delta is taken to describe another rock and dropped.
///
/// **[A]**: positions are written with Rust's round-trip float format, so an
/// unchanged layout matches exactly; this only absorbs a reformatted file.
/// Falsified by a layout change smaller than this that keeps the type.
pub const POSITION_TOLERANCE: f64 = 1.0;

/// `minUniverseAsteroid` **[V]**. The first rock id.
pub const MIN_ROCK_ID: i64 = 70_000_000;

/// One past `maxUniverseAsteroid` (79,999,999) **[V]**.
pub const END_ROCK_ID: i64 = 80_000_000;

/// Ids reserved per belt: `id = MIN_ROCK_ID + ordinal * IDS_PER_BELT + n`.
///
/// The largest belt in this build holds 186 rocks **[V-data]**, and 40,928
/// belts × 200 ends at 78,185,600, inside the range. A belt with more than
/// this many rocks keeps the first 200 and drops the rest, rather than
/// spilling into the next belt's ids.
pub const IDS_PER_BELT: i64 = 200;

/// `ASTEROID_EXP_SCALE`, `ballRadius.py` **[V]**.
pub const ASTEROID_EXP_SCALE: f64 = 4e-5;

/// A rock's signature radius, for the lock timer.
///
/// Attribute 552's `defaultValue`, which every ore type falls back to because
/// none carries the attribute **[V-data]**. The client reads the same number
/// for its own progress bar (`docs/lanes/asteroids.md` §4.2).
pub const SIGNATURE_RADIUS: f64 = 100.0;

/// The fewest authored quantities an ore group needs before a rock's
/// quantity is drawn from them. **[A]**
///
/// Below this the pool is too thin to be a distribution (a fixture, or a
/// group the ore-anomaly rooms barely use) and the radius law's own span
/// sizes the rock ([`FALLBACK_SPAN`]). Falsified by a real group whose pool
/// is thinner than this and still wanted.
pub const MIN_POOL: usize = 20;

/// The fallback pick, in unit sizes: a radius log-uniform in `[unit_size,
/// min(max_radius, FALLBACK_SPAN x unit_size)]`. **[A]** Only a group with no
/// pool reaches it. Falsified by a TQ rock of such an ore far outside it.
pub const FALLBACK_SPAN: f64 = 6.0;

/// How far the field's centre lies from the belt point, as a fraction of
/// [`outer_radius`]. **[A]** The ore-anomaly rooms' centroid sits 10-29 km
/// (median 19 km) from the warp-in. Falsified by a TQ belt where a pilot
/// lands in the middle of a ring.
pub const FIELD_OFFSET_FRACTION: f64 = 0.4;

/// The field's vertical semi-axis, as a fraction of [`outer_radius`]. **[A]**
/// The rooms' vertical/horizontal spread is 0.32-0.79 (median 0.51).
/// Falsified by a TQ belt visibly thicker or flatter than this.
pub const FIELD_THICKNESS: f64 = 0.4;

/// No rock surface is nearer the belt centre than this. **[A]**
///
/// A warp to a belt lands `WARP_STANDOFF` (1 km) from the centre, since a
/// belt has radius 0 and no warp-in point. 10 km keeps every rock surface at
/// least 9 km from the landing point, so a ship never arrives inside a model.
pub const INNER_RADIUS: f64 = 10_000.0;

/// The field's outer (horizontal) radius: `max(30 km, 5 km × √rocks)`. **[A]**
pub fn outer_radius(rocks: usize) -> f64 {
    (5_000.0 * (rocks as f64).sqrt()).max(30_000.0)
}

/// Half the old ring's thickness, kept for `moon_belt`. **[A]**
pub const HALF_HEIGHT: f64 = 8_000.0;

/// The least surface-to-surface gap between two rocks. **[A]**
pub const GAP: f64 = 1_000.0;

/// Placement attempts per rock before the field grows by 10 %. **[A]**
const TRIES: usize = 64;

/// **How much ore a belt rock holds**: the quantities CCP authored for each
/// ore group in the ore-anomaly rooms **[V-data]**. In a dungeon object the
/// `radius` of an asteroid is its QUANTITY, not a ball radius
/// (`dungeonHelper.py` `SetObjectQuantity`/`GetObjectQuantity` store and read
/// `dunRadius` as the quantity; `GetObjectRadius` converts it with
/// `ComputeRadiusFromQuantity`) **[V]**.
///
/// **[A]**: a belt rock has the quantity distribution of its group in those
/// rooms. Falsified by a TQ survey of a belt rock far outside it (null
/// Arkonor of 50k+ units).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quantities {
    /// Ore group to the sorted, rounded quantities of its rocks.
    pub by_group: BTreeMap<i64, Vec<i64>>,
    /// Every category-25 (asteroid) type to its group.
    pub group_of: BTreeMap<i64, i64>,
}

/// The asteroid category.
const ASTEROID_CATEGORY: i64 = 25;

/// The archetype tag of the ore-anomaly rooms, read by tag not by number.
const ORE_ANOMALY_TAG: &str = "feature_ore_anomalies";

impl Quantities {
    /// From the loaded content; empty when it lacks either table.
    pub fn from_content(content: &crate::content::Content) -> Quantities {
        let (Some(dungeons), Some(types)) = (
            content.try_get::<crate::dungeon_data::Table>(),
            content.try_get::<crate::typedata::Table>(),
        ) else {
            return Quantities::default();
        };
        let q = Quantities::from_tables(dungeons, types);
        let rocks: usize = q.by_group.values().map(Vec::len).sum();
        println!(
            "[asteroid] rock quantities: {rocks} anomaly rocks in {} ore groups",
            q.by_group.len()
        );
        q
    }

    /// Walk the dungeons whose archetype carries the ore-anomaly tag; keep
    /// the objects that are asteroid types with a quantity of at least 1.
    pub fn from_tables(
        dungeons: &crate::dungeon_data::Table,
        types: &crate::typedata::Table,
    ) -> Quantities {
        let mut out = Quantities::default();
        for t in types.types().filter(|t| t.category_id == ASTEROID_CATEGORY) {
            out.group_of.insert(t.type_id, t.group_id);
        }
        for d in dungeons.dungeons.values() {
            let tagged = dungeons
                .archetypes
                .get(&d.archetype)
                .is_some_and(|a| a.content_tags.iter().any(|t| t == ORE_ANOMALY_TAG));
            if !tagged {
                continue;
            }
            for o in &d.objects {
                let (Some(group), Some(r)) = (out.group_of.get(&o.type_id).copied(), o.radius)
                else {
                    continue;
                };
                if r >= 1.0 {
                    out.by_group
                        .entry(group)
                        .or_default()
                        .push(r.round() as i64);
                }
            }
        }
        for pool in out.by_group.values_mut() {
            pool.sort_unstable();
        }
        out
    }

    /// One rock's quantity: from the group's pool when it has [`MIN_POOL`]
    /// quantities, else a radius log-uniform over [`FALLBACK_SPAN`] unit
    /// sizes, floored to a whole quantity.
    pub(crate) fn draw(&self, type_id: i64, law: &Law, rng: &mut Mix) -> i64 {
        let pool = self
            .group_of
            .get(&type_id)
            .and_then(|g| self.by_group.get(g))
            .filter(|p| p.len() >= MIN_POOL);
        match pool {
            Some(p) => p[((rng.unit() * p.len() as f64) as usize).min(p.len() - 1)],
            None => {
                let lo = law.unit_size;
                let hi = law.max_radius.min(FALLBACK_SPAN * law.unit_size).max(lo);
                law.snap(lo * (hi / lo).powf(rng.unit())).0
            }
        }
    }
}

/// One ore type's radius law: the three attributes `ballRadius.py` reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Law {
    /// `asteroidRadiusGrowthFactor` (1980).
    pub growth: f64,
    /// `asteroidRadiusUnitSize` (1981).
    pub unit_size: f64,
    /// `asteroidMaxRadius` (2727).
    pub max_radius: f64,
}

impl Law {
    /// `AsteroidQuantityToRadius` **[V]**, the same expression in the same
    /// order:
    ///
    /// ```text
    /// min(maxRadius, unitSize * exp(4e-5 * (max(1, q) - 1) * growthFactor))
    /// ```
    pub fn radius(&self, quantity: i64) -> f64 {
        if self.is_cloud() {
            // `ComputeRadiusFromQuantity` for `groupHarvestableCloud`
            // **[V]**: `quantity * GetRadius(type) / 10`; a negative
            // quantity reads as 1, zero stays zero.
            let q = if quantity < 0 { 1 } else { quantity };
            return q as f64 * self.unit_size;
        }
        let q = quantity.max(1) as f64;
        let r = self.unit_size * (ASTEROID_EXP_SCALE * (q - 1.0) * self.growth).exp();
        r.min(self.max_radius)
    }

    /// `AsteroidRadiusToQuantity` **[V]**, not rounded:
    /// `1 + ln(radius / unitSize) / 4e-5 / growthFactor`.
    pub fn quantity(&self, radius: f64) -> f64 {
        if self.is_cloud() {
            // `ComputeQuantityFromRadius`, cloud branch **[V]**.
            return radius / self.unit_size;
        }
        1.0 + (radius / self.unit_size).ln() * (1.0 / ASTEROID_EXP_SCALE / self.growth)
    }

    /// The whole quantity for a wanted radius, and the radius it really
    /// gives.
    ///
    /// **The radius sent is derived from the quantity, never the other way
    /// round**, so that what the pilot sees and what a later mining packet
    /// takes out of the rock agree under the client's own law.
    pub fn snap(&self, wanted: f64) -> (i64, f64) {
        if self.is_cloud() {
            let q = self.quantity(wanted).floor().max(1.0) as i64;
            return (q, self.radius(q));
        }
        let q = self.quantity(wanted.min(self.max_radius)).floor().max(1.0) as i64;
        (q, self.radius(q))
    }

    /// **The linear cloud law** of a harvestable cloud (group 711), as
    /// the client computes it (`ballparkCommon/ballRadius.py:16-17,33-35`
    /// **[V]**): `radius = quantity x GetRadius(type) / 10`, no cap.
    /// Encoded as `growth == 0` (an ore law's growth is always > 0), with
    /// `unit_size` the metres one unit adds (`type_radius / 10`).
    pub fn cloud(type_radius: f64) -> Law {
        Law {
            growth: 0.0,
            unit_size: type_radius / 10.0,
            max_radius: f64::INFINITY,
        }
    }

    /// Whether this is the linear cloud law ([`Law::cloud`]).
    pub fn is_cloud(&self) -> bool {
        self.growth == 0.0
    }

    pub(crate) fn is_usable(&self) -> bool {
        if self.is_cloud() {
            return self.unit_size.is_finite() && self.unit_size > 0.0;
        }
        [self.growth, self.unit_size, self.max_radius]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
    }
}

/// One row of the extract: so many rocks of one ore in one belt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub belt_id: i64,
    /// The belt's index in the client's whole count table. Decides the ids.
    pub ordinal: i64,
    pub type_id: i64,
    pub count: i64,
    pub solar_system_id: i64,
    pub law: Law,
}

/// One line of the extract, or `None` for a comment, a blank or a bad line
/// (see [`Counts::parse`]).
fn parse_line(line: &str) -> Option<Row> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 8 {
        return None;
    }
    let ints: Vec<Option<i64>> = f[..5].iter().map(|s| s.parse().ok()).collect();
    let reals: Vec<Option<f64>> = f[5..8].iter().map(|s| s.parse().ok()).collect();
    let [
        Some(belt_id),
        Some(ordinal),
        Some(type_id),
        Some(count),
        Some(system),
    ] = ints[..]
    else {
        return None;
    };
    let [Some(growth), Some(unit_size), Some(max_radius)] = reals[..] else {
        return None;
    };
    let law = Law {
        growth,
        unit_size,
        max_radius,
    };
    if count <= 0 || ordinal < 0 || law.is_cloud() || !law.is_usable() {
        return None;
    }
    // An ordinal past the range would mint ids outside it.
    if MIN_ROCK_ID + (ordinal + 1) * IDS_PER_BELT > END_ROCK_ID {
        return None;
    }
    Some(Row {
        belt_id,
        ordinal,
        type_id,
        count,
        solar_system_id: system,
        law,
    })
}

/// The extract `tools/asteroids.py` writes, parsed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counts(Vec<Row>);

impl Counts {
    pub fn load(path: &std::path::Path) -> std::io::Result<Counts> {
        Ok(Counts::parse(&std::fs::read_to_string(path)?))
    }

    /// **A bad line is skipped, not fatal**, as in `celestial::Catalogue`:
    /// rocks are a generated convenience, and one malformed row should cost
    /// that row. Columns: `belt_id ordinal type_id count system growth
    /// unit_size max_radius`; `#` starts a comment.
    pub fn parse(text: &str) -> Counts {
        Counts(text.lines().filter_map(parse_line).collect())
    }

    pub fn rows(&self) -> &[Row] {
        &self.0
    }
}

/// One belt, as the index keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Belt {
    solar_system_id: i64,
    /// The catalogue's position for it: the centre of its ring.
    centre: [f64; 3],
    /// Its rocks when whole: the file's counts, capped at [`IDS_PER_BELT`]
    /// as [`rocks_for_belt`] caps them.
    total: i64,
}

/// **The offset index**: where each system's rows sit in the extract, and
/// each belt's centre and static count. Built once at boot; a system's rocks
/// are read back from the file only when it materialises.
///
/// About 4,000 systems and 41,000 belts on the bundle, against 2.1M rocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Index {
    path: PathBuf,
    /// System id to the byte ranges `[start, end)` of its rows, in file
    /// order. Adjacent rows share a range, so a system the file keeps
    /// together is one read.
    systems: BTreeMap<i64, Vec<(u64, u64)>>,
    belts: BTreeMap<i64, Belt>,
    quantities: Arc<Quantities>,
}

impl Index {
    /// Index every row whose belt **the catalogue has as a belt**, the same
    /// filter [`Field::generate`] applies: a belt with no position has no
    /// rocks. A bad line costs itself, as in [`Counts::parse`].
    pub fn build(
        path: &Path,
        celestials: &crate::celestial::Catalogue,
        quantities: &Quantities,
    ) -> std::io::Result<Index> {
        let bytes = std::fs::read(path)?;
        let mut index = Index {
            path: path.to_owned(),
            quantities: Arc::new(quantities.clone()),
            ..Index::default()
        };
        let mut at = 0usize;
        while at < bytes.len() {
            let end = bytes[at..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| at + i + 1);
            let row = std::str::from_utf8(&bytes[at..end])
                .ok()
                .and_then(parse_line);
            if let Some(row) = row
                && let Some(belt) = celestials
                    .get(row.belt_id)
                    .filter(|c| c.kind == crate::celestial::Kind::Belt)
            {
                index
                    .belts
                    .entry(row.belt_id)
                    .or_insert(Belt {
                        solar_system_id: row.solar_system_id,
                        centre: belt.position,
                        total: 0,
                    })
                    .total += row.count;
                let ranges = index.systems.entry(row.solar_system_id).or_default();
                match ranges.last_mut() {
                    Some(last) if last.1 == at as u64 => last.1 = end as u64,
                    _ => ranges.push((at as u64, end as u64)),
                }
            }
            at = end;
        }
        for b in index.belts.values_mut() {
            b.total = b.total.min(IDS_PER_BELT);
        }
        Ok(index)
    }

    /// One system's rows, read back from the file by their ranges: exactly
    /// the rows of `Counts::parse(file)` in this system whose belt the
    /// catalogue has, in file order.
    pub fn rows_of(&self, solar_system_id: i64) -> std::io::Result<Vec<Row>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(ranges) = self.systems.get(&solar_system_id) else {
            return Ok(Vec::new());
        };
        let mut file = std::fs::File::open(&self.path)?;
        let mut out = Vec::new();
        for &(start, end) in ranges {
            let mut buf = vec![0; (end - start) as usize];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut buf)?;
            // A row that is not where the index put it (the file changed
            // under a running server) is skipped, never laid out elsewhere.
            out.extend(
                String::from_utf8_lossy(&buf)
                    .lines()
                    .filter_map(parse_line)
                    .filter(|r| {
                        r.solar_system_id == solar_system_id && self.belts.contains_key(&r.belt_id)
                    }),
            );
        }
        Ok(out)
    }

    /// One system's rocks, whole: what [`Field::generate`] lays out there.
    fn generate(&self, rows: &[Row]) -> Vec<Rock> {
        let mut by_belt: BTreeMap<i64, Vec<Row>> = BTreeMap::new();
        for r in rows {
            by_belt.entry(r.belt_id).or_default().push(*r);
        }
        let mut out = Vec::new();
        for (belt_id, rows) in by_belt {
            if let Some(belt) = self.belts.get(&belt_id) {
                out.extend(rocks_for_belt(
                    belt_id,
                    belt.centre,
                    &rows,
                    &self.quantities,
                ));
            }
        }
        out
    }

    /// Every system with at least one indexed row, ascending.
    pub fn systems(&self) -> impl Iterator<Item = i64> + '_ {
        self.systems.keys().copied()
    }

    /// The rocks the whole universe would hold, every belt whole.
    pub fn static_len(&self) -> i64 {
        self.belts.values().map(|b| b.total).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.belts.is_empty()
    }
}

/// A rock's `n`: its ordinal within its belt, from its id.
pub fn ordinal_in_belt(rock_id: i64) -> i64 {
    (rock_id - MIN_ROCK_ID).rem_euclid(IDS_PER_BELT)
}

/// **One offline bite out of a rock in a system that is not laid out** (E1,
/// `crate::econ_agents`): nobody is there, so the field holds no ball to
/// shrink and no pilot to tell, and the bite is exactly what [`sync`]'s
/// capture would have written for it -- the rock's `rock` delta, as it is
/// after the bite -- so the next materialisation lays it out smaller, or not
/// at all. Answers the units taken (at most the rock held).
pub fn settle_bite_cold(w: &mut Registry, rock: &Rock, units: i64) -> i64 {
    let taken = units.clamp(0, rock.quantity);
    if taken == 0 {
        return 0;
    }
    let left = rock.quantity - taken;
    let after = Rock {
        quantity: left,
        radius: if left > 0 {
            rock.law.radius(left)
        } else {
            rock.radius
        },
        ..*rock
    };
    let (k, v) = delta_row(&after);
    w.deltas_mut().put(KIND, k, v);
    w.mark_dirty();
    taken
}

/// The `rock` delta for a rock as it is now: key `(belt, n)`, value `type_id
/// x y z remaining` (ruling 4).
pub fn delta_row(r: &Rock) -> (Vec<String>, Vec<String>) {
    (
        vec![r.belt_id.to_string(), ordinal_in_belt(r.id).to_string()],
        vec![
            r.type_id.to_string(),
            r.position[0].to_string(),
            r.position[1].to_string(),
            r.position[2].to_string(),
            r.quantity.to_string(),
        ],
    )
}

/// A generated rock with its delta applied: `Ok(Some)` with the remainder,
/// `Ok(None)` for a gone rock, `Err` for a delta that does not describe this
/// rock (another type, another place, a remainder it never had, or a value
/// that does not parse), which is dropped.
fn apply_delta(rock: Rock, value: &[String]) -> Result<Option<Rock>, ()> {
    let [t, x, y, z, left] = value else {
        return Err(());
    };
    let int = |s: &String| s.parse::<i64>().map_err(|_| ());
    let real = |s: &String| s.parse::<f64>().map_err(|_| ());
    let (type_id, left) = (int(t)?, int(left)?);
    let at = [real(x)?, real(y)?, real(z)?];
    if type_id != rock.type_id
        || crate::space_state::distance(at, rock.position).is_nan()
        || crate::space_state::distance(at, rock.position) > POSITION_TOLERANCE
        || left >= rock.quantity
    {
        return Err(());
    }
    if left <= 0 {
        return Ok(None);
    }
    Ok(Some(Rock {
        quantity: left,
        radius: rock.law.radius(left),
        ..rock
    }))
}

/// One system's materialisation, worked out from `&self` so the caller can
/// read the deltas beside it ([`Field::plan`], then [`Field::install`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub solar_system_id: i64,
    /// The system's rocks, deltas applied; a gone rock is absent.
    pub rocks: Vec<Rock>,
    /// Delta keys that describe no rock here (ruling 4): the caller deletes
    /// them.
    pub stale: Vec<Vec<String>>,
}

/// One rock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rock {
    pub id: i64,
    /// The ore type, which is also the slim item's `typeID`: category 25,
    /// from which the client derives everything else **[V]**.
    pub type_id: i64,
    pub belt_id: i64,
    pub solar_system_id: i64,
    /// Units of ore, as the client's law reads this rock's radius.
    pub quantity: i64,
    /// Metres. `law.radius(quantity)` exactly.
    pub radius: f64,
    pub position: [f64; 3],
    /// This ore's radius law, kept so a mined rock can be re-sized by the
    /// same three numbers that sized it.
    pub law: Law,
}

/// What one bite out of a rock did. See [`Field::mine`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mined {
    pub rock_id: i64,
    pub type_id: i64,
    pub solar_system_id: i64,
    /// Units actually taken: never more than were asked for, nor than the
    /// rock held.
    pub taken: i64,
    /// Units left. Zero means the rock is gone from the field.
    pub left: i64,
    pub radius_before: f64,
    /// `law.radius(left)` while anything is left; the old radius otherwise,
    /// since a removed ball has no size to change to.
    pub radius: f64,
}

impl Mined {
    pub fn depleted(&self) -> bool {
        self.left == 0
    }

    /// Whether the radius the client was sent is now wrong. Compared as
    /// `f32`, which is what the ball carries on the wire: a change smaller
    /// than that is not a change the client could draw.
    pub fn shrank(&self) -> bool {
        !self.depleted() && (self.radius as f32) != (self.radius_before as f32)
    }
}

impl Rock {
    /// Where a warp, an approach or an orbit aims, for `Registry::ball_target`.
    ///
    /// No warp-in point: `HasWarpInPoint` is for celestials of 90 km and up,
    /// and a rock is neither. A warp aims at the rock and the standoff
    /// clears its surface, exactly as for a ship.
    pub fn target(&self) -> crate::model::Target {
        crate::model::Target {
            position: self.position,
            radius: self.radius,
            warp_in: None,
        }
    }
}

/// Every rock the server knows, by id, and which of them each system holds.
///
/// Empty is the ordinary case: a server started without `--belt-rocks` has
/// none, every lookup misses, and the bootstrap is byte-identical to before
/// this module existed.
///
/// **`by_system` is a function of `rocks`** and is never written on its own:
/// [`Field::new`] builds it, and every other path that adds or removes a rock
/// ([`Field::mine`], [`Field::install`], [`Field::drop_system`],
/// [`Field::restore`]) keeps it in step. It exists because the content
/// bundle covers every belt in the universe, and [`Field::in_system`], which
/// every ballpark bind and every scan calls, used to filter all of them to
/// find one system's (`docs/review/V3-runtime.md` §2.2).
///
/// **A lazy field** (from [`Field::lazy`]) holds only the systems that are
/// materialised; see the module header.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Field {
    rocks: BTreeMap<i64, Rock>,
    /// System id to its rocks' ids, ascending: the order `rocks` iterates
    /// in, so `in_system` yields the same rows in the same order as the
    /// filter it replaced.
    by_system: BTreeMap<i64, Vec<i64>>,
    /// `None` for an eager field.
    lazy: Option<Lazy>,
}

/// What a lazy field adds: its index, which systems are laid out, and the
/// bites not yet written to the deltas.
#[derive(Debug, Clone, PartialEq)]
struct Lazy {
    index: Arc<Index>,
    materialised: BTreeSet<i64>,
    /// Rock id to the rock after its last bite (quantity 0: gone), until
    /// [`sync`] writes it as a `rock` delta.
    changed: BTreeMap<i64, Rock>,
}

impl Field {
    /// Lay out every belt the extract names **and the catalogue has as a
    /// belt**. A belt the catalogue lacks has no position, so no rocks: the
    /// system filter is whatever `--celestials` already serves.
    pub fn generate(
        counts: &Counts,
        celestials: &crate::celestial::Catalogue,
        quantities: &Quantities,
    ) -> Field {
        let mut by_belt: BTreeMap<i64, Vec<Row>> = BTreeMap::new();
        for r in counts.rows() {
            by_belt.entry(r.belt_id).or_default().push(*r);
        }
        let mut rocks = BTreeMap::new();
        for (belt_id, rows) in by_belt {
            let Some(belt) = celestials
                .get(belt_id)
                .filter(|c| c.kind == crate::celestial::Kind::Belt)
            else {
                continue;
            };
            for rock in rocks_for_belt(belt_id, belt.position, &rows, quantities) {
                rocks.insert(rock.id, rock);
            }
        }
        Field::new(rocks)
    }

    /// A field of exactly these rocks, keyed by their ids. A later rock with
    /// an id already seen replaces the earlier one, as a map insert does.
    pub fn from_rocks(rocks: impl IntoIterator<Item = Rock>) -> Field {
        Field::new(rocks.into_iter().map(|r| (r.id, r)).collect())
    }

    /// An empty field over an index: no system is laid out until [`sync`]
    /// (or [`Field::plan`] and [`Field::install`]) materialises it.
    pub fn lazy(index: Index) -> Field {
        Field {
            lazy: Some(Lazy {
                index: Arc::new(index),
                materialised: BTreeSet::new(),
                changed: BTreeMap::new(),
            }),
            ..Field::default()
        }
    }

    /// The eager constructor: the map, and the index built from it.
    /// Iterating the map in id order pushes each system's ids already sorted.
    fn new(rocks: BTreeMap<i64, Rock>) -> Field {
        let mut by_system: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for r in rocks.values() {
            by_system.entry(r.solar_system_id).or_default().push(r.id);
        }
        Field {
            rocks,
            by_system,
            lazy: None,
        }
    }

    /// Whether this field materialises per system.
    pub fn is_lazy(&self) -> bool {
        self.lazy.is_some()
    }

    /// Whether a system's rocks are laid out. Always true for an eager
    /// field, which lays out everything at once.
    pub fn is_materialised(&self, solar_system_id: i64) -> bool {
        self.lazy
            .as_ref()
            .is_none_or(|l| l.materialised.contains(&solar_system_id))
    }

    /// The rocks the field holds with every belt whole: the index's count
    /// for a lazy field, the rocks themselves for an eager one.
    pub fn static_len(&self) -> i64 {
        self.lazy
            .as_ref()
            .map_or(self.rocks.len() as i64, |l| l.index.static_len())
    }

    /// **Work out one system's materialisation**: lay out its belts from
    /// the file, then apply each `rock` delta of those belts. A delta whose
    /// key is not `(belt, n)` of a rock laid out here, or whose type or
    /// position disagrees, is stale (ruling 4): the rock stays whole and the
    /// caller deletes the delta. An eager field plans nothing.
    ///
    /// An unreadable file is logged and gives the system no rocks: rocks
    /// are a generated convenience, and the server keeps serving.
    pub fn plan(&self, solar_system_id: i64, deltas: &Deltas) -> Plan {
        let mut plan = Plan {
            solar_system_id,
            rocks: Vec::new(),
            stale: Vec::new(),
        };
        let Some(lazy) = &self.lazy else {
            return plan;
        };
        let rows = lazy.index.rows_of(solar_system_id).unwrap_or_else(|e| {
            crate::outln!(
                "[asteroid] {}: {e}; system {solar_system_id} gets no rocks",
                lazy.index.path.display()
            );
            Vec::new()
        });
        let generated = lazy.index.generate(&rows);
        let belts: BTreeSet<i64> = generated.iter().map(|r| r.belt_id).collect();
        // `(belt, n)` to the delta row that names it.
        type Found<'a> = BTreeMap<(i64, i64), (&'a Vec<String>, &'a Vec<String>)>;
        let mut found: Found = BTreeMap::new();
        let all = deltas.of(KIND);
        for belt in belts {
            let b = belt.to_string();
            for (k, v) in all
                .range(vec![b.clone()]..)
                .take_while(|(k, _)| k.first() == Some(&b))
            {
                match k.get(1).and_then(|n| n.parse::<i64>().ok()) {
                    Some(n) if k.len() == 2 => {
                        found.insert((belt, n), (k, v));
                    }
                    _ => plan.stale.push(k.clone()),
                }
            }
        }
        for rock in generated {
            match found.remove(&(rock.belt_id, ordinal_in_belt(rock.id))) {
                None => plan.rocks.push(rock),
                Some((k, v)) => match apply_delta(rock, v) {
                    Ok(Some(left)) => plan.rocks.push(left),
                    Ok(None) => {}
                    Err(()) => {
                        plan.stale.push(k.clone());
                        plan.rocks.push(rock);
                    }
                },
            }
        }
        plan.stale
            .extend(found.into_values().map(|(k, _)| k.clone()));
        plan
    }

    /// Lay out a planned system. A system already laid out, or an eager
    /// field, is left as it is.
    pub fn install(&mut self, plan: Plan) {
        let Some(lazy) = &mut self.lazy else {
            return;
        };
        if !lazy.materialised.insert(plan.solar_system_id) {
            return;
        }
        for r in plan.rocks {
            self.insert(r);
        }
    }

    /// Forget a system's rocks: it went cold. Bites not yet captured stay
    /// captured-to-be, so nothing mined is lost. An eager field keeps its
    /// rocks.
    pub fn drop_system(&mut self, solar_system_id: i64) {
        let Some(lazy) = &mut self.lazy else {
            return;
        };
        lazy.materialised.remove(&solar_system_id);
        for id in self.by_system.remove(&solar_system_id).unwrap_or_default() {
            self.rocks.remove(&id);
        }
    }

    /// The materialised systems, ascending (none for an eager field).
    pub fn materialised(&self) -> impl Iterator<Item = i64> + '_ {
        self.lazy
            .iter()
            .flat_map(|l| l.materialised.iter().copied())
    }

    /// Every rock bitten since the last call, as it is now (quantity 0 for
    /// a gone rock), in id order. [`sync`] writes them as deltas.
    pub fn take_changed(&mut self) -> Vec<Rock> {
        self.lazy
            .as_mut()
            .map(|l| std::mem::take(&mut l.changed).into_values().collect())
            .unwrap_or_default()
    }

    /// **The downtime reset**: the delta keys of every rock row of a belt
    /// the index knows, mined out or partly mined. All of them go at the
    /// rollover. A belt the index lacks resets nothing; an eager field
    /// resets nothing.
    pub fn reset_keys(&self, rows: &Rows) -> Vec<Vec<String>> {
        let Some(lazy) = &self.lazy else {
            return Vec::new();
        };
        rows.keys()
            .filter(|k| {
                k.first()
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_some_and(|belt| lazy.index.belts.contains_key(&belt))
                    && k.get(1).and_then(|s| s.parse::<i64>().ok()).is_some()
            })
            .cloned()
            .collect()
    }

    /// Put regrown rocks back into the materialised systems, whole, at their
    /// original id and position: the rocks [`rocks_for_belt`] lays out at
    /// those `(belt, n)`. A key of a cold system needs nothing here (its
    /// next materialisation lays it out whole); a rock already present (a
    /// partly mined one) is **replaced** by the whole rock, so a belt never
    /// holds more than its static count. Returns the rocks that were not
    /// present, the ones whose balls need announcing; a replaced rock's ball
    /// keeps its old radius in a client until it reloads the grid [A].
    pub fn restore(&mut self, keys: &[Vec<String>]) -> Vec<Rock> {
        let Some(lazy) = &self.lazy else {
            return Vec::new();
        };
        let mut wanted: BTreeMap<i64, BTreeSet<(i64, i64)>> = BTreeMap::new();
        for k in keys {
            let (Some(belt), Some(n)) = (
                k.first().and_then(|s| s.parse::<i64>().ok()),
                k.get(1).and_then(|s| s.parse::<i64>().ok()),
            ) else {
                continue;
            };
            let Some(b) = lazy.index.belts.get(&belt) else {
                continue;
            };
            if lazy.materialised.contains(&b.solar_system_id) {
                wanted
                    .entry(b.solar_system_id)
                    .or_default()
                    .insert((belt, n));
            }
        }
        let index = Arc::clone(&lazy.index);
        let mut back = Vec::new();
        for (system, want) in wanted {
            let rows = index.rows_of(system).unwrap_or_default();
            for rock in index.generate(&rows) {
                if want.contains(&(rock.belt_id, ordinal_in_belt(rock.id))) {
                    let present = self.rocks.contains_key(&rock.id);
                    self.insert(rock);
                    if !present {
                        back.push(rock);
                    }
                }
            }
        }
        back
    }

    /// Add one rock that is not a belt's: a site rock (W14 CP28a,
    /// `crate::site`). A rock already present is left as it is.
    pub fn add(&mut self, rock: Rock) {
        self.insert(rock);
    }

    /// Take one rock out, with its index entry: a site leaving space (W14
    /// CP28a). `None` for an id that is not a rock.
    pub fn remove(&mut self, rock_id: i64) -> Option<Rock> {
        let rock = self.rocks.remove(&rock_id)?;
        self.unindex(rock.solar_system_id, rock_id);
        Some(rock)
    }

    /// Add one rock and its index entry.
    fn insert(&mut self, rock: Rock) {
        if self.rocks.insert(rock.id, rock).is_some() {
            return;
        }
        let ids = self.by_system.entry(rock.solar_system_id).or_default();
        if let Err(at) = ids.binary_search(&rock.id) {
            ids.insert(at, rock.id);
        }
    }

    /// Take up to `want` units out of a rock.
    ///
    /// **The quantity goes down by exactly what is returned as `taken`**, and
    /// the radius is re-derived from what is left by the rock's own law --
    /// never scaled, so a rock mined in a hundred bites is the same size as
    /// one mined in one. A rock left with nothing is removed: the field holds
    /// no empty rocks, so the bootstrap of a pilot who arrives later does not
    /// draw one. `None` for an id that is not a rock (or no longer is).
    pub fn mine(&mut self, rock_id: i64, want: i64) -> Option<Mined> {
        let rock = self.rocks.get_mut(&rock_id)?;
        let taken = want.clamp(0, rock.quantity);
        let radius_before = rock.radius;
        rock.quantity -= taken;
        if rock.quantity > 0 {
            rock.radius = rock.law.radius(rock.quantity);
        }
        let mined = Mined {
            rock_id,
            type_id: rock.type_id,
            solar_system_id: rock.solar_system_id,
            taken,
            left: rock.quantity,
            radius_before,
            radius: rock.radius,
        };
        let after = *rock;
        // A site rock (W14 CP28a) is not persisted: no `rock` delta. Nor is a
        // moon belt rock (MN-C): `moon_belt` writes its own rows.
        if let (Some(lazy), false) = (
            &mut self.lazy,
            crate::site::is_instance_id(rock_id) || crate::moon_belt::is_moon_id(rock_id),
        ) {
            lazy.changed.insert(rock_id, after);
        }
        if mined.depleted() {
            self.rocks.remove(&rock_id);
            self.unindex(mined.solar_system_id, rock_id);
        }
        Some(mined)
    }

    /// Drop one id from its system's list, and the list when it empties, so
    /// a mined-out system compares equal to one generated without rocks.
    fn unindex(&mut self, solar_system_id: i64, rock_id: i64) {
        let Some(ids) = self.by_system.get_mut(&solar_system_id) else {
            return;
        };
        if let Ok(at) = ids.binary_search(&rock_id) {
            ids.remove(at);
        }
        if ids.is_empty() {
            self.by_system.remove(&solar_system_id);
        }
    }

    pub fn get(&self, id: i64) -> Option<&Rock> {
        self.rocks.get(&id)
    }

    /// The rocks in one system, in id order so two bootstraps of the same
    /// world build the same blob.
    ///
    /// Read from the index: the cost is this system's rocks, not the
    /// universe's. Exactly the rows, in exactly the order, of
    /// `iter().filter(|r| r.solar_system_id == solar_system_id)`.
    pub fn in_system(&self, solar_system_id: i64) -> impl Iterator<Item = &Rock> {
        #[cfg(debug_assertions)]
        self.check_index(solar_system_id);
        self.by_system
            .get(&solar_system_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.rocks.get(id))
    }

    /// The debug build's cross-check of the index, in the manner of
    /// `Roster::in_system` (`space_state.rs`).
    ///
    /// **Always**, at the cost of this system's rocks plus one pass over the
    /// systems: the list is strictly ascending, every id is a rock of this
    /// system, and the lists together hold exactly as many ids as the field
    /// has rocks. A rock missing from its own list would have to sit in
    /// another to keep that count, where the query for that system fails.
    ///
    /// **And the full rescan** (the old filter, compared row for row) while
    /// the field is at most `RESCAN_UP_TO` rocks: every test fixture and
    /// sandbox. Not for the content bundle, where it would put back in the
    /// debug build the very cost the index removes. [A] a tuning bound;
    /// falsified by an index bug that only a field larger than it shows.
    #[cfg(debug_assertions)]
    fn check_index(&self, solar_system_id: i64) {
        const RESCAN_UP_TO: usize = 50_000;
        let ids = self
            .by_system
            .get(&solar_system_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let indexed: usize = self.by_system.values().map(Vec::len).sum();
        debug_assert!(
            indexed == self.rocks.len()
                && ids.windows(2).all(|w| w[0] < w[1])
                && ids.iter().all(|id| {
                    self.rocks
                        .get(id)
                        .is_some_and(|r| r.solar_system_id == solar_system_id)
                }),
            "rock index out of step in system {solar_system_id}: a path changed \
             the rocks without going through Field::new or Field::mine"
        );
        if self.rocks.len() <= RESCAN_UP_TO {
            debug_assert!(
                self.rocks
                    .values()
                    .filter(|r| r.solar_system_id == solar_system_id)
                    .map(|r| r.id)
                    .eq(ids.iter().copied()),
                "rock index differs from the filter in system {solar_system_id}"
            );
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Rock> {
        self.rocks.values()
    }

    pub fn len(&self) -> usize {
        self.rocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rocks.is_empty()
    }
}

/// Read `--belt-rocks FILE` and lay it out against the loaded celestials,
/// with an error a player can act on.
///
/// **Fatal when it yields nothing**, as `load_celestials` is: a flag the
/// player typed that silently produced an empty belt would be found out by
/// warping there. The usual cause is a missing `--celestials`, so the
/// message says so.
pub fn load(
    path: &std::path::Path,
    celestials: &crate::celestial::Catalogue,
) -> Result<Field, String> {
    let counts = Counts::load(path).map_err(|e| format!("--belt-rocks {}: {e}", path.display()))?;
    if counts.rows().is_empty() {
        return Err(format!(
            "--belt-rocks {}: no rows. Regenerate it with tools/asteroids.py",
            path.display()
        ));
    }
    let field = Field::generate(&counts, celestials, &Quantities::default());
    if field.is_empty() {
        return Err(format!(
            "--belt-rocks {}: none of its belts is in the celestial catalogue. \
             Pass --celestials for the same system (tools/celestials.py)",
            path.display()
        ));
    }
    Ok(field)
}

/// Index `--belt-rocks FILE` against the loaded celestials: the served,
/// lazy field (CP14'). The same errors as [`load`], for the same reasons.
pub fn index(
    path: &Path,
    celestials: &crate::celestial::Catalogue,
    quantities: &Quantities,
) -> Result<Field, String> {
    let index = Index::build(path, celestials, quantities)
        .map_err(|e| format!("--belt-rocks {}: {e}", path.display()))?;
    if index.is_empty() {
        return Err(format!(
            "--belt-rocks {}: no row of it is a belt in the celestial catalogue. \
             Regenerate it with tools/asteroids.py, and pass --celestials for the \
             same system (tools/celestials.py)",
            path.display()
        ));
    }
    Ok(Field::lazy(index))
}

/// **The world-clock stage** (registered right after `liveness` in
/// `tick::STAGES`): write every bite as a `rock` delta, drop the rocks of
/// every system that went cold, and lay out every system that is warm or hot
/// and not yet laid out. An eager field has nothing to do.
///
/// A system laid out while a pilot is already in space there (one who
/// arrived between ticks) gets its rocks as an `AddBalls2`, like any ball
/// that appears in a running park.
pub fn sync(w: &mut Registry, _now: std::time::Instant, _ft: crate::damage::BlueTime) {
    if !w.rocks().is_lazy() {
        return;
    }
    capture(w);
    let (cold, warm): (Vec<i64>, Vec<i64>) = {
        use crate::liveness::{Liveness, Tier};
        let live = w.ext::<Liveness>();
        let f = w.rocks();
        let cold = f
            .materialised()
            .filter(|s| live.tier(*s) == Tier::Cold)
            .collect();
        let warm = f
            .lazy
            .iter()
            .flat_map(|l| l.index.systems())
            .filter(|s| live.tier(*s) != Tier::Cold && !f.is_materialised(*s))
            .collect();
        (cold, warm)
    };
    for s in cold {
        w.rocks_mut().drop_system(s);
    }
    for s in warm {
        let plan = w.rocks().plan(s, w.deltas());
        if !plan.stale.is_empty() {
            crate::outln!(
                "[asteroid] system {s}: dropped {} rock deltas that match no rock",
                plan.stale.len()
            );
            let d = w.deltas_mut();
            for k in &plan.stale {
                d.del(KIND, k);
            }
        }
        w.rocks_mut().install(plan);
        let rocks: Vec<Rock> = w.rocks().in_system(s).copied().collect();
        announce(w, s, &rocks);
    }
}

/// Write every bite since the last capture as a `rock` delta.
fn capture(w: &mut Registry) {
    let changed = w.rocks_mut().take_changed();
    if changed.is_empty() {
        return;
    }
    let d = w.deltas_mut();
    for r in &changed {
        let (k, v) = delta_row(r);
        d.put(KIND, k, v);
    }
}

/// **The rollover step** (CP25 registers it): every belt resets whole
/// ([`Field::reset_keys`]); delete every rock delta and put the rocks of
/// laid-out systems back, announcing those that were gone. Returns how many
/// deltas were dropped. `_days` is unused: one day or ten, the reset is the
/// same.
pub fn on_rollover(w: &mut Registry, _days: u32) -> usize {
    if !w.rocks().is_lazy() {
        return 0;
    }
    capture(w);
    let keys = w.rocks().reset_keys(w.deltas().of(KIND));
    if keys.is_empty() {
        return 0;
    }
    let d = w.deltas_mut();
    for k in &keys {
        d.del(KIND, k);
    }
    let back = w.rocks_mut().restore(&keys);
    let mut by_system: BTreeMap<i64, Vec<Rock>> = BTreeMap::new();
    for r in back {
        by_system.entry(r.solar_system_id).or_default().push(r);
    }
    for (s, rocks) in by_system {
        announce(w, s, &rocks);
    }
    keys.len()
}

/// Rocks appearing in a running park: one `AddBalls2` addressed to every
/// character in space in the system, on the clock. Nobody there, nothing
/// sent.
///
/// **Each pilot gets their own grid's rocks, and only those** (GRID-B): the
/// batch is cut per viewer as it leaves (`crate::grid::filter`, through
/// `BallsAdded`'s `narrowed`), so a pilot at one belt is sent that belt and
/// a pilot docked-out at the station is sent nothing. One batch here and the
/// cut at the drain, rather than a cut here, because the drain is where the
/// model of each park is written: a rock cut here would also have to be
/// recorded there. `tests::a_warm_system_announces_rocks_per_viewer_grid`.
fn announce(w: &mut Registry, solar_system_id: i64, rocks: &[Rock]) {
    if rocks.is_empty() {
        return;
    }
    let to = crate::dungeon::audience(w, solar_system_id);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    let park = crate::destiny::Park {
        current_time: stamp,
        balls: rocks.iter().map(crate::destiny::rock_ball).collect(),
    };
    let extra = rocks
        .iter()
        .map(|r| (crate::destiny::rock_slim(r), None))
        .collect();
    if let Some(added) = crate::destiny::BallsAdded::new(to, stamp, park, extra) {
        w.schedule(std::time::Duration::ZERO, added);
    }
}

/// `splitmix64`: a small, fixed, dependency-free PRNG, so a belt is laid out
/// the same way on every start. Not for anything but layout.
pub(crate) struct Mix(pub(crate) u64);

impl Mix {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Where the field's centre is: the belt point plus [`FIELD_OFFSET_FRACTION`]
/// x [`outer_radius`] in a seeded horizontal direction. **[A]**
pub fn field_centre(belt_id: i64, centre: [f64; 3], rocks: usize) -> [f64; 3] {
    let mut rng = Mix(belt_id as u64 ^ 0xF1E1_DCE7);
    let angle = rng.unit() * std::f64::consts::TAU;
    let d = FIELD_OFFSET_FRACTION * outer_radius(rocks);
    [
        centre[0] + d * angle.cos(),
        centre[1],
        centre[2] + d * angle.sin(),
    ]
}

/// One belt's rocks, laid out. Deterministic in its inputs.
///
/// **The counts are the data and are never cut** (except past
/// [`IDS_PER_BELT`]): rows are taken in ascending type order, `n` runs
/// across the whole belt, and each rock gets the id `MIN_ROCK_ID +
/// ordinal * IDS_PER_BELT + n`. A rock's quantity is drawn by
/// [`Quantities::draw`] and its radius follows by the ore's [`Law`].
///
/// Placement is rejection sampling in a flattened ellipsoid **[A]** around
/// [`field_centre`]: horizontal semi-axis `outer`, vertical
/// [`FIELD_THICKNESS`] x `outer`. A rock is kept when its surface is at
/// least `INNER_RADIUS` from the belt point (the warp-in) and at least `GAP`
/// clear of every rock already placed. After [`TRIES`] misses the field
/// grows by 10 % and the rock tries again -- a crowded belt gets wider, it
/// does not get fewer rocks.
pub fn rocks_for_belt(
    belt_id: i64,
    centre: [f64; 3],
    rows: &[Row],
    quantities: &Quantities,
) -> Vec<Rock> {
    let mut rows: Vec<Row> = rows
        .iter()
        .filter(|r| r.belt_id == belt_id)
        .copied()
        .collect();
    rows.sort_by_key(|r| r.type_id);
    let Some(ordinal) = rows.first().map(|r| r.ordinal) else {
        return Vec::new();
    };
    let total: i64 = rows.iter().map(|r| r.count).sum::<i64>().min(IDS_PER_BELT);
    let mut rng = Mix(belt_id as u64 ^ 0xA57E_801D);
    let mut outer = outer_radius(total as usize);
    let middle = field_centre(belt_id, centre, total as usize);
    let mut placed: Vec<Rock> = Vec::with_capacity(total as usize);
    let mut n = 0i64;
    'rows: for row in &rows {
        for _ in 0..row.count {
            if n >= IDS_PER_BELT {
                break 'rows;
            }
            let quantity = quantities.draw(row.type_id, &row.law, &mut rng);
            let radius = row.law.radius(quantity);
            let position = loop {
                let mut found = None;
                for _ in 0..TRIES {
                    // Uniform in the unit ball by rejection, then stretched.
                    let (x, y, z) = (
                        rng.unit() * 2.0 - 1.0,
                        rng.unit() * 2.0 - 1.0,
                        rng.unit() * 2.0 - 1.0,
                    );
                    if x * x + y * y + z * z > 1.0 {
                        continue;
                    }
                    let p = [
                        middle[0] + x * outer,
                        middle[1] + y * FIELD_THICKNESS * outer,
                        middle[2] + z * outer,
                    ];
                    if crate::space_state::distance(p, centre) - radius < INNER_RADIUS {
                        continue;
                    }
                    let clear = placed.iter().all(|o| {
                        crate::space_state::distance(p, o.position) - radius - o.radius >= GAP
                    });
                    if clear {
                        found = Some(p);
                        break;
                    }
                }
                match found {
                    Some(p) => break p,
                    None => outer *= 1.1,
                }
            };
            placed.push(Rock {
                id: MIN_ROCK_ID + ordinal * IDS_PER_BELT + n,
                type_id: row.type_id,
                belt_id,
                solar_system_id: row.solar_system_id,
                quantity,
                radius,
                position,
                law: row.law,
            });
            n += 1;
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::Catalogue;

    /// The shape `tools/asteroids.py` writes. The numbers are a fixture, not
    /// the client's: each test asserts a property that holds for any law.
    const COUNTS: &str = "\
# solarSystemID\t30000001
#belt_id\tordinal\ttype_id\tcount\tsystem\tgrowth\tunit_size\tmax_radius
40000003\t0\t1224\t22\t30000001\t0.5\t90.0\t16255.0
40000003\t0\t1230\t30\t30000001\t0.5\t90.0\t16255.0
40000003\t0\t17459\t10\t30000001\t0.5\t600.0\t16255.0
40000006\t1\t1228\t27\t30000001\t0.5\t90.0\t16255.0
40000099\t9\t1228\t5\t30000001\t0.5\t90.0\t16255.0
";

    const CELESTIALS: &str = "\
belt\t40000003\t15\t0.0\t161967513600.0\t21288837120.0\t-73505464320.0
belt\t40000006\t15\t0.0\t-1000000.0\t0.0\t5000000.0
planet\t40000002\t11\t5060000.0\t161891117336.0\t21288951986.0\t-73529712226.0
";

    fn field() -> Field {
        Field::generate(
            &Counts::parse(COUNTS),
            &Catalogue::parse(CELESTIALS),
            &Quantities::default(),
        )
    }

    /// Every row is read, a bad one costs itself, nothing is an error.
    #[test]
    fn the_extract_parses_and_a_bad_row_is_skipped() {
        let c = Counts::parse(COUNTS);
        assert_eq!(c.rows().len(), 5);
        assert_eq!(c.rows()[0].type_id, 1224);
        assert_eq!(c.rows()[0].law.unit_size, 90.0);
        let bad = Counts::parse(
            "40000003\t0\t1224\t22\t30000001\t0.5\t90.0\t16255.0\n\
             40000003\tx\t1224\t22\t30000001\t0.5\t90.0\t16255.0\n\
             40000003\t0\t1224\t22\t30000001\n\
             40000003\t0\t1224\t22\t30000001\t0.0\t90.0\t16255.0\n\
             40000003\t0\t1224\t0\t30000001\t0.5\t90.0\t16255.0\n\
             40000003\t50000\t1224\t2\t30000001\t0.5\t90.0\t16255.0\n",
        );
        assert_eq!(bad.rows().len(), 1, "{bad:?}");
        assert!(Counts::parse("").rows().is_empty());
        assert!(Field::default().is_empty());
    }

    /// A belt the catalogue does not have as a belt gets no rocks: there is
    /// nowhere to put them.
    #[test]
    fn a_belt_missing_from_the_catalogue_has_no_rocks() {
        let f = field();
        assert!(f.iter().all(|r| r.belt_id != 40_000_099));
        assert!(
            Field::generate(
                &Counts::parse(COUNTS),
                &Catalogue::default(),
                &Quantities::default()
            )
            .is_empty()
        );
    }

    /// Per belt, rocks per type are the file's counts and nothing else.
    #[test]
    fn the_counts_are_the_data() {
        let f = field();
        for row in Counts::parse(COUNTS)
            .rows()
            .iter()
            .filter(|r| r.belt_id != 40_000_099)
        {
            let n = f
                .iter()
                .filter(|r| r.belt_id == row.belt_id && r.type_id == row.type_id)
                .count();
            assert_eq!(n as i64, row.count, "{row:?}");
        }
        assert_eq!(f.len(), 22 + 30 + 10 + 27);
    }

    /// The same inputs lay out the same field.
    #[test]
    fn the_layout_is_deterministic() {
        assert_eq!(field(), field());
    }

    /// Ids are in the client's asteroid range, unique, and the formula.
    #[test]
    fn ids_are_in_the_asteroid_range_and_follow_the_ordinal() {
        let f = field();
        for belt in [40_000_003, 40_000_006] {
            let mut ids: Vec<i64> = f
                .iter()
                .filter(|r| r.belt_id == belt)
                .map(|r| r.id)
                .collect();
            ids.sort_unstable();
            let ordinal = if belt == 40_000_003 { 0 } else { 1 };
            for (n, id) in ids.iter().enumerate() {
                assert_eq!(*id, MIN_ROCK_ID + ordinal * IDS_PER_BELT + n as i64);
            }
        }
        for r in f.iter() {
            assert!((70_000_000..80_000_000).contains(&r.id));
            assert!(!(40_000_000..50_000_000).contains(&r.id), "celestial range");
            assert!(r.id < crate::item::MIN_PLAYER_ITEM);
        }
    }

    /// Every radius is the client's law applied to a whole quantity, and the
    /// inverse gives that quantity back.
    #[test]
    fn every_radius_is_the_clients_law() {
        let counts = Counts::parse(COUNTS);
        let f = field();
        for r in f.iter() {
            let law = counts
                .rows()
                .iter()
                .find(|x| x.type_id == r.type_id)
                .unwrap()
                .law;
            assert!(r.quantity >= 1);
            let expect = (law.unit_size * (4e-5 * (r.quantity as f64 - 1.0) * law.growth).exp())
                .min(law.max_radius);
            assert_eq!(r.radius as f32, expect as f32, "{r:?}");
            if r.radius < law.max_radius {
                assert!(
                    (law.quantity(r.radius) - r.quantity as f64).abs() < 1.0,
                    "{r:?}"
                );
            }
        }
    }

    /// The law's own fixed points: one unit is the unit size, and the cap
    /// holds.
    #[test]
    fn the_law_meets_its_ends() {
        let law = Law {
            growth: 0.5,
            unit_size: 90.0,
            max_radius: 16_255.0,
        };
        assert_eq!(law.radius(1), 90.0);
        assert_eq!(law.radius(0), 90.0, "max(1, q)");
        assert_eq!(law.radius(10_000_000), 16_255.0);
        assert_eq!(law.quantity(90.0), 1.0);
    }

    /// No two rocks overlap, and every surface is clear of where a ship
    /// warping to the belt lands, by more than any hull we fly.
    #[test]
    fn rocks_clear_each_other_and_the_warp_landing() {
        let f = field();
        let cat = Catalogue::parse(CELESTIALS);
        let hull = crate::destiny::hull::FIXTURE
            .iter()
            .map(|h| f64::from(h.radius))
            .fold(0.0, f64::max);
        let rocks: Vec<&Rock> = f.iter().collect();
        for (i, a) in rocks.iter().enumerate() {
            for b in &rocks[i + 1..] {
                let d = crate::space_state::distance(a.position, b.position);
                assert!(d >= a.radius + b.radius, "{a:?} overlaps {b:?}");
            }
            let belt = cat.get(a.belt_id).unwrap().position;
            for from in [[0.0; 3], [1e12, -3e11, 2e10], [-5e11, 0.0, 9e11]] {
                let land =
                    crate::destiny::warp_arrival(from, belt, crate::space_state::WARP_STANDOFF);
                let surface = crate::space_state::distance(land, a.position) - a.radius;
                assert!(surface > hull, "{a:?} is {surface} m from the landing");
            }
        }
    }

    /// A bite takes what it can, the quantity falls by exactly that, and the
    /// radius is the law's for what is left.
    #[test]
    fn mining_takes_what_is_there_and_resizes_by_the_law() {
        let mut f = field();
        let rock = *f.iter().next().unwrap();
        let m = f.mine(rock.id, 100).expect("a rock");
        assert_eq!(m.taken, 100.min(rock.quantity));
        assert_eq!(m.left, rock.quantity - m.taken);
        let after = *f.get(rock.id).expect("still there");
        assert_eq!(after.quantity, m.left);
        assert_eq!(after.radius, rock.law.radius(after.quantity));
        assert_eq!(m.radius, after.radius);
        assert!(after.radius <= rock.radius, "a rock never grows");
        assert_eq!(f.mine(rock.id, -5).map(|m| m.taken), Some(0));
        assert_eq!(f.mine(MIN_ROCK_ID + 199, 1), None, "not a rock");
    }

    /// Asking for more than is left takes the rest, and the rock is gone.
    #[test]
    fn a_rock_mined_out_leaves_the_field() {
        let mut f = field();
        let rock = *f.iter().next().unwrap();
        let before = f.len();
        let m = f.mine(rock.id, rock.quantity + 1_000).expect("a rock");
        assert_eq!(m.taken, rock.quantity);
        assert!(m.depleted());
        assert!(!m.shrank(), "a removed ball has no size to change to");
        assert_eq!(f.get(rock.id), None);
        assert_eq!(f.len(), before - 1);
        assert_eq!(f.mine(rock.id, 1), None);
    }

    /// `from_rocks` keys by id, so a fixture field is an ordinary field.
    #[test]
    fn a_field_can_be_built_from_rocks() {
        let rocks: Vec<Rock> = field().iter().copied().take(3).collect();
        let f = Field::from_rocks(rocks.clone());
        assert_eq!(f.len(), 3);
        for r in rocks {
            assert_eq!(f.get(r.id), Some(&r));
        }
    }

    /// Three systems, belts interleaved across them, so one system's ids
    /// are not one contiguous run. Invented ids and laws.
    const MULTI_COUNTS: &str = "\
40000003\t0\t1224\t6\t30000001\t0.5\t90.0\t16255.0
40000003\t0\t17459\t3\t30000001\t0.5\t600.0\t16255.0
40000006\t1\t1228\t5\t30000002\t0.5\t90.0\t16255.0
40000007\t2\t1230\t4\t30000001\t0.5\t90.0\t16255.0
40000008\t3\t1224\t7\t30000003\t0.5\t90.0\t16255.0
40000009\t4\t1228\t2\t30000002\t0.5\t90.0\t16255.0
";

    const MULTI_CELESTIALS: &str = "\
belt\t40000003\t15\t0.0\t1000000.0\t0.0\t0.0
belt\t40000006\t15\t0.0\t-1000000.0\t0.0\t5000000.0
belt\t40000007\t15\t0.0\t9000000.0\t0.0\t0.0
belt\t40000008\t15\t0.0\t0.0\t3000000.0\t0.0
belt\t40000009\t15\t0.0\t0.0\t0.0\t-7000000.0
";

    /// The systems the fixture has, and two it does not.
    const SYSTEMS: [i64; 5] = [30_000_001, 30_000_002, 30_000_003, 30_000_004, 0];

    /// `in_system` against the filter it replaced: the same rows, in the
    /// same order, for every system.
    fn assert_index_is_the_filter(f: &Field, when: &str) {
        for s in SYSTEMS {
            let indexed: Vec<&Rock> = f.in_system(s).collect();
            let filtered: Vec<&Rock> = f.iter().filter(|r| r.solar_system_id == s).collect();
            assert_eq!(indexed, filtered, "system {s}, {when}");
        }
    }

    /// The property, over every mutation path: after `generate`, after each
    /// bite and each mine-out in a seeded random order until the field is
    /// empty, and for `from_rocks` over a shuffled, duplicated subset.
    #[test]
    fn in_system_is_the_filter_after_every_mutation() {
        let generated = Field::generate(
            &Counts::parse(MULTI_COUNTS),
            &Catalogue::parse(MULTI_CELESTIALS),
            &Quantities::default(),
        );
        assert!(
            SYSTEMS[..3]
                .iter()
                .all(|s| generated.in_system(*s).next().is_some()),
            "the fixture spans three systems"
        );
        assert_index_is_the_filter(&generated, "generate");
        for seed in 1..=8u64 {
            let mut f = generated.clone();
            let mut rng = Mix(seed);
            while !f.is_empty() {
                let ids: Vec<i64> = f.iter().map(|r| r.id).collect();
                let id = ids[(rng.next() % ids.len() as u64) as usize];
                let q = f.get(id).unwrap().quantity;
                // Half the bites take the rest, half take a part.
                let want = if rng.next().is_multiple_of(2) {
                    q
                } else {
                    (rng.next() % q as u64) as i64
                };
                let m = f.mine(id, want).expect("a rock");
                assert_index_is_the_filter(&f, &format!("seed {seed}, after {m:?}"));
            }
            assert_eq!(f, Field::default(), "mined out is the empty field");
            let mut subset: Vec<Rock> = generated
                .iter()
                .filter(|_| !rng.next().is_multiple_of(3))
                .copied()
                .collect();
            for i in (1..subset.len()).rev() {
                subset.swap(i, (rng.next() % (i as u64 + 1)) as usize);
            }
            if let Some(first) = subset.first().copied() {
                // A duplicate id moved to another system: the later wins.
                subset.push(Rock {
                    solar_system_id: 30_000_004,
                    ..first
                });
            }
            let built = Field::from_rocks(subset);
            assert_index_is_the_filter(&built, &format!("seed {seed}, from_rocks"));
            assert_eq!(built, Field::from_rocks(built.iter().copied()));
        }
    }

    /// Rocks are filtered by system, and a rock aims a warp at itself.
    #[test]
    fn a_rock_is_found_by_id_and_by_system() {
        let f = field();
        let any = *f.iter().next().unwrap();
        assert_eq!(f.get(any.id), Some(&any));
        assert_eq!(f.in_system(30_000_001).count(), f.len());
        assert_eq!(f.in_system(30_000_002).count(), 0);
        assert_eq!(
            f.get(MIN_ROCK_ID + 199),
            None,
            "an id in range is not a rock"
        );
        let t = any.target();
        assert_eq!(t.position, any.position);
        assert_eq!(t.radius, any.radius);
        assert_eq!(t.warp_in, None);
    }

    // ---- CP14': lazy rocks and regrowth. Every id and row is invented.

    use crate::liveness::COOL_AFTER;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `MULTI_COUNTS` with a comment, a bad line and a belt the catalogue
    /// lacks among the rows, so one system's rows are two ranges.
    const LAZY_COUNTS: &str = "\
#belt_id\tordinal\ttype_id\tcount\tsystem\tgrowth\tunit_size\tmax_radius
40000003\t0\t1224\t6\t30000001\t0.5\t90.0\t16255.0
40000003\t0\t17459\t3\t30000001\t0.5\t600.0\t16255.0
40000006\t1\t1228\t5\t30000002\t0.5\t90.0\t16255.0
not\ta\trow
40000007\t2\t1230\t4\t30000001\t0.5\t90.0\t16255.0
40000099\t9\t1230\t4\t30000002\t0.5\t90.0\t16255.0
40000008\t3\t1224\t7\t30000003\t0.5\t90.0\t16255.0
40000009\t4\t1228\t2\t30000002\t0.5\t90.0\t16255.0
";

    const A: i64 = 30_000_001;
    const ALICE: i64 = 1_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    /// A blue-clock origin; any value works.
    const T0: crate::damage::BlueTime = 1_000 * COOL_AFTER;

    static FILES: AtomicUsize = AtomicUsize::new(0);

    /// A fixture file on disk, removed on drop.
    struct TmpFile(PathBuf);

    impl TmpFile {
        fn new(text: &str) -> TmpFile {
            let p = std::env::temp_dir().join(format!(
                "eve-asteroid-{}-{}.tsv",
                std::process::id(),
                FILES.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::write(&p, text).expect("write the fixture");
            TmpFile(p)
        }
    }

    impl Drop for TmpFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn lazy_field(file: &TmpFile) -> Field {
        index(
            &file.0,
            &Catalogue::parse(MULTI_CELESTIALS),
            &Quantities::default(),
        )
        .expect("an index")
    }

    fn eager() -> Field {
        Field::generate(
            &Counts::parse(LAZY_COUNTS),
            &Catalogue::parse(MULTI_CELESTIALS),
            &Quantities::default(),
        )
    }

    /// What `sync` does for one system, without a registry.
    fn materialise(f: &mut Field, s: i64, d: &mut Deltas) {
        let plan = f.plan(s, d);
        for k in &plan.stale {
            d.del(KIND, k);
        }
        f.install(plan);
    }

    /// What `sync` does with the bites, without a registry.
    fn capture_into(f: &mut Field, d: &mut Deltas) {
        for r in f.take_changed() {
            let (k, v) = delta_row(&r);
            d.put(KIND, k, v);
        }
    }

    fn rows_in(f: &Field, s: i64) -> Vec<Rock> {
        f.in_system(s).copied().collect()
    }

    /// The index reads back exactly each system's rows of the file, in
    /// file order, from a system split into two ranges too; a belt the
    /// catalogue lacks is not indexed; a missing file is an error.
    #[test]
    fn the_offset_index_finds_every_systems_rows_in_a_fixture_file() {
        let file = TmpFile::new(LAZY_COUNTS);
        let cat = Catalogue::parse(MULTI_CELESTIALS);
        let ix = Index::build(&file.0, &cat, &Quantities::default()).expect("an index");
        assert_eq!(ix.systems().collect::<Vec<_>>(), SYSTEMS[..3].to_vec());
        assert!(ix.systems[&A].len() > 1, "the fixture splits a system");
        let all = Counts::parse(LAZY_COUNTS);
        for s in SYSTEMS {
            let want: Vec<Row> = all
                .rows()
                .iter()
                .filter(|r| r.solar_system_id == s && cat.get(r.belt_id).is_some())
                .copied()
                .collect();
            assert_eq!(ix.rows_of(s).expect("readable"), want, "system {s}");
        }
        assert_eq!(ix.static_len() as usize, eager().len());
        assert!(
            Index::build(
                &file.0.with_extension("missing"),
                &cat,
                &Quantities::default()
            )
            .is_err()
        );
        assert!(index(&file.0, &Catalogue::default(), &Quantities::default()).is_err());
    }

    /// **A cold system holds no rock.** A lazy field starts empty; a pilot
    /// in space lays out that system and no other (these have no gates, so
    /// no warm neighbour), and their park gets the rocks as one batch; the
    /// system cools to cold after `COOL_AFTER` and its rocks go.
    #[test]
    fn a_cold_system_holds_no_rock() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut reg = world(&file);
        assert!(reg.rocks().is_empty() && reg.rocks().is_lazy());
        tick(&mut reg, T0);
        assert!(reg.rocks().is_empty(), "nobody anywhere: every system cold");

        in_space(&mut reg, A);
        let queued = reg.scheduled_count();
        tick(&mut reg, T0);
        assert_eq!(rows_in(reg.rocks(), A), rows_in(&eager(), A));
        assert_eq!(reg.rocks().len(), eager().in_system(A).count());
        for s in [30_000_002, 30_000_003] {
            assert_eq!(reg.rocks().in_system(s).count(), 0, "system {s}");
            assert!(!reg.rocks().is_materialised(s));
        }
        assert_eq!(reg.scheduled_count(), queued + 1, "one AddBalls2 to Alice");

        reg.roster_mut().leave(ALICE);
        tick(&mut reg, T0 + COOL_AFTER - 1);
        assert!(reg.rocks().is_materialised(A), "warm keeps its rocks");
        tick(&mut reg, T0 + COOL_AFTER);
        assert!(reg.rocks().is_empty());
        assert!(!reg.rocks().is_materialised(A));
    }

    /// Materialise, take a bite and mine a rock out, drop the system, lay
    /// it out again: the bitten rock keeps its remainder (and the radius
    /// the law gives it), the mined-out one stays gone, the rest are whole.
    #[test]
    fn materialise_mine_drop_and_rematerialise_keeps_the_remainder() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut f = lazy_field(&file);
        let mut d = Deltas::default();
        materialise(&mut f, A, &mut d);
        let whole = rows_in(&f, A);
        assert_eq!(whole, rows_in(&eager(), A));
        let (a, b) = (whole[0], whole[1]);
        let bit = f.mine(a.id, a.quantity / 2).expect("a rock");
        f.mine(b.id, b.quantity).expect("a rock");
        capture_into(&mut f, &mut d);
        assert_eq!(d.of(KIND).len(), 2);

        f.drop_system(A);
        assert!(f.is_empty() && !f.is_materialised(A));
        materialise(&mut f, A, &mut d);
        let again = *f.get(a.id).expect("the bitten rock");
        assert_eq!(again.quantity, bit.left);
        assert_eq!(again.radius, a.law.radius(bit.left));
        assert_eq!(again.position, a.position);
        assert_eq!(f.get(b.id), None, "gone stays gone");
        assert_eq!(f.len(), whole.len() - 1);
        for r in &whole[2..] {
            assert_eq!(f.get(r.id), Some(r), "untouched rocks are whole");
        }
    }

    /// **A restart keeps a gone rock gone**, through the registry: the
    /// stage writes the bites as deltas, the snapshot carries them, and a
    /// fresh field over the same file lays the system out with them.
    #[test]
    fn a_restart_keeps_a_gone_rock_gone() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut reg = world(&file);
        in_space(&mut reg, A);
        tick(&mut reg, T0);
        let rocks = rows_in(reg.rocks(), A);
        let (gone, part) = (rocks[0], rocks[1]);
        reg.rocks_mut()
            .mine(gone.id, gone.quantity)
            .expect("a rock");
        let bit = reg.rocks_mut().mine(part.id, 1).expect("a rock");
        tick(&mut reg, T0 + 1);
        assert_eq!(reg.deltas().of(KIND).len(), 2, "captured");

        let mut again = world(&file);
        again.apply_snapshot(reg.to_snapshot());
        in_space(&mut again, A);
        tick(&mut again, T0);
        assert_eq!(again.rocks().get(gone.id), None);
        assert_eq!(
            again.rocks().get(part.id).map(|r| r.quantity),
            Some(bit.left)
        );
        assert_eq!(again.rocks().in_system(A).count(), rocks.len() - 1);
    }

    /// **The delta key is `(belt, n)`**, `n` counting from 0 in each belt,
    /// and a delta whose type, position or remainder does not fit the rock
    /// laid out at that key is dropped and the rock is whole; so is a key
    /// with no rock and a key that does not parse.
    #[test]
    fn the_delta_key_is_belt_and_n_and_a_disagreeing_delta_is_dropped() {
        let file = TmpFile::new(LAZY_COUNTS);
        let f = lazy_field(&file);
        let rocks = rows_in(&eager(), A);
        let first_of = |belt| rocks.iter().find(|r| r.belt_id == belt).unwrap().id;
        assert_eq!(ordinal_in_belt(first_of(40_000_003)), 0);
        assert_eq!(ordinal_in_belt(first_of(40_000_007)), 0);
        let gone = |r: Rock| delta_row(&Rock { quantity: 0, ..r });
        let (r0, r1, r2, r3) = (rocks[0], rocks[1], rocks[2], rocks[3]);
        let (k0, v0) = gone(r0);
        assert_eq!(
            k0,
            vec![r0.belt_id.to_string(), ordinal_in_belt(r0.id).to_string()]
        );

        let mut d = Deltas::default();
        d.put(KIND, k0.clone(), v0.clone());
        let (k1, mut v1) = gone(r1);
        v1[0] = (r1.type_id + 1).to_string();
        d.put(KIND, k1.clone(), v1);
        let (k2, mut v2) = gone(r2);
        v2[1] = (r2.position[0] + 2.0 * POSITION_TOLERANCE).to_string();
        d.put(KIND, k2.clone(), v2);
        let (k3, v3) = delta_row(&Rock {
            quantity: r3.quantity + 1,
            ..r3
        });
        d.put(KIND, k3.clone(), v3);
        let kn = vec![r0.belt_id.to_string(), (IDS_PER_BELT - 1).to_string()];
        d.put(KIND, kn.clone(), v0.clone());
        let kx = vec![r0.belt_id.to_string(), "x".to_owned()];
        d.put(KIND, kx.clone(), v0);

        let plan = f.plan(A, &d);
        let stale: BTreeSet<Vec<String>> = plan.stale.iter().cloned().collect();
        assert_eq!(stale, BTreeSet::from([k1, k2, k3, kn, kx]));
        assert!(plan.rocks.iter().all(|r| r.id != r0.id), "an honest delta");
        for r in [r1, r2, r3] {
            assert_eq!(plan.rocks.iter().find(|x| x.id == r.id), Some(&r));
        }
        assert_eq!(plan.rocks.len(), rocks.len() - 1);
    }

    /// A fixture ore row for the one-belt tests below: invented ids.
    fn law_90() -> Law {
        Law {
            growth: 0.5,
            unit_size: 90.0,
            max_radius: 16_255.0,
        }
    }

    fn fixture_rows(count: i64) -> Vec<Row> {
        vec![Row {
            belt_id: 40_000_003,
            ordinal: 0,
            type_id: 991,
            count,
            solar_system_id: 30_000_001,
            law: law_90(),
        }]
    }

    /// A rock's quantity comes from its group's pool, and its radius follows
    /// by the ore's law. (The old code picked a radius in 1,500-6,000 m.)
    #[test]
    fn a_rocks_quantity_is_drawn_from_its_groups_pool() {
        let pool = vec![1_234i64; 10]
            .into_iter()
            .chain(vec![5_678i64; 10])
            .collect::<Vec<_>>();
        let mut q = Quantities::default();
        q.by_group.insert(9100, pool);
        q.group_of.insert(991, 9100);
        let rocks = rocks_for_belt(40_000_003, [0.0; 3], &fixture_rows(40), &q);
        assert_eq!(rocks.len(), 40);
        for r in &rocks {
            assert!([1_234, 5_678].contains(&r.quantity), "{}", r.quantity);
            assert_eq!(r.radius, r.law.radius(r.quantity));
        }
    }

    /// Only the ore-anomaly rooms feed the pools; a combat room's ore and
    /// every non-ore object are left out.
    #[test]
    fn quantities_come_from_ore_anomaly_rooms_only() {
        use crate::dungeon_data::{Archetype, Dungeon, Object, Table};
        let obj = |type_id, radius| Object {
            type_id,
            radius: Some(radius),
            ..Object::default()
        };
        let mut dungeons = Table::default();
        for (id, arch, tag) in [(1, 10, "feature_ore_anomalies"), (2, 11, "activity_combat")] {
            dungeons.archetypes.insert(
                arch,
                Archetype {
                    id: arch,
                    content_tags: vec![tag.to_owned()],
                    ..Archetype::default()
                },
            );
            dungeons.dungeons.insert(
                id,
                Dungeon {
                    id,
                    archetype: arch,
                    objects: vec![obj(991, 777.4), obj(5, 999.0)],
                    ..Dungeon::default()
                },
            );
        }
        let types = crate::typedata::Table::parse(
            "type\t991\t9100\t25\t1\t1\t0.1\t0.0\t0.0\t0.0\t0\t\t\tOre\n\
             type\t5\t9200\t6\t1\t1\t0.1\t0.0\t0.0\t0.0\t0\t\t\tShip\n",
        )
        .expect("types");
        let q = Quantities::from_tables(&dungeons, &types);
        assert_eq!(q.by_group.len(), 1);
        assert_eq!(q.by_group[&9100], vec![777]);
        assert_eq!(q.group_of.get(&991), Some(&9100));
        assert!(!q.group_of.contains_key(&5));
    }

    /// With no pool (a fixture) the law's own span sizes the rock: radius
    /// between one unit size and the fallback span.
    #[test]
    fn a_fixture_without_pools_still_sizes_by_the_law() {
        let rocks = rocks_for_belt(
            40_000_003,
            [0.0; 3],
            &fixture_rows(30),
            &Quantities::default(),
        );
        assert_eq!(rocks.len(), 30);
        let law = law_90();
        for r in &rocks {
            assert!(r.quantity >= 1);
            assert_eq!(r.radius, law.radius(r.quantity));
            assert!(r.radius >= law.unit_size && r.radius <= FALLBACK_SPAN * law.unit_size);
        }
    }

    /// The field gathers round its offset centre, not the belt point, and the
    /// warp-in stays clear of every rock.
    #[test]
    fn a_belts_rocks_gather_round_the_offset_field_centre() {
        let centre = [1.0e6, 2.0e6, -3.0e6];
        let n = 120;
        let rocks = rocks_for_belt(40_000_003, centre, &fixture_rows(n), &Quantities::default());
        assert_eq!(rocks.len(), n as usize);
        let middle = field_centre(40_000_003, centre, n as usize);
        assert!(crate::space_state::distance(middle, centre) > 1_000.0);
        let mean = rocks.iter().fold([0.0; 3], |m, r| {
            [
                m[0] + r.position[0] / n as f64,
                m[1] + r.position[1] / n as f64,
                m[2] + r.position[2] / n as f64,
            ]
        });
        assert!(
            crate::space_state::distance(mean, middle) < crate::space_state::distance(mean, centre),
            "the mean position is nearer the field centre than the belt point"
        );
        for (i, r) in rocks.iter().enumerate() {
            assert!(
                crate::space_state::distance(r.position, centre) - r.radius >= INNER_RADIUS,
                "rock {i} too near the warp-in"
            );
            for o in &rocks[..i] {
                assert!(
                    crate::space_state::distance(r.position, o.position) - r.radius - o.radius
                        >= GAP
                );
            }
        }
    }

    /// **The downtime reset.** At the rollover a mined-out rock and a partly
    /// mined rock both come back whole, at their original id and position,
    /// with their deltas gone; a belt never holds more than its static count.
    #[test]
    fn a_partly_mined_and_a_mined_out_rock_both_come_back_whole_at_rollover() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut f = lazy_field(&file);
        let mut d = Deltas::default();
        let s = 30_000_002;
        materialise(&mut f, s, &mut d);
        let whole = rows_in(&f, s);
        let gone = *whole.iter().find(|r| r.belt_id == 40_000_006).unwrap();
        let part = *whole.iter().find(|r| r.belt_id == 40_000_009).unwrap();
        f.mine(gone.id, gone.quantity).expect("a rock");
        f.mine(part.id, 1).expect("a rock");
        capture_into(&mut f, &mut d);
        assert_eq!(d.of(KIND).len(), 2);

        let keys = f.reset_keys(d.of(KIND));
        assert_eq!(keys.len(), 2, "mined out and partly mined alike");
        for k in &keys {
            d.del(KIND, k);
        }
        assert_eq!(f.restore(&keys), vec![gone], "the gone rock is announced");
        assert_eq!(f.get(part.id), Some(&part), "the bitten rock is whole");
        assert_eq!(f.get(gone.id), Some(&gone));
        assert_eq!(rows_in(&f, s), whole);
        assert_index_is_the_filter(&f, "after a reset");
        assert!(d.of(KIND).is_empty());
        assert!(f.reset_keys(d.of(KIND)).is_empty());
    }

    /// Laid out system by system, the field holds per belt and type exactly
    /// the file's counts [V rule: `asteroid_scanning_service:216-226`], and
    /// is the eager field rock for rock.
    #[test]
    fn the_materialised_count_by_type_equals_the_files_counts() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut f = lazy_field(&file);
        let mut d = Deltas::default();
        for s in SYSTEMS {
            materialise(&mut f, s, &mut d);
        }
        let cat = Catalogue::parse(MULTI_CELESTIALS);
        for row in Counts::parse(LAZY_COUNTS).rows() {
            let n = f
                .iter()
                .filter(|r| r.belt_id == row.belt_id && r.type_id == row.type_id)
                .count() as i64;
            let want = if cat.get(row.belt_id).is_some() {
                row.count
            } else {
                0
            };
            assert_eq!(n, want, "{row:?}");
        }
        assert!(f.iter().eq(eager().iter()));
        assert_eq!(f.len() as i64, f.static_len());
    }

    /// RIDX's property over the lazy paths: install, mine, drop, regrow.
    #[test]
    fn in_system_is_the_filter_after_materialise_drop_and_regrow() {
        let file = TmpFile::new(LAZY_COUNTS);
        let mut f = lazy_field(&file);
        let mut d = Deltas::default();
        assert_index_is_the_filter(&f, "lazy, empty");
        for s in SYSTEMS {
            materialise(&mut f, s, &mut d);
            assert_index_is_the_filter(&f, &format!("install {s}"));
        }
        let ids: Vec<i64> = f.iter().map(|r| r.id).step_by(2).collect();
        for id in ids {
            f.mine(id, i64::MAX).expect("a rock");
        }
        capture_into(&mut f, &mut d);
        assert_index_is_the_filter(&f, "mined");
        f.drop_system(30_000_002);
        assert_index_is_the_filter(&f, "dropped");
        let keys = f.reset_keys(d.of(KIND));
        f.restore(&keys);
        assert_index_is_the_filter(&f, "regrown");
        for k in &keys {
            d.del(KIND, k);
        }
        materialise(&mut f, 30_000_002, &mut d);
        assert_index_is_the_filter(&f, "laid out again");
        assert!(f.iter().eq(eager().iter()), "everything regrown");
    }

    fn world(file: &TmpFile) -> Registry {
        Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: Catalogue::parse(MULTI_CELESTIALS),
            rocks: lazy_field(file),
            ..crate::model::SandboxConfig::default()
        })
    }

    fn in_space(reg: &mut Registry, system: i64) {
        use crate::space_state::{Motion, Presence};
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    }

    /// The two stages, in `tick::STAGES` order.
    fn tick(reg: &mut Registry, t: crate::damage::BlueTime) {
        crate::liveness::refresh(reg, std::time::Instant::now(), t);
        sync(reg, std::time::Instant::now(), t);
    }

    /// **A warm system's rocks reach each pilot as that pilot's grid**
    /// (GRID-B): one pilot at each belt, and each is sent its own belt's
    /// rocks and not the other's; a pilot at the station 1 AU away is sent
    /// none. The cut is the drain's (`crate::grid::filter`), so this drains.
    #[test]
    fn a_warm_system_announces_rocks_per_viewer_grid() {
        use crate::grid::tests as g;
        let mut reg = g::world();
        for who in [g::ALICE, g::BOB, g::CAROL] {
            g::undock(&mut reg, who);
        }
        g::put(&mut reg, g::ALICE, g::BELT_A_AT);
        g::put(&mut reg, g::BOB, g::BELT_B_AT);
        let _ = g::settle(&mut reg);
        let mut rocks = g::belt_rocks(g::BELT_A, g::BELT_A_AT, MIN_ROCK_ID + 500);
        rocks.extend(g::belt_rocks(g::BELT_B, g::BELT_B_AT, MIN_ROCK_ID + 600));
        let a: BTreeSet<i64> = rocks
            .iter()
            .filter(|r| r.belt_id == g::BELT_A)
            .map(|r| r.id)
            .collect();
        let b: BTreeSet<i64> = rocks
            .iter()
            .filter(|r| r.belt_id == g::BELT_B)
            .map(|r| r.id)
            .collect();
        announce(&mut reg, g::SYSTEM, &rocks);
        let out = g::drain(&mut reg);
        assert_eq!(g::added_to(&out, g::ALICE), a, "Alice's belt only");
        assert_eq!(g::added_to(&out, g::BOB), b, "Bob's belt only");
        assert!(
            g::sent_to(&out, g::CAROL).is_empty(),
            "nothing at the station"
        );
    }
}
