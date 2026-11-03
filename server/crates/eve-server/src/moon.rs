//! What a moon is made of: the composition table (R-MOON packet MN-B).
//!
//! Spec: `docs/lanes/mn-b.md`. Pure: no registry, no clock, no I/O. The belts
//! that carry this ore into space are `crate::moon_belt` (MN-C).
//!
//! ## What the client fixes and what is ours
//!
//! - **The ore universe is the client's, read from the type data at boot.**
//!   A moon ore is a type in one of the five groups
//!   `groupUbiquitousMoonAsteroids` 1884 .. `groupExceptionalMoonAsteroids`
//!   1923 **[V]** (`inventorycommon/const.py:1160-1164`; the reprocessing
//!   service's own `MOON_ORES` set, `reprocessing/__init__.py:46-53`, and the
//!   ledger's `MOON_ORE_GROUPS`, `ledgerUtil.py:34-39`, read the same five).
//!   Which base ore a variant is of, and how rich the variant is, are two type
//!   attributes the client's own ore ledger reads: `attributeOreBasicType`
//!   2711 and `attributeAsteroidMetaLevel` 2699 (`dogma/const.py:1422-1423`,
//!   `ledgerUtil.py:48-68`) **[V]**. The radius law's three attributes are
//!   1980, 1981 and 2727 (`dogma/const.py:1208-1210`) **[V]**. So no ore id is
//!   written in this file: [`OreTable::from_types`] finds them, and the
//!   compressed twins (same basic type, same meta level, a hundredth of the
//!   volume **[V-data]**) are told from the raw ore by the larger volume, not
//!   by a name.
//! - **Which moon holds what is ours, and `[A]` throughout.** The client ships
//!   no moon composition (`docs/content/README.md` §1 lists moon ore among the
//!   EvEJS gaps, and the 157,905-row moon-mining file of EvEJS was judged
//!   invented). What is retail's is the *shape*: five rarity bands, ubiquitous
//!   in every space, rarer ones in low and null. The numbers are drawn from the
//!   world seed, so a world has the same moons on every start.
//! - **A scan shows a fraction per ore, rounded to two places**: the client's
//!   gauge reads `mathext.clamp(round(quantity, 2), 0, 1.0)`
//!   (`eve/client/script/ui/inflight/moonscan.py`, R-MOON §1.5) **[V]**. A
//!   [`Share`] is therefore a whole percent, summing to exactly 100, so a scan
//!   shows exactly what the belt will hold.

use crate::asteroid::Law;
use crate::celestial::Band;
use crate::seed::{self, Stream};
use crate::typedata::TypeRow;
use std::collections::BTreeMap;

/// `groupUbiquitousMoonAsteroids` **[V]**.
pub const GROUP_UBIQUITOUS: i64 = 1884;
/// `groupCommonMoonAsteroids` **[V]**.
pub const GROUP_COMMON: i64 = 1920;
/// `groupUncommonMoonAsteroids` **[V]**.
pub const GROUP_UNCOMMON: i64 = 1921;
/// `groupRareMoonAsteroids` **[V]**.
pub const GROUP_RARE: i64 = 1922;
/// `groupExceptionalMoonAsteroids` **[V]**.
pub const GROUP_EXCEPTIONAL: i64 = 1923;

/// `attributeOreBasicType` **[V]** (`dogma/const.py:1422`).
pub const ATTR_ORE_BASIC_TYPE: i64 = 2711;
/// `attributeAsteroidMetaLevel` **[V]** (`dogma/const.py:1423`).
pub const ATTR_ASTEROID_META_LEVEL: i64 = 2699;
/// `attributeAsteroidRadiusGrowthFactor` **[V]**.
pub const ATTR_GROWTH: i64 = 1980;
/// `attributeAsteroidRadiusUnitSize` **[V]**.
pub const ATTR_UNIT_SIZE: i64 = 1981;
/// `attributeAsteroidMaxRadius` **[V]**.
pub const ATTR_MAX_RADIUS: i64 = 2727;

