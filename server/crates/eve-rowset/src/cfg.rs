//! The concrete `config.GetMulti*Ex` replies of `docs/reference/login-path.md` §8
//! row 9, built from plain Rust data.
//!
//! `cfg.eveowners`, `cfg.evelocations` and `cfg.corptickernames` are
//! `cfg.Recordset`s whose `dbMultiFetcher` is one of these calls. A miss
//! during `Prime` fetches the whole batch; keys not returned are remembered
//! in `knownLuzers` and never asked for again, so a reply that drops a key
//! permanently loses it for the session.
//!
//! # The column order is not ours to choose
//!
//! **Confirmed** from the bytecode of `eveCfg.LoadEveOwners` and
//! `eveCfg.LoadEveLocations`: before any remote call, the client pre-sets
//!
//! ```python
//! self.eveowners.header    = ['ownerID', 'ownerName', 'typeID', 'gender', 'ownerNameID']
//! self.evelocations.header = ['locationID', 'locationName', 'solarSystemID',
//!                             'x', 'y', 'z', 'locationNameID']
//! ```
//!
//! and fills `Recordset.data` with `blue.DBRow`s packed against
//! `GetOwnersRowDescriptor()` / `GetLocationRowDescriptor()`, whose columns
//! are in exactly that order. `Recordset._Prime` then **replaces**
//! `self.header` with whatever the server sent, while leaving those
//! pre-loaded rows in place. A server that reorders, renames or drops a
//! column therefore corrupts the client's own NPC data, not just its reply.
//!
//! (`docs/reference/dbrow.md` §2's worked example lists `ownerNameID` before `gender`.
//! The client's source has `gender` fourth; `docs/reference/login-path.md` §8 agrees
//! with the source. The order below follows the source.)

use crate::crowset::CRowset;
use crate::row::Rowset;
use crate::{RowsetError, row};
use eve_marshal::dbrow::{DbRowDescriptor, DbValue};
use eve_marshal::{DbType, PyValue};

/// `eveCfg.GetOwnersRowDescriptor()`, column for column. **Confirmed.**
pub const OWNER_COLUMNS: [(&str, DbType); 5] = [
    ("ownerID", DbType::I4),
    ("ownerName", DbType::WStr),
    ("typeID", DbType::I4),
    ("gender", DbType::I2),
    ("ownerNameID", DbType::I4),
];

/// `eveCfg.GetLocationRowDescriptor()`, column for column. **Confirmed.**
pub const LOCATION_COLUMNS: [(&str, DbType); 7] = [
    ("locationID", DbType::I8),
    ("locationName", DbType::WStr),
    ("solarSystemID", DbType::I8),
    ("x", DbType::R5),
    ("y", DbType::R5),
    ("z", DbType::R5),
    ("locationNameID", DbType::I4),
];

/// `npcs.npccorporations.CorporationTickerProps`, the header
/// `cfg.corptickernames` is pre-set with. **Confirmed** from that module's
/// constant pool.
///
/// There is **no `DBRowDescriptor` for these on the client** — the client
/// pre-populates the recordset from `namedtuple`s, not packed rows — so this
/// crate offers only the `(header, data)` form for corporation tickers.
/// Inventing column types for a `CRowset` here would be a guess.
pub const CORP_TICKER_COLUMNS: [&str; 8] = [
    "corporationID",
    "tickerName",
    "shape1",
    "shape2",
    "shape3",
    "color1",
    "color2",
    "color3",
];

/// The descriptor `config.GetMultiOwnersEx` rows are packed against.
pub fn owners_descriptor() -> DbRowDescriptor {
    descriptor(&OWNER_COLUMNS)
}

/// The descriptor `config.GetMultiLocationsEx` rows are packed against.
pub fn locations_descriptor() -> DbRowDescriptor {
    descriptor(&LOCATION_COLUMNS)
}

/// Both column lists are fixed and valid, so a failure here is a bug in this
/// module rather than something a caller can cause.
fn descriptor(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(columns).expect("a fixed, valid column list")
}

/// One row of `config.GetMultiOwnersEx`.
///
/// `gender` and `owner_name_id` are optional because the client's own data
/// leaves them `None` for corporations and factions
/// (`eveCfg.LoadEveOwners`), which is what the NULL bitmap is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub owner_id: i32,
    pub owner_name: String,
    /// `typeID`, e.g. `const.typeCharacter` / `typeCorporation` /
    /// `typeFaction`. `EveOwners.groupID` derives from it, and
    /// `IsCharacter()`/`IsCorporation()` from that, so it must not be left
    /// at 0 for anything the UI classifies.
    pub type_id: i32,
    pub gender: Option<i16>,
    /// A localization message id. `None` means "use `ownerName` verbatim",
    /// which is what a server with no localization table wants.
    pub owner_name_id: Option<i32>,
}

