//! Marshal type tags for client build V24.01 (3619879).
//!
//! Recovered by disassembling `Marshal::ReadObject` in `blue.dll` and the
//! initializer of its type-handler table, then fingerprinting each handler by
//! the Python C-API it calls and the number of bytes it checks for. Method and
//! evidence: `docs/archive/wire-format.md`; tooling: `tools/re/`.
//!
//! The dispatch, read off the instructions:
//!
//! ```text
//! movsx ecx, byte [stream]   ; the tag byte
//! and   ecx, 0x3f            ; type index  = tag & TYPE_MASK
//! shr   eax, 6 / and al, 1   ; shared flag = tag & SHARED_FLAG
//! movups xmm0, [table + idx*16]
//! movq  r9, xmm0 ; test r9,r9 ; je <"Invalid type tag %d in stream.">
//! call  r9                   ; handler(ctx, stream, shared_flag)
//! ```
//!
//! A tag byte therefore carries a 6-bit type code plus one flag bit; bit 7 is
//! discarded. A type code whose handler slot is NULL is rejected by the client.

/// The type code occupies the low 6 bits of a tag byte.
pub const TYPE_MASK: u8 = 0x3F;
/// Bit 6 marks an object that enters the stream's shared/back-reference map.
pub const SHARED_FLAG: u8 = 0x40;

/// Split a tag byte into `(type_code, shared)` exactly as the client does.
pub fn split(tag: u8) -> (u8, bool) {
    (tag & TYPE_MASK, tag & SHARED_FLAG != 0)
}

/// Type codes whose handler slot is explicitly NULL in this build. The client
/// raises "Invalid type tag %d in stream." for these.
pub const INVALID_TYPE_CODES: [u8; 6] = [0x00, 0x0C, 0x18, 0x1D, 0x1E, 0x2D];

/// Highest type code with a handler.
///
/// **A ceiling on handlers, not on the format.** CCP's own header says
/// *"type id's are from 1 to 63"* and caps the enum's last entry at
/// `TY_LONG = 47` = `0x2F`, which is this value -- so we are exactly at the
/// top of what exists today, with 16 unused codes below the shared-flag bit
/// (§50). A future build may use one; nothing here assumes it will not.
pub const MAX_TYPE_CODE: u8 = 0x2F;

/// Does this build accept this type code?
pub fn is_valid_type_code(code: u8) -> bool {
    code <= MAX_TYPE_CODE && !INVALID_TYPE_CODES.contains(&code)
}

/// Does a shared-flagged object of this type claim a shared-object id?
///
/// **The flag alone is not enough.** Only these fourteen handlers call
/// `RegisterShared`; for every other type the `0x40` bit is simply ignored,
/// so a shared-flagged int32 consumes no table entry (`docs/archive/marshal-tags.md`,
/// "The shared-object mechanism"). Getting this wrong desynchronises the
/// whole table, because the k-th *registering* object claims id `table[k]` --
/// an off-by-one here mis-assigns every later id rather than just one.
///
/// Note that `0x19`, `0x1A`, `0x1B` and `0x1C` are excluded: a back-reference
/// does not itself become shareable.
pub fn consumes_shared_slot(type_code: u8) -> bool {
    use self::code as c;
    matches!(
        type_code,
        c::GLOBAL
            | c::STRING
            | c::STRING_ALT
            | c::TUPLE
            | c::LIST
            | c::DICT
            | c::INSTANCE
            | c::PICKLE
            | c::OBJECT_22
            | c::OBJECT_23
            | c::LONG
            | c::TUPLE1
            | c::TUPLE2
            | c::LIST1
    )
}

/// Type codes, named from their handlers' own code. Each is backed by a
/// Python C-API call, a referenced singleton, a read width, or the
/// MarshalStream constructor — see the evidence string in [`HANDLERS`] and
/// `docs/archive/marshal-tags.md`.
pub mod code {
    // --- singletons and cached constants (zero payload bytes) ---
    /// References `_Py_NoneStruct`.
    pub const NONE: u8 = 0x01;
    /// Cached `PyInt_FromLong(-1)` from the MarshalStream object.
    pub const INT_MINUS_ONE: u8 = 0x07;
    /// Cached `PyInt_FromLong(0)`.
    pub const INT_ZERO: u8 = 0x08;
    /// Cached `PyInt_FromLong(1)`.
    pub const INT_ONE: u8 = 0x09;
    /// Cached `PyFloat_FromDouble(0.0)`.
    pub const REAL_ZERO: u8 = 0x0B;
    /// Cached `PyString_FromString("")`.
    pub const STRING_EMPTY: u8 = 0x0E;
    /// References `_Py_TrueStruct`.
    pub const TRUE: u8 = 0x1F;
    /// References `_Py_ZeroStruct` (Python 2's `Py_False`).
    pub const FALSE: u8 = 0x20;
    /// `PyTuple_New(0)`, no payload.
    pub const TUPLE_EMPTY: u8 = 0x24;
    /// `PyList_New(0)`, no payload.
    pub const LIST_EMPTY: u8 = 0x26;
    /// `PyUnicodeUCS2_FromString("")`, no payload.
    pub const UNICODE_EMPTY: u8 = 0x28;

