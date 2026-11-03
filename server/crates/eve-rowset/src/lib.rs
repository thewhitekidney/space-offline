//! The data containers the EVE client expects back from service calls:
//! `utillib.KeyVal`, `carbon.common.script.sys.row.Row`, the three
//! `crowset` types, and the plain `(header, data)` tuple that `cfg.Recordset`
//! accepts.
//!
//! This crate sits on [`eve_marshal`] and produces [`PyValue`] trees. It
//! exists so a service implementation can answer `config.GetMultiOwnersEx`
//! with a list of owners rather than by hand-assembling tag bytes.
//!
//! Sources of truth, all read from the client's own **unobfuscated** Python
//! in `out/pyc/` rather than inferred:
//!
//! - `carbon/common/script/sys/crowset.pyc` — `CRowset(list)`,
//!   `CIndexedRowset(dict)`, `CFilterRowset(dict)` and their `__init__`
//!   signatures.
//! - `carbon/common/script/sys/row.pyc` — `Row(object)` with `__slots__` and
//!   `__getstate__`/`__setstate__`.
//! - `carbon/common/script/sys/cfg.pyc` — `Recordset._Prime`, which decides
//!   what a `config.GetMulti*Ex` reply may be.
//! - `eve/common/script/sys/eveCfg.pyc` — `GetOwnersRowDescriptor`,
//!   `GetLocationRowDescriptor`, and the pre-set `Recordset.header` lists.
//! - `packages/utillib/__init__.pyc` — `class KeyVal:`, an **old-style**
//!   class, so tag `0x17`.
//! - `carbon/common/lib/whitelist.pyc` — [`whitelist`].
//!
//! # Which wrapper each container uses, and why
//!
//! The client's reader builds a `0x22` as `header[0](*header[1])`
//! (`docs/reference/object-wrapper.md` §2, confirmed from `0x1802de268`). Every
//! container here except `KeyVal` is therefore emitted as a `0x22` whose
//! callable is the class and whose args are **the class's own `__init__`
//! arguments**, which the client's source gives us directly:
//!
//! | container | wrapper | args |
//! |---|---|---|
//! | `KeyVal` | `0x17` | — (old-style class; state is a dict `__dict__.update`s) |
//! | `Row` | `0x22` | `(header, line)` |
//! | `CRowset` | `0x22` | `(header, rows)` |
//! | `CIndexedRowset` | `0x22` | `(header, columnName)` + dict-setitem stream |
//! | `CFilterRowset` | `0x22` | `(header, columnName)` + dict-setitem stream |
//! | `blue.DBRowDescriptor` | `0x22` | `(columns,)` — see [`eve_marshal::dbrow`] |
//!
//! The alternative for the four new-style classes is `0x23`, which is what
//! the client's own writer emits for them (`__reduce_ex__(2)` on a `list` or
//! `dict` subclass yields `copyreg.__newobj__`, so `0x1802e04c4` picks
//! `0x23`), reconstructing with `cls.__new__(cls)` plus a state dict and the
//! append streams. Both are accepted by the reader; `0x22` is chosen here
//! because the constructor signature is read directly off the client's source
//! and needs no assumption about which attribute names the state dict must
//! carry. That choice is **inferred** to be equivalent for the client, from
//! the reader's semantics; it has not been checked against a live client.
//!
//! # `KeyVal` is `0x17`, not `0x22`
//!
//! **Confirmed**: `class KeyVal:` in `utillib/__init__.py` has an empty bases
//! tuple, so in Python 2.7 it is an old-style class and `WriteObject` takes
//! the `PyInstance_Type` branch. It defines no `__setstate__`, so the reader
//! falls through to `inst.__dict__.update(state)` and the state must be a
//! **dict**. `__init__` is never called.

#![forbid(unsafe_code)]

use std::fmt;

pub mod cfg;
pub mod crowset;
pub mod keyval;
pub mod row;
pub mod whitelist;

pub use cfg::{Location, Owner};
pub use crowset::{CFilterRowset, CIndexedRowset, CRowset};
pub use keyval::KeyVal;
pub use row::{Row, Rowset};
pub use whitelist::is_whitelisted;

use eve_marshal::DbRowError;

/// Anything that can go wrong building a container.
///
/// Deliberately distinct from [`eve_marshal::WireError`]: this layer is above
/// the codec and every failure here is a server-side programming error, not a
/// malformed stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowsetError {
    /// A row with the wrong number of cells for its header.
    ColumnCountMismatch { want: usize, got: usize },
    /// Two columns with the same name. `header.index(name)` and
    /// `Row.__getattr__` both resolve by first match, so the second would be
    /// unreachable.
    DuplicateColumn,
    /// A header with no columns. `Recordset._Prime` immediately does
    /// `header.index(keycolumn)`, which would raise.
    NoColumns,
    /// A column name that is not in the descriptor being indexed or filtered.
    UnknownColumn,
    /// A row whose descriptor is not the rowset's own. `CRowset` keeps one
    /// header for every row and the client indexes by position, so mixing
    /// descriptors silently misreads.
    DescriptorMismatch,
    /// The underlying packed row refused the values.
    Row(DbRowError),
}

impl fmt::Display for RowsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowsetError::ColumnCountMismatch { want, got } => {
                write!(f, "header has {want} columns but the row has {got}")
            }
            RowsetError::DuplicateColumn => write!(f, "duplicate column name"),
            RowsetError::NoColumns => write!(f, "a rowset needs at least one column"),
            RowsetError::UnknownColumn => write!(f, "no such column in the descriptor"),
            RowsetError::DescriptorMismatch => {
                write!(f, "row descriptor differs from the rowset's")
            }
            RowsetError::Row(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RowsetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RowsetError::Row(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DbRowError> for RowsetError {
    fn from(e: DbRowError) -> Self {
        RowsetError::Row(e)
    }
}
