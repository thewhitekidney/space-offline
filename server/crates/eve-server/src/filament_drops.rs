//! **Filament drops** (FIL-DROP, issue #61): how a filament gets into a
//! closed world from an NPC kill or a hacked site, so the Abyss and the
//! random-jump filaments need no GM.
//!
//! The authored table `content/pve/filament-drops.tsv` gives, per source (an
//! NPC class or a site archetype), kind (abyssal key or random-jump filament)
//! and security band, a chance in parts per million. No type id is authored:
//! [`candidates`] reads the filament types from the loaded type data (abyssal
//! keys by their group, random-jump filaments by `random_jump`'s test), and
//! [`roll`] picks one.
//!
//! - **tier** of an abyssal key follows the system's band
//!   ([`tier_for_band`]) **[A]**; falsified by players finding the tiers they
//!   get useless for the space they rat in. A band with no key of that tier
//!   falls back to the next lower tier that has one.
//! - **weather** is `any`, or a word the key type's name must contain
//!   (case-blind) **[A]**; falsified by key types whose names do not carry
//!   their weather.
//! - **rates** are all **[A]** until the owner decides the filament sources.
//! - **off**: a row whose `source` is `off` (the world switch), or
//!   [`Table::enabled`] set false by a world setting, drops nothing.
//!
//! This module is pure: [`roll`] takes a [`Stream`] and the candidates and
//! returns a type id. The two loot paths call it (the maintainer's lines are
//! in the lane report): the NPC wreck's loot (`combat::npc_wreck`) with
//! `Source::Npc` and the NPC's class, and a hacked can's loot
//! (`hacking_loot::on_won`) with `Source::Site` and the site archetype.

use crate::authored::Row;
use crate::celestial::Band;
use crate::content::{Boxed, Content, Sources, TableSpec};
use crate::seed::{self, Stream};

/// The authored file, relative to `--authored`.
pub const FILE: &str = "pve/filament-drops.tsv";

/// Its header, copied into `docs/content/schema.md`.
pub const HEADER: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/pve/filament-drops.tsv");

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "filament_drops",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    Ok(Box::new(Table::from_rows(&file.parse_rows(parse_row))))
}

/// One draw's denominator: `chance_ppm` is parts per million.
pub const PPM: u64 = 1_000_000;

/// A domain tag in the loot stream's parts, so a filament draw never meets
/// another loot draw on the same id.
const STREAM_TAG: u64 = 0x0046_494c;

/// `[A]` the abyssal tier a kill in each band drops: high 1, low 2, null and
/// wormhole 3. Falsified by players finding the tiers they get useless for
/// the space they rat in (for example null-sec ratters who only ever run
/// tier 3, or high-sec ones who cannot use a tier 1 key).
pub fn tier_for_band(band: Band) -> i64 {
    match band {
        Band::High => 1,
        Band::Low => 2,
        Band::Null | Band::Wormhole | Band::Void => 3,
    }
}

/// Where a drop is rolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A killed NPC; the name is its class.
    Npc,
    /// A hacked can; the name is its site's archetype.
    Site,
}

/// What a filament is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An abyssal key.
    Abyss,
    /// A random-jump filament.
    Jump,
}

/// One authored row.
#[derive(Debug, Clone, PartialEq)]
pub struct DropRow {
    pub source: Source,
    /// Lower-case; `*` is any.
    pub name: String,
    pub kind: Kind,
    /// `None` is every band.
    pub bands: Option<Vec<Band>>,
    pub chance_ppm: u64,
    /// Lower-case; `any` is `None`.
    pub weather: Option<String>,
}

/// A filament type a drop may name, read from the type data.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub type_id: i64,
    pub kind: Kind,
    /// An abyssal key's tier; 0 for a jump filament.
    pub tier: i64,
    /// The type's name, lower-case, for the weather match.
    pub name: String,
}