    // --- fixed-width scalars ---
    /// 8 bytes, `PyLong_FromLongLong`.
    pub const INT64: u8 = 0x03;
    /// 4 bytes, `PyInt_FromLong`.
    pub const INT32: u8 = 0x04;
    /// 2 bytes, `PyInt_FromLong`.
    pub const INT16: u8 = 0x05;
    /// 1 byte, `PyInt_FromLong`.
    pub const INT8: u8 = 0x06;
    /// 8 bytes, `PyFloat_FromDouble`.
    pub const REAL: u8 = 0x0A;

    // --- strings ---
    /// Exactly ONE raw byte, the character. **No length field.**
    pub const STRING_CHAR: u8 = 0x0F;
    /// `u8` length with **no `0xFF` escape**, then that many bytes; so this
    /// form cannot carry 256 bytes or more.
    pub const STRING_SHORT: u8 = 0x10;
    /// Escaped length, then bytes; interned when `len <= 30`. The general form.
    pub const STRING: u8 = 0x0D;
    /// A second spelling decoded by the *same handler* as [`STRING`]; what
    /// makes the encoder choose between them is unknown.
    ///
    /// **CCP calls it `TY_BUFFER`** (§50). That answers the "unknown": the
    /// encoder picks it for a `buffer`, not for a `str`, and the reader
    /// happens to treat the two identically. The name is the fact; the
    /// behaviour we measured is unchanged.
    pub const STRING_ALT: u8 = 0x13;
    /// `u8` index, 1-based, into `blue.marshal.stringTableRev`.
    pub const STRING_TABLE_IDX: u8 = 0x11;

    // --- unicode ---
    /// Escaped count of **UTF-16 code units**, then `2 * n` bytes, LE.
    pub const UNICODE_UTF16: u8 = 0x12;
    /// Exactly TWO bytes: one UTF-16 code unit. **No length field.**
    pub const UNICODE_CHAR: u8 = 0x29;
    /// Escaped **byte** count, then that many bytes of UTF-8.
    ///
    /// **CCP calls it `TY_UTF8_OBSOLETE`**, commented as unused since the
    /// Python 3 upgrade (§50). Still decoded here, because what the client
    /// accepts is what matters and this costs nothing.
    pub const UNICODE_UTF8: u8 = 0x2E;

    // --- containers ---
    /// Escaped count, then that many objects.
    pub const TUPLE: u8 = 0x14;
    /// Escaped count, then that many objects.
    pub const LIST: u8 = 0x15;
    /// Escaped **pair** count, then `n` x (value, then key).
    pub const DICT: u8 = 0x16;
    /// One object, no count field.
    pub const TUPLE1: u8 = 0x25;
    /// Two objects, no count field.
    pub const TUPLE2: u8 = 0x2C;
    /// One object, no count field.
    pub const LIST1: u8 = 0x27;

