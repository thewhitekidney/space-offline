//! **What the miners of a system can supply** (lane GB-FIX): the minerals
//! the ores in a system's belts refine into.
//!
//! **Why.** The closed economy has no NPC seller of minerals (O5-d): a
//! builder's materials are what the miners of its own station's system
//! sell into its standing bids, and a miner only mines the belts of that
//! system. The stage-B gate soak (`tools/soak.sh --closed --society`, 24
//! game hours, school system with Veldspar and Scordite only) ended with
//! seven builders holding two blueprints each and **no job installed**:
//! both blueprints needed a mineral (Mexallon, Nocxium) that no rock there
//! refines into, so the bid for it stood for the whole run, and the
//! builder, which works one blueprint at a time, never looked at anything
//! else. A player sees this at a glance (the ore is not in the belt); a bot
//! needs the table.
//!
//! **The table** is built once from the bundle: `belts/rocks.tsv` (which
//! ore types are in which system) and the refine yields the server already
//! loads (`reprocess::Table`, `types/materials.tsv`). A **mineral is
//! gated** when some ore anywhere yields it; a gated mineral is **supplied
//! in a system** when an ore in that system's belts yields it. A material
//! no ore yields (a component, a datacore, an ice product, a PI good) is
//! not gated: other careers make it, and this table does not speak for
//! them. The bundle is registered as one more content table
//! (`eve-bots` inserts it beside the bundle's own), so the builder reads it
//! with `content.try_get::<Supply>()` and **without a bundle there is no
//! table and nothing is filtered**, as before.
//!
//! **[A]** that a miner mines only the belts of the system it lives in and
//! sells at a station of that system. Falsified by a closed soak whose
//! builders sit on a blueprint that needs a mineral only a neighbouring
//! system's rocks yield (the filter then costs them a job they could have
//! built from a miner's trip).

use eve_server::content::Content;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The minerals the ores in each system's belts yield, and every mineral
/// any ore yields (the gated ones).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Supply {
    /// `system -> minerals its rocks refine into`.
    minable: BTreeMap<i64, BTreeSet<i64>>,
    /// Every mineral some ore, anywhere, yields.
    gated: BTreeSet<i64>,
    /// `system -> mineral -> units of it one m3 of the system's rock yields`
    /// (P5): the belt's rocks weighted by their ore volume. Empty for a
    /// table built without sizes ([`Supply::from_rocks`]).
    per_m3: BTreeMap<i64, BTreeMap<i64, f64>>,
}

/// What a belt rock of one ore type is made of, for [`Supply::from_sized`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OreFacts {
    /// `(mineral, units one unit of ore refines into)`.
    pub minerals: Vec<(i64, f64)>,
    /// One unit of ore, m3.
    pub volume: f64,
    /// Ore units in a typical rock of the type.
    pub rock_units: f64,
}

impl Supply {
    /// From `(system, ore type, rock count)` and `facts`: as
    /// [`Supply::from_rocks`], and per system the units of each mineral an
    /// m3 of its rock yields (rocks weighted by count x size).
    pub fn from_sized(
        rocks: impl IntoIterator<Item = (i64, i64, i64)>,
        facts: impl Fn(i64) -> OreFacts,
    ) -> Supply {
        let mut s = Supply::default();
        // system -> (mineral -> units, total m3)
        let mut acc: BTreeMap<i64, (BTreeMap<i64, f64>, f64)> = BTreeMap::new();
        for (system, ore, count) in rocks {
            let f = facts(ore);
            s.gated.extend(f.minerals.iter().map(|&(m, _)| m));
            s.minable
                .entry(system)
                .or_default()
                .extend(f.minerals.iter().map(|&(m, _)| m));
            let ore_units = count.max(0) as f64 * f.rock_units;
            let e = acc.entry(system).or_default();
            e.1 += ore_units * f.volume;
            for &(m, per) in &f.minerals {
                *e.0.entry(m).or_default() += ore_units * per;
            }
        }
        for (system, (units, m3)) in acc {
            if m3 > 0.0 {
                s.per_m3.insert(
                    system,
                    units.into_iter().map(|(m, u)| (m, u / m3)).collect(),
                );
            }
        }
        s
    }

    /// Units of `mineral` one m3 of `system`'s rock yields; `None` when the
    /// table has no sizes for the system.
    pub fn units_per_m3(&self, system: i64, mineral: i64) -> Option<f64> {
        let d = self.per_m3.get(&system)?;
        Some(d.get(&mineral).copied().unwrap_or(0.0))
    }

    /// Whether the table knows the sizes of `system`'s rocks.
    pub fn sized(&self, system: i64) -> bool {
        self.per_m3.contains_key(&system)
    }

