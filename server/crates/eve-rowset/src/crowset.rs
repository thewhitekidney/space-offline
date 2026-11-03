//! The three `carbon.common.script.sys.crowset` containers: `CRowset`,
//! `CIndexedRowset` and `CFilterRowset`.
//!
//! **Confirmed** from `carbon/common/script/sys/crowset.pyc`:
//!
//! ```python
//! class CRowset(list):
//!     __guid__ = 'dbutil.CRowset'
//!     __passbyvalue__ = 1
//!     def __init__(self, header, rows):
//!         list.__init__(self, rows)
//!         self.header = header
//!     @property
//!     def columns(self):  return self.header.Keys()
//!     def Index(self, columnName): ...        # header.Keys().index(columnName)
//!     def InsertNew(self, listOfValues):  self.append(blue.DBRow(self.header, listOfValues))
//!
//! class CIndexedRowset(dict):
//!     def __init__(self, header, columnName): ...
//! class CFilterRowset(dict):
//!     def __init__(self, header, columnName, indexName=None,
//!                  allowDuplicateCompoundKeys=False, giveMeSets=False): ...
//! ```
//!
//! Two things follow and are worth stating because they drive the API here:
//!
//! - `header` is a **`blue.DBRowDescriptor`**, not a list of names:
//!   `InsertNew` passes it straight to `blue.DBRow(...)` and `columns` calls
//!   `.Keys()` on it. **Confirmed** from those two call sites.
//! - the client indexes rows **by position** (`header.Index(col)` then
//!   `row[ix]`), so the column order the server declares is the order the
//!   client uses, and every row in one rowset must share one descriptor.
//!
//! `ROWSETVERSION = 3` appears only in `SaveRowset`/`LoadRowset`, the **disk**
//! format, and never on the network. **Confirmed** — it is a `blue.ResFile`
//! path in both directions.

use crate::{RowsetError, whitelist};
use eve_marshal::dbrow::{DbRow, DbRowDescriptor, DbValue};
use eve_marshal::{ObjectForm, PyValue};

/// **Confirmed** whitelist entry. Note the dotted path, not the `__guid__`
/// `dbutil.CRowset`: the `0x02` resolver imports the module named by the
/// string, and `dbutil` is not a module in this build.
pub const CROWSET_CLASS: &[u8] = b"carbon.common.script.sys.crowset.CRowset";
/// **Confirmed** whitelist entry.
pub const CINDEXED_ROWSET_CLASS: &[u8] = b"carbon.common.script.sys.crowset.CIndexedRowset";
/// **Confirmed** whitelist entry.
pub const CFILTER_ROWSET_CLASS: &[u8] = b"carbon.common.script.sys.crowset.CFilterRowset";

/// A `CRowset`: one descriptor and a list of packed rows.
#[derive(Debug, Clone, PartialEq)]
pub struct CRowset {
    descriptor: DbRowDescriptor,
    rows: Vec<DbRow>,
}

impl CRowset {
    pub fn new(descriptor: DbRowDescriptor) -> Self {
        CRowset {
            descriptor,
            rows: Vec::new(),
        }
    }

    /// Append a row built from `values`, which are checked against the
    /// rowset's descriptor. The `CRowset.InsertNew` equivalent.
    pub fn push_values(&mut self, values: Vec<DbValue>) -> Result<&mut Self, RowsetError> {
        let row = DbRow::new(self.descriptor.clone(), values)?;
        self.rows.push(row);
        Ok(self)
    }

    /// Append a ready-made row. Refuses one built from a different
    /// descriptor, because the client reads every row of a rowset through
    /// the single `header`.
    pub fn push_row(&mut self, row: DbRow) -> Result<&mut Self, RowsetError> {
        if row.descriptor() != &self.descriptor {
            return Err(RowsetError::DescriptorMismatch);
        }
        self.rows.push(row);
        Ok(self)
    }

    pub fn descriptor(&self) -> &DbRowDescriptor {
        &self.descriptor
    }