    // --- references, wrappers, other ---
    /// Escaped 1-based index into the shared-object array.
    pub const SHARED_REF: u8 = 0x1B;
    /// `u32` checksum then one object; checked unless `skipCrcCheck`.
    pub const CRC_OBJECT: u8 = 0x1C;
    /// One object, handed to the stream's Load callback.
    pub const CALLBACK: u8 = 0x19;
    /// Raw, self-delimiting cPickle data.
    pub const PICKLE: u8 = 0x21;
    /// Escaped length (read and discarded), then raw cPickle data.
    pub const PICKLE_SIZED: u8 = 0x1A;
    /// Escaped length, then a global's name; resolved against a whitelist.
    pub const GLOBAL: u8 = 0x02;
    /// Escaped length, then LE two's-complement bytes
    /// (`_PyLong_FromByteArray`, signed).
    pub const LONG: u8 = 0x2F;
    /// Old-style instance: a class-name string, then the state object.
    ///
    /// The name is `"<module>.<ClassName>"`, built by the writer at
    /// `0x1802e0142` with `PyString_FromFormat("%s.%s", ...)` from the class
    /// dict's `__module__` and the class's `cl_name`, then tagged `0x17` at
    /// `0x1802e016c`. The reader resolves it against `globalsWhitelist`.
    /// **No terminator and no append streams** — unlike `0x22`/`0x23`.
    pub const INSTANCE: u8 = 0x17;
    /// Object, 2.2 form: header tuple `(callable, args)` or
    /// `(callable, args, state)`, then a list-append stream, then a
    /// `__setitem__` stream. Built as `callable(*args)`.
    ///
    /// **CCP calls it `TY_REDUCE`** (§50) -- it is `__reduce__`'s output,
    /// which is exactly the `(callable, args, state)` shape we measured.
    pub const OBJECT_22: u8 = 0x22;
    /// Object, 2.3 form: header tuple `((cls, *args),)` or
    /// `((cls, *args), state)`, then the same two streams. Built as
    /// `cls.__new__(cls, *args)`.
    ///
    /// **CCP calls it `TY_NEWOBJ`** (§50), which names the `__new__` call
    /// this form exists for.
    pub const OBJECT_23: u8 = 0x23;
    /// `blue.DBRow`: a descriptor object, an escaped-length nibble-RLE blob,
    /// then one object per object-typed column. Reader `0x1802fe410`, which
    /// type-checks the descriptor and takes the unpacked length from it
    /// rather than from the stream. See [`crate::dbrow`].
    pub const DBROW: u8 = 0x2A;
    /// A length-prefixed opaque byte blob: escaped length, then that many raw
    /// bytes, copied verbatim into a lazily-unmarshalled object
    /// (`0x1802db760` -> `0x1802da9a0`, which just allocates and memcpys).
    /// Named "SubStream" by convention; the shape is confirmed, the name is
    /// inferred.
    pub const SUBSTREAM: u8 = 0x2B;
    /// Terminator for each of the two streams `0x22`/`0x23` carry.
    ///
    /// **Not a type code** — it never appears in the handler table, which is
    /// why slot `0x2D` is NULL and why [`INVALID_TYPE_CODES`] lists it. It is
    /// peeked at `0x1802de701` and `0x1802de879` and consumed on the spot.
    /// Both streams are always terminated, even when empty, so every
    /// `0x22`/`0x23` carries exactly two of these bytes.
    ///
    /// **CCP calls it `TY_MARK`**, described as a marker for dynamic lists
    /// and other dynamic forms (§50) -- so it is a general terminator and
    /// not specific to the object-ex forms, which is why dispatching it as a
    /// value is invalid.
    pub const OBJECT_EX_END: u8 = 0x2D;
}

/// Type codes that consume an entry from the header's shared-object ID table
/// when their tag carries [`SHARED_FLAG`]. Every other code ignores the flag
/// entirely — a shared-flagged `int32` does **not** consume an entry, which a
/// decoder must respect or the ID numbering drifts.
pub const REGISTERS_SHARED: [u8; 14] = [
    code::GLOBAL,
    code::STRING,
    code::STRING_ALT,
    code::TUPLE,
    code::LIST,
    code::DICT,
    code::INSTANCE,
    code::PICKLE,
    code::OBJECT_22,
    code::OBJECT_23,
    code::LONG,
    code::TUPLE1,
    code::TUPLE2,
    code::LIST1,
];

/// Does a tag of this type consume a shared-object ID when flagged shared?
pub fn registers_shared(code: u8) -> bool {
    REGISTERS_SHARED.contains(&code)
}

