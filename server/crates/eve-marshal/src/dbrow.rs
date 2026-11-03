//! `blue.DBRow` (type code `0x2A`) and the `blue.DBRowDescriptor` that shapes
//! it: the packed-row container every `CRowset` is built from.
//!
//! Source of truth: `docs/reference/dbrow.md`, plus an independent re-reading of the
//! three functions that matter, done while writing this module:
//!
//! - **layout builder** `0x1802f9c60` — region order, bool bits, NULL bitmap,
//!   object slots, and the `dbtype -> size class` table at `0x1802fa0c8`.
//! - **RLE decoder** `0x1802fe210` — nibble order and the two run kinds.
//! - **DBRow reader** `0x1802fe410` — the field order on the wire.
//!
//! Claims below are marked **confirmed** when they were read off those
//! instructions (or off the client's own unobfuscated Python) and **inferred**
//! otherwise. Nothing here is taken from canonical/folklore descriptions of
//! the format.
//!
//! The wire form, **confirmed** from `0x1802fe410` field for field:
//!
//! ```text
//! u8      tag        0x2A
//! ...     descriptor one complete marshal object; type-checked against
//!                    blue.DBRowDescriptor, else "Hacker Warning! Invalid type
//!                    in marshal data, expected blue.DBRowDescriptor."
//! u8/u32  blobLen    the usual escaped length (1 byte, or 0xFF + u32 LE)
//! u8[]    blob       nibble RLE of the packed row
//! ...     values     one marshal object per object column, declaration order
//! ```
//!
//! The reader takes the unpacked length from the descriptor, never from the
//! stream, and zero-fills whatever the RLE did not produce
//! (`0x1802fe2cf`). **Confirmed.**

use crate::constants::DbType;
use crate::value::{ObjectForm, PyValue};
use std::collections::HashSet;
use std::fmt;

/// The dotted global the descriptor's `0x22` header must name.
///
/// **Confirmed**: `blue.DBRowDescriptor` is the first entry of the client's
/// own `carbon/common/lib/whitelist.py`, and the `0x22` handler builds the
/// object with `PyObject_CallObject(header[0], header[1])`
/// (`0x1802de239`/`0x1802de253`/`0x1802de268`).
pub const DESCRIPTOR_GLOBAL: &[u8] = b"blue.DBRowDescriptor";

/// The dotted global for a row itself. Never emitted by this module — a
/// `0x2A` carries no class name — but a server needs the spelling when it
/// wants `blue.DBRow` in a header tuple. **Confirmed** from the whitelist.
pub const ROW_GLOBAL: &[u8] = b"blue.DBRow";

/// Largest `DBTYPE` the descriptor constructor accepts.
///
/// **Confirmed** from the layout builder: `cmp r8d, 0x87; ja` at
/// `0x1802f9d3e` jumps to `"DBRowDescriptor doesn't support data type %d"`.
/// This is why `const.py`'s `DBTYPE_BYTES = 508` cannot be the real value.
pub const MAX_DB_TYPE: u16 = 0x87;

/// A nibble below this is a literal run of `8 - n` bytes; at or above it, a
/// run of `n - 7` zero bytes. **Confirmed** at `0x1802fe276`-`0x1802fe2a3`.
const RLE_ZERO_THRESHOLD: u8 = 8;

/// Longest run either nibble kind can express: `8 - 0` literals, or
/// `15 - 7` zeros. **Confirmed** — both fall out of the nibble width.
const RLE_MAX_RUN: usize = 8;

/// Anything that can go wrong building, packing or parsing a `DBRow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbRowError {
    /// A column name beginning with `__`. **Confirmed**: the layout builder
    /// tests the first two bytes for `_` at `0x1802f9d1f`/`0x1802f9d2f` and
    /// raises before looking at the type.
    ReservedColumnName,
    /// Two columns with the same name. The client does not check this, but
    /// `header.Index(name)` and `Row.__getattr__` both resolve by first
    /// match, so a duplicate silently shadows. Refused here.
    DuplicateColumnName,
    /// `DBTYPE_EMPTY`. Declarable by the client, but the NULL bitmap is
    /// *sized* by the count of non-EMPTY columns (`0x1802f9dfd`) while the
    /// getter *indexes* it by raw declaration index (`0x1802fc0b0`), so the
    /// two disagree as soon as an EMPTY column is present. We never emit one
    /// and refuse to model one.
    EmptyColumn,
    /// `DBTYPE_DBTIMESTAMP` (135). Declarable, but the getter's jump table
    /// stops at `0x82` (`0x1802fc119`), so reading the column raises
    /// "Unexpected db column type encountered: %d". **Confirmed.** Use
    /// [`DbType::FileTime`].
    UnreadableColumn,
    /// A descriptor with no columns: `blobSize` would be 0 and every index
    /// out of range.
    NoColumns,
    /// A row whose value count does not match its descriptor.
    ColumnCountMismatch { want: usize, got: usize },
    /// A value whose kind cannot be stored in the column's `DBTYPE`.
    TypeMismatch {
        column: usize,
        want: DbType,
        got: &'static str,
    },
    /// An integer that does not fit the column's width.
    ValueOutOfRange { column: usize, value: i64 },
    /// The descriptor object on the wire was not the `0x22` shape this
    /// module (and the client's `__reduce__`) emits.
    MalformedDescriptor(&'static str),
    /// A `DBTYPE` outside the 18 the client's size-class table maps.
    UnknownDbType(u16),
    /// An RLE stream that would decode past `blobSize`. The client raises
    /// "Invalid RLE string" (`0x1802fe2f7`).
    InvalidRle,
}

impl fmt::Display for DbRowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbRowError::ReservedColumnName => {
                write!(f, "a DBRow column name may not begin with `__`")
            }
            DbRowError::DuplicateColumnName => write!(f, "duplicate DBRow column name"),
            DbRowError::EmptyColumn => write!(f, "DBTYPE_EMPTY columns are not supported"),
            DbRowError::UnreadableColumn => write!(
                f,
                "DBTYPE_DBTIMESTAMP can be declared but never read; use FILETIME"
            ),
            DbRowError::NoColumns => write!(f, "a DBRowDescriptor needs at least one column"),
            DbRowError::ColumnCountMismatch { want, got } => {
                write!(f, "descriptor has {want} columns but the row has {got}")
            }
            DbRowError::TypeMismatch { column, want, got } => write!(
                f,
                "column {column} is {want:?} and cannot hold a {got} value"
            ),
            DbRowError::ValueOutOfRange { column, value } => {
                write!(f, "value {value} does not fit column {column}")
            }
            DbRowError::MalformedDescriptor(why) => {
                write!(f, "malformed DBRowDescriptor: {why}")
            }
            DbRowError::UnknownDbType(t) => write!(f, "unknown DBTYPE {t}"),
            DbRowError::InvalidRle => write!(f, "invalid RLE string"),
        }
    }
}

impl std::error::Error for DbRowError {}