    /// From `(system, ore type)` pairs and `yields`, the refine table: the
    /// mineral types one ore refines into.
    pub fn from_rocks(
        rocks: impl IntoIterator<Item = (i64, i64)>,
        yields: impl Fn(i64) -> Vec<i64>,
    ) -> Supply {
        let mut s = Supply::default();
        for (system, ore) in rocks {
            let minerals = yields(ore);
            s.gated.extend(minerals.iter().copied());
            s.minable.entry(system).or_default().extend(minerals);
        }
        s
    }

    /// From the text of `belts/rocks.tsv` (`#belt_id ordinal type_id count
    /// system ...`, tab separated, `#` comments).
    pub fn parse(rocks: &str, yields: impl Fn(i64) -> Vec<i64>) -> Result<Supply, String> {
        let rows = Self::rows(rocks)?;
        Ok(Supply::from_rocks(
            rows.into_iter().map(|(s, o, _)| (s, o)),
            yields,
        ))
    }

    /// [`Supply::parse`] with the rock counts and sizes ([`Supply::from_sized`]).
    pub fn parse_sized(rocks: &str, facts: impl Fn(i64) -> OreFacts) -> Result<Supply, String> {
        Ok(Supply::from_sized(Self::rows(rocks)?, facts))
    }

    /// `(system, ore type, count)` per row of `belts/rocks.tsv`.
    fn rows(rocks: &str) -> Result<Vec<(i64, i64, i64)>, String> {
        let mut pairs = Vec::new();
        for (n, line) in rocks.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            let int = |i: usize| -> Result<i64, String> {
                cols.get(i)
                    .ok_or_else(|| format!("rocks line {}: {} columns, want 5", n + 1, cols.len()))?
                    .parse::<i64>()
                    .map_err(|e| format!("rocks line {}, column {}: {e}", n + 1, i + 1))
            };
            pairs.push((int(4)?, int(2)?, int(3)?));
        }
        Ok(pairs)
    }

    /// The table of the bundle at `dir` over the refine yields `content`
    /// holds; `None` when the bundle has no rocks file or `content` no
    /// refine table.
    pub fn open(dir: &Path, content: &Content) -> Result<Option<Supply>, String> {
        let (bundle, _) = eve_server::content::Bundle::open(dir)?;
        let Some(path) = bundle.path(eve_server::content::ROCKS) else {
            return Ok(None);
        };
        let Some(refine) = content.try_get::<eve_server::reprocess::Table>() else {
            return Ok(None);
        };
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        // P5: rock size (the dungeon rooms' group means, else 1 unit [A]:
        // falsified by a bundle whose `Quantities` is empty yet whose belt
        // mixes differ in rock size) and ore volume (the type table).
        let q = eve_server::asteroid::Quantities::from_content(content);
        let types = content.try_get::<eve_server::typedata::Table>();
        let all: Vec<f64> = q
            .by_group
            .values()
            .map(|p| p.iter().sum::<i64>() as f64 / p.len() as f64)
            .collect();
        let overall = if all.is_empty() {
            1.0
        } else {
            all.iter().sum::<f64>() / all.len() as f64
        };
        Supply::parse_sized(&text, |ore| {
            let portion = refine.portion_size(ore).max(1) as f64;
            let rock_units = q
                .group_of
                .get(&ore)
                .and_then(|g| q.by_group.get(g))
                .filter(|p| !p.is_empty())
                .map_or(overall, |p| p.iter().sum::<i64>() as f64 / p.len() as f64);
            OreFacts {
                minerals: refine
                    .materials(ore)
                    .iter()
                    .map(|&(m, n)| (m, n as f64 / portion))
                    .collect(),
                volume: types
                    .and_then(|t| t.get(ore))
                    .map_or(0.0, |r| r.volume)
                    .max(0.0),
                rock_units,
            }
        })
        .map(Some)
    }

    /// Whether some ore somewhere yields `mineral`.
    pub fn gates(&self, mineral: i64) -> bool {
        self.gated.contains(&mineral)
    }

    /// Whether a miner of `system` can bring `material`: it is not a mineral
    /// at all, or an ore in `system` yields it.
    pub fn supplies(&self, system: i64, material: i64) -> bool {
        !self.gates(material)
            || self
                .minable
                .get(&system)
                .is_some_and(|m| m.contains(&material))
    }

    /// The materials of `materials` (`(type, quantity)` per run) that
    /// `system`'s miners cannot bring, in order.
    pub fn missing(&self, system: i64, materials: &[(i64, i64)]) -> Vec<i64> {
        materials
            .iter()
            .map(|&(t, _)| t)
            .filter(|&t| !self.supplies(system, t))
            .collect()
    }

    /// Whether every material of a recipe can come from `system`'s miners.
    pub fn can_make(&self, system: i64, materials: &[(i64, i64)]) -> bool {
        self.missing(system, materials).is_empty()
    }

    /// The minerals `system`'s rocks yield.
    pub fn minable_in(&self, system: i64) -> impl Iterator<Item = i64> + '_ {
        self.minable.get(&system).into_iter().flatten().copied()
    }

    /// How many systems have rocks.
    pub fn systems(&self) -> usize {
        self.minable.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids only.
    const HOME: i64 = 39_100_001;
    const FAR: i64 = 39_100_002;
    const BARE: i64 = 39_100_003;
    const ORE_A: i64 = 38_000_001;
    const ORE_B: i64 = 38_000_002;
    const ORE_C: i64 = 38_000_003;
    const MIN_X: i64 = 37_000_001;
    const MIN_Y: i64 = 37_000_002;
    const MIN_Z: i64 = 37_000_003;
    /// Not a mineral: no ore yields it.
    const COMPONENT: i64 = 37_000_999;

    fn yields(ore: i64) -> Vec<i64> {
        match ore {
            ORE_A => vec![MIN_X],
            ORE_B => vec![MIN_X, MIN_Y],
            ORE_C => vec![MIN_Z],
            _ => Vec::new(),
        }
    }

    fn table() -> Supply {
        Supply::from_rocks([(HOME, ORE_A), (HOME, ORE_B), (FAR, ORE_C)], yields)
    }

    #[test]
    fn a_system_supplies_what_its_ores_refine_into_and_no_more() {
        let s = table();
        assert!(s.supplies(HOME, MIN_X) && s.supplies(HOME, MIN_Y));
        assert!(!s.supplies(HOME, MIN_Z), "Z is mined only at FAR");
        assert!(s.supplies(FAR, MIN_Z) && !s.supplies(FAR, MIN_X));
        assert_eq!(s.minable_in(HOME).collect::<Vec<_>>(), vec![MIN_X, MIN_Y]);
        assert_eq!(s.systems(), 2);
    }

    #[test]
    fn a_system_without_rocks_supplies_no_mineral() {
        let s = table();
        assert!(!s.supplies(BARE, MIN_X));
        assert_eq!(s.minable_in(BARE).count(), 0);
    }

    #[test]
    fn a_material_no_ore_yields_is_never_gated() {
        let s = table();
        assert!(!s.gates(COMPONENT));
        assert!(s.supplies(BARE, COMPONENT), "other careers make it");
        assert!(s.can_make(BARE, &[(COMPONENT, 5)]));
    }

    #[test]
    fn a_recipe_is_makeable_only_when_every_mineral_is_supplied() {
        let s = table();
        assert!(s.can_make(HOME, &[(MIN_X, 100), (MIN_Y, 10), (COMPONENT, 1)]));
        let r = [(MIN_X, 100), (MIN_Z, 1)];
        assert!(!s.can_make(HOME, &r));
        assert_eq!(s.missing(HOME, &r), vec![MIN_Z]);
        assert!(s.can_make(HOME, &[]), "no materials, nothing missing");
    }

    #[test]
    fn the_rocks_file_parses_by_its_columns_and_a_short_row_is_refused() {
        let text = format!(
            "#belt_id\tordinal\ttype_id\tcount\tsystem\tgrowth\n\
             49000001\t0\t{ORE_A}\t12\t{HOME}\t0.5\n\
             \n\
             49000002\t1\t{ORE_C}\t3\t{FAR}\t0.5\n"
        );
        assert_eq!(Supply::parse(&text, yields).unwrap(), table_a_c());
        let short = "49000001\t0\t1\n";
        assert!(Supply::parse(short, yields).unwrap_err().contains("line 1"));
        let bad = "49000001\t0\tx\t1\t2\n";
        assert!(Supply::parse(bad, yields).unwrap_err().contains("column 3"));
    }

    #[test]
    fn units_per_m3_weigh_rocks_by_count_and_size() {
        let facts = |ore: i64| match ore {
            ORE_A => OreFacts {
                minerals: vec![(MIN_X, 4.0)],
                volume: 0.1,
                rock_units: 1000.0,
            },
            ORE_B => OreFacts {
                minerals: vec![(MIN_Y, 1.0)],
                volume: 1.0,
                rock_units: 100.0,
            },
            _ => OreFacts::default(),
        };
        let s = Supply::from_sized([(HOME, ORE_A, 3), (HOME, ORE_B, 1)], facts);
        // 3000 ore units x 0.1 m3 + 100 x 1 m3 = 400 m3.
        assert_eq!(s.units_per_m3(HOME, MIN_X), Some(12_000.0 / 400.0));
        assert_eq!(s.units_per_m3(HOME, MIN_Y), Some(100.0 / 400.0));
        assert_eq!(s.units_per_m3(HOME, MIN_Z), Some(0.0));
        assert_eq!(s.units_per_m3(FAR, MIN_X), None);
        assert!(s.supplies(HOME, MIN_Y) && s.sized(HOME));
    }

    fn table_a_c() -> Supply {
        Supply::from_rocks([(HOME, ORE_A), (FAR, ORE_C)], yields)
    }
}