/// Every valid type code: its handler address in `blue.dll` and the evidence
/// that identifies it. Note `0x0D` and `0x13` share one handler — the table
/// has exactly one duplicate.
pub const HANDLERS: [(u8, Option<u64>, &str); 42] = [
    (0x01, Some(0x1802dcd60), "None: references _Py_NoneStruct"),
    (
        0x02,
        Some(0x1802ddd60),
        "global by name: escaped len + name, whitelisted",
    ),
    (
        0x03,
        Some(0x1802dccf0),
        "int64: reads 8, PyLong_FromLongLong",
    ),
    (0x04, Some(0x1802dcc80), "int32: reads 4, PyInt_FromLong"),
    (0x05, Some(0x1802dcc10), "int16: reads 2, PyInt_FromLong"),
    (0x06, Some(0x1802dcbb0), "int8: reads 1, PyInt_FromLong"),
    (
        0x07,
        Some(0x1802dcba0),
        "int -1: cached PyInt_FromLong(-1) at ctx+0x38",
    ),
    (0x08, Some(0x1802dcb90), "int 0: cached at ctx+0x40"),
    (0x09, Some(0x1802dcb80), "int 1: cached at ctx+0x48"),
    (0x0A, Some(0x1802dcb10), "f64: reads 8, PyFloat_FromDouble"),
    (
        0x0B,
        Some(0x1802dcb00),
        "f64 0.0: cached PyFloat_FromDouble(0.0) at ctx+0x50",
    ),
    (
        0x0D,
        Some(0x1802dc5a0),
        "string: escaped len, interned when <= 30, registers shared",
    ),
    (
        0x0E,
        Some(0x1802dcab0),
        "empty string: cached PyString_FromString(\"\") at ctx+0x58",
    ),
    (
        0x0F,
        Some(0x1802dca40),
        "1-char string: exactly 1 raw byte, no length field",
    ),
    (
        0x10,
        Some(0x1802dc9b0),
        "short string: u8 length with no 0xFF escape",
    ),
    (
        0x11,
        Some(0x1802dc910),
        "string table: u8 1-based index into stringTableRev",
    ),
    (
        0x12,
        Some(0x1802dc7e0),
        "unicode: escaped count of UTF-16 code units, 2n bytes LE",
    ),
    (
        0x13,
        Some(0x1802dc5a0),
        "string: SAME handler as 0x0D (the table's one duplicate)",
    ),
    (
        0x14,
        Some(0x1802dd610),
        "tuple: escaped count, then n objects",
    ),
    (
        0x15,
        Some(0x1802dd3b0),
        "list: escaped count, then n objects",
    ),
    (
        0x16,
        Some(0x1802dd1d0),
        "dict: escaped pair count, then n x (value, key)",
    ),
    (
        0x17,
        Some(0x1802dde40),
        "old-style instance; reserves shared slot first; layout open",
    ),
    (
        0x19,
        Some(0x1802dd0d0),
        "callback: one object passed to the stream's Load callback",
    ),
    (
        0x1A,
        Some(0x1802dd010),
        "cPickle, escaped length read and discarded",
    ),
    (
        0x1B,
        Some(0x1802dce60),
        "shared back-reference: escaped 1-based index",
    ),
    (
        0x1C,
        Some(0x1802dcda0),
        "CRC-checked object: u32 checksum then one object",
    ),
    (0x1F, Some(0x1802dcae0), "True: references _Py_TrueStruct"),
    (0x20, Some(0x1802dcac0), "False: references _Py_ZeroStruct"),
    (
        0x21,
        Some(0x1802dcf60),
        "cPickle, self-delimiting, no length",
    ),
    (
        0x22,
        Some(0x1802de1c0),
        "object 2.2 form; reserves shared slot first; layout open",
    ),
    (
        0x23,
        Some(0x1802de430),
        "object 2.3 form; reserves shared slot first; layout open",
    ),
    (
        0x24,
        Some(0x1802dc590),
        "empty tuple: xor ecx,ecx; jmp PyTuple_New",
    ),
    (
        0x25,
        Some(0x1802dd8b0),
        "1-tuple: one object, no count field (inferred)",
    ),
    (
        0x26,
        Some(0x1802dd600),
        "empty list: xor ecx,ecx; jmp PyList_New",
    ),
    (
        0x27,
        Some(0x1802dd550),
        "1-list: one object, no count field (inferred)",
    ),
    (
        0x28,
        Some(0x1802dc7d0),
        "empty unicode: jmp PyUnicodeUCS2_FromString on a zero byte",
    ),
    (
        0x29,
        Some(0x1802dc740),
        "1-char unicode: exactly 2 bytes, no length field",
    ),
    (
        0x2A,
        Some(0x1802dcd90),
        "DBRow: 5-byte tail jump to the packed-row reader 0x1802fe410",
    ),
    (
        0x2B,
        Some(0x1802dcd80),
        "SubStream: 5-byte tail jump to 0x1802db760, escaped len + raw bytes",
    ),
    (
        0x2C,
        Some(0x1802dd7d0),
        "2-tuple: two objects, no count field (inferred)",
    ),
    (
        0x2E,
        Some(0x1802dc620),
        "unicode: escaped byte count, PyUnicodeUCS2_DecodeUTF8",
    ),
    (
        0x2F,
        Some(0x1802de9a0),
        "long: escaped len, _PyLong_FromByteArray LE signed",
    ),
];

