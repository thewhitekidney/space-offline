//! `carbon.common.script.sys.row.Row`, and the plain `(header, data)` tuple
//! that `cfg.Recordset` accepts.
//!
//! **Confirmed** from `carbon/common/script/sys/row.pyc`:
//!
//! ```python
//! class Row(object):
//!     __guid__ = 'util.Row'
//!     __slots__ = ('header', 'line')
//!     def __init__(self, header=None, line=None):
//!         if header and line and len(header) != len(line):
//!             raise ValueError('header and line must be equal length lists')
//!     def __getattr__(self, name):
//!         return self.line[self.header.index(name)]
//!     def __getstate__(self):  return (self.header, self.line)
//!     def __setstate__(self, state):  self.header, self.line = state
//! ```
//!
//! So `header` is a sequence of **column-name strings** that must support
//! `.index(name)`, and `line` is the parallel sequence of values. A Python
//! `list` of `str` satisfies both; `.index` exists on `tuple` too, but a list
//! is what the client's own code passes.
//!
//! `Row` is a new-style class, so the client's own writer would emit tag
//! `0x23` for it. We emit `0x22` with the constructor arguments instead —
//! see the table in the crate docs.

use crate::RowsetError;
use eve_marshal::{ObjectForm, PyValue};

/// The dotted name for the `0x22` header's callable. **Confirmed** as a
/// whitelist entry; note it is the dotted path, not the `__guid__`
/// `util.Row`.
pub const ROW_CLASS: &[u8] = b"carbon.common.script.sys.row.Row";

/// A single named row: a header of column names and a parallel line of
/// values.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    header: Vec<Vec<u8>>,
    line: Vec<PyValue>,
}

impl Row {
    /// Build a row, checking the two sequences line up. The client's
    /// `__init__` raises `ValueError` on a mismatch, so a wrong length would
    /// abort the whole read.
    pub fn new(header: &[&str], line: Vec<PyValue>) -> Result<Self, RowsetError> {
        let header = check_header(header)?;
        if header.len() != line.len() {
            return Err(RowsetError::ColumnCountMismatch {
                want: header.len(),
                got: line.len(),
            });
        }
        Ok(Row { header, line })
    }

    pub fn header(&self) -> &[Vec<u8>] {
        &self.header
    }

    pub fn line(&self) -> &[PyValue] {
        &self.line
    }

    /// Value of a named column, the way `Row.__getattr__` resolves it.
    pub fn get(&self, column: &str) -> Option<&PyValue> {
        let i = self.header.iter().position(|c| c == column.as_bytes())?;
        self.line.get(i)
    }

    /// The `0x22` ObjectEx: `Row(header, line)`.
    pub fn to_pyvalue(&self) -> PyValue {
        PyValue::ObjectEx {
            form: ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(ROW_CLASS.to_vec()),
                PyValue::Tuple(vec![
                    name_list(&self.header),
                    PyValue::List(self.line.clone()),
                ]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        }
    }
}

/// A `(header, data)` tuple: the cheapest reply shape `cfg.Recordset` accepts,
/// and the one that needs no whitelisted class at all.
///
/// **Confirmed** from `cfg.Recordset._Prime`, which tests the reply in this
/// order:
///
/// ```python
/// fk = fetch(list(keysToGet))
/// if type(fk) == types.TupleType:          # <-- this branch
///     self.header, data = fk
/// elif hasattr(fk, '__columns__'):
///     self.header = fk.__columns__; data = [list(fk)]
/// elif hasattr(fk, 'columns'):
///     self.header = fk.columns; data = fk
/// ...
/// if self.keyidx is None:
///     self.keyidx = self.header.index(self.keycolumn)
/// for i in data:
///     self.data[i[ix]] = i
/// ```
///
/// The tuple branch is tested **first** and with an exact `type ==`, so a
/// 2-tuple of `(list-of-name, list-of-row)` is matched before anything else
/// is even probed. `header` only has to support `.index(name)` and each row
/// only has to support integer indexing, both of which a plain Python list
/// does. Every later use goes through `cfg.Row`, whose `__getattr__` is
/// `self.line[self.header.index(name)]` — so a list row works exactly as a
/// `DBRow` does.
///
/// This is therefore the lowest-risk shape for the `config.GetMulti*Ex`
/// replies of `docs/reference/login-path.md` §8 row 9. [`crate::CRowset`] is the
/// higher-fidelity alternative (it is what a real proxy sends) and both are
/// provided.
#[derive(Debug, Clone, PartialEq)]
pub struct Rowset {
    columns: Vec<Vec<u8>>,
    rows: Vec<Vec<PyValue>>,
}

impl Rowset {
    pub fn new(columns: &[&str]) -> Result<Self, RowsetError> {
        Ok(Rowset {
            columns: check_header(columns)?,
            rows: Vec::new(),
        })
    }