/// Size class of a `DBTYPE`, exactly as the client's own table assigns it.
///
/// **Confirmed** by dumping the byte table at `0x1802fa0c8` (`0x00..=0x87`)
/// through the jump table at `0x1802fa0a8` and reading which `sil` constant
/// each target sets. The result is a total function on the 18 declarable
/// types and an error for all 118 others:
///
/// | class | width | types |
/// |---|---|---|
/// | 0 | one bit | `BOOL` |
/// | 1 | 1 byte | `I1`, `UI1` |
/// | 2 | 2 bytes | `I2`, `UI2` |
/// | 3 | 4 bytes | `I4`, `R4`, `UI4` |
/// | 4 | 8 bytes | `R5`, `CY`, `I8`, `UI8`, `FILETIME`, `DBTIMESTAMP` |
/// | 5 | object slot | `BYTES`, `STR`, `WSTR` |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeClass {
    /// One bit in the bool region.
    Bit,
    /// 1, 2, 4 or 8 bytes in the fixed-scalar area.
    Fixed(usize),
    /// No blob bytes; a separate marshal object after the blob.
    Object,
}

impl SizeClass {
    /// The class of `t`. Total: every [`DbType`] except `Empty` has one.
    pub fn of(t: DbType) -> Option<SizeClass> {
        Some(match t {
            DbType::Empty => return None,
            DbType::Bool => SizeClass::Bit,
            DbType::I1 | DbType::Ui1 => SizeClass::Fixed(1),
            DbType::I2 | DbType::Ui2 => SizeClass::Fixed(2),
            DbType::I4 | DbType::R4 | DbType::Ui4 => SizeClass::Fixed(4),
            DbType::R5
            | DbType::Cy
            | DbType::I8
            | DbType::Ui8
            | DbType::FileTime
            | DbType::DbTimestamp => SizeClass::Fixed(8),
            DbType::Bytes | DbType::Str | DbType::WStr => SizeClass::Object,
        })
    }
}

/// Where one column lives once the layout is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `width` little-endian bytes at `offset` in the unpacked blob.
    Fixed { offset: usize, width: usize },
    /// One bit, LSB-first (`byte = i >> 3`, `bit = i & 7`). **Confirmed**
    /// from the `bt` at `0x1802fc0b0`.
    Bit(usize),
    /// The n-th trailing marshal object, in declaration order.
    Object(usize),
}

/// One declared column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Python 2 `str`, so bytes rather than text.
    pub name: Vec<u8>,
    pub db_type: DbType,
}

/// A `blue.DBRowDescriptor`: the column list plus the packed layout the
/// client derives from it.
///
/// Construction computes the layout once; [`DbRow`] then only needs to copy
/// bytes. The layout is **confirmed** from the builder at `0x1802f9c60`:
///
/// ```text
/// F              = 8*n8 + 4*n4 + 2*n2 + n1   ; fixed-scalar area, bytes
/// offset[8-byte] = 0
/// offset[4-byte] = 8*n8
/// offset[2-byte] = 8*n8 + 4*n4
/// offset[1-byte] = 8*n8 + 4*n4 + 2*n2
/// boolBitBase    = 8*F
/// nullBitBase    = 8*F + nBool
/// blobSize       = ceil((nullBitBase + nColumns) / 8)
/// objectBase     = roundup8(blobSize)        ; byte offset, 8 per object
/// ```
///
/// The four fixed regions are ordered **largest first**, and within a region
/// the columns keep **declaration order** — the builder walks the columns
/// once and hands each the next slot of its own region's cursor
/// (`0x1802f9fe3`). That is a region ordering, not a permutation of the
/// columns, and reading it as a permutation is the classic way to get this
/// format wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRowDescriptor {
    columns: Vec<Column>,
    slots: Vec<Slot>,
    blob_size: usize,
    null_bit_base: usize,
    object_count: usize,
}

impl DbRowDescriptor {
    /// Build a descriptor, computing the packed layout.
    ///
    /// Refuses what the client would refuse (names starting with `__`, an
    /// unmapped `DBTYPE`) and additionally refuses two types the client
    /// accepts but cannot round-trip: see [`DbRowError::EmptyColumn`] and
    /// [`DbRowError::UnreadableColumn`].
    pub fn new(columns: Vec<Column>) -> Result<Self, DbRowError> {
        if columns.is_empty() {
            return Err(DbRowError::NoColumns);
        }
        // A hash set, not a scan of the earlier columns: the column list is
        // client-supplied, and the scan was quadratic (40,000 columns, a
        // 0.5 MB stream, took 9 s to refuse). Q-FUZZ.
        let mut seen: HashSet<&[u8]> = HashSet::with_capacity(columns.len());
        for c in columns.iter() {
            if c.name.starts_with(b"__") {
                return Err(DbRowError::ReservedColumnName);
            }
            if !seen.insert(c.name.as_slice()) {
                return Err(DbRowError::DuplicateColumnName);
            }
            match c.db_type {
                DbType::Empty => return Err(DbRowError::EmptyColumn),
                DbType::DbTimestamp => return Err(DbRowError::UnreadableColumn),
                _ => {}
            }
        }

        // Pass 1: count each size class. Mirrors 0x1802f9d05's loop.
        let (mut n1, mut n2, mut n4, mut n8, mut n_bool, mut n_obj) = (0, 0, 0, 0, 0, 0);
        for c in &columns {
            match SizeClass::of(c.db_type).ok_or(DbRowError::EmptyColumn)? {
                SizeClass::Bit => n_bool += 1,
                SizeClass::Fixed(1) => n1 += 1,
                SizeClass::Fixed(2) => n2 += 1,
                SizeClass::Fixed(4) => n4 += 1,
                SizeClass::Fixed(_) => n8 += 1,
                SizeClass::Object => n_obj += 1,
            }
        }

        // Pass 2: region bases, largest first. Mirrors 0x1802f9e60's loop,
        // which walks the class cursors from 8-byte down to 1-byte.
        let fixed_bytes = 8 * n8 + 4 * n4 + 2 * n2 + n1;
        let mut cursor_8 = 0usize;
        let mut cursor_4 = 8 * n8;
        let mut cursor_2 = 8 * n8 + 4 * n4;
        let mut cursor_1 = 8 * n8 + 4 * n4 + 2 * n2;
        let mut bool_bit = 8 * fixed_bytes;
        let null_bit_base = 8 * fixed_bytes + n_bool;
        // Every column gets a NULL bit, because EMPTY is refused above.
        let blob_size = (null_bit_base + columns.len()).div_ceil(8);
        let object_base = blob_size.next_multiple_of(8);
        let mut object_cursor = object_base;

        // Pass 3: hand each column the next slot of its own region, in
        // declaration order. Mirrors 0x1802f9f94's loop.
        let mut slots = Vec::with_capacity(columns.len());
        for c in &columns {
            let slot = match SizeClass::of(c.db_type).ok_or(DbRowError::EmptyColumn)? {
                SizeClass::Bit => {
                    let b = bool_bit;
                    bool_bit += 1;
                    Slot::Bit(b)
                }
                SizeClass::Fixed(w) => {
                    let cursor = match w {
                        1 => &mut cursor_1,
                        2 => &mut cursor_2,
                        4 => &mut cursor_4,
                        _ => &mut cursor_8,
                    };
                    let offset = *cursor;
                    *cursor += w;
                    Slot::Fixed { offset, width: w }
                }
                SizeClass::Object => {
                    // The reader stores the i-th trailing object at byte
                    // objectBase + 8*i (0x1802fe68e); we only need the index.
                    let i = (object_cursor - object_base) / 8;
                    object_cursor += 8;
                    Slot::Object(i)
                }
            };
            slots.push(slot);
        }

        Ok(DbRowDescriptor {
            columns,
            slots,
            blob_size,
            null_bit_base,
            object_count: n_obj,
        })
    }