/// Codes whose payload layout this crate can encode and decode.
pub const IMPLEMENTED: [u8; 32] = [
    code::GLOBAL,
    code::DBROW,
    code::SUBSTREAM,
    code::NONE,
    code::INT_MINUS_ONE,
    code::INT_ZERO,
    code::INT_ONE,
    code::REAL_ZERO,
    code::STRING_EMPTY,
    code::TRUE,
    code::FALSE,
    code::TUPLE_EMPTY,
    code::LIST_EMPTY,
    code::UNICODE_EMPTY,
    code::INT64,
    code::INT32,
    code::INT16,
    code::INT8,
    code::REAL,
    code::STRING_CHAR,
    code::STRING_SHORT,
    code::STRING,
    code::STRING_ALT,
    code::UNICODE_UTF16,
    code::UNICODE_CHAR,
    code::UNICODE_UTF8,
    code::TUPLE,
    code::LIST,
    code::DICT,
    code::INSTANCE,
    code::OBJECT_22,
    code::OBJECT_23,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_splits_into_type_and_shared_flag() {
        assert_eq!(split(0x01), (0x01, false));
        assert_eq!(split(0x41), (0x01, true));
        assert_eq!(split(0x14), (0x14, false));
        assert_eq!(split(0x54), (0x14, true));
        // bit 7 is discarded by the client's `and ecx, 0x3f`
        assert_eq!(split(0x81), (0x01, false));
        assert_eq!(split(0xC1), (0x01, true));
    }

    #[test]
    fn invalid_type_codes_are_rejected() {
        for c in INVALID_TYPE_CODES {
            assert!(!is_valid_type_code(c), "0x{c:02x} must be invalid");
        }
        assert!(!is_valid_type_code(0x30));
        assert!(!is_valid_type_code(0x3F));
    }

    #[test]
    fn implemented_codes_are_valid_and_distinct() {
        let mut seen = Vec::new();
        for c in IMPLEMENTED {
            assert!(is_valid_type_code(c), "0x{c:02x} implemented but invalid");
            assert!(!seen.contains(&c), "0x{c:02x} listed twice");
            seen.push(c);
        }
    }

    #[test]
    fn string_and_string_alt_share_one_handler() {
        // The table's only duplicate; 0x13 is indistinguishable from 0x0D at
        // decode time.
        let a = HANDLERS
            .iter()
            .find(|&&(c, _, _)| c == code::STRING)
            .unwrap()
            .1;
        let b = HANDLERS
            .iter()
            .find(|&&(c, _, _)| c == code::STRING_ALT)
            .unwrap()
            .1;
        assert_eq!(a, b);
        assert_eq!(a, Some(0x1802dc5a0));
    }

    #[test]
    fn only_fourteen_codes_register_shared_objects() {
        assert_eq!(REGISTERS_SHARED.len(), 14);
        // 0x25, 0x27 and 0x2C each call RegisterShared (0x1802dbdf0) when flagged
        // [V disassembly of 0x1802dd8b0, 0x1802dd550, 0x1802dd7d0]
        assert!(registers_shared(code::TUPLE1));
        assert!(registers_shared(code::TUPLE2));
        assert!(registers_shared(code::LIST1));
        assert!(registers_shared(code::TUPLE));
        assert!(registers_shared(code::DICT));
        // scalars ignore the shared flag entirely
        assert!(!registers_shared(code::INT32));
        assert!(!registers_shared(code::NONE));
        assert!(!registers_shared(code::SHARED_REF));
        for c in REGISTERS_SHARED {
            assert!(is_valid_type_code(c), "0x{c:02x}");
        }
    }

    #[test]
    fn handler_table_covers_exactly_the_valid_codes() {
        assert_eq!(HANDLERS.len(), 42);
        for (c, _, why) in HANDLERS {
            assert!(is_valid_type_code(c), "0x{c:02x} tabulated but invalid");
            assert!(!why.is_empty(), "0x{c:02x} has no evidence note");
        }
        let valid: Vec<u8> = (0..=MAX_TYPE_CODE)
            .filter(|&c| is_valid_type_code(c))
            .collect();
        assert_eq!(valid.len(), HANDLERS.len());
        for c in valid {
            assert!(
                HANDLERS.iter().any(|&(h, _, _)| h == c),
                "0x{c:02x} missing"
            );
        }
    }

    #[test]
    fn every_implemented_code_has_a_handler_address() {
        for c in IMPLEMENTED {
            let (_, addr, _) = HANDLERS.iter().find(|&&(h, _, _)| h == c).unwrap();
            assert!(addr.is_some(), "0x{c:02x} named without a handler address");
        }
    }
}