/// The meta level of the base ore: `GetMetaLevelByTypeID` answers 1 for a
/// type with no value (`ledgerUtil.py:67`) **[V]**.
pub const META_BASE: i64 = 1;

/// The richer variants' meta levels (3 and 5) **[V-data]**: the +15 % and the
/// +100 % ores of the same family, e.g. Brimful and Glistening.
pub const META_RICH: i64 = 3;
pub const META_RICHEST: i64 = 5;

/// A moon's ore rarity: the band the ore's group names. Ascending rarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rarity {
    /// R4, ubiquitous.
    Ubiquitous,
    /// R8, common.
    Common,
    /// R16, uncommon.
    Uncommon,
    /// R32, rare.
    Rare,
    /// R64, exceptional.
    Exceptional,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [
        Rarity::Ubiquitous,
        Rarity::Common,
        Rarity::Uncommon,
        Rarity::Rare,
        Rarity::Exceptional,
    ];

    /// The ore group of this rarity **[V]**.
    pub fn group_id(self) -> i64 {
        match self {
            Rarity::Ubiquitous => GROUP_UBIQUITOUS,
            Rarity::Common => GROUP_COMMON,
            Rarity::Uncommon => GROUP_UNCOMMON,
            Rarity::Rare => GROUP_RARE,
            Rarity::Exceptional => GROUP_EXCEPTIONAL,
        }
    }

    /// The rarity an ore group is, or `None` for any other group.
    pub fn of_group(group_id: i64) -> Option<Rarity> {
        Rarity::ALL.into_iter().find(|r| r.group_id() == group_id)
    }

    /// 0 for ubiquitous .. 4 for exceptional.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The weight a moon draws this rarity at: halved per tier. **[A]**
    /// Falsifier: the soak's `[econ]` report shows a rarity out of supply or
    /// in glut (R-MOON A2).
    pub fn weight(self) -> u64 {
        1 << (Rarity::Exceptional.index() - self.index())
    }

    /// Units of ore a belt holds when one moon is wholly this rarity (10 m3
    /// a unit **[V-data]**). **[A]** R-MOON A3; falsifier: goo price against
    /// bot mining income in the soak.
    pub fn total_units(self) -> i64 {
        match self {
            Rarity::Ubiquitous => 40_000,
            Rarity::Common => 20_000,
            Rarity::Uncommon => 10_000,
            Rarity::Rare => 5_000,
            Rarity::Exceptional => 2_500,
        }
    }
}

/// The rarities a moon in `band` may carry. **[A]** R-MOON A4: high-sec only
/// the ubiquitous four, low-sec up to uncommon, null-sec all five, and no moon
/// ore in wormhole or void space. Falsifier: the owner's risk preference.
pub fn allowed(band: Band) -> &'static [Rarity] {
    match band {
        Band::High => &Rarity::ALL[..1],
        Band::Low => &Rarity::ALL[..3],
        Band::Null => &Rarity::ALL,
        Band::Wormhole | Band::Void => &[],
    }
}

/// One moon ore type, as the type data describes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ore {
    pub type_id: i64,
    pub rarity: Rarity,
    /// The base ore of the family (`oreBasicType`).
    pub basic: i64,
    /// 1, 3 or 5.
    pub meta_level: i64,
    /// This ore's radius law (attributes 1980, 1981, 2727).
    pub law: Law,
}

/// Every uncompressed moon ore the type data holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OreTable {
    /// `(basic, meta level)` to the raw ore.
    ores: BTreeMap<(i64, i64), Ore>,
    /// Per rarity, the base ores that have a base-quality type, ascending.
    bases: [Vec<i64>; 5],
}