    /// Convenience constructor from `(name, type)` pairs.
    pub fn from_pairs(pairs: &[(&str, DbType)]) -> Result<Self, DbRowError> {
        DbRowDescriptor::new(
            pairs
                .iter()
                .map(|&(n, t)| Column {
                    name: n.as_bytes().to_vec(),
                    db_type: t,
                })
                .collect(),
        )
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Column names, in declaration order — what `header.Keys()` returns on
    /// the client and therefore what a `CRowset`'s `.columns` is.
    pub fn names(&self) -> Vec<Vec<u8>> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Position of a named column, or `None`. Mirrors `header.Index(name)`.
    pub fn index_of(&self, name: &[u8]) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// Unpacked blob length in bytes, the figure the reader takes from the
    /// descriptor rather than from the stream.
    pub fn blob_size(&self) -> usize {
        self.blob_size
    }

    /// Bit index of the first NULL flag; flag for column `i` is at
    /// `null_bit_base() + i`.
    pub fn null_bit_base(&self) -> usize {
        self.null_bit_base
    }

    /// How many marshal objects follow the blob.
    pub fn object_count(&self) -> usize {
        self.object_count
    }

    /// The `0x22` ObjectEx the client's reader accepts, and that its own
    /// `__reduce__` (`0x1802fa790`) emits: header
    /// `(<global blue.DBRowDescriptor>, ((name, type), ...))`, then the two
    /// mandatory `0x2D` terminators — which [`crate::save`] writes for every
    /// `ObjectEx`, and whose omission makes the descriptor unreadable.
    pub fn to_pyvalue(&self) -> PyValue {
        let cols = PyValue::Tuple(
            self.columns
                .iter()
                .map(|c| {
                    PyValue::Tuple(vec![
                        PyValue::Bytes(c.name.clone()),
                        PyValue::Int(c.db_type as u16 as i64),
                    ])
                })
                .collect(),
        );
        PyValue::ObjectEx {
            form: ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(DESCRIPTOR_GLOBAL.to_vec()),
                // The call takes ONE argument, the tuple of columns, so the
                // args tuple has a single element.
                PyValue::Tuple(vec![cols]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        }
    }

    /// Parse the shape [`DbRowDescriptor::to_pyvalue`] writes.
    pub fn from_pyvalue(v: &PyValue) -> Result<Self, DbRowError> {
        let PyValue::ObjectEx { form, header, .. } = v else {
            return Err(DbRowError::MalformedDescriptor("not an ObjectEx"));
        };
        if *form != ObjectForm::Call {
            return Err(DbRowError::MalformedDescriptor("not the 0x22 form"));
        }
        let PyValue::Tuple(h) = header.as_ref() else {
            return Err(DbRowError::MalformedDescriptor("header is not a tuple"));
        };
        if h.len() < 2 {
            return Err(DbRowError::MalformedDescriptor("header tuple too short"));
        }
        match &h[0] {
            PyValue::Global(n) if n == DESCRIPTOR_GLOBAL => {}
            _ => {
                return Err(DbRowError::MalformedDescriptor(
                    "callable is not the blue.DBRowDescriptor global",
                ));
            }
        }
        let PyValue::Tuple(args) = &h[1] else {
            return Err(DbRowError::MalformedDescriptor("args is not a tuple"));
        };
        let [PyValue::Tuple(cols)] = args.as_slice() else {
            return Err(DbRowError::MalformedDescriptor(
                "args is not a 1-tuple of columns",
            ));
        };
        let mut columns = Vec::with_capacity(cols.len());
        for c in cols {
            let PyValue::Tuple(pair) = c else {
                return Err(DbRowError::MalformedDescriptor("column is not a tuple"));
            };
            let [name, ty] = pair.as_slice() else {
                return Err(DbRowError::MalformedDescriptor("column is not a 2-tuple"));
            };
            let name = match name {
                PyValue::Bytes(b) => b.clone(),
                PyValue::Unicode(s) => s.as_bytes().to_vec(),
                _ => {
                    return Err(DbRowError::MalformedDescriptor(
                        "column name is not a string",
                    ));
                }
            };
            let PyValue::Int(t) = ty else {
                return Err(DbRowError::MalformedDescriptor("column type is not an int"));
            };
            let t = u16::try_from(*t).map_err(|_| DbRowError::UnknownDbType(u16::MAX))?;
            let db_type = DbType::from_u16(t).ok_or(DbRowError::UnknownDbType(t))?;
            columns.push(Column { name, db_type });
        }
        DbRowDescriptor::new(columns)
    }
}

/// One cell of a [`DbRow`].
///
/// The variants are the shapes the client's column getter produces, not the
/// shapes SQL would. In particular `Cy` is a **scaled integer**: the getter
/// divides by 10000.0 (**confirmed**, `docs/reference/dbrow.md` §3), so store
/// `isk * 10000`.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    /// Sets the column's NULL bit, so the client reads `None` whatever the
    /// bytes say. **Confirmed** from the getter's `bt` at `0x1802fc0b0`.
    Null,
    Bool(bool),
    /// Every integer width, plus `CY` and `FILETIME`.
    ///
    /// `UI8` values at or above `2^63` are carried as the equivalent negative
    /// `i64` bit pattern: the bytes on the wire are right and the client's
    /// `PyLong_FromUnsignedLongLong` reads them back positive, but this type
    /// cannot name them. `UI4` is read back **sign-extended** by the client
    /// (same handler as `I4`), so values at or above `2^31` come back
    /// negative on the client side. Both **confirmed**.
    Int(i64),
    /// `R4` is stored as `f32`, so a round trip through an `R4` column is
    /// only exact for `f32`-representable values.
    Real(f64),
    /// A `BYTES`/`STR`/`WSTR` column: no blob bytes, one marshal object
    /// appended after the blob in declaration order.
    Object(PyValue),
}

impl DbValue {
    fn kind(&self) -> &'static str {
        match self {
            DbValue::Null => "null",
            DbValue::Bool(_) => "bool",
            DbValue::Int(_) => "int",
            DbValue::Real(_) => "real",
            DbValue::Object(_) => "object",
        }
    }
}

/// A `blue.DBRow`: a descriptor plus one value per column.
///
/// Values are validated against the descriptor at construction so that
/// encoding stays infallible, matching the rest of this crate.
#[derive(Debug, Clone, PartialEq)]
pub struct DbRow {
    descriptor: DbRowDescriptor,
    values: Vec<DbValue>,
}

impl DbRow {
    /// Check `values` against `descriptor` and build a row.
    pub fn new(descriptor: DbRowDescriptor, values: Vec<DbValue>) -> Result<Self, DbRowError> {
        if values.len() != descriptor.columns.len() {
            return Err(DbRowError::ColumnCountMismatch {
                want: descriptor.columns.len(),
                got: values.len(),
            });
        }
        for (i, (c, v)) in descriptor.columns.iter().zip(&values).enumerate() {
            check_value(i, c.db_type, v)?;
        }
        Ok(DbRow { descriptor, values })
    }

    pub fn descriptor(&self) -> &DbRowDescriptor {
        &self.descriptor
    }

    pub fn values(&self) -> &[DbValue] {
        &self.values
    }