/// The table, with its world switch.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    rows: Vec<DropRow>,
    /// False when a world setting or an `off` row turned the drops off.
    pub enabled: bool,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            rows: Vec::new(),
            enabled: true,
        }
    }
}

impl Table {
    /// Invented rows, for tests: never a shipped rate.
    pub fn fixture() -> Table {
        Table::default()
    }

    /// The table of already-parsed rows; `None` rows (the `on` and `off`
    /// switch rows) are folded in: any `off` disables.
    fn from_rows(rows: &[Parsed]) -> Table {
        let mut t = Table::default();
        for r in rows {
            match r {
                Parsed::Drop(d) => t.rows.push(d.clone()),
                Parsed::Off => t.enabled = false,
                Parsed::On => {}
            }
        }
        t
    }

    /// The table from authored text (tests, and the loader's pure core).
    pub fn parse(text: &str) -> Result<Table, String> {
        let (rows, _) = crate::authored::parse(text, HEADER)?;
        let parsed: Vec<Parsed> = rows.iter().filter_map(|r| parse_row(r).ok()).collect();
        Ok(Table::from_rows(&parsed))
    }

    pub fn rows(&self) -> &[DropRow] {
        &self.rows
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Parsed {
    Drop(DropRow),
    On,
    Off,
}

fn parse_bands(s: &str) -> Result<Option<Vec<Band>>, String> {
    if s == "*" {
        return Ok(None);
    }
    s.chars()
        .map(|c| match c {
            'h' => Ok(Band::High),
            'l' => Ok(Band::Low),
            'n' => Ok(Band::Null),
            'w' => Ok(Band::Wormhole),
            _ => Err(format!("band {s:?} has {c:?}, not h, l, n or w")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn parse_row(r: &Row) -> Result<Parsed, String> {
    let source = match r.field(0) {
        "on" => return Ok(Parsed::On),
        "off" => return Ok(Parsed::Off),
        "npc" => Source::Npc,
        "site" => Source::Site,
        s => return Err(format!("source {s:?} is not npc, site, on or off")),
    };
    let kind = match r.field(2) {
        "abyss" => Kind::Abyss,
        "jump" => Kind::Jump,
        k => return Err(format!("kind {k:?} is not abyss or jump")),
    };
    let ppm = r.i64(4)?;
    if !(0..=PPM as i64).contains(&ppm) {
        return Err(format!("chance_ppm {ppm} is outside 0..=1000000"));
    }
    Ok(Parsed::Drop(DropRow {
        source,
        name: r.field(1).to_lowercase(),
        kind,
        bands: parse_bands(r.field(3))?,
        chance_ppm: ppm as u64,
        weather: match r.field(5).to_lowercase().as_str() {
            "any" | "-" | "" => None,
            w => Some(w.to_owned()),
        },
    }))
}

/// Every filament type in the loaded type data: abyssal keys (their group,
/// with a tier) and random-jump filaments, by type id.
pub fn candidates(content: &Content) -> Vec<Candidate> {
    let table = crate::typedata::installed_in(content).table();
    let mut out: Vec<Candidate> = Vec::new();
    for t in table.types().filter(|t| t.published) {
        if t.group_id == crate::abyss::KEY_GROUP {
            if let Some(tier) = t.attribute(crate::abyss::ATTR_TIER) {
                out.push(Candidate {
                    type_id: t.type_id,
                    kind: Kind::Abyss,
                    tier: tier as i64,
                    name: t.name.to_lowercase(),
                });
            }
        } else if crate::random_jump::filament_type(content, t.type_id).is_some() {
            out.push(Candidate {
                type_id: t.type_id,
                kind: Kind::Jump,
                tier: 0,
                name: t.name.to_lowercase(),
            });
        }
    }
    out.sort_by_key(|c| c.type_id);
    out
}

/// The stream for the drop of one kill or can: `id` is the NPC's or the
/// can's, so a replay draws the same.
pub fn stream(seed: u64, source: Source, id: i64) -> Stream {
    seed::stream(
        seed,
        seed::Kind::Loot,
        &[STREAM_TAG, source as u64, id as u64],
    )
}

/// The filament type, if any, a `source` named `name` in `band` drops. One
/// draw per matching row, in file order; the first hit picks a candidate of
/// its kind (an abyssal key of [`tier_for_band`], or the nearest lower tier
/// that has one; and of its weather), uniformly. A hit with no candidate
/// drops nothing. Draws are always taken, so a table's rows never shift each
/// other's results.
pub fn roll(
    table: &Table,
    source: Source,
    name: &str,
    band: Band,
    candidates: &[Candidate],
    s: &mut Stream,
) -> Option<i64> {
    if !table.enabled {
        return None;
    }
    let name = name.to_lowercase();
    let mut won: Option<i64> = None;
    for row in &table.rows {
        if row.source != source
            || (row.name != "*" && row.name != name)
            || row.bands.as_ref().is_some_and(|b| !b.contains(&band))
        {
            continue;
        }
        let hit = s.range(0, PPM) < row.chance_ppm;
        let pick = s.next();
        if hit && won.is_none() {
            won = choose(row, band, candidates, pick);
        }
    }
    won
}

fn choose(row: &DropRow, band: Band, candidates: &[Candidate], pick: u64) -> Option<i64> {
    let fits = |c: &&Candidate| {
        c.kind == row.kind
            && row
                .weather
                .as_ref()
                .is_none_or(|w| row.kind == Kind::Jump || c.name.contains(w.as_str()))
    };
    let mut pool: Vec<&Candidate> = candidates.iter().filter(fits).collect();
    if row.kind == Kind::Abyss {
        let want = tier_for_band(band);
        let best = pool.iter().map(|c| c.tier).filter(|t| *t <= want).max()?;
        pool.retain(|c| c.tier == best);
    }
    if pool.is_empty() {
        return None;
    }
    Some(pool[(pick % pool.len() as u64) as usize].type_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented rows and types: no shipped rate, no client id.
    const ROWS: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
        npc\tgizmo\tabyss\thl\t1000000\tany\tinvented\n\
        npc\tgizmo\tjump\t*\t0\tany\tinvented\n\
        npc\t*\tjump\tw\t1000000\tany\tinvented\n\
        npc\twidget\tabyss\t*\t1000000\tzephyr\tinvented\n\
        site\tgadget\tjump\tw\t1000000\tany\tinvented\n\
        site\tgadget\tbogus\tw\t5\tany\tinvented\n";

    fn cand(type_id: i64, kind: Kind, tier: i64, name: &str) -> Candidate {
        Candidate {
            type_id,
            kind,
            tier,
            name: name.to_owned(),
        }
    }

    fn pool() -> Vec<Candidate> {
        vec![
            cand(9001, Kind::Abyss, 1, "calm zephyr key"),
            cand(9002, Kind::Abyss, 1, "calm quill key"),
            cand(9003, Kind::Abyss, 3, "wild zephyr key"),
            cand(9101, Kind::Jump, 0, "hop"),
        ]
    }

    fn run(t: &Table, src: Source, name: &str, band: Band, seed: u64) -> Option<i64> {
        roll(t, src, name, band, &pool(), &mut stream(seed, src, 7))
    }

    #[test]
    fn tier_follows_the_band_and_falls_back_lower() {
        let t = Table::parse(ROWS).unwrap();
        // High space wants tier 1: both tier-1 keys are possible, tier 3 never.
        let seen: Vec<_> = (0..40)
            .filter_map(|n| run(&t, Source::Npc, "gizmo", Band::High, n))
            .collect();
        assert!(seen.contains(&9001) && seen.contains(&9002));
        assert!(!seen.contains(&9003));
        // Low wants tier 2: none exists, so tier 1 again.
        assert!(matches!(
            run(&t, Source::Npc, "gizmo", Band::Low, 1),
            Some(9001 | 9002)
        ));
        // A tier-3 band finds tier 3 for the weather-bound widget row.
        assert_eq!(run(&t, Source::Npc, "widget", Band::Null, 1), Some(9003));
    }

    #[test]
    fn band_name_and_source_filter_rows() {
        let t = Table::parse(ROWS).unwrap();
        // gizmo has no row for null space; the any-name jump row hits in a wormhole.
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::Null, 3), None);
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::Wormhole, 3), Some(9101));
        assert!(run(&t, Source::Npc, "GIZMO", Band::High, 3).is_some());
        assert_eq!(
            run(&t, Source::Site, "gadget", Band::Wormhole, 3),
            Some(9101)
        );
        assert_eq!(run(&t, Source::Site, "gadget", Band::High, 3), None);
        assert_eq!(run(&t, Source::Site, "gizmo", Band::High, 3), None);
    }

    #[test]
    fn weather_matches_the_type_name() {
        let t = Table::parse(ROWS).unwrap();
        for n in 0..20 {
            let got = run(&t, Source::Npc, "widget", Band::High, n);
            assert_eq!(got, Some(9001), "only the zephyr key of tier 1");
        }
    }

    #[test]
    fn a_zero_chance_row_and_an_empty_pool_drop_nothing() {
        let t = Table::parse(ROWS).unwrap();
        let mut s = stream(1, Source::Npc, 7);
        assert_eq!(
            roll(&t, Source::Npc, "gizmo", Band::Null, &pool(), &mut s),
            None
        );
        let mut s = stream(1, Source::Npc, 7);
        assert_eq!(
            roll(&t, Source::Npc, "gizmo", Band::High, &[], &mut s),
            None
        );
    }

    #[test]
    fn fixed_seed_is_deterministic_and_chance_is_a_rate() {
        let t = Table::parse(
            "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
             npc\tgizmo\tjump\t*\t250000\tany\tinvented\n",
        )
        .unwrap();
        assert_eq!(
            run(&t, Source::Npc, "gizmo", Band::High, 5),
            run(&t, Source::Npc, "gizmo", Band::High, 5)
        );
        let hits = (0..4000)
            .filter(|n| run(&t, Source::Npc, "gizmo", Band::High, *n).is_some())
            .count();
        assert!((800..1200).contains(&hits), "{hits} of 4000 at 25%");
    }

    #[test]
    fn off_row_and_setting_turn_drops_off() {
        let off = format!("{ROWS}off\t*\t-\t*\t0\t-\tinvented\n");
        let t = Table::parse(&off).unwrap();
        assert!(!t.enabled);
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::High, 1), None);
        let mut t = Table::parse(ROWS).unwrap();
        assert!(run(&t, Source::Npc, "gizmo", Band::High, 1).is_some());
        t.enabled = false;
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::High, 1), None);
    }

    #[test]
    fn bad_rows_are_skipped_not_fatal() {
        let t = Table::parse(ROWS).unwrap();
        assert_eq!(t.rows().len(), 5, "the bogus-kind row is skipped");
        let bad = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
                   npc\ta\tjump\tx\t5\tany\twhy\n\
                   npc\ta\tjump\t*\t2000000\tany\twhy\n";
        assert!(Table::parse(bad).unwrap().rows().is_empty());
    }

    #[test]
    fn the_shipped_file_parses_clean_and_stays_on() {
        let (rows, skipped) = crate::authored::parse(SHIPPED, HEADER).unwrap();
        assert!(skipped.is_empty());
        let parsed: Vec<_> = rows.iter().map(parse_row).collect();
        assert!(parsed.iter().all(|p| p.is_ok()), "{parsed:?}");
        let t = Table::parse(SHIPPED).unwrap();
        assert!(t.enabled && !t.rows().is_empty());
    }
}