impl OreTable {
    /// Find the moon ores among `rows`. A type counts when its group is one
    /// of the five and it carries `oreBasicType` and a usable radius law; of
    /// two types with the same basic type and meta level (the raw ore and its
    /// compressed twin) the one with the larger volume is the raw ore.
    pub fn from_types<'a>(rows: impl IntoIterator<Item = &'a TypeRow>) -> OreTable {
        let mut best: BTreeMap<(i64, i64), (f64, Ore)> = BTreeMap::new();
        for t in rows {
            let Some(rarity) = Rarity::of_group(t.group_id) else {
                continue;
            };
            let Some(basic) = t.attribute(ATTR_ORE_BASIC_TYPE).map(|v| v as i64) else {
                continue;
            };
            let meta = t
                .attribute(ATTR_ASTEROID_META_LEVEL)
                .map_or(META_BASE, |v| v as i64);
            let (Some(growth), Some(unit_size), Some(max_radius)) = (
                t.attribute(ATTR_GROWTH),
                t.attribute(ATTR_UNIT_SIZE),
                t.attribute(ATTR_MAX_RADIUS),
            ) else {
                continue;
            };
            let law = Law {
                growth,
                unit_size,
                max_radius,
            };
            if ![growth, unit_size, max_radius]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
            {
                continue;
            }
            let ore = Ore {
                type_id: t.type_id,
                rarity,
                basic,
                meta_level: meta,
                law,
            };
            let slot = best.entry((basic, meta)).or_insert((t.volume, ore));
            // The larger volume is the raw ore; on a tie the lower id.
            if t.volume > slot.0 || (t.volume == slot.0 && t.type_id < slot.1.type_id) {
                *slot = (t.volume, ore);
            }
        }
        let mut table = OreTable::default();
        for ((basic, meta), (_, ore)) in best {
            if meta == META_BASE && ore.type_id == basic {
                table.bases[ore.rarity.index()].push(basic);
            }
            table.ores.insert((basic, meta), ore);
        }
        table
    }

    /// The base ores of a rarity, ascending.
    pub fn bases(&self, rarity: Rarity) -> &[i64] {
        &self.bases[rarity.index()]
    }

    /// The raw ore of a family at a meta level.
    pub fn ore(&self, basic: i64, meta_level: i64) -> Option<&Ore> {
        self.ores.get(&(basic, meta_level))
    }

    /// The ore a type id is, if the table holds it.
    pub fn ores_by_type(&self, type_id: i64) -> Option<&Ore> {
        self.ores.values().find(|o| o.type_id == type_id)
    }

    /// The rarity of a base ore.
    pub fn rarity_of(&self, basic: i64) -> Option<Rarity> {
        self.ore(basic, META_BASE).map(|o| o.rarity)
    }

    pub fn is_empty(&self) -> bool {
        self.bases.iter().all(Vec::is_empty)
    }
}

/// One ore of a moon's makeup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    /// The base ore (`oreBasicType`); the belt turns it into a quality.
    pub basic: i64,
    pub rarity: Rarity,
    /// Whole percent, 1..=100.
    pub percent: u8,
}

impl Share {
    /// What a scan shows: a fraction in `[0, 1]` at two places.
    pub fn fraction(&self) -> f64 {
        f64::from(self.percent) / 100.0
    }
}

/// The salt of a moon's composition stream. **Never reuse it** for another
/// generator (the discriminants of `seed::Kind` are for the same reason).
const SALT_COMPOSITION: u64 = 0x4d4f_4f4e_0000_0001;

/// How many ores a moon holds: 1, 2 or 3, at 50 / 35 / 15 percent. **[A]**
/// R-MOON §4.2; falsifier as [`Rarity::weight`].
pub const ORE_COUNT_ODDS: [(u64, usize); 3] = [(50, 1), (85, 2), (100, 3)];

/// A share's raw weight is drawn from this range, so no ore of three is
/// under a ninth of the moon. **[A]**
pub const SHARE_WEIGHT: (u64, u64) = (20, 100);

