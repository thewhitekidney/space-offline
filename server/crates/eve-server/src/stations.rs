//! Every NPC station's owner and operation: what turns a station in the
//! celestial catalogue into one a pilot can dock at (FX4).
//!
//! **The catalogue knows where a station is; this knows whose it is.** The
//! position, type and system come from `universe/celestials.tsv`
//! (`crate::celestial`), and the owner, operation and region from
//! `stations/stations.tsv` (`docs/content/schema.md`). Together they make a
//! [`crate::service::character::StationRef`] for any of the 5,210 stations,
//! not only the ones some character was seeded at --
//! `Registry::station_ref`.
//!
//! **The name is the client's own.** `EveConfig.LoadEveLocations` writes
//! every NPC station into `cfg.evelocations` from its own static data,
//! named by `GetNPCStationNameFromLocalRow` and placed at its own `x, y, z`
//! (`eveCfg.pyc:896-902`) **[V]**, so a bracket names a station without
//! asking us, and the position we serve has to be that one.
//!
//! With no bundle the table is empty and only seeded stations exist, which
//! is the behaviour before this module.

use crate::content::{Boxed, Origin, Sources, TableSpec};
use std::collections::BTreeMap;

/// The registration row in `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "stations",
    load,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// The bundle file, and the header `schema.md` freezes.
pub const FILE: (&str, &str) = crate::econ_exchange::STATIONS;

/// One `stations/stations.tsv` row, the columns docking reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Station {
    pub system_id: i64,
    pub region_id: i64,
    pub owner_id: i64,
    pub operation_id: i64,
}

/// Station id -> its row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    rows: BTreeMap<i64, Station>,
}

impl Table {
    pub fn get(&self, station_id: i64) -> Option<&Station> {
        self.rows.get(&station_id)
    }

    /// The stations an owner holds: solar system -> station count.
    pub fn by_owner(&self, owner_id: i64) -> BTreeMap<i64, i64> {
        let mut out = BTreeMap::new();
        for st in self.rows.values().filter(|s| s.owner_id == owner_id) {
            *out.entry(st.system_id).or_insert(0) += 1;
        }
        out
    }

    /// The ids of the stations an owner holds, in id order.
    pub fn ids_of_owner(&self, owner_id: i64) -> Vec<(i64, i64)> {
        self.rows
            .iter()
            .filter(|(_, s)| s.owner_id == owner_id)
            .map(|(id, s)| (*id, s.system_id))
            .collect()
    }

    /// Every station in id order (MAP-STATIONS).
    pub fn iter(&self) -> impl Iterator<Item = (i64, &Station)> {
        self.rows.iter().map(|(id, s)| (*id, s))
    }

    /// Solar system -> how many stations it holds (MAP-STATIONS).
    pub fn count_by_system(&self) -> BTreeMap<i64, i64> {
        let mut out = BTreeMap::new();
        for st in self.rows.values() {
            *out.entry(st.system_id).or_insert(0) += 1;
        }
        out
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// From the file's data lines. A malformed line is an error naming it.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
        let mut rows = BTreeMap::new();
        for (i, l) in lines.iter().enumerate() {
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let (id, row) = parse_row(&f)
                .ok_or_else(|| format!("{}: data line {}: {:?}", FILE.0, i + 1, l.as_ref()))?;
            rows.insert(id, row);
        }
        Ok(Table { rows })
    }

    #[cfg(test)]
    pub fn insert(&mut self, station_id: i64, row: Station) {
        self.rows.insert(station_id, row);
    }
}

fn parse_row(f: &[&str]) -> Option<(i64, Station)> {
    let [id, system, region, owner, op, _services, _eff, _take] = f[..] else {
        return None;
    };
    Some((
        id.parse().ok()?,
        Station {
            system_id: system.parse().ok()?,
            region_id: region.parse().ok()?,
            owner_id: owner.parse().ok()?,
            operation_id: op.parse().ok()?,
        },
    ))
}

fn load(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => {
            src.note(Origin::Bundle);
            Table::parse(&b.data_lines(FILE.0, FILE.1)?)?
        }
        None => Table::default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented ids (`schema.md` §1.1), never a client value.
    #[test]
    fn a_row_reads_owner_operation_and_region() {
        let t = Table::parse(&["69000001\t39000001\t19000001\t1999001\t99\t7,21\t0.5\t0.05"])
            .expect("parses");
        assert_eq!(
            t.get(69_000_001),
            Some(&Station {
                system_id: 39_000_001,
                region_id: 19_000_001,
                owner_id: 1_999_001,
                operation_id: 99,
            })
        );
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn by_owner_counts_stations_per_system() {
        let mut t = Table::default();
        let row = |system_id, owner_id| Station {
            system_id,
            region_id: 1,
            owner_id,
            operation_id: 1,
        };
        t.insert(60_001, row(30_001, 1_001));
        t.insert(60_002, row(30_001, 1_001));
        t.insert(60_003, row(30_002, 1_001));
        t.insert(60_004, row(30_003, 1_002));
        let m = t.by_owner(1_001);
        assert_eq!(m.len(), 2);
        assert_eq!(m[&30_001], 2);
        assert_eq!(m[&30_002], 1);
        assert_eq!(t.by_owner(1_002).len(), 1);
        assert!(t.by_owner(9).is_empty());
    }

    #[test]
    fn a_short_row_is_an_error_naming_its_line() {
        let e = Table::parse(&["69000001\t39000001"]).expect_err("short");
        assert!(e.contains("data line 1"), "{e}");
    }
}