    /// The unpacked blob, before RLE. Exposed because the exact-byte tests
    /// and any future capture comparison want it.
    pub fn pack(&self) -> Vec<u8> {
        let d = &self.descriptor;
        let mut blob = vec![0u8; d.blob_size];
        for (i, (slot, value)) in d.slots.iter().zip(&self.values).enumerate() {
            if matches!(value, DbValue::Null) {
                // A set NULL bit makes the client return None regardless of
                // the bytes, so the zero fill is the whole of the payload.
                set_bit(&mut blob, d.null_bit_base + i);
                continue;
            }
            match (slot, value) {
                (Slot::Bit(b), DbValue::Bool(true)) => set_bit(&mut blob, *b),
                (Slot::Bit(_), DbValue::Bool(false)) => {}
                (Slot::Fixed { offset, width }, v) => {
                    let bytes = fixed_bytes(d.columns[i].db_type, v);
                    blob[*offset..*offset + *width].copy_from_slice(&bytes[..*width]);
                }
                // Object columns occupy no blob bytes; Bool/Null handled above.
                _ => {}
            }
        }
        blob
    }

    /// The marshal objects that follow the blob, in declaration order.
    pub fn objects(&self) -> Vec<PyValue> {
        let mut out = Vec::with_capacity(self.descriptor.object_count);
        for (slot, value) in self.descriptor.slots.iter().zip(&self.values) {
            if matches!(slot, Slot::Object(_)) {
                out.push(match value {
                    DbValue::Object(v) => v.clone(),
                    // A NULL object column still occupies its slot; the
                    // reader pre-fills the slots with None (0x1802fe51f) but
                    // then overwrites every one of them from the stream, so
                    // the object must still be sent.
                    _ => PyValue::None,
                });
            }
        }
        out
    }

    /// Rebuild a row from an unpacked blob and the trailing objects.
    pub fn unpack(
        descriptor: DbRowDescriptor,
        blob: &[u8],
        objects: Vec<PyValue>,
    ) -> Result<Self, DbRowError> {
        if objects.len() != descriptor.object_count {
            return Err(DbRowError::ColumnCountMismatch {
                want: descriptor.object_count,
                got: objects.len(),
            });
        }
        let mut values = Vec::with_capacity(descriptor.columns.len());
        for (i, (slot, column)) in descriptor.slots.iter().zip(&descriptor.columns).enumerate() {
            if get_bit(blob, descriptor.null_bit_base + i) {
                values.push(DbValue::Null);
                continue;
            }
            values.push(match slot {
                Slot::Bit(b) => DbValue::Bool(get_bit(blob, *b)),
                Slot::Fixed { offset, width } => {
                    read_fixed(column.db_type, &blob[*offset..*offset + *width])
                }
                Slot::Object(n) => DbValue::Object(objects[*n].clone()),
            });
        }
        Ok(DbRow { descriptor, values })
    }
}

fn check_value(column: usize, t: DbType, v: &DbValue) -> Result<(), DbRowError> {
    let mismatch = || DbRowError::TypeMismatch {
        column,
        want: t,
        got: v.kind(),
    };
    if matches!(v, DbValue::Null) {
        return Ok(());
    }
    match t {
        DbType::Bool => matches!(v, DbValue::Bool(_))
            .then_some(())
            .ok_or_else(mismatch),
        DbType::R4 | DbType::R5 => matches!(v, DbValue::Real(_))
            .then_some(())
            .ok_or_else(mismatch),
        DbType::Bytes | DbType::Str | DbType::WStr => matches!(v, DbValue::Object(_))
            .then_some(())
            .ok_or_else(mismatch),
        DbType::Empty | DbType::DbTimestamp => Err(mismatch()),
        // Every remaining type is an integer of some width.
        _ => {
            let DbValue::Int(n) = v else {
                return Err(mismatch());
            };
            let (lo, hi) = int_range(t);
            if *n < lo || *n > hi {
                return Err(DbRowError::ValueOutOfRange { column, value: *n });
            }
            Ok(())
        }
    }
}

/// Accepted range per integer `DBTYPE`.
///
/// The unsigned widths accept the signed range too, because the client's own
/// writer packs a Python int straight into the slot and its `UI4` getter
/// sign-extends; refusing `-1` for a `UI4` would refuse something the client
/// round-trips.
fn int_range(t: DbType) -> (i64, i64) {
    match t {
        DbType::I1 => (i8::MIN as i64, i8::MAX as i64),
        DbType::Ui1 => (i8::MIN as i64, u8::MAX as i64),
        DbType::I2 => (i16::MIN as i64, i16::MAX as i64),
        DbType::Ui2 => (i16::MIN as i64, u16::MAX as i64),
        DbType::I4 => (i32::MIN as i64, i32::MAX as i64),
        DbType::Ui4 => (i32::MIN as i64, u32::MAX as i64),
        // I8 / UI8 / CY / FILETIME all occupy the full 8 bytes.
        _ => (i64::MIN, i64::MAX),
    }
}

/// Little-endian bytes for a fixed-width column. **Confirmed**: the getter
/// uses plain loads with no byte swap.
fn fixed_bytes(t: DbType, v: &DbValue) -> [u8; 8] {
    let mut out = [0u8; 8];
    match (t, v) {
        (DbType::R4, DbValue::Real(f)) => out[..4].copy_from_slice(&(*f as f32).to_le_bytes()),
        (DbType::R5, DbValue::Real(f)) => out.copy_from_slice(&f.to_le_bytes()),
        (_, DbValue::Int(n)) => out.copy_from_slice(&n.to_le_bytes()),
        _ => {}
    }
    out
}

fn read_fixed(t: DbType, bytes: &[u8]) -> DbValue {
    match t {
        DbType::R4 => {
            DbValue::Real(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64)
        }
        DbType::R5 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(bytes);
            DbValue::Real(f64::from_le_bytes(b))
        }
        // Signed widths sign-extend; UI1/UI2 widen; UI4 is read SIGNED by the
        // client (same handler as I4), so we mirror that rather than widen.
        DbType::I1 => DbValue::Int(bytes[0] as i8 as i64),
        DbType::Ui1 => DbValue::Int(bytes[0] as i64),
        DbType::I2 => DbValue::Int(i16::from_le_bytes([bytes[0], bytes[1]]) as i64),
        DbType::Ui2 => DbValue::Int(u16::from_le_bytes([bytes[0], bytes[1]]) as i64),
        DbType::I4 | DbType::Ui4 => {
            DbValue::Int(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64)
        }
        _ => {
            let mut b = [0u8; 8];
            b.copy_from_slice(bytes);
            DbValue::Int(i64::from_le_bytes(b))
        }
    }
}

/// `byte = i >> 3`, `bit = i & 7` — LSB-first, matching the client's `bt`.
fn set_bit(blob: &mut [u8], i: usize) {
    blob[i >> 3] |= 1 << (i & 7);
}

fn get_bit(blob: &[u8], i: usize) -> bool {
    blob.get(i >> 3).is_some_and(|b| b & (1 << (i & 7)) != 0)
}