impl Owner {
    /// An owner with no gender and no localized name — the common case for a
    /// player character, corporation or alliance.
    pub fn new(owner_id: i32, owner_name: impl Into<String>, type_id: i32) -> Self {
        Owner {
            owner_id,
            owner_name: owner_name.into(),
            type_id,
            gender: None,
            owner_name_id: None,
        }
    }

    #[must_use]
    pub fn with_gender(mut self, gender: i16) -> Self {
        self.gender = Some(gender);
        self
    }

    fn values(&self) -> Vec<DbValue> {
        vec![
            DbValue::Int(self.owner_id.into()),
            DbValue::Object(PyValue::Unicode(self.owner_name.clone())),
            DbValue::Int(self.type_id.into()),
            match self.gender {
                Some(g) => DbValue::Int(g.into()),
                None => DbValue::Null,
            },
            match self.owner_name_id {
                Some(n) => DbValue::Int(n.into()),
                None => DbValue::Null,
            },
        ]
    }

    fn cells(&self) -> Vec<PyValue> {
        self.values().iter().map(cell).collect()
    }
}

/// One row of `config.GetMultiLocationsEx`.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub location_id: i64,
    pub location_name: String,
    /// `None` for a region or constellation; the client's own loader leaves
    /// it `None` for both (`eveCfg.LoadEveLocations`).
    pub solar_system_id: Option<i64>,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub location_name_id: Option<i32>,
}

impl Location {
    pub fn new(location_id: i64, location_name: impl Into<String>) -> Self {
        Location {
            location_id,
            location_name: location_name.into(),
            solar_system_id: None,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            location_name_id: None,
        }
    }

    #[must_use]
    pub fn at(mut self, solar_system_id: i64, x: f64, y: f64, z: f64) -> Self {
        self.solar_system_id = Some(solar_system_id);
        self.x = x;
        self.y = y;
        self.z = z;
        self
    }

    fn values(&self) -> Vec<DbValue> {
        vec![
            DbValue::Int(self.location_id),
            DbValue::Object(PyValue::Unicode(self.location_name.clone())),
            match self.solar_system_id {
                Some(s) => DbValue::Int(s),
                None => DbValue::Null,
            },
            DbValue::Real(self.x),
            DbValue::Real(self.y),
            DbValue::Real(self.z),
            match self.location_name_id {
                Some(n) => DbValue::Int(n.into()),
                None => DbValue::Null,
            },
        ]
    }

    fn cells(&self) -> Vec<PyValue> {
        self.values().iter().map(cell).collect()
    }
}

/// `config.GetMultiOwnersEx` as a `CRowset` of packed rows — what a real
/// proxy sends.
pub fn owners_rowset(owners: &[Owner]) -> CRowset {
    let mut rs = CRowset::new(owners_descriptor());
    for o in owners {
        rs.push_values(o.values())
            .expect("owner values match the fixed owner descriptor");
    }
    rs
}

/// `config.GetMultiOwnersEx` as the `(header, data)` tuple.
///
/// Cheaper and lower-risk than [`owners_rowset`]: `Recordset._Prime` tests
/// for a tuple first and with an exact `type ==`, and this form needs no
/// whitelisted class, no descriptor and no packed blob. Prefer it unless
/// something downstream specifically wants `DBRow`s.
pub fn owners_header_data(owners: &[Owner]) -> Rowset {
    let mut rs = rowset(&OWNER_COLUMNS);
    for o in owners {
        rs.push(o.cells()).expect("owner cells match the header");
    }
    rs
}

/// `config.GetMultiLocationsEx` as a `CRowset`.
pub fn locations_rowset(locations: &[Location]) -> CRowset {
    let mut rs = CRowset::new(locations_descriptor());
    for l in locations {
        rs.push_values(l.values())
            .expect("location values match the fixed location descriptor");
    }
    rs
}

/// `config.GetMultiLocationsEx` as the `(header, data)` tuple.
pub fn locations_header_data(locations: &[Location]) -> Rowset {
    let mut rs = rowset(&LOCATION_COLUMNS);
    for l in locations {
        rs.push(l.cells()).expect("location cells match the header");
    }
    rs
}

/// `config.GetMultiCorpTickerNamesEx` as the `(header, data)` tuple. Only
/// `corporationID` and `tickerName` are ever read
/// (`CrpTickerNames.__getattr__`); the six shape/colour columns exist in the
/// header and are sent as `None`.
pub fn corp_ticker_header_data(tickers: &[(i32, String)]) -> Rowset {
    let mut rs = Rowset::new(&CORP_TICKER_COLUMNS).expect("a fixed, valid header");
    for (id, name) in tickers {
        rs.push(vec![
            PyValue::Int((*id).into()),
            PyValue::Unicode(name.clone()),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
        ])
        .expect("corp ticker cells match the header");
    }
    rs
}