/// **A moon's makeup**, deterministic from `(seed, moon_id)` and the band:
/// one to three ores, each of a rarity the band allows (drawn with
/// [`Rarity::weight`]) and no ore twice, as whole percents summing to 100,
/// largest first. Empty when the band allows none, or the table has no ore of
/// any allowed rarity.
pub fn composition(seed: u64, moon_id: i64, band: Band, table: &OreTable) -> Vec<Share> {
    let rarities: Vec<Rarity> = allowed(band)
        .iter()
        .copied()
        .filter(|r| !table.bases(*r).is_empty())
        .collect();
    if rarities.is_empty() {
        return Vec::new();
    }
    let mut rng = Stream::new(seed::mix(seed, &[SALT_COMPOSITION, moon_id as u64]));
    let roll = rng.range(0, 100);
    let count = ORE_COUNT_ODDS
        .iter()
        .find(|(upto, _)| roll < *upto)
        .map_or(1, |(_, n)| *n);
    let total: u64 = rarities.iter().map(|r| r.weight()).sum();
    let mut picked: Vec<(i64, Rarity, u64)> = Vec::new();
    for _ in 0..count {
        // A few tries for a fresh ore, then the moon has one fewer.
        for _ in 0..8 {
            let mut at = rng.range(0, total);
            let rarity = rarities
                .iter()
                .copied()
                .find(|r| {
                    if at < r.weight() {
                        return true;
                    }
                    at -= r.weight();
                    false
                })
                .unwrap_or(rarities[0]);
            let bases = table.bases(rarity);
            let basic = bases[rng.range(0, bases.len() as u64) as usize];
            if picked.iter().all(|p| p.0 != basic) {
                let w = rng.range(SHARE_WEIGHT.0, SHARE_WEIGHT.1 + 1);
                picked.push((basic, rarity, w));
                break;
            }
        }
    }
    apportion(&picked)
}

/// Whole percents out of raw weights, by the largest remainder, each at
/// least 1 and summing to exactly 100; largest share first, ties by ore id.
fn apportion(picked: &[(i64, Rarity, u64)]) -> Vec<Share> {
    let sum: u64 = picked.iter().map(|p| p.2).sum();
    if sum == 0 {
        return Vec::new();
    }
    let mut out: Vec<(Share, u64)> = picked
        .iter()
        .map(|&(basic, rarity, w)| {
            let exact = w * 100;
            (
                Share {
                    basic,
                    rarity,
                    percent: (exact / sum).max(1) as u8,
                },
                exact % sum,
            )
        })
        .collect();
    let mut have: i64 = out.iter().map(|(s, _)| i64::from(s.percent)).sum();
    // Hand out what the floors left, biggest remainder first.
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by(|a, b| out[*b].1.cmp(&out[*a].1).then(a.cmp(b)));
    let mut i = 0;
    while have < 100 {
        out[order[i % order.len()]].0.percent += 1;
        have += 1;
        i += 1;
    }
    // A forced minimum may have overshot: take it back from the biggest.
    while have > 100 {
        let big = (0..out.len())
            .max_by_key(|k| out[*k].0.percent)
            .unwrap_or(0);
        out[big].0.percent -= 1;
        have -= 1;
    }
    let mut shares: Vec<Share> = out.into_iter().map(|(s, _)| s).collect();
    shares.sort_by(|a, b| b.percent.cmp(&a.percent).then(a.basic.cmp(&b.basic)));
    shares
}

/// The quality of a rock: base 70 %, the +15 % variant 25 %, the +100 %
/// variant 5 %. **[A]** R-MOON §4.2; falsifier: the mining ledger's mix in the
/// soak against the goo market.
pub const QUALITY_ODDS: [(f64, i64); 3] =
    [(0.70, META_BASE), (0.95, META_RICH), (1.0, META_RICHEST)];