    /// Append a row, checking its width against the header.
    pub fn push(&mut self, row: Vec<PyValue>) -> Result<&mut Self, RowsetError> {
        if row.len() != self.columns.len() {
            return Err(RowsetError::ColumnCountMismatch {
                want: self.columns.len(),
                got: row.len(),
            });
        }
        self.rows.push(row);
        Ok(self)
    }

    pub fn columns(&self) -> &[Vec<u8>] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<PyValue>] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The 2-tuple `(header, data)`.
    pub fn to_pyvalue(&self) -> PyValue {
        PyValue::Tuple(vec![
            name_list(&self.columns),
            PyValue::List(self.rows.iter().map(|r| PyValue::List(r.clone())).collect()),
        ])
    }
}

/// A Python list of `str`, which is what both `Row.header` and a
/// `(header, data)` header are in the client's own code.
fn name_list(names: &[Vec<u8>]) -> PyValue {
    PyValue::List(names.iter().map(|n| PyValue::Bytes(n.clone())).collect())
}

fn check_header(columns: &[&str]) -> Result<Vec<Vec<u8>>, RowsetError> {
    if columns.is_empty() {
        return Err(RowsetError::NoColumns);
    }
    for (i, c) in columns.iter().enumerate() {
        if columns[..i].contains(c) {
            return Err(RowsetError::DuplicateColumn);
        }
    }
    Ok(columns.iter().map(|c| c.as_bytes().to_vec()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::{Decoder, Reader, Writer, save, tag};

    #[test]
    fn a_row_is_a_0x22_calling_the_constructor() {
        let r = Row::new(&["a", "b"], vec![PyValue::Int(1), PyValue::None]).unwrap();
        let mut w = Writer::new();
        save(&r.to_pyvalue(), &mut w);

        // Hand-written: 0x22, a 2-tuple header of (global, args), then the
        // two mandatory terminators.
        let mut want = vec![
            tag::code::OBJECT_22,
            tag::code::TUPLE2,
            tag::code::GLOBAL,
            ROW_CLASS.len() as u8,
        ];
        want.extend_from_slice(ROW_CLASS);
        want.extend_from_slice(&[
            tag::code::TUPLE2, // args = (header, line)
            tag::code::LIST,
            2,
            tag::code::STRING_CHAR,
            b'a',
            tag::code::STRING_CHAR,
            b'b',
            tag::code::LIST,
            2,
            tag::code::INT_ONE,
            tag::code::NONE,
            tag::code::OBJECT_EX_END,
            tag::code::OBJECT_EX_END,
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_row_refuses_a_line_of_the_wrong_width() {
        assert_eq!(
            Row::new(&["a", "b"], vec![PyValue::Int(1)]),
            Err(RowsetError::ColumnCountMismatch { want: 2, got: 1 })
        );
        assert_eq!(Row::new(&[], vec![]), Err(RowsetError::NoColumns));
        assert_eq!(
            Row::new(&["a", "a"], vec![PyValue::None, PyValue::None]),
            Err(RowsetError::DuplicateColumn)
        );
    }

    #[test]
    fn a_row_resolves_a_column_by_name_like_getattr_does() {
        let r = Row::new(&["x", "y"], vec![PyValue::Int(7), PyValue::Int(9)]).unwrap();
        assert_eq!(r.get("y"), Some(&PyValue::Int(9)));
        assert_eq!(r.get("z"), None);
    }

    #[test]
    fn a_header_data_tuple_is_exactly_a_tuple_of_two_lists() {
        let mut rs = Rowset::new(&["ownerID", "ownerName"]).unwrap();
        rs.push(vec![PyValue::Int(1), PyValue::Unicode("Sys".into())])
            .unwrap();
        let mut w = Writer::new();
        save(&rs.to_pyvalue(), &mut w);

        // Hand-written: a 2-tuple (TUPLE2, no count byte), a list of two
        // name strings, then a list of one row list.
        let mut want = vec![
            tag::code::TUPLE2,
            tag::code::LIST,
            2,
            tag::code::STRING_SHORT,
            7,
        ];
        want.extend_from_slice(b"ownerID");
        want.extend_from_slice(&[tag::code::STRING_SHORT, 9]);
        want.extend_from_slice(b"ownerName");
        want.extend_from_slice(&[
            tag::code::LIST1, // one row
            tag::code::LIST,
            2,
            tag::code::INT_ONE,
            tag::code::UNICODE_UTF8,
            3,
            b'S',
            b'y',
            b's',
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_header_data_tuple_round_trips() {
        let mut rs = Rowset::new(&["a", "b", "c"]).unwrap();
        rs.push(vec![PyValue::Int(1), PyValue::None, PyValue::Bool(false)])
            .unwrap();
        rs.push(vec![PyValue::Int(2), PyValue::Real(0.5), PyValue::None])
            .unwrap();
        let v = rs.to_pyvalue();
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_header_data_row_must_match_the_header_width() {
        let mut rs = Rowset::new(&["a", "b"]).unwrap();
        assert_eq!(
            rs.push(vec![PyValue::None]),
            Err(RowsetError::ColumnCountMismatch { want: 2, got: 1 })
        );
        assert!(rs.is_empty());
    }
}