fn rowset(columns: &[(&str, DbType)]) -> Rowset {
    let names: Vec<&str> = columns.iter().map(|&(n, _)| n).collect();
    Rowset::new(&names).expect("a fixed, valid header")
}

/// A `(header, data)` cell for a packed value. The `(header, data)` form has
/// no column types, so a NULL becomes `None` and everything else its plain
/// Python equivalent — which is what the client would have read out of a
/// `DBRow` anyway.
fn cell(v: &DbValue) -> PyValue {
    match v {
        DbValue::Null => PyValue::None,
        DbValue::Bool(b) => PyValue::Bool(*b),
        DbValue::Int(n) => PyValue::Int(*n),
        DbValue::Real(f) => PyValue::Real(*f),
        DbValue::Object(o) => o.clone(),
    }
}

/// Column names of a descriptor, as `header.Keys()` would return them.
pub fn column_names(d: &DbRowDescriptor) -> Vec<Vec<u8>> {
    d.names()
}

/// The `Row` form of one owner, for the single-key `config.GetOwnersEx`.
pub fn owner_row(owner: &Owner) -> Result<row::Row, RowsetError> {
    let names: Vec<&str> = OWNER_COLUMNS.iter().map(|&(n, _)| n).collect();
    row::Row::new(&names, owner.cells())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::{Decoder, Reader, Writer, save, tag};

    fn ccp_zoo() -> Owner {
        // 1000182 / typeCorporation-ish; the exact ids do not matter, the
        // shape does.
        Owner::new(1_000_182, "CCP Zoo", 1378)
    }

    #[test]
    fn the_owner_descriptor_is_the_clients_own_order_and_layout() {
        let d = owners_descriptor();
        let names: Vec<String> = d
            .names()
            .iter()
            .map(|n| String::from_utf8(n.clone()).unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["ownerID", "ownerName", "typeID", "gender", "ownerNameID"],
            "this order is pre-set by eveCfg.LoadEveOwners and must not change"
        );
        // Three I4s, one I2, one object: F = 14, nullBitBase = 112,
        // blobSize = ceil(117/8) = 15.
        assert_eq!(d.blob_size(), 15);
        assert_eq!(d.null_bit_base(), 112);
        assert_eq!(d.object_count(), 1);
    }

    #[test]
    fn the_location_descriptor_is_the_clients_own_order_and_layout() {
        let d = locations_descriptor();
        let names: Vec<String> = d
            .names()
            .iter()
            .map(|n| String::from_utf8(n.clone()).unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "locationID",
                "locationName",
                "solarSystemID",
                "x",
                "y",
                "z",
                "locationNameID"
            ]
        );
        // Five 8-byte columns, one I4, one object: F = 44, nullBitBase = 352.
        assert_eq!(d.blob_size(), 45);
        assert_eq!(d.null_bit_base(), 352);
    }

    /// The same byte sequence as `eve_marshal::dbrow`'s exact-byte test, but
    /// reached through this crate's owner builder, so the two layers cannot
    /// drift apart.
    #[test]
    fn one_owner_rowset_row_is_exactly_the_hand_worked_bytes() {
        let rs = owners_rowset(&[ccp_zoo()]);
        let mut w = Writer::new();
        save(&rs.to_pyvalue(), &mut w);
        let bytes = w.into_bytes();

        // The packed blob, written out by hand: ownerID at 0, typeID at 4,
        // ownerNameID at 8, gender at 12, NULL bitmap byte at 14 with bits
        // for columns 3 and 4 set.
        //   F6 42 0F 00 | 62 05 00 00 | 00 00 00 00 | 00 00 | 18
        // RLE: literal(3), zero(1), literal(2), zero(8), literal(1)
        //   -> nibbles 5,8,6,15,7 -> 85 F6 42 0F F6 62 05 07 18
        let blob: &[u8] = &[9, 0x85, 0xF6, 0x42, 0x0F, 0xF6, 0x62, 0x05, 0x07, 0x18];
        let at = bytes
            .windows(blob.len())
            .position(|w| w == blob)
            .expect("the hand-worked blob must appear verbatim");
        // The 0x2A tag is immediately followed by the descriptor, which ends
        // with its two terminators, which are followed by the blob length.
        assert_eq!(bytes[at - 1], tag::code::OBJECT_EX_END);
        assert_eq!(bytes[at - 2], tag::code::OBJECT_EX_END);
        // And the trailing object is the WSTR column's value.
        let after = at + blob.len();
        assert_eq!(bytes[after], tag::code::UNICODE_UTF8);
        assert_eq!(bytes[after + 1], 7);
        assert_eq!(&bytes[after + 2..after + 9], b"CCP Zoo");
    }

    #[test]
    fn an_owner_header_data_tuple_is_exactly_these_bytes() {
        let rs = owners_header_data(&[Owner::new(1, "Sys", 0)]);
        let mut w = Writer::new();
        save(&rs.to_pyvalue(), &mut w);

        // Hand-written: (header, data) with header a list of five name
        // strings and data a one-element list of a five-element list.
        let mut want = vec![tag::code::TUPLE2, tag::code::LIST, 5];
        for n in ["ownerID", "ownerName", "typeID", "gender", "ownerNameID"] {
            want.extend_from_slice(&[tag::code::STRING_SHORT, n.len() as u8]);
            want.extend_from_slice(n.as_bytes());
        }
        want.extend_from_slice(&[
            tag::code::LIST1,
            tag::code::LIST,
            5,
            tag::code::INT_ONE, // ownerID = 1
            tag::code::UNICODE_UTF8,
            3,
            b'S',
            b'y',
            b's',
            tag::code::INT_ZERO, // typeID = 0
            tag::code::NONE,     // gender
            tag::code::NONE,     // ownerNameID
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn owners_round_trip_in_both_shapes() {
        let owners = [
            ccp_zoo(),
            Owner::new(90_000_001, "Some Pilot", 1373).with_gender(1),
            Owner {
                owner_id: 1,
                owner_name: "EVE System".into(),
                type_id: 0,
                gender: None,
                owner_name_id: Some(123_456),
            },
        ];
        for v in [
            owners_rowset(&owners).to_pyvalue(),
            owners_header_data(&owners).to_pyvalue(),
        ] {
            let mut w = Writer::new();
            save(&v, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
            assert_eq!(r.remaining(), 0);
        }
    }

    #[test]
    fn locations_round_trip_in_both_shapes() {
        let locs = [
            Location::new(10_000_002, "The Forge"),
            Location::new(60_003_760, "Jita IV - Moon 4 - Caldari Navy Assembly Plant")
                .at(30_000_142, -1.07e14, -1.5e13, 1.0e14),
        ];
        for v in [
            locations_rowset(&locs).to_pyvalue(),
            locations_header_data(&locs).to_pyvalue(),
        ] {
            let mut w = Writer::new();
            save(&v, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
            assert_eq!(r.remaining(), 0);
        }
    }

    #[test]
    fn the_key_column_is_findable_by_name_in_both_shapes() {
        // Recordset._Prime does header.index(keycolumn) before anything
        // else, so the key column has to be present under its exact name.
        let d = owners_descriptor();
        assert_eq!(d.index_of(b"ownerID"), Some(0));
        assert_eq!(locations_descriptor().index_of(b"locationID"), Some(0));
        let rs = owners_header_data(&[ccp_zoo()]);
        assert_eq!(
            rs.columns().iter().position(|c| c == b"ownerID"),
            Some(0),
            "the keycolumn cfg.eveowners was constructed with"
        );
        assert_eq!(
            column_names(&d),
            rs.columns(),
            "both shapes must advertise identical headers"
        );
    }

    #[test]
    fn corp_tickers_carry_the_clients_own_eight_column_header() {
        let rs = corp_ticker_header_data(&[(1_000_002, "CORP".into())]);
        assert_eq!(rs.columns().len(), 8);
        assert_eq!(rs.columns()[0], b"corporationID");
        assert_eq!(rs.columns()[1], b"tickerName");
        let v = rs.to_pyvalue();
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
    }

    #[test]
    fn an_owner_row_is_a_single_named_row() {
        let r = owner_row(&ccp_zoo()).unwrap();
        assert_eq!(r.get("ownerID"), Some(&PyValue::Int(1_000_182)));
        assert_eq!(r.get("gender"), Some(&PyValue::None));
        let v = r.to_pyvalue();
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut rd = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut rd).unwrap(), v);
    }

    #[test]
    fn an_empty_batch_is_still_a_valid_reply() {
        // A key the server does not know is remembered as a knownLuzer, so
        // an empty reply is a legitimate answer and must not be malformed.
        for v in [
            owners_rowset(&[]).to_pyvalue(),
            owners_header_data(&[]).to_pyvalue(),
            locations_rowset(&[]).to_pyvalue(),
            locations_header_data(&[]).to_pyvalue(),
        ] {
            let mut w = Writer::new();
            save(&v, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
            assert_eq!(r.remaining(), 0);
        }
    }
}