/// Decode the nibble RLE, exactly as `0x1802fe210` does.
///
/// Control nibbles are taken **low nibble first**, then high. `n < 8` copies
/// `8 - n` literal bytes; `n >= 8` emits `n - 7` zero bytes. A run that would
/// pass `blob_size` is an error; a stream that stops short simply leaves the
/// rest zero, because the reader zero-fills the remainder
/// (`0x1802fe2cf`). All **confirmed**.
pub fn rle_decode(src: &[u8], blob_size: usize) -> Result<Vec<u8>, DbRowError> {
    let mut out = vec![0u8; blob_size];
    let mut pos = 0usize; // destination cursor
    let mut i = 0usize; // source cursor
    let mut control = 0u8;
    let mut high = false; // which nibble of `control` comes next
    while i < src.len() {
        if pos >= blob_size {
            // The client leaves the loop here and returns success, ignoring
            // whatever source bytes are left.
            return Ok(out);
        }
        let n = if high {
            control >> 4
        } else {
            control = src[i];
            i += 1;
            control & 0x0F
        };
        high = !high;
        if n >= RLE_ZERO_THRESHOLD {
            let run = (n - (RLE_ZERO_THRESHOLD - 1)) as usize;
            if pos + run > blob_size {
                return Err(DbRowError::InvalidRle);
            }
            // The destination is already zero-filled.
            pos += run;
        } else {
            let run = RLE_MAX_RUN - n as usize;
            if pos + run > blob_size {
                return Err(DbRowError::InvalidRle);
            }
            for _ in 0..run {
                if i >= src.len() {
                    // Source exhausted mid-literal: the client stops and
                    // zero-fills the rest rather than failing.
                    return Ok(out);
                }
                out[pos] = src[i];
                pos += 1;
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Encode a blob as nibble RLE.
///
/// Greedy and deterministic: a zero byte starts a zero run of up to 8, any
/// other byte starts a literal run of up to 8. The client's own encoder also
/// trims trailing zero-nibble runs (whole output bytes only); we do not,
/// because the trim is a pure size optimisation and the reader zero-fills
/// either way. The only hard requirement is never to decode to more than
/// `blobSize` bytes, which this cannot do: it emits exactly `blob.len()`.
pub fn rle_encode(blob: &[u8]) -> Vec<u8> {
    let mut nibbles: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < blob.len() {
        if blob[i] == 0 {
            let mut run = 0usize;
            while run < RLE_MAX_RUN && i + run < blob.len() && blob[i + run] == 0 {
                run += 1;
            }
            nibbles.push(RLE_ZERO_THRESHOLD - 1 + run as u8);
            i += run;
        } else {
            let mut run = 0usize;
            while run < RLE_MAX_RUN && i + run < blob.len() && blob[i + run] != 0 {
                run += 1;
            }
            nibbles.push(RLE_MAX_RUN as u8 - run as u8);
            // The literal bytes follow the control byte they belong to, so
            // they are collected separately below.
            i += run;
        }
    }
    // Second pass: pack the nibbles two per control byte (low first) and
    // interleave each literal run's bytes after the control byte that
    // completes it, which is what the decoder's single cursor expects.
    let mut out = Vec::with_capacity(blob.len());
    let mut pending: Vec<u8> = Vec::new();
    let mut src = 0usize;
    let mut control = 0u8;
    let mut have_low = false;
    for n in nibbles {
        let run = if n >= RLE_ZERO_THRESHOLD {
            let run = (n - (RLE_ZERO_THRESHOLD - 1)) as usize;
            src += run;
            None
        } else {
            let run = RLE_MAX_RUN - n as usize;
            let slice = blob[src..src + run].to_vec();
            src += run;
            Some(slice)
        };
        if have_low {
            out.push(control | (n << 4));
            out.append(&mut pending);
            if let Some(mut s) = run {
                out.append(&mut s);
            }
            have_low = false;
        } else {
            control = n;
            have_low = true;
            pending = run.unwrap_or_default();
        }
    }
    if have_low {
        out.push(control);
        out.append(&mut pending);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tag;
    use crate::value::{Decoder, save};
    use crate::{Reader, Writer};

    /// `eveCfg.GetOwnersRowDescriptor`, read off the client's own
    /// decompiled `eve/common/script/sys/eveCfg.pyc`. The column **order**
    /// matters: `cfg.eveowners` is pre-populated with client-built DBRows
    /// using this order, and `Recordset._Prime` then replaces the header
    /// with whatever the server sent, so a server that reorders the columns
    /// corrupts the client's own pre-loaded NPC rows.
    fn owners() -> DbRowDescriptor {
        DbRowDescriptor::from_pairs(&[
            ("ownerID", DbType::I4),
            ("ownerName", DbType::WStr),
            ("typeID", DbType::I4),
            ("gender", DbType::I2),
            ("ownerNameID", DbType::I4),
        ])
        .unwrap()
    }

    /// `eveCfg.GetLocationRowDescriptor`, same source.
    fn locations() -> DbRowDescriptor {
        DbRowDescriptor::from_pairs(&[
            ("locationID", DbType::I8),
            ("locationName", DbType::WStr),
            ("solarSystemID", DbType::I8),
            ("x", DbType::R5),
            ("y", DbType::R5),
            ("z", DbType::R5),
            ("locationNameID", DbType::I4),
        ])
        .unwrap()
    }

    #[test]
    fn size_classes_match_the_clients_own_table() {
        // Dumped from the byte table at 0x1802fa0c8 through the jump table
        // at 0x1802fa0a8: these are the only 18 declarable types, and the
        // class each one lands in.
        let cases: [(DbType, Option<SizeClass>); 18] = [
            (DbType::Empty, None),
            (DbType::Bool, Some(SizeClass::Bit)),
            (DbType::I1, Some(SizeClass::Fixed(1))),
            (DbType::Ui1, Some(SizeClass::Fixed(1))),
            (DbType::I2, Some(SizeClass::Fixed(2))),
            (DbType::Ui2, Some(SizeClass::Fixed(2))),
            (DbType::I4, Some(SizeClass::Fixed(4))),
            (DbType::R4, Some(SizeClass::Fixed(4))),
            (DbType::Ui4, Some(SizeClass::Fixed(4))),
            (DbType::R5, Some(SizeClass::Fixed(8))),
            (DbType::Cy, Some(SizeClass::Fixed(8))),
            (DbType::I8, Some(SizeClass::Fixed(8))),
            (DbType::Ui8, Some(SizeClass::Fixed(8))),
            (DbType::FileTime, Some(SizeClass::Fixed(8))),
            (DbType::DbTimestamp, Some(SizeClass::Fixed(8))),
            (DbType::Bytes, Some(SizeClass::Object)),
            (DbType::Str, Some(SizeClass::Object)),
            (DbType::WStr, Some(SizeClass::Object)),
        ];
        for (t, want) in cases {
            assert_eq!(SizeClass::of(t), want, "{t:?}");
            assert!(t as u16 <= MAX_DB_TYPE, "{t:?} exceeds the 0x87 ceiling");
        }
    }

    #[test]
    fn regions_are_ordered_largest_first_but_keep_declaration_order() {
        // Declaration order deliberately scrambled across the regions, so a
        // decoder that permuted the columns instead of partitioning them
        // into regions would disagree here.
        let d = DbRowDescriptor::from_pairs(&[
            ("a1", DbType::I1),
            ("b8", DbType::I8),
            ("c2", DbType::I2),
            ("d4", DbType::I4),
            ("e8", DbType::Ui8),
            ("f1", DbType::Ui1),
            ("g4", DbType::R4),
            ("h2", DbType::Ui2),
        ])
        .unwrap();
        // n8 = 2, n4 = 2, n2 = 2, n1 = 2  =>  F = 16 + 8 + 4 + 2 = 30
        // 8-byte region at 0, 4-byte at 16, 2-byte at 24, 1-byte at 28.
        let want: [(&str, usize, usize); 8] = [
            ("b8", 0, 8),
            ("e8", 8, 8),
            ("d4", 16, 4),
            ("g4", 20, 4),
            ("c2", 24, 2),
            ("h2", 26, 2),
            ("a1", 28, 1),
            ("f1", 29, 1),
        ];
        for (name, offset, width) in want {
            let i = d.index_of(name.as_bytes()).unwrap();
            assert_eq!(
                d.slots[i],
                Slot::Fixed { offset, width },
                "column {name} (declared at index {i})"
            );
        }
        assert_eq!(d.null_bit_base(), 8 * 30, "no bools, so nullBitBase = 8*F");
        assert_eq!(d.blob_size(), 31, "ceil((240 + 8) / 8)");
        assert_eq!(d.object_count(), 0);
    }

    #[test]
    fn bool_bits_sit_after_the_fixed_area_and_before_the_null_bitmap() {
        let d = DbRowDescriptor::from_pairs(&[
            ("flagA", DbType::Bool),
            ("n", DbType::I4),
            ("flagB", DbType::Bool),
        ])
        .unwrap();
        // F = 4, so bool bits are 32 and 33, and the NULL bitmap starts at 34.
        assert_eq!(d.slots[0], Slot::Bit(32));
        assert_eq!(d.slots[2], Slot::Bit(33));
        assert_eq!(d.null_bit_base(), 34);
        // ceil((34 + 3) / 8) = 5
        assert_eq!(d.blob_size(), 5);
    }

    #[test]
    fn object_columns_take_no_blob_bytes_but_do_take_a_null_bit() {
        let d = DbRowDescriptor::from_pairs(&[("s", DbType::WStr), ("n", DbType::I2)]).unwrap();
        assert_eq!(d.slots[0], Slot::Object(0));
        assert_eq!(
            d.slots[1],
            Slot::Fixed {
                offset: 0,
                width: 2
            }
        );
        // F = 2 (only the I2), nullBitBase = 16, 2 null bits => 3 bytes.
        assert_eq!(d.null_bit_base(), 16);
        assert_eq!(d.blob_size(), 3);
        assert_eq!(d.object_count(), 1);
    }

    #[test]
    fn the_owners_layout_is_what_the_hand_worked_example_says() {
        let d = owners();
        // n4 = 3 (ownerID, typeID, ownerNameID), n2 = 1 (gender), nObj = 1.
        // F = 12 + 2 = 14; nullBitBase = 112; blobSize = ceil(117/8) = 15.
        assert_eq!(
            d.slots[0],
            Slot::Fixed {
                offset: 0,
                width: 4
            }
        ); // ownerID
        assert_eq!(d.slots[1], Slot::Object(0)); // ownerName
        assert_eq!(
            d.slots[2],
            Slot::Fixed {
                offset: 4,
                width: 4
            }
        ); // typeID
        assert_eq!(
            d.slots[3],
            Slot::Fixed {
                offset: 12,
                width: 2
            }
        ); // gender, the only 2-byte column, after all three 4-byte ones
        assert_eq!(
            d.slots[4],
            Slot::Fixed {
                offset: 8,
                width: 4
            }
        ); // ownerNameID
        assert_eq!(d.null_bit_base(), 112);
        assert_eq!(d.blob_size(), 15);
        assert_eq!(d.object_count(), 1);
    }

    #[test]
    fn the_locations_layout_puts_the_i4_after_every_eight_byte_column() {
        let d = locations();
        // n8 = 5 (locationID, solarSystemID, x, y, z), n4 = 1, nObj = 1.
        // F = 40 + 4 = 44; nullBitBase = 352; blobSize = ceil(359/8) = 45.
        for (i, offset) in [(0usize, 0usize), (2, 8), (3, 16), (4, 24), (5, 32)] {
            assert_eq!(d.slots[i], Slot::Fixed { offset, width: 8 }, "column {i}");
        }
        assert_eq!(d.slots[1], Slot::Object(0));
        assert_eq!(
            d.slots[6],
            Slot::Fixed {
                offset: 40,
                width: 4
            },
            "locationNameID lands after the 8-byte region"
        );
        assert_eq!(d.null_bit_base(), 352);
        assert_eq!(d.blob_size(), 45);
    }

    // --- the NULL bitmap -------------------------------------------------

    #[test]
    fn a_set_null_bit_is_what_makes_a_column_read_as_none() {
        let d = owners();
        let row = DbRow::new(
            d,
            vec![
                DbValue::Int(1_000_182),
                DbValue::Object(PyValue::Unicode("CCP Zoo".into())),
                DbValue::Int(1378),
                DbValue::Null,
                DbValue::Null,
            ],
        )
        .unwrap();
        let blob = row.pack();
        assert_eq!(blob.len(), 15);
        // Columns 3 and 4 are NULL, so bits 112+3 and 112+4 are set and
        // nothing else in the bitmap byte is.
        assert_eq!(blob[14], 0b0001_1000, "null bits are LSB-first from 112");
        // Inverting the sense would make the three present columns read as
        // None, so assert the present ones are clear too.
        for i in 0..3 {
            assert!(!get_bit(&blob, 112 + i), "column {i} must not be flagged");
        }
    }

    #[test]
    fn a_null_scalar_still_leaves_its_bytes_zero() {
        let d = DbRowDescriptor::from_pairs(&[("n", DbType::I4)]).unwrap();
        let row = DbRow::new(d, vec![DbValue::Null]).unwrap();
        assert_eq!(row.pack(), vec![0, 0, 0, 0, 0b1]);
    }

    #[test]
    fn two_object_columns_keep_declaration_order() {
        // With only one object column every ordering looks identical, so
        // this is the case that pins the mapping from declaration index to
        // trailing-object index.
        let d = DbRowDescriptor::from_pairs(&[
            ("first", DbType::WStr),
            ("n", DbType::I4),
            ("second", DbType::Str),
            ("third", DbType::Bytes),
        ])
        .unwrap();
        assert_eq!(d.slots[0], Slot::Object(0));
        assert_eq!(d.slots[2], Slot::Object(1));
        assert_eq!(d.slots[3], Slot::Object(2));
        assert_eq!(d.object_count(), 3);

        let row = DbRow::new(
            d,
            vec![
                DbValue::Object(PyValue::Unicode("one".into())),
                DbValue::Int(5),
                DbValue::Object(PyValue::Bytes(b"two".to_vec())),
                DbValue::Object(PyValue::Bytes(b"three".to_vec())),
            ],
        )
        .unwrap();
        // The reader stores the i-th object it reads in the i-th object slot
        // (0x1802fe68e), so the order on the wire is declaration order.
        assert_eq!(
            row.objects(),
            vec![
                PyValue::Unicode("one".into()),
                PyValue::Bytes(b"two".to_vec()),
                PyValue::Bytes(b"three".to_vec()),
            ]
        );

        // And the whole row survives a trip through the codec, which it
        // cannot do if the slot indices collide.
        let v = PyValue::DbRow(Box::new(row));
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_null_object_column_still_sends_an_object() {
        // The reader overwrites every pre-set None slot from the stream
        // (0x1802fe670), so the object count on the wire is fixed by the
        // descriptor, not by which values happen to be null.
        let d = DbRowDescriptor::from_pairs(&[("s", DbType::WStr)]).unwrap();
        let row = DbRow::new(d, vec![DbValue::Null]).unwrap();
        assert_eq!(row.objects(), vec![PyValue::None]);
    }

    // --- nibble RLE ------------------------------------------------------

    #[test]
    fn rle_control_nibbles_are_low_then_high() {
        // Control byte 0x87: low nibble 7 -> copy 8-7 = 1 literal byte;
        // high nibble 8 -> 8-7 = 1 zero byte. Written out by hand from the
        // documented layout, not produced by rle_encode.
        let got = rle_decode(&[0x87, 0xAA], 2).unwrap();
        assert_eq!(got, vec![0xAA, 0x00]);
        // Swap the nibbles and the output swaps with them.
        let got = rle_decode(&[0x78, 0xAA], 2).unwrap();
        assert_eq!(got, vec![0x00, 0xAA]);
    }

    #[test]
    fn rle_run_lengths_are_8_minus_n_and_n_minus_7() {
        // n = 0 -> 8 literals; n = 15 -> 8 zeros.
        let src = [0x00, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(rle_decode(&src, 8).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(rle_decode(&[0x0F], 8).unwrap(), vec![0u8; 8]);
        // n = 8 is the shortest zero run: exactly one byte.
        assert_eq!(rle_decode(&[0x08], 1).unwrap(), vec![0u8]);
    }

    #[test]
    fn rle_refuses_to_decode_past_the_descriptors_blob_size() {
        // One zero run of 8 into a 7-byte blob: "Invalid RLE string".
        assert_eq!(rle_decode(&[0x0F], 7), Err(DbRowError::InvalidRle));
        // And a literal run that would overrun, likewise.
        assert_eq!(rle_decode(&[0x00, 1, 2, 3], 3), Err(DbRowError::InvalidRle));
    }

    #[test]
    fn a_short_rle_stream_zero_fills_the_rest() {
        // The reader takes the length from the descriptor, so a stream that
        // stops early is legal and the remainder is zero.
        assert_eq!(rle_decode(&[], 4).unwrap(), vec![0u8; 4]);
        assert_eq!(rle_decode(&[0x08], 4).unwrap(), vec![0u8; 4]);
    }

    #[test]
    fn rle_round_trips_over_awkward_blobs() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![1],
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0],
            (0u8..=40).collect(),
            vec![1, 0, 2, 0, 3, 0, 4, 0, 5],
            {
                let mut v = vec![0u8; 20];
                v[19] = 0xFF;
                v
            },
        ];
        for blob in cases {
            let encoded = rle_encode(&blob);
            let decoded = rle_decode(&encoded, blob.len()).unwrap();
            assert_eq!(decoded, blob, "blob {blob:?} encoded as {encoded:?}");
        }
    }

    #[test]
    fn rle_encodes_the_owners_blob_to_exactly_nine_bytes() {
        // Hand-worked from the layout: literal(3) F6 42 0F, zero(1),
        // literal(2) 62 05, zero(8), literal(1) 18. Nibbles 5,8,6,15,7
        // pack low-first into 0x85, 0xF6, 0x07 with each run's literals
        // following the control byte that completes it.
        let blob = vec![
            0xF6, 0x42, 0x0F, 0x00, 0x62, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x18,
        ];
        assert_eq!(
            rle_encode(&blob),
            vec![0x85, 0xF6, 0x42, 0x0F, 0xF6, 0x62, 0x05, 0x07, 0x18]
        );
    }

    // --- descriptor validation -------------------------------------------

    #[test]
    fn a_descriptor_refuses_what_the_client_refuses() {
        assert_eq!(
            DbRowDescriptor::from_pairs(&[("__secret", DbType::I4)]),
            Err(DbRowError::ReservedColumnName)
        );
        assert_eq!(
            DbRowDescriptor::from_pairs(&[("a", DbType::I4), ("a", DbType::I2)]),
            Err(DbRowError::DuplicateColumnName)
        );
        assert_eq!(DbRowDescriptor::from_pairs(&[]), Err(DbRowError::NoColumns));
        // One leading underscore is fine; the client tests two.
        assert!(DbRowDescriptor::from_pairs(&[("_ok", DbType::I4)]).is_ok());
    }

    #[test]
    fn a_descriptor_refuses_the_two_unusable_types() {
        assert_eq!(
            DbRowDescriptor::from_pairs(&[("e", DbType::Empty)]),
            Err(DbRowError::EmptyColumn)
        );
        assert_eq!(
            DbRowDescriptor::from_pairs(&[("t", DbType::DbTimestamp)]),
            Err(DbRowError::UnreadableColumn)
        );
    }

    #[test]
    fn a_row_is_checked_against_its_descriptor() {
        let d = DbRowDescriptor::from_pairs(&[("n", DbType::I2), ("s", DbType::Str)]).unwrap();
        assert_eq!(
            DbRow::new(d.clone(), vec![DbValue::Int(1)]),
            Err(DbRowError::ColumnCountMismatch { want: 2, got: 1 })
        );
        assert_eq!(
            DbRow::new(
                d.clone(),
                vec![DbValue::Int(70_000), DbValue::Object(PyValue::None)]
            ),
            Err(DbRowError::ValueOutOfRange {
                column: 0,
                value: 70_000
            })
        );
        assert_eq!(
            DbRow::new(d, vec![DbValue::Int(1), DbValue::Int(2)]),
            Err(DbRowError::TypeMismatch {
                column: 1,
                want: DbType::Str,
                got: "int"
            })
        );
    }

    #[test]
    fn integer_widths_accept_exactly_their_range() {
        for (t, lo, hi) in [
            (DbType::I1, -128i64, 127i64),
            (DbType::Ui1, -128, 255),
            (DbType::I2, -32768, 32767),
            (DbType::Ui2, -32768, 65535),
            (DbType::I4, -2147483648, 2147483647),
            (DbType::Ui4, -2147483648, 4294967295),
        ] {
            let d = DbRowDescriptor::from_pairs(&[("n", t)]).unwrap();
            assert!(
                DbRow::new(d.clone(), vec![DbValue::Int(lo)]).is_ok(),
                "{t:?}"
            );
            assert!(
                DbRow::new(d.clone(), vec![DbValue::Int(hi)]).is_ok(),
                "{t:?}"
            );
            assert!(
                DbRow::new(d.clone(), vec![DbValue::Int(hi + 1)]).is_err(),
                "{t:?} accepted {}",
                hi + 1
            );
            assert!(
                DbRow::new(d, vec![DbValue::Int(lo - 1)]).is_err(),
                "{t:?} accepted {}",
                lo - 1
            );
        }
    }

    // --- the descriptor object on the wire -------------------------------

    #[test]
    fn a_descriptor_is_a_0x22_with_the_global_and_a_one_tuple_of_columns() {
        let d = DbRowDescriptor::from_pairs(&[("n", DbType::I4)]).unwrap();
        let mut w = Writer::new();
        save(&d.to_pyvalue(), &mut w);
        // Written out by hand from docs/reference/object-wrapper.md §8 and
        // docs/reference/dbrow.md §2, including the two mandatory terminators.
        let mut want = vec![
            tag::code::OBJECT_22,
            tag::code::TUPLE2, // (callable, args)
            tag::code::GLOBAL,
            DESCRIPTOR_GLOBAL.len() as u8,
        ];
        want.extend_from_slice(DESCRIPTOR_GLOBAL);
        want.extend_from_slice(&[
            tag::code::TUPLE1,      // args = (cols,)
            tag::code::TUPLE1,      // cols = one column
            tag::code::TUPLE2,      // the column pair
            tag::code::STRING_CHAR, // "n"
            b'n',
            tag::code::INT8, // DBTYPE_I4
            3,
            tag::code::OBJECT_EX_END,
            tag::code::OBJECT_EX_END,
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_descriptor_survives_a_trip_through_pyvalue() {
        for d in [owners(), locations()] {
            let v = d.to_pyvalue();
            assert_eq!(DbRowDescriptor::from_pyvalue(&v).unwrap(), d);
        }
    }

    #[test]
    fn a_descriptor_with_the_wrong_global_is_refused() {
        let PyValue::ObjectEx { header, .. } = owners().to_pyvalue() else {
            panic!()
        };
        let PyValue::Tuple(h) = *header else { panic!() };
        let bad = PyValue::ObjectEx {
            form: ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"blue.DBRow".to_vec()),
                h[1].clone(),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        };
        assert!(matches!(
            DbRowDescriptor::from_pyvalue(&bad),
            Err(DbRowError::MalformedDescriptor(_))
        ));
        // And the 0x23 form is not the shape the client's reader builds a
        // descriptor from.
        let wrong_form = PyValue::ObjectEx {
            form: ObjectForm::New,
            header: Box::new(PyValue::Tuple(h)),
            list: Vec::new(),
            dict: Vec::new(),
        };
        assert!(matches!(
            DbRowDescriptor::from_pyvalue(&wrong_form),
            Err(DbRowError::MalformedDescriptor(_))
        ));
    }

    // --- the whole row on the wire ---------------------------------------

    fn ccp_zoo() -> DbRow {
        DbRow::new(
            owners(),
            vec![
                DbValue::Int(1_000_182),
                DbValue::Object(PyValue::Unicode("CCP Zoo".into())),
                DbValue::Int(1378),
                DbValue::Null,
                DbValue::Null,
            ],
        )
        .unwrap()
    }

    /// Every byte of one `0x2A`, written out from the documented layout
    /// rather than recomputed with the encoder. This is the test that
    /// catches a misunderstanding shared by both directions.
    #[test]
    fn one_dbrow_is_exactly_these_bytes() {
        let mut w = Writer::new();
        save(&PyValue::DbRow(Box::new(ccp_zoo())), &mut w);

        let mut want: Vec<u8> = vec![
            0x2A, // tag
            // --- the descriptor, a 0x22 ObjectEx ---
            0x22, 0x2C, // header 2-tuple
            0x02, // global by name
            20,
        ];
        want.extend_from_slice(b"blue.DBRowDescriptor");
        want.extend_from_slice(&[
            0x25, // args 1-tuple
            0x14, // cols tuple
            5,
        ]);
        for (name, ty) in [
            ("ownerID", 3u16),
            ("ownerName", 130),
            ("typeID", 3),
            ("gender", 2),
            ("ownerNameID", 3),
        ] {
            want.push(0x2C); // the (name, type) 2-tuple
            want.push(0x10); // short string
            want.push(name.len() as u8);
            want.extend_from_slice(name.as_bytes());
            if ty <= 127 {
                want.push(0x06); // int8
                want.push(ty as u8);
            } else {
                want.push(0x05); // int16
                want.extend_from_slice(&ty.to_le_bytes());
            }
        }
        want.extend_from_slice(&[
            0x2D, // the list-append stream, empty
            0x2D, // the dict-setitem stream, empty
        ]);
        // --- the blob: escaped length, then nibble RLE ---
        // Unpacked, 15 bytes: ownerID at 0, typeID at 4, ownerNameID at 8,
        // gender at 12, NULL bitmap byte at 14 (bits 112..116).
        //   F6 42 0F 00 | 62 05 00 00 | 00 00 00 00 | 00 00 | 18
        want.extend_from_slice(&[9, 0x85, 0xF6, 0x42, 0x0F, 0xF6, 0x62, 0x05, 0x07, 0x18]);
        // --- one object per object column, declaration order ---
        want.extend_from_slice(&[0x2E, 7]); // unicode, UTF-8 form, 7 bytes
        want.extend_from_slice(b"CCP Zoo");

        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_dbrow_round_trips_through_the_decoder() {
        for row in [
            ccp_zoo(),
            DbRow::new(
                locations(),
                vec![
                    DbValue::Int(60_003_760),
                    DbValue::Object(PyValue::Unicode("Jita IV - Moon 4".into())),
                    DbValue::Int(30_000_142),
                    DbValue::Real(-1.0),
                    DbValue::Real(0.5),
                    DbValue::Real(1.25e7),
                    DbValue::Null,
                ],
            )
            .unwrap(),
            DbRow::new(
                DbRowDescriptor::from_pairs(&[
                    ("f", DbType::Bool),
                    ("g", DbType::Bool),
                    ("cy", DbType::Cy),
                    ("ft", DbType::FileTime),
                    ("b", DbType::Bytes),
                    ("tiny", DbType::I1),
                ])
                .unwrap(),
                vec![
                    DbValue::Bool(true),
                    DbValue::Bool(false),
                    DbValue::Int(123_450_000),
                    DbValue::Int(133_700_000_000_000_000),
                    DbValue::Object(PyValue::Bytes(b"blob".to_vec())),
                    DbValue::Int(-7),
                ],
            )
            .unwrap(),
        ] {
            let v = PyValue::DbRow(Box::new(row));
            let mut w = Writer::new();
            save(&v, &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            let got = Decoder::new().load(&mut r).unwrap();
            assert_eq!(got, v);
            assert_eq!(r.remaining(), 0);
        }
    }

    #[test]
    fn a_dbrow_whose_descriptor_is_not_one_is_refused() {
        // A 0x2A whose first child is an ordinary tuple must fail rather
        // than be interpreted; the client raises "Hacker Warning! Invalid
        // type in marshal data, expected blue.DBRowDescriptor."
        let bytes = [0x2A, tag::code::TUPLE_EMPTY, 0x00];
        let mut r = Reader::new(&bytes);
        let err = Decoder::new().load(&mut r).unwrap_err();
        assert_eq!(
            err,
            crate::WireError::Malformed {
                at: 1,
                why: "0x2A descriptor is not a blue.DBRowDescriptor"
            }
        );
    }

    #[test]
    fn a_substream_is_an_escaped_length_then_raw_bytes() {
        let v = PyValue::SubStream(vec![0x7E, 0, 0, 0, 0, tag::code::NONE]);
        let mut w = Writer::new();
        save(&v, &mut w);
        assert_eq!(
            w.as_slice(),
            &[0x2B, 6, 0x7E, 0, 0, 0, 0, tag::code::NONE][..]
        );
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_global_is_an_escaped_length_then_the_dotted_name() {
        let v = PyValue::Global(b"blue.DBRow".to_vec());
        let mut w = Writer::new();
        save(&v, &mut w);
        let mut want = vec![0x02, 10];
        want.extend_from_slice(b"blue.DBRow");
        assert_eq!(w.as_slice(), &want[..]);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
    }
}