    pub fn rows(&self) -> &[DbRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The `0x22` ObjectEx: `CRowset(header, rows)`.
    ///
    /// The rows go in the **args tuple** as a plain list rather than through
    /// the list-append stream, because `__init__` already does
    /// `list.__init__(self, rows)`. Using the append stream instead would
    /// also work (and is what the client's own `0x23` writer does), but it
    /// would need the `0x23` form and a state dict naming `header`.
    pub fn to_pyvalue(&self) -> PyValue {
        debug_assert!(whitelist::is_whitelisted(CROWSET_CLASS));
        object_ex(
            CROWSET_CLASS,
            vec![
                self.descriptor.to_pyvalue(),
                PyValue::List(self.rows.iter().map(row_value).collect()),
            ],
            Vec::new(),
        )
    }

    /// Group the rows by one column, the way `CRowset.Index` does.
    pub fn index(&self, column: &str) -> Result<CIndexedRowset, RowsetError> {
        let i = self
            .descriptor
            .index_of(column.as_bytes())
            .ok_or(RowsetError::UnknownColumn)?;
        Ok(CIndexedRowset {
            descriptor: self.descriptor.clone(),
            column: column.as_bytes().to_vec(),
            entries: self
                .rows
                .iter()
                .map(|r| (key_of(&r.values()[i]), r.clone()))
                .collect(),
        })
    }

    /// Bucket the rows by one column, the way `CRowset.Filter` does with its
    /// default arguments: each key maps to a `CRowset` of the rows that
    /// carry it, in input order.
    pub fn filter(&self, column: &str) -> Result<CFilterRowset, RowsetError> {
        let i = self
            .descriptor
            .index_of(column.as_bytes())
            .ok_or(RowsetError::UnknownColumn)?;
        let mut groups: Vec<(PyValue, Vec<DbRow>)> = Vec::new();
        for r in &self.rows {
            let key = key_of(&r.values()[i]);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some(g) => g.1.push(r.clone()),
                None => groups.push((key, vec![r.clone()])),
            }
        }
        Ok(CFilterRowset {
            descriptor: self.descriptor.clone(),
            column: column.as_bytes().to_vec(),
            groups,
        })
    }
}

/// A `CIndexedRowset`: a `dict` of key -> one row.
///
/// The client builds it with `CIndexedRowset(header, columnName)` and then
/// fills the dict, which on the wire is the `0x22` **dict-setitem stream**:
/// `key, value` pairs until `0x2D`. That is the opposite order to tag `0x16`
/// — confirmed from `obj.__setitem__(first, second)` at `0x1802de8f7`.
#[derive(Debug, Clone, PartialEq)]
pub struct CIndexedRowset {
    descriptor: DbRowDescriptor,
    column: Vec<u8>,
    entries: Vec<(PyValue, DbRow)>,
}

impl CIndexedRowset {
    pub fn descriptor(&self) -> &DbRowDescriptor {
        &self.descriptor
    }

    pub fn column(&self) -> &[u8] {
        &self.column
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_pyvalue(&self) -> PyValue {
        debug_assert!(whitelist::is_whitelisted(CINDEXED_ROWSET_CLASS));
        object_ex(
            CINDEXED_ROWSET_CLASS,
            vec![
                self.descriptor.to_pyvalue(),
                PyValue::Bytes(self.column.clone()),
            ],
            self.entries
                .iter()
                .map(|(k, r)| (k.clone(), row_value(r)))
                .collect(),
        )
    }
}

/// A `CFilterRowset`: a `dict` of key -> `CRowset` of the matching rows.
#[derive(Debug, Clone, PartialEq)]
pub struct CFilterRowset {
    descriptor: DbRowDescriptor,
    column: Vec<u8>,
    groups: Vec<(PyValue, Vec<DbRow>)>,
}

impl CFilterRowset {
    pub fn descriptor(&self) -> &DbRowDescriptor {
        &self.descriptor
    }