/// The meta level a rock rolls: `unit` is uniform in `[0, 1)`.
pub fn quality(unit: f64) -> i64 {
    QUALITY_ODDS
        .iter()
        .find(|(upto, _)| unit < *upto)
        .map_or(META_BASE, |(_, m)| *m)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::typedata::Table;

    /// Invented ids, real groups (the client's constants). Four base ores
    /// per rarity at `100 * rarity + n`, each with a rich and a richest
    /// variant and a compressed twin of every one.
    pub(crate) fn fixture_text() -> String {
        let mut s = String::new();
        let groups = [
            GROUP_UBIQUITOUS,
            GROUP_COMMON,
            GROUP_UNCOMMON,
            GROUP_RARE,
            GROUP_EXCEPTIONAL,
        ];
        for (k, g) in groups.iter().enumerate() {
            for n in 0..4i64 {
                let basic = 990_000 + 1_000 * k as i64 + 10 * n;
                for (i, meta) in [META_BASE, META_RICH, META_RICHEST].iter().enumerate() {
                    let id = if *meta == META_BASE {
                        basic
                    } else {
                        basic + i as i64
                    };
                    let attrs =
                        format!("1980=0.75;1981=600.0;2699={meta}.0;2711={basic}.0;2727=5000.0");
                    s += &format!(
                        "type\t{id}\t{g}\t25\t1\t100\t10.0\t0.0\t4000.0\t1.0\t0\t\t{attrs}\tOre {id}\n"
                    );
                    // Its compressed twin, one rank up in id.
                    s += &format!(
                        "type\t{}\t{g}\t25\t1\t100\t0.1\t0.0\t4000.0\t1.0\t0\t\t{attrs}\tPacked {id}\n",
                        id + 500
                    );
                }
            }
        }
        // Not a moon ore: the right attributes in a plain ore group.
        s += "type\t990999\t458\t25\t1\t100\t0.1\t0.0\t4000.0\t1.0\t0\t\t1980=0.5;1981=90.0;2699=1.0;2711=990999.0;2727=5000.0\tPlain\n";
        s
    }

    pub(crate) fn table() -> OreTable {
        let t = Table::parse(&fixture_text()).expect("the fixture parses");
        OreTable::from_types(t.types())
    }

    const SEED: u64 = 0x5eed_0000_f1c5_7e57;

    #[test]
    fn the_table_finds_raw_moon_ores_only() {
        let t = table();
        for r in Rarity::ALL {
            assert_eq!(t.bases(r).len(), 4, "{r:?}");
        }
        let basic = t.bases(Rarity::Rare)[0];
        let base = t.ore(basic, META_BASE).expect("base");
        assert_eq!(base.type_id, basic, "the raw ore, not its packed twin");
        assert_eq!(base.law.unit_size, 600.0);
        let rich = t.ore(basic, META_RICH).expect("rich");
        assert_ne!(rich.type_id, basic);
        assert!(t.ore(basic, META_RICHEST).is_some());
        assert!(
            t.ore(990_999, META_BASE).is_none(),
            "a plain ore is not a moon ore"
        );
        assert_eq!(t.rarity_of(basic), Some(Rarity::Rare));
        for o in t.ores.values() {
            assert!(o.type_id % 1_000 < 500, "a packed twin got in: {o:?}");
        }
    }

    #[test]
    fn composition_is_deterministic_from_the_seed() {
        let t = table();
        for moon in [40_000_001, 40_000_002, 40_123_456] {
            let a = composition(SEED, moon, Band::Null, &t);
            assert_eq!(a, composition(SEED, moon, Band::Null, &t));
            assert!(!a.is_empty());
        }
        let diff = (0..200)
            .filter(|m| {
                composition(SEED, *m, Band::Null, &t) != composition(SEED + 1, *m, Band::Null, &t)
            })
            .count();
        assert!(diff > 150, "another seed is another universe: {diff}");
        let first = composition(SEED, 0, Band::Null, &t);
        assert!(
            (1..200).any(|m| composition(SEED, m, Band::Null, &t) != first),
            "moons differ"
        );
    }

    #[test]
    fn composition_respects_the_band() {
        let t = table();
        let seen = |band| {
            let mut s = std::collections::BTreeSet::new();
            for m in 0..3_000 {
                for sh in composition(SEED, m, band, &t) {
                    s.insert(sh.rarity);
                }
            }
            s
        };
        assert_eq!(seen(Band::High), [Rarity::Ubiquitous].into());
        assert_eq!(
            seen(Band::Low),
            [Rarity::Ubiquitous, Rarity::Common, Rarity::Uncommon].into()
        );
        assert_eq!(seen(Band::Null), Rarity::ALL.into());
        assert!(composition(SEED, 1, Band::Wormhole, &t).is_empty());
        assert!(composition(SEED, 1, Band::Void, &t).is_empty());
    }

    #[test]
    fn fractions_sum_to_one() {
        let t = table();
        let mut counts = [0usize; 4];
        for m in 0..2_000 {
            let c = composition(SEED, m, Band::Null, &t);
            assert!((1..=3).contains(&c.len()), "{c:?}");
            let sum: u32 = c.iter().map(|s| u32::from(s.percent)).sum();
            assert_eq!(sum, 100, "{c:?}");
            assert!(c.iter().all(|s| s.percent >= 1));
            let f: f64 = c.iter().map(Share::fraction).sum();
            assert!((f - 1.0).abs() < 1e-9);
            let mut ids: Vec<i64> = c.iter().map(|s| s.basic).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), c.len(), "no ore twice");
            assert!(c.windows(2).all(|w| w[0].percent >= w[1].percent));
            counts[c.len()] += 1;
        }
        assert!(counts[1] > counts[2] && counts[2] > counts[3], "{counts:?}");
        assert!(counts[3] > 0);
    }

    #[test]
    fn a_rarer_ore_is_rarer() {
        let t = table();
        let mut by = [0usize; 5];
        for m in 0..4_000 {
            for s in composition(SEED, m, Band::Null, &t) {
                by[s.rarity.index()] += 1;
            }
        }
        assert!(
            by.windows(2).all(|w| w[0] > w[1]),
            "each tier less common than the last: {by:?}"
        );
        assert!(by[4] > 0);
    }

    #[test]
    fn a_table_without_a_rarity_skips_it() {
        let text = fixture_text();
        let only_common: String = text
            .lines()
            .filter(|l| {
                l.split('\t')
                    .nth(2)
                    .and_then(|g| g.parse::<i64>().ok())
                    .is_some_and(|g| g == GROUP_COMMON)
            })
            .map(|l| format!("{l}\n"))
            .collect();
        let t = Table::parse(&only_common).expect("parses");
        let ores = OreTable::from_types(t.types());
        for m in 0..200 {
            let c = composition(SEED, m, Band::Null, &ores);
            assert!(!c.is_empty() && c.iter().all(|s| s.rarity == Rarity::Common));
        }
        assert!(composition(SEED, 1, Band::Null, &OreTable::default()).is_empty());
        assert!(OreTable::default().is_empty());
    }

    #[test]
    fn rarity_weights_halve_and_totals_follow_the_spec_shape() {
        let w: Vec<u64> = Rarity::ALL.iter().map(|r| r.weight()).collect();
        assert!(w.windows(2).all(|p| p[0] == 2 * p[1]));
        let u: Vec<i64> = Rarity::ALL.iter().map(|r| r.total_units()).collect();
        assert!(u.windows(2).all(|p| p[0] > p[1]));
        assert_eq!(Rarity::of_group(GROUP_RARE), Some(Rarity::Rare));
        assert_eq!(Rarity::of_group(458), None);
    }

    #[test]
    fn quality_follows_the_odds() {
        assert_eq!(quality(0.0), META_BASE);
        assert_eq!(quality(QUALITY_ODDS[0].0 - 1e-9), META_BASE);
        assert_eq!(quality(QUALITY_ODDS[0].0), META_RICH);
        assert_eq!(quality(QUALITY_ODDS[1].0), META_RICHEST);
        assert_eq!(quality(0.999_999), META_RICHEST);
    }

    #[test]
    fn apportion_always_makes_one_hundred() {
        for a in 20..=100u64 {
            for b in [20u64, 55, 100] {
                for c in [20u64, 99] {
                    let p = [
                        (1, Rarity::Common, a),
                        (2, Rarity::Common, b),
                        (3, Rarity::Rare, c),
                    ];
                    let s = apportion(&p);
                    assert_eq!(s.iter().map(|x| u32::from(x.percent)).sum::<u32>(), 100);
                    assert!(s.iter().all(|x| x.percent >= 1));
                }
            }
        }
        assert!(apportion(&[]).is_empty());
    }
}