    pub fn column(&self) -> &[u8] {
        &self.column
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn to_pyvalue(&self) -> PyValue {
        debug_assert!(whitelist::is_whitelisted(CFILTER_ROWSET_CLASS));
        object_ex(
            CFILTER_ROWSET_CLASS,
            vec![
                self.descriptor.to_pyvalue(),
                PyValue::Bytes(self.column.clone()),
            ],
            self.groups
                .iter()
                .map(|(k, rows)| {
                    let bucket = CRowset {
                        descriptor: self.descriptor.clone(),
                        rows: rows.clone(),
                    };
                    (k.clone(), bucket.to_pyvalue())
                })
                .collect(),
        )
    }
}

/// `0x22` with a global callable, an args tuple and a dict-setitem stream.
fn object_ex(class: &[u8], args: Vec<PyValue>, dict: Vec<(PyValue, PyValue)>) -> PyValue {
    PyValue::ObjectEx {
        form: ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(class.to_vec()),
            PyValue::Tuple(args),
        ])),
        // The list-append stream stays empty; both terminators are still
        // written, which `eve_marshal::save` does unconditionally.
        list: Vec::new(),
        dict,
    }
}

fn row_value(row: &DbRow) -> PyValue {
    PyValue::DbRow(Box::new(row.clone()))
}

/// A dict key for the value of an indexed column.
///
/// `CIndexedRowset.Build` does `self[row[columnName]] = row`, so the key is
/// whatever the column holds. A NULL column keys on `None`, which is legal
/// Python and is what the client would do too.
fn key_of(v: &DbValue) -> PyValue {
    match v {
        DbValue::Null => PyValue::None,
        DbValue::Bool(b) => PyValue::Bool(*b),
        DbValue::Int(n) => PyValue::Int(*n),
        DbValue::Real(f) => PyValue::Real(*f),
        DbValue::Object(o) => o.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::{DbType, Decoder, Reader, Writer, save, tag};

    fn two_col() -> DbRowDescriptor {
        DbRowDescriptor::from_pairs(&[("n", DbType::I4), ("s", DbType::WStr)]).unwrap()
    }

    #[test]
    fn an_empty_crowset_is_exactly_these_bytes() {
        let rs = CRowset::new(DbRowDescriptor::from_pairs(&[("n", DbType::I4)]).unwrap());
        let mut w = Writer::new();
        save(&rs.to_pyvalue(), &mut w);

        // Hand-written from docs/reference/object-wrapper.md §8 and the CRowset
        // constructor: 0x22, (global, (descriptor, rows)), 2D 2D.
        let mut want = vec![
            tag::code::OBJECT_22,
            tag::code::TUPLE2, // (callable, args)
            tag::code::GLOBAL,
            CROWSET_CLASS.len() as u8,
        ];
        want.extend_from_slice(CROWSET_CLASS);
        want.extend_from_slice(&[
            tag::code::TUPLE2, // args = (header, rows)
            // --- the header: a 0x22 blue.DBRowDescriptor ---
            tag::code::OBJECT_22,
            tag::code::TUPLE2,
            tag::code::GLOBAL,
            20,
        ]);
        want.extend_from_slice(b"blue.DBRowDescriptor");
        want.extend_from_slice(&[
            tag::code::TUPLE1,      // args = (cols,)
            tag::code::TUPLE1,      // one column
            tag::code::TUPLE2,      // (name, type)
            tag::code::STRING_CHAR, // "n"
            b'n',
            tag::code::INT8, // DBTYPE_I4
            3,
            tag::code::OBJECT_EX_END, // descriptor's list stream
            tag::code::OBJECT_EX_END, // descriptor's dict stream
            // --- the rows: an empty list ---
            tag::code::LIST_EMPTY,
            // --- the CRowset's own two terminators ---
            tag::code::OBJECT_EX_END,
            tag::code::OBJECT_EX_END,
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_crowset_round_trips_with_its_rows() {
        let mut rs = CRowset::new(two_col());
        rs.push_values(vec![
            DbValue::Int(1),
            DbValue::Object(PyValue::Unicode("one".into())),
        ])
        .unwrap();
        rs.push_values(vec![DbValue::Int(2), DbValue::Null])
            .unwrap();
        assert_eq!(rs.len(), 2);
        let v = rs.to_pyvalue();
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_crowset_refuses_a_row_from_another_descriptor() {
        let mut rs = CRowset::new(two_col());
        let other = DbRow::new(
            DbRowDescriptor::from_pairs(&[("n", DbType::I4)]).unwrap(),
            vec![DbValue::Int(1)],
        )
        .unwrap();
        assert_eq!(rs.push_row(other), Err(RowsetError::DescriptorMismatch));
    }

    #[test]
    fn a_crowset_refuses_values_the_descriptor_cannot_hold() {
        let mut rs = CRowset::new(two_col());
        assert!(matches!(
            rs.push_values(vec![DbValue::Int(1)]),
            Err(RowsetError::Row(_))
        ));
    }

    #[test]
    fn an_indexed_rowset_puts_its_pairs_in_the_setitem_stream_key_first() {
        let mut rs = CRowset::new(two_col());
        rs.push_values(vec![
            DbValue::Int(42),
            DbValue::Object(PyValue::Unicode("x".into())),
        ])
        .unwrap();
        let ix = rs.index("n").unwrap();
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.column(), b"n");

        let mut w = Writer::new();
        save(&ix.to_pyvalue(), &mut w);
        let bytes = w.into_bytes();
        // The first terminator closes the (empty) list stream; the key must
        // be the very next object, and it must be the column value, 42.
        let first = bytes
            .iter()
            .position(|&b| b == tag::code::OBJECT_EX_END)
            .unwrap();
        // The descriptor nested inside carries terminators of its own, so
        // find the outermost pair by walking from the end instead.
        let last = bytes.len() - 1;
        assert_eq!(bytes[last], tag::code::OBJECT_EX_END, "dict stream ends");
        assert!(first < last);
        // Decode and check the structure rather than guess at offsets.
        let mut r = Reader::new(&bytes);
        let got = Decoder::new().load(&mut r).unwrap();
        let PyValue::ObjectEx { list, dict, .. } = &got else {
            panic!("not an ObjectEx")
        };
        assert!(list.is_empty(), "the list stream must stay empty");
        assert_eq!(dict.len(), 1);
        assert_eq!(dict[0].0, PyValue::Int(42), "key comes first");
        assert!(matches!(dict[0].1, PyValue::DbRow(_)), "value is the row");
    }

    #[test]
    fn a_filter_rowset_buckets_rows_and_nests_a_crowset_per_key() {
        let mut rs = CRowset::new(two_col());
        for (n, s) in [(1, "a"), (2, "b"), (1, "c")] {
            rs.push_values(vec![
                DbValue::Int(n),
                DbValue::Object(PyValue::Unicode(s.into())),
            ])
            .unwrap();
        }
        let fr = rs.filter("n").unwrap();
        assert_eq!(fr.len(), 2, "two distinct keys");
        let v = fr.to_pyvalue();
        let PyValue::ObjectEx { dict, .. } = &v else {
            panic!()
        };
        assert_eq!(dict[0].0, PyValue::Int(1));
        let PyValue::ObjectEx { header, .. } = &dict[0].1 else {
            panic!("bucket is not an ObjectEx")
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!()
        };
        assert_eq!(h[0], PyValue::Global(CROWSET_CLASS.to_vec()));

        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn indexing_an_unknown_column_is_refused() {
        let rs = CRowset::new(two_col());
        assert_eq!(rs.index("nope"), Err(RowsetError::UnknownColumn));
        assert_eq!(rs.filter("nope"), Err(RowsetError::UnknownColumn));
    }

    #[test]
    fn a_null_column_keys_on_none() {
        let mut rs = CRowset::new(two_col());
        rs.push_values(vec![DbValue::Int(1), DbValue::Null])
            .unwrap();
        let ix = rs.index("s").unwrap();
        let PyValue::ObjectEx { dict, .. } = ix.to_pyvalue() else {
            panic!()
        };
        assert_eq!(dict[0].0, PyValue::None);
    }
}
