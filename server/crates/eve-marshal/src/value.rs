//! The marshal object layer: a [`PyValue`] tree, and load/save over it.
//!
//! Built on the tag table recovered in [`crate::tag`]. The guiding rule is
//! that **nothing is guessed**: a tag whose payload layout was not established
//! from the client returns [`WireError::UnsupportedTag`] carrying the tag and
//! the offset, rather than being decoded on a canonical assumption that might
//! be wrong for this build.
//!
//! That makes this as much an instrument as a codec. [`Decoder::trace`]
//! records every `(offset, tag)` it saw (opt in with [`Decoder::with_trace`]),
//! so feeding it a real captured packet
//! produces both a list of which tags actually occur on the wire and the exact
//! point where decoding stopped. Closing the remaining gaps is then a matter
//! of reading the trace, not of searching.
//!
//! Layout facts used here, all confirmed (see `docs/archive/wire-format.md`):
//!
//! - A tag byte is `type = tag & 0x3F`, `shared = tag & 0x40`; bit 7 discarded.
//! - Counts and string lengths use the one-byte / `0xFF`-escape-to-`u32`
//!   convention ([`Reader::size`]) — corroborated by every container and
//!   string handler reading 1 then 4 bytes.
//! - Integer tags carry exactly 1, 2, 4 or 8 little-endian bytes; the real tag
//!   carries 8.

use crate::dbrow::{self, DbRow, DbRowDescriptor};
use crate::tag;
use crate::{Reader, WireError, Writer};

/// An unmarshalled Python value.
///
/// Only variants whose wire layout is established appear here. Dict entries
/// are kept in **wire order** as an opaque pair, because whether the client
/// writes value-then-key or key-then-value is not yet confirmed — see
/// [`PyValue::Dict`].
#[derive(Debug, Clone, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    /// Any of the four integer widths; the decoder does not preserve which tag
    /// carried it, and the encoder picks the narrowest that fits.
    Int(i64),
    /// An integer the client must see as a Python `long` (FILETIMEs and other
    /// 64-bit fields): always encoded with the arbitrary-precision tag `0x2F` (8 bytes) whatever its
    /// magnitude, so a small value cannot arrive as an `int` and make
    /// `isinstance(value, long)` fail. Encode-only: the decoder yields
    /// [`PyValue::Int`] for every integer tag.
    Long(i64),
    Real(f64),
    /// Python 2 `str`: bytes, not text.
    Bytes(Vec<u8>),
    /// Python 2 `unicode`, already decoded to UTF-8.
    Unicode(String),
    Tuple(Vec<PyValue>),
    List(Vec<PyValue>),
    /// Entries in wire order, each pair being `(value, key)` — the dict
    /// handler reads the value first and the key second (confirmed from
    /// `PyDict_SetItem(dict, key=second, value=first)` at `0x1802dd2f7`).
    /// [`PyValue::dict_as_kv`] presents them the other way round.
    Dict(Vec<(PyValue, PyValue)>),
    /// Tag `0x19`: one object handed to the stream's **Load callback**.
    ///
    /// The writer reaches this at exactly one site — the `0x17` branch. When
    /// the Save callback returns non-`None` for an old-style instance, the
    /// writer emits `0x19` followed by *the returned object* instead of the
    /// instance (`docs/archive/marshal-tags.md` Gap 9). It registers no shared slot.
    ///
    /// On the client this is how a **bound-object reference** arrives.
    /// `ObjectCall.GetObjectID` returns
    /// `blue.marshal.Save((objectID, refID))` — a byte string — and
    /// `ObjectCall.ParseObjectID` loads that nested stream back and builds a
    /// `MachoObjectConnection` from it. So the child here is normally
    /// [`PyValue::Bytes`] holding a whole second marshal stream, which is why
    /// this carries an arbitrary child rather than a parsed reference:
    /// the tag means "ask the callback", and what the callback expects is the
    /// caller's business. `eve_proto::object_reference` builds the bound-object
    /// form.
    ///
    /// A stream containing this tag is only loadable by a reader that was
    /// given a callback; the client raises "Unmarshal stream contains custom
    /// data but I have no callback method" otherwise. The client passes
    /// `ParseObjectID` when loading a `CallRsp` payload, so a reference is
    /// only legal there.
    Callback(Box<PyValue>),
    /// Tag `0x17`: an old-style Python instance.
    ///
    /// This is what every `MachoPacket` is. `class MachoPacket():` compiles
    /// with `BUILD_TUPLE 0` — an empty bases tuple — so in Python 2.7 it and
    /// all its subclasses are old-style, and `WriteObject` takes the
    /// `PyInstance_Type` branch.
    Instance {
        /// `"<module>.<ClassName>"`, e.g.
        /// `carbon.common.script.net.machoNetPacket.CallRsp`. The writer
        /// builds it with `PyString_FromFormat("%s.%s", ...)` at
        /// `0x1802e0142` from the class dict's `__module__` and the class's
        /// `cl_name`. **It must appear in `globalsWhitelist`** or the reader
        /// raises `HACKER WARNING! object %s.%s is not in whitelist`.
        ///
        /// `__module__` can be overridden in a class body, so the name is not
        /// always derivable from the source file's path. Take names from the
        /// whitelist text — reproduced in `docs/reference/object-wrapper.md`.
        class: Vec<u8>,
        /// Applied as `__setstate__(state)` if the **class** defines it, else
        /// `instance.__dict__.update(state)`. `__init__` is never called:
        /// the reader uses `PyInstance_NewRaw`.
        state: Box<PyValue>,
    },
    /// Tags `0x22` and `0x23`: the two `__reduce_ex__(2)` forms.
    ObjectEx {
        form: ObjectForm,
        /// `(callable, args)` or `(callable, args, state)` for
        /// [`ObjectForm::Call`]; `((cls, *args),)` or `((cls, *args), state)`
        /// for [`ObjectForm::New`].
        header: Box<PyValue>,
        /// Items appended with `append`, terminated by
        /// [`tag::code::OBJECT_EX_END`].
        list: Vec<PyValue>,
        /// Items set with `__setitem__`, as **`(key, value)`** — the opposite
        /// order to [`PyValue::Dict`], and the easiest bug in the format to
        /// ship. Confirmed from
        /// `PyObject_CallMethodObjArgs(obj, "__setitem__", first, second)`.
        dict: Vec<(PyValue, PyValue)>,
    },
    /// Tag `0x02`: a global, named by a dotted `module.Name` string.
    ///
    /// The handler reads an **escaped length then raw name bytes** — not a
    /// nested marshal object (confirmed: `0x1802ddd60` calls the string
    /// reader `0x1802ddc00`, which is the same `u8` / `0xFF + u32 LE`
    /// convention as every other length in this format). It then imports the
    /// module, `getattr`s the name, and checks the result against
    /// `blue.marshal.globalsWhitelist`, raising
    /// `HACKER WARNING! object %s.%s is not in whitelist` on a miss.
    ///
    /// This is how a type object reaches the wire — `WriteObject` writes one
    /// as `"%s.%s"` of `__module__` and `__name__` (`0x1802dfc70`) — and so
    /// how the callable of an [`PyValue::ObjectEx`] header is encoded.
    Global(Vec<u8>),
    /// Tag `0x2A`: a `blue.DBRow`. See [`crate::dbrow`].
    DbRow(Box<DbRow>),
    /// Tag `0x2B`: an escaped length followed by that many opaque bytes.
    ///
    /// The handler allocates a buffer and memcpys the bytes into a
    /// lazily-unmarshalled object (`0x1802db760` -> `0x1802da9a0`);
    /// **confirmed** that nothing else is read and that the payload is never
    /// parsed at read time. That the bytes are themselves a complete marshal
    /// stream (header + one object) is **inferred** from how the client's own
    /// `CallReq.payload` substream is used, not from this handler.
    SubStream(Vec<u8>),
}

/// Which `__reduce_ex__(2)` form an [`PyValue::ObjectEx`] uses.
///
/// `WriteObject` picks between them at `0x1802e02a0` by testing whether the
/// reduce callable's `__name__` is `"__newobj__"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectForm {
    /// Tag `0x22`: reconstructed as `header[0](*header[1])`.
    Call,
    /// Tag `0x23`: reconstructed as `header[0][0].__new__(*header[0])`, so
    /// the inner tuple still has the class as element 0.
    New,
}

impl PyValue {
    /// Interpret dict entries as `(key, value)` under the canonical
    /// value-then-key wire order.
    ///
    /// The order is **confirmed**: the dict handler at `0x1802dd1d0` does
    /// `A = ReadObject(); B = ReadObject(); PyDict_SetItem(dict, B, A)`, so
    /// the first object of each pair is the value and the second is the key.
    pub fn dict_as_kv(&self) -> Option<Vec<(&PyValue, &PyValue)>> {
        match self {
            PyValue::Dict(entries) => Some(entries.iter().map(|(v, k)| (k, v)).collect()),
            _ => None,
        }
    }
}

/// A ceiling on what one decode may build, for input from an untrusted peer.
///
/// Every [`PyValue`] the decoder creates is charged one node and an estimate
/// of its bytes ([`size_of::<PyValue>`] plus its own heap payload). A
/// `0x1B` back-reference deep-clones its target, so it is charged the
/// target's whole cost again, and so is the copy the shared table keeps of
/// a registering object. Without that, one shared list and a list of
/// references to it build a tree many times the size of the stream
/// (`docs/review/V2-wire.md` F2). Running out is
/// `WireError::Malformed { why: "decode budget" }`.
///
/// The budget is **opt-in** ([`Decoder::with_budget`]). Only the server's
/// inbound path uses it; the bots and `eve-probe` decode the server's own
/// responses, which can be large, with the unbudgeted default
/// (`docs/review/plan-review.md` M4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    /// [`PyValue`]s created, clones included.
    pub nodes: usize,
    /// Estimated bytes those values hold.
    pub bytes: usize,
}

impl Budget {
    /// The server's budget for one client payload.
    ///
    /// [A] Two million nodes and 64 MiB. A client payload is at most
    /// `machoNet.dropPacketThreshold` = 5,000,000 bytes [V]
    /// (`eve-proto/src/frame.rs:33`); one that size made of realistic rows
    /// costs about half a million nodes and 25 MB here, which this test
    /// pins: `value::tests::a_real_client_blob_decodes_under_the_default_budget`.
    /// The ceiling keeps one connection's decode to tens of megabytes
    /// however the stream fans out. Falsifier: a real client's payload
    /// refused with `decode budget` in the server log.
    pub const INBOUND: Budget = Budget {
        nodes: 2_000_000,
        bytes: 64 << 20,
    };
}

/// The stack a thread needs to decode a stream at the full depth the client
/// allows (1000), in either build profile.
///
/// Measured (`docs/lanes/ip12.md`, V2 F8), worst nesting shape per level:
/// about 6.2 KB in a debug build and under 1 KB in release, so a debug build
/// overflows the 2 MiB default a spawned thread gets, and the overflow aborts
/// the whole process. A thread that decodes a peer's bytes is spawned with at
/// least this much. Pinned by `value::tests::depth_limit_fits_the_decode_stack`.
pub const DECODE_STACK: usize = 8 << 20;

/// Bytes one [`PyValue`] is charged on top of its own heap payload.
const NODE_BYTES: usize = std::mem::size_of::<PyValue>();

/// The heap bytes a value owns itself, not counting its children (which are
/// charged as nodes of their own).
fn own_bytes(v: &PyValue) -> usize {
    match v {
        PyValue::Bytes(b) | PyValue::Global(b) | PyValue::SubStream(b) => b.len(),
        PyValue::Unicode(s) => s.len(),
        PyValue::Instance { class, .. } => class.len(),
        PyValue::DbRow(row) => std::mem::size_of::<DbRow>() + row.descriptor().blob_size(),
        _ => 0,
    }
}

/// Reads a [`PyValue`] tree.
pub struct Decoder {
    /// `(offset, tag_byte)` for every tag read, in order. Diagnostic, and
    /// only filled after [`Decoder::with_trace`]: on the server's path it
    /// would cost 16 bytes per tag for nothing.
    pub trace: Vec<(usize, u8)>,
    tracing: bool,
    /// `None`: unbudgeted (the default).
    budget: Option<Budget>,
    /// What this decode has built so far, clones included. Counted whether
    /// or not there is a budget, because a shared slot's cost is read off it.
    spent: Budget,
    /// The cost of building each shared slot's value, charged again for
    /// every `0x1B` that clones it.
    shared_cost: Vec<Budget>,
    /// The height (levels, a leaf being 1) of the value in each shared slot.
    /// A `0x1B` clones its target in place, so the clone's height adds to the
    /// depth the reference sits at; without this a 1.8 KB stream built a value
    /// 1,800 deep and a few hundred KB one hundreds of thousands deep, past
    /// the client's limit of 1000 and past any stack (Q-FUZZ).
    shared_height: Vec<u32>,
    /// The tallest child of the value being built, and the height of the
    /// value just finished: how `load` learns a value's height without every
    /// arm reporting it.
    child_max: u32,
    last_height: u32,
    /// Objects flagged `SHARED_FLAG`, in the order they were seen. The stream
    /// header carries a count of these ("mapcount"); how a later
    /// back-reference names one is **not yet established**, so this is
    /// collected for diagnosis and not yet used for resolution.
    /// Values registered as shared, indexed by **slot** (`id - 1`).
    ///
    /// Sized from the header's id table, and sparse: a slot stays `None`
    /// until the object that claims it has been built. A `0x1B` naming an
    /// empty slot is the client's
    /// `ValueError "Invalid TY_REFERENCE in stream"`.
    pub shared: Vec<Option<PyValue>>,
    /// The header's 1-based shared-object ids, in stream order. The k-th
    /// *registering* object claims `shared_ids[k]` — the table is ids, not
    /// offsets, and an id is not a position.
    shared_ids: Vec<u32>,
    /// How many registering objects we have seen, i.e. the next index into
    /// `shared_ids`.
    shared_seen: usize,
    depth: u32,
    max_depth: u32,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        // The client's own ReadObject aborts at 1000 ("Maximum recursion depth
        // of %d reached"); match it so we reject what it would reject.
        Decoder {
            trace: Vec::new(),
            tracing: false,
            budget: None,
            spent: Budget::default(),
            shared_cost: Vec::new(),
            shared_height: Vec::new(),
            child_max: 0,
            last_height: 0,
            shared: Vec::new(),
            shared_ids: Vec::new(),
            shared_seen: 0,
            depth: 0,
            max_depth: 1000,
        }
    }

    /// A decoder for a stream that carries shared objects.
    ///
    /// `ids` is the header's map table, as [`take_map_table`] returns it. A
    /// stream whose `mapcount` is zero needs only [`Decoder::new`]; one with
    /// shared objects **must** be given its table, or every `0x1B` in it is
    /// unresolvable and the slot accounting is meaningless.
    pub fn with_shared(ids: Vec<u32>) -> Self {
        // **Sized by `mapcount`, not by the largest id.** Both the client's
        // own `0x1B` path and its table capacity bound an id by `mapcount`
        // (`1 <= index <= mapcount`), so `ids.len()` is the right size and
        // any id outside that range is a malformed stream, rejected in
        // `claim_shared_slot`.
        //
        // Sizing from `max(id)` instead -- which is what this did first --
        // lets the stream choose the allocation: one bogus entry read as a
        // negative `i32` becomes a four-billion-element `u32`, and the
        // process is killed before it can reject anything. Pre-auth network
        // data decides these bytes, so the bound has to come from the format,
        // not from the data.
        let slots = ids.len();
        Decoder {
            shared: vec![None; slots],
            shared_cost: vec![Budget::default(); slots],
            shared_height: vec![0; slots],
            shared_ids: ids,
            ..Decoder::new()
        }
    }

    /// Refuse to build more than `budget` allows. See [`Budget`]; the
    /// server's inbound path passes [`Budget::INBOUND`].
    ///
    /// ```
    /// use eve_marshal::tag::code::{LIST, NONE};
    /// use eve_marshal::value::Budget;
    /// use eve_marshal::{Decoder, Reader, WireError};
    /// // A list of three Nones is four nodes.
    /// let bytes = [LIST, 3, NONE, NONE, NONE];
    /// let tight = Budget { nodes: 3, bytes: usize::MAX };
    /// let err = Decoder::new().with_budget(tight).load(&mut Reader::new(&bytes));
    /// assert!(matches!(err, Err(WireError::Malformed { why: "decode budget", .. })));
    /// ```
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Record every `(offset, tag)` in [`Decoder::trace`].
    pub fn with_trace(mut self) -> Self {
        self.tracing = true;
        self
    }

    /// What this decode has built so far, clones included.
    pub fn spent(&self) -> Budget {
        self.spent
    }

    /// Add `cost` to what has been spent, and fail if that passes the
    /// budget.
    fn charge(&mut self, cost: Budget, at: usize) -> Result<(), WireError> {
        self.spent.nodes = self.spent.nodes.saturating_add(cost.nodes);
        self.spent.bytes = self.spent.bytes.saturating_add(cost.bytes);
        match self.budget {
            Some(b) if self.spent.nodes > b.nodes || self.spent.bytes > b.bytes => {
                Err(WireError::Malformed {
                    at,
                    why: "decode budget",
                })
            }
            _ => Ok(()),
        }
    }

    /// Claim the next shared slot, as `RegisterShared` does.
    ///
    /// Mirrors the client's own overflow check: it raises
    /// `RuntimeError "Shared object table overflow"` when more objects
    /// register than the header promised.
    fn claim_shared_slot(&mut self, at: usize) -> Result<usize, WireError> {
        let id = *self
            .shared_ids
            .get(self.shared_seen)
            .ok_or(WireError::Malformed {
                at,
                why: "shared object table overflow",
            })?;
        self.shared_seen += 1;
        let slot = (id as usize).checked_sub(1).ok_or(WireError::Malformed {
            at,
            why: "shared object id is 0, but ids are 1-based",
        })?;
        // `1 <= id <= mapcount`, as the client requires.
        if slot >= self.shared.len() {
            return Err(WireError::Malformed {
                at,
                why: "shared object id is outside 1..=mapcount",
            });
        }
        Ok(slot)
    }

    /// Decode one value from `r`.
    pub fn load(&mut self, r: &mut Reader<'_>) -> Result<PyValue, WireError> {
        if self.depth >= self.max_depth {
            return Err(WireError::Malformed {
                at: r.pos(),
                why: "recursion limit",
            });
        }
        let at = r.pos();
        let raw = r.u8()?;
        if self.tracing {
            self.trace.push((at, raw));
        }
        let (code, shared) = tag::split(raw);

        if !tag::is_valid_type_code(code) {
            // Exactly what the client rejects with
            // "Invalid type tag %d in stream."
            return Err(WireError::UnknownTag { at, tag: raw });
        }

        // Reserve the slot *before* the body is read, so the k-th
        // registering object claims the k-th id even if its children
        // register too. Only fourteen handlers register at all: the flag is
        // ignored on everything else, and counting it would desynchronise
        // every later id.
        let slot = if shared && tag::consumes_shared_slot(code) {
            Some(self.claim_shared_slot(at)?)
        } else {
            None
        };
        let before = self.spent;
        let outer_max = std::mem::replace(&mut self.child_max, 0);
        let v = self.load_body(r, code, at)?;
        // A reference's arm set `last_height` to its target's; everything
        // else is one level above its tallest child.
        let height = if code == tag::code::SHARED_REF {
            self.last_height
        } else {
            1 + self.child_max
        };
        self.child_max = outer_max;
        self.last_height = height;
        // A back-reference was charged its target's whole cost in its arm,
        // before the clone; everything else is charged here, after its
        // children.
        if code != tag::code::SHARED_REF {
            self.charge(
                Budget {
                    nodes: 1,
                    bytes: NODE_BYTES + own_bytes(&v),
                },
                at,
            )?;
        }
        if let Some(slot) = slot {
            let cost = Budget {
                nodes: self.spent.nodes - before.nodes,
                bytes: self.spent.bytes - before.bytes,
            };
            // The table keeps its own copy, so that copy is charged too.
            self.charge(cost, at)?;
            self.shared_cost[slot] = cost;
            self.shared_height[slot] = height;
            self.shared[slot] = Some(v.clone());
        }
        Ok(v)
    }

    /// One value's body, its tag already read.
    ///
    /// A thin dispatch, with every arm in its own `#[inline(never)]`
    /// function. A 1000-deep stream keeps one frame of `load`, of this and of
    /// one arm per level, and in a debug build a frame holds a slot for every
    /// temporary of every arm it contains; keeping the arms apart keeps each
    /// level to its own arm's (V2 F8; the measurement is in
    /// `docs/lanes/ip12.md`).
    #[inline(always)]
    fn load_body(&mut self, r: &mut Reader<'_>, code: u8, at: usize) -> Result<PyValue, WireError> {
        use tag::code as c;
        match code {
            c::TUPLE1 | c::TUPLE2 | c::LIST1 | c::TUPLE | c::LIST | c::DICT | c::CALLBACK => {
                self.load_container(r, code, at)
            }
            c::INSTANCE | c::OBJECT_22 | c::OBJECT_23 => self.load_wrapper(r, code, at),
            c::DBROW => self.load_dbrow(r, at),
            _ => self.load_leaf(r, code, at),
        }
    }

    /// Tuples, lists, dicts and `0x19`.
    #[inline(never)]
    fn load_container(
        &mut self,
        r: &mut Reader<'_>,
        code: u8,
        at: usize,
    ) -> Result<PyValue, WireError> {
        use tag::code as c;
        match code {
            // --- containers ---
            c::TUPLE1 => Ok(PyValue::Tuple(vec![self.child(r)?])),
            c::TUPLE2 => Ok(PyValue::Tuple(vec![self.child(r)?, self.child(r)?])),
            c::LIST1 => Ok(PyValue::List(vec![self.child(r)?])),
            c::TUPLE => {
                let n = r.size()?;
                self.guard_count(n, r)?;
                let mut out = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    out.push(self.child(r)?);
                }
                Ok(PyValue::Tuple(out))
            }
            c::LIST => {
                let n = r.size()?;
                self.guard_count(n, r)?;
                let mut out = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    out.push(self.child(r)?);
                }
                Ok(PyValue::List(out))
            }
            // The count is a PAIR count, and each pair is value then key --
            // confirmed from PyDict_SetItem(dict, key=second, value=first) at
            // 0x1802dd2f7. Stored here in wire order.
            c::DICT => {
                let n = r.size()?;
                self.guard_count(n.saturating_mul(2), r)?;
                let mut out = Vec::with_capacity(n.min(512));
                for _ in 0..n {
                    let value = self.child(r)?;
                    let key = self.child(r)?;
                    out.push((value, key));
                }
                Ok(PyValue::Dict(out))
            }

            // --- custom data via the stream's Load callback ---
            //
            // One child object and nothing else; no shared registration.
            c::CALLBACK => Ok(PyValue::Callback(Box::new(self.child(r)?))),
            _ => Err(WireError::UnsupportedTag { at, tag: code }),
        }
    }

    /// The arms that never recurse: scalars, strings, back-references,
    /// globals and substreams.
    #[inline(never)]
    fn load_leaf(&mut self, r: &mut Reader<'_>, code: u8, at: usize) -> Result<PyValue, WireError> {
        use tag::code as c;
        match code {
            // --- singletons and cached constants: zero payload bytes ---
            c::NONE => Ok(PyValue::None),
            c::TRUE => Ok(PyValue::Bool(true)),
            c::FALSE => Ok(PyValue::Bool(false)),
            c::INT_MINUS_ONE => Ok(PyValue::Int(-1)),
            c::INT_ZERO => Ok(PyValue::Int(0)),
            c::INT_ONE => Ok(PyValue::Int(1)),
            c::REAL_ZERO => Ok(PyValue::Real(0.0)),
            c::STRING_EMPTY => Ok(PyValue::Bytes(Vec::new())),
            c::UNICODE_EMPTY => Ok(PyValue::Unicode(String::new())),
            c::TUPLE_EMPTY => Ok(PyValue::Tuple(Vec::new())),
            c::LIST_EMPTY => Ok(PyValue::List(Vec::new())),

            // --- fixed-width scalars ---
            c::INT8 => Ok(PyValue::Int(i64::from(r.u8()? as i8))),
            c::INT16 => Ok(PyValue::Int(i64::from(r.i16()?))),
            c::INT32 => Ok(PyValue::Int(i64::from(r.i32()?))),
            c::INT64 => Ok(PyValue::Int(r.i64()?)),
            c::REAL => Ok(PyValue::Real(r.f64()?)),

            // --- strings ---
            // Exactly one raw byte; there is no length field.
            c::STRING_CHAR => Ok(PyValue::Bytes(r.bytes(1)?.to_vec())),
            // A plain u8 length with no 0xFF escape.
            c::STRING_SHORT => {
                let n = usize::from(r.u8()?);
                Ok(PyValue::Bytes(r.bytes(n)?.to_vec()))
            }
            c::STRING | c::STRING_ALT => {
                let n = r.size()?;
                Ok(PyValue::Bytes(r.bytes(n)?.to_vec()))
            }
            // Arbitrary-precision int: escaped length, then that many bytes,
            // little-endian, **signed** (`_PyLong_FromByteArray(p, len, 1, 1)`).
            // Length 0 is the integer 0.
            //
            // The client uses it for Windows FILETIMEs, which is how it first
            // reached us: a `PingReq` carries its hop timings as `2F 08` plus
            // eight bytes. Those fit an `i64` exactly, and so does every value
            // we have seen — but the tag can carry a true bignum, so anything
            // that does not fit is refused rather than silently truncated.
            // Leading sign-extension bytes are accepted, because a 9-byte
            // encoding of a positive number is still an `i64`.
            c::LONG => {
                let at = r.pos();
                let n = r.size()?;
                let bytes = r.bytes(n)?;
                long_from_le_signed(bytes).ok_or(WireError::Malformed {
                    at,
                    why: "arbitrary-precision int does not fit in i64",
                })
            }

            // One byte: a 1-based index into the build's shared string table.
            // The client uses it for the strings it sends most often — the
            // first real client's very first service call carried its kwargs
            // key this way — so this is a routine tag, not an exotic one.
            //
            // An index outside the table is a hard error rather than a
            // placeholder string: the table is build-specific, so an index we
            // cannot resolve means we are talking to a build this table does
            // not describe, and inventing a value there would corrupt
            // everything downstream of it quietly.
            c::STRING_TABLE_IDX => {
                let at = r.pos();
                let idx = usize::from(r.u8()?);
                let s = idx
                    .checked_sub(1)
                    .and_then(|i| crate::string_table::STRING_TABLE.get(i))
                    .ok_or(WireError::Malformed {
                        at,
                        why: "string table index out of range for this build",
                    })?;
                Ok(PyValue::Bytes(s.as_bytes().to_vec()))
            }

            // --- unicode ---
            // One UTF-16 code unit, two bytes, no length field.
            c::UNICODE_CHAR => decode_utf16(r.bytes(2)?, at),
            // Count is in UTF-16 CODE UNITS, so 2*n bytes follow.
            c::UNICODE_UTF16 => {
                let units = r.size()?;
                let bytes = units.checked_mul(2).ok_or(WireError::Malformed {
                    at,
                    why: "utf-16 length overflow",
                })?;
                decode_utf16(r.bytes(bytes)?, at)
            }
            // Count is in BYTES.
            c::UNICODE_UTF8 => {
                let n = r.size()?;
                let b = r.bytes(n)?;
                match core::str::from_utf8(b) {
                    Ok(s) => Ok(PyValue::Unicode(s.to_owned())),
                    Err(_) => Err(WireError::Malformed {
                        at,
                        why: "invalid utf-8",
                    }),
                }
            }

            // --- shared back-reference: an escaped 1-based id ---
            //
            // `0x1B`, not `0x1C` -- `0x1C` is a CRC-checked object. The load
            // path requires `1 <= id <= mapcount` and a slot that has
            // actually been filled, and raises
            // `ValueError "Invalid TY_REFERENCE in stream"` otherwise.
            //
            // A slot that is still empty means the stream refers back to an
            // object whose own body is not finished -- a cycle. We refuse
            // that rather than invent a value: the client can build one
            // because it registers a mutable container before filling it,
            // and we return owned values, so this is a real limit and not a
            // decoding mistake to paper over.
            c::SHARED_REF => {
                let id = r.size()?;
                let slot = id.checked_sub(1).ok_or(WireError::Malformed {
                    at,
                    why: "shared object id is 0, but ids are 1-based",
                })?;
                // Charged before the clone, so a fan-out fails before it
                // allocates. A slot that is empty or out of range costs
                // nothing and is refused just below.
                let cost = self.shared_cost.get(slot).copied().unwrap_or_default();
                self.charge(cost, at)?;
                match self.shared.get(slot) {
                    Some(Some(v)) => {
                        let h = self.shared_height[slot];
                        // The clone sits at this depth: its levels count.
                        if self.depth as usize + h as usize > self.max_depth as usize {
                            return Err(WireError::Malformed {
                                at,
                                why: "recursion limit",
                            });
                        }
                        self.last_height = h;
                        Ok(v.clone())
                    }
                    Some(None) => Err(WireError::Malformed {
                        at,
                        why: "shared back-reference to an object still being built (a cycle)",
                    }),
                    None => Err(WireError::Malformed {
                        at,
                        why: "shared back-reference past the end of the table",
                    }),
                }
            }

            // --- global by name (escaped length, then raw bytes) ---
            c::GLOBAL => {
                let n = r.size()?;
                Ok(PyValue::Global(r.bytes(n)?.to_vec()))
            }

            // --- opaque substream (escaped length, then raw bytes) ---
            c::SUBSTREAM => {
                let n = r.size()?;
                Ok(PyValue::SubStream(r.bytes(n)?.to_vec()))
            }

            // Known type, layout not established. Deliberately not guessed.
            _ => Err(WireError::UnsupportedTag { at, tag: code }),
        }
    }

    /// `0x17`, `0x22` and `0x23`.
    #[inline(never)]
    fn load_wrapper(
        &mut self,
        r: &mut Reader<'_>,
        code: u8,
        at: usize,
    ) -> Result<PyValue, WireError> {
        use tag::code as c;
        match code {
            // --- object wrappers (docs/reference/object-wrapper.md) ---
            c::INSTANCE => {
                let name = self.child(r)?;
                let class = match name {
                    PyValue::Bytes(b) => b,
                    PyValue::Unicode(s) => s.into_bytes(),
                    _ => {
                        return Err(WireError::Malformed {
                            at,
                            why: "0x17 class name is not a string",
                        });
                    }
                };
                let state = self.child(r)?;
                Ok(PyValue::Instance {
                    class,
                    state: Box::new(state),
                })
            }
            c::OBJECT_22 | c::OBJECT_23 => {
                let form = if code == c::OBJECT_22 {
                    ObjectForm::Call
                } else {
                    ObjectForm::New
                };
                let header = self.child(r)?;
                let list = self.read_append_stream(r)?;
                let dict = self.read_setitem_stream(r)?;
                Ok(PyValue::ObjectEx {
                    form,
                    header: Box::new(header),
                    list,
                    dict,
                })
            }
            _ => Err(WireError::UnsupportedTag { at, tag: code }),
        }
    }

    /// `0x2A`, a packed row.
    #[inline(never)]
    fn load_dbrow(&mut self, r: &mut Reader<'_>, at: usize) -> Result<PyValue, WireError> {
        let descriptor_at = r.pos();
        let descriptor_value = self.child(r)?;
        let descriptor =
            DbRowDescriptor::from_pyvalue(&descriptor_value).map_err(|_| WireError::Malformed {
                at: descriptor_at,
                why: "0x2A descriptor is not a blue.DBRowDescriptor",
            })?;
        let blob_len = r.size()?;
        let blob = dbrow::rle_decode(r.bytes(blob_len)?, descriptor.blob_size()).map_err(|_| {
            WireError::Malformed {
                at,
                why: "invalid RLE string",
            }
        })?;
        let mut objects = Vec::with_capacity(descriptor.object_count());
        for _ in 0..descriptor.object_count() {
            objects.push(self.child(r)?);
        }
        let row = DbRow::unpack(descriptor, &blob, objects).map_err(|_| WireError::Malformed {
            at,
            why: "0x2A row does not match its descriptor",
        })?;
        Ok(PyValue::DbRow(Box::new(row)))
    }

    /// Objects until [`tag::code::OBJECT_EX_END`].
    ///
    /// The terminator is peeked and only consumed if it matches, because the
    /// handler at `0x1802de701` does exactly that: `cmp cl, 0x2d` on the raw
    /// byte, with **no shared-flag strip**, so a flagged `0x6D` would not
    /// terminate the stream.
    fn read_append_stream(&mut self, r: &mut Reader<'_>) -> Result<Vec<PyValue>, WireError> {
        let mut out = Vec::new();
        loop {
            if r.peek_u8()? == tag::code::OBJECT_EX_END {
                r.u8()?;
                return Ok(out);
            }
            out.push(self.child(r)?);
        }
    }

    /// `(key, value)` pairs until [`tag::code::OBJECT_EX_END`].
    fn read_setitem_stream(
        &mut self,
        r: &mut Reader<'_>,
    ) -> Result<Vec<(PyValue, PyValue)>, WireError> {
        let mut out = Vec::new();
        loop {
            if r.peek_u8()? == tag::code::OBJECT_EX_END {
                r.u8()?;
                return Ok(out);
            }
            let key = self.child(r)?;
            let value = self.child(r)?;
            out.push((key, value));
        }
    }

    fn child(&mut self, r: &mut Reader<'_>) -> Result<PyValue, WireError> {
        self.depth += 1;
        let v = self.load(r);
        self.depth -= 1;
        self.child_max = self.child_max.max(self.last_height);
        v
    }

    /// Refuse a count that cannot possibly be satisfied by the bytes left, so
    /// a hostile length cannot make us allocate.
    fn guard_count(&self, n: usize, r: &Reader<'_>) -> Result<(), WireError> {
        if n > r.remaining() {
            return Err(WireError::Malformed {
                at: r.pos(),
                why: "count exceeds remaining bytes",
            });
        }
        Ok(())
    }
}

/// Encode a [`PyValue`]. Only values this crate can also decode are
/// encodable, so a round trip is always meaningful.
pub fn save(v: &PyValue, w: &mut Writer) {
    use tag::code as c;
    match v {
        PyValue::None => w.u8(c::NONE),
        PyValue::Bool(true) => w.u8(c::TRUE),
        PyValue::Bool(false) => w.u8(c::FALSE),
        PyValue::Int(i) => match *i {
            // The cached-constant tags: one byte on the wire, no payload.
            -1 => w.u8(c::INT_MINUS_ONE),
            0 => w.u8(c::INT_ZERO),
            1 => w.u8(c::INT_ONE),
            n => {
                if let Ok(b) = i8::try_from(n) {
                    w.u8(c::INT8);
                    w.u8(b as u8);
                } else if let Ok(h) = i16::try_from(n) {
                    w.u8(c::INT16);
                    w.i16(h);
                } else if let Ok(m) = i32::try_from(n) {
                    w.u8(c::INT32);
                    w.i32(m);
                } else {
                    w.u8(c::INT64);
                    w.i64(n);
                }
            }
        },
        PyValue::Long(n) => {
            // Tag 0x2F (TY_LONG): escaped length 8, then eight little-endian
            // two's-complement bytes, the shape the client itself sends.
            w.u8(c::LONG);
            w.size(8);
            w.bytes(&n.to_le_bytes());
        }
        PyValue::Real(f) => {
            // to_bits, not == 0.0, so -0.0 keeps its sign rather than
            // collapsing into the cached +0.0 constant.
            if f.to_bits() == 0 {
                w.u8(c::REAL_ZERO);
            } else {
                w.u8(c::REAL);
                w.f64(*f);
            }
        }
        PyValue::Bytes(b) => save_bytes(b, w),
        PyValue::Unicode(s) => {
            if s.is_empty() {
                w.u8(c::UNICODE_EMPTY);
            } else {
                // UTF-8 form: the count is a byte count, which avoids having
                // to re-encode to UTF-16 and count code units.
                w.u8(c::UNICODE_UTF8);
                w.size(s.len());
                w.bytes(s.as_bytes());
            }
        }
        PyValue::Tuple(items) => match items.len() {
            0 => w.u8(c::TUPLE_EMPTY),
            1 => {
                w.u8(c::TUPLE1);
                save(&items[0], w);
            }
            2 => {
                w.u8(c::TUPLE2);
                save(&items[0], w);
                save(&items[1], w);
            }
            n => {
                w.u8(c::TUPLE);
                w.size(n);
                for i in items {
                    save(i, w);
                }
            }
        },
        PyValue::List(items) => match items.len() {
            0 => w.u8(c::LIST_EMPTY),
            1 => {
                w.u8(c::LIST1);
                save(&items[0], w);
            }
            n => {
                w.u8(c::LIST);
                w.size(n);
                for i in items {
                    save(i, w);
                }
            }
        },
        PyValue::Dict(entries) => {
            w.u8(c::DICT);
            w.size(entries.len());
            // Wire order is value then key.
            for (value, key) in entries {
                save(value, w);
                save(key, w);
            }
        }
        PyValue::Callback(child) => {
            // Tag then the object the Save callback returned, with no length
            // and no terminator -- the reader takes exactly one child.
            w.u8(c::CALLBACK);
            save(child, w);
        }
        PyValue::Instance { class, state } => {
            w.u8(c::INSTANCE);
            save_bytes(class, w);
            save(state, w);
            // No terminator: an exhaustive caller scan of the two stream
            // helpers (`0x1802de6b0`, `0x1802de800`) found them called only
            // from the 0x22 and 0x23 handlers, so 0x17 neither emits nor
            // consumes OBJECT_EX_END.
        }
        PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } => {
            w.u8(match form {
                ObjectForm::Call => c::OBJECT_22,
                ObjectForm::New => c::OBJECT_23,
            });
            save(header, w);
            for item in list {
                save(item, w);
            }
            // Both terminators are unconditional: the helpers are called on
            // every success path, so an empty stream is still one 0x2D byte.
            w.u8(c::OBJECT_EX_END);
            for (key, value) in dict {
                save(key, w);
                save(value, w);
            }
            w.u8(c::OBJECT_EX_END);
        }
        PyValue::Global(name) => {
            w.u8(c::GLOBAL);
            w.size(name.len());
            w.bytes(name);
        }
        PyValue::SubStream(bytes) => {
            w.u8(c::SUBSTREAM);
            w.size(bytes.len());
            w.bytes(bytes);
        }
        PyValue::DbRow(row) => {
            w.u8(c::DBROW);
            // The descriptor is one complete marshal object; the reader
            // type-checks it and reads the unpacked length out of it.
            save(&row.descriptor().to_pyvalue(), w);
            let blob = dbrow::rle_encode(&row.pack());
            w.size(blob.len());
            w.bytes(&blob);
            // One object per object-typed column, declaration order.
            for obj in row.objects() {
                save(&obj, w);
            }
        }
    }
}

/// Encode a Python 2 `str`, picking the narrowest tag that fits.
///
/// Factored out of [`save`] because the `0x17` wrapper writes a class-name
/// string that is not a [`PyValue`] of its own.
fn save_bytes(b: &[u8], w: &mut Writer) {
    use tag::code as c;
    match b.len() {
        0 => w.u8(c::STRING_EMPTY),
        1 => {
            w.u8(c::STRING_CHAR);
            w.u8(b[0]);
        }
        n if n <= 0xFF => {
            w.u8(c::STRING_SHORT);
            w.u8(n as u8);
            w.bytes(b);
        }
        n => {
            w.u8(c::STRING);
            w.size(n);
            w.bytes(b);
        }
    }
}

/// Decode little-endian UTF-16 bytes into a `PyValue::Unicode`.
fn decode_utf16(bytes: &[u8], at: usize) -> Result<PyValue, WireError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(WireError::Malformed {
            at,
            why: "odd utf-16 byte count",
        });
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    match char::decode_utf16(units).collect::<Result<String, _>>() {
        Ok(s) => Ok(PyValue::Unicode(s)),
        Err(_) => Err(WireError::Malformed {
            at,
            why: "invalid utf-16",
        }),
    }
}

/// The first byte of an uncompressed marshal stream, confirmed from the
/// client's read path (`machoNetTransport`: anything else is zlib).
pub const STREAM_MARKERS: [u8; 2] = [0x7D, 0x7E];

/// A marshal stream's header, as validated by `Marshal::Load` at
/// `0x1802e1670` in `blue.dll`.
///
/// Layout, read off the instructions:
///
/// ```text
/// u8  marker     0x7D or 0x7E, else "invalid marshal header"
/// u8  extra      ONLY when marker == 0x7D; for 0x7E the field is set to 0
///                without consuming a byte
/// i32 mapcount   little-endian; negative is rejected with
///                "invalid mapcount in marshal stream, %d."
/// ```
///
/// The stream must then hold at least `mapcount * 4` further bytes, or Load
/// fails with "Too little data in marshal stream ... mapcount in header is
/// %d". Those bytes are **1-based shared-object IDs**, not stream offsets:
/// `RegisterShared` consumes them in order as `slot = table[n] - 1`.
///
/// **There is no CRC in the header.** `Marshal::Load` is the header validator
/// and references no CRC string. There *is* a CRC in the protocol, but as type
/// code `0x1C` (a `u32` checksum plus one object), which is what
/// `blue.marshal.Load`'s `skipCrcCheck` gates. What the `0x7D` `extra` byte
/// means is still undetermined, which is why we only ever emit the `0x7E`
/// form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub marker: u8,
    /// The byte following a `0x7D` marker; `0` when the marker was `0x7E`.
    pub extra: u8,
    /// Number of shared objects; the offset table is `mapcount * 4` bytes.
    pub mapcount: i32,
}

impl Header {
    /// Bytes the shared-object offset table occupies.
    pub fn map_table_len(&self) -> usize {
        (self.mapcount as usize).saturating_mul(4)
    }
}

/// Parse and consume a stream header, applying every check `Marshal::Load`
/// applies, in the same order.
/// Decode tag `0x2F`'s body: `len` bytes, little-endian, two's complement.
///
/// `None` when the value genuinely does not fit an `i64`. Bytes beyond the
/// eighth are allowed as long as they are pure sign extension, so the client's
/// choice of width never matters — only the value does.
fn long_from_le_signed(bytes: &[u8]) -> Option<PyValue> {
    if bytes.is_empty() {
        return Some(PyValue::Int(0));
    }
    let negative = bytes[bytes.len() - 1] & 0x80 != 0;
    // Sign-extend into eight bytes, then check that anything above the eighth
    // byte was only ever that same extension.
    let fill = if negative { 0xFF } else { 0x00 };
    let mut buf = [fill; 8];
    for (i, &b) in bytes.iter().enumerate() {
        if i < 8 {
            buf[i] = b;
        } else if b != fill {
            return None;
        }
    }
    Some(PyValue::Int(i64::from_le_bytes(buf)))
}

pub fn take_header(r: &mut Reader<'_>) -> Result<Header, WireError> {
    let at = r.pos();
    let marker = r.u8()?;
    if !STREAM_MARKERS.contains(&marker) {
        return Err(WireError::Malformed {
            at,
            why: "invalid marshal header",
        });
    }
    // **`0x7D` is not an alternate magic, it is a *version* marker**, and the
    // difference is a four-byte one.
    //
    // `blue.marshal.Save(object, callback=None, useChecksum=0, version=0,
    // stringMap=None)` writes `0x7E` followed by a `uint32` shared-object
    // mapcount when `version == 0`, and `0x7D` followed by a **one-byte
    // version and no mapcount word** when it is anything else. The reader
    // mirrors it, so the three streams the client accepts are `0x7E ...`,
    // `0x7D 00 <mapcount> ...` and `0x7D <v!=0> ...`.
    //
    // So the mapcount is present exactly when the version is zero. Reading it
    // unconditionally -- which this did -- would swallow the first four bytes
    // of the *body* of a version-1 stream and then fail somewhere unrelated.
    //
    // Latent rather than live: `machobase.Dumps` passes no `version`, so this
    // client only ever writes `0x7E`. Fixed anyway, because the failure would
    // present as a corrupt body rather than as a version we do not support.
    // Found by the `evejs-crosscheck` lane, which read it out of the writer at
    // `0x1802da8f8` and the reader at `0x1802e1728`; it closes
    // `docs/archive/wire-format.md` gap 4 and `docs/archive/marshal-tags.md` gap 10.
    let extra = if marker == 0x7D { r.u8()? } else { 0 };
    let mc_at = r.pos();
    let mapcount = if extra == 0 { r.i32()? } else { 0 };
    if mapcount < 0 {
        return Err(WireError::Malformed {
            at: mc_at,
            why: "invalid mapcount",
        });
    }
    if r.remaining() < (mapcount as usize).saturating_mul(4) {
        return Err(WireError::Malformed {
            at: r.pos(),
            why: "stream too short for map table",
        });
    }
    Ok(Header {
        marker,
        extra,
        mapcount,
    })
}

/// A complete marshal stream, split into its three parts.
///
/// See [`take_stream`] for why the parts are where they are.
#[derive(Debug, Clone)]
pub struct Stream<'a> {
    pub header: Header,
    /// The header's 1-based shared-object ids, in stream order.
    pub shared_ids: Vec<u32>,
    /// The object's bytes: everything between the header and the id table.
    pub body: &'a [u8],
}

/// Split a complete stream into header, shared-object id table, and body.
///
/// **The id table is at the END of the stream, not after the header.** This
/// took a real client payload to establish and is worth stating plainly,
/// because the obvious reading of the format is wrong:
///
/// ```text
/// 7e 01 00 00 00   marker 0x7E, mapcount = 1
/// 14 04            TUPLE of 4   <- the object starts here, immediately
/// 09               1
/// 13 0e "LogClientStats"
/// ...
/// <4 bytes>        the one shared id, at the very end
/// ```
///
/// `Marshal::Load`'s own check is that the stream *holds at least*
/// `mapcount * 4` further bytes -- a length validation, which reads like a
/// position if you are not careful. It is not one: the writer cannot know the
/// ids until the body is written, so it appends the table afterwards and
/// `Load` seeks to the end for it.
///
/// Every stream we had seen until this `eventLog` call had `mapcount == 0`,
/// which makes the two readings indistinguishable -- the failure mode of a
/// format detail that only one payload in a thousand exercises.
pub fn take_stream(bytes: &[u8]) -> Result<Stream<'_>, WireError> {
    let mut r = Reader::new(bytes);
    let header = take_header(&mut r)?;
    let body_start = r.pos();
    let table_len = header.map_table_len();
    let body_end = bytes
        .len()
        .checked_sub(table_len)
        .ok_or(WireError::Malformed {
            at: body_start,
            why: "stream too short for its shared-object id table",
        })?;
    if body_end < body_start {
        return Err(WireError::Malformed {
            at: body_start,
            why: "shared-object id table overlaps the header",
        });
    }
    let mut tr = Reader::new(&bytes[body_end..]);
    let mut shared_ids = Vec::with_capacity(header.mapcount.max(0) as usize);
    for _ in 0..header.mapcount.max(0) {
        shared_ids.push(tr.i32()? as u32);
    }
    Ok(Stream {
        header,
        shared_ids,
        body: &bytes[body_start..body_end],
    })
}

/// Write a header for a stream carrying no shared objects: the `0x7E` form,
/// which needs no `extra` byte and is the minimal valid header.
pub fn write_empty_header(w: &mut Writer) {
    w.u8(0x7E);
    w.i32(0);
}

#[cfg(test)]
mod tests {
    /// A small `Long` still takes the long tag (`0x2F`); a small `Int`
    /// keeps its compact form.
    #[test]
    fn a_small_long_is_pinned_to_the_int64_tag() {
        for n in [0i64, 1, -1, 127, 40_000, 2_000_000_000] {
            let mut w = Writer::new();
            save(&PyValue::Long(n), &mut w);
            let b = w.into_bytes();
            assert_eq!(b.len(), 10, "tag + length + 8 bytes for {n}");
            assert_eq!(b[0], tag::code::LONG);
            assert_eq!(b[1], 8);
            assert_eq!(&b[2..], &n.to_le_bytes());
            let mut w = Writer::new();
            save(&PyValue::Int(n), &mut w);
            assert_ne!(w.into_bytes()[0], tag::code::LONG, "Int {n} stays compact");
        }
    }

    /// Decoding yields `Int` for every integer tag, LONG included: the
    /// variant is encode-only and carries the same number.
    #[test]
    fn a_long_decodes_as_an_int_with_the_same_value() {
        for n in [0i64, 5, 134_368_857_685_491_816, -7] {
            let mut w = Writer::new();
            save(&PyValue::Long(n), &mut w);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            let got = Decoder::new().load(&mut r).expect("decode");
            assert_eq!(got, PyValue::Int(n));
            assert_eq!(r.remaining(), 0);
        }
        // The client's own small ints stay Int.
        let mut r = Reader::new(&[0x06, 0x07]);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), PyValue::Int(7));
    }
    /// **`0x7D` carries a version, and a non-zero one has no mapcount.**
    ///
    /// The bug this pins: reading the mapcount unconditionally consumed four
    /// bytes of the body on a version-1 stream, which would have surfaced as
    /// a corrupt value somewhere downstream rather than as an unsupported
    /// version. `0x7E` and `0x7D 00` both do carry one.
    #[test]
    fn a_versioned_header_has_no_mapcount_word() {
        // 0x7E: version 0 implicitly, and a mapcount word follows. Declared
        // as 2 with the eight bytes of map table it promises, because
        // `take_header` validates that the table fits.
        let mut r = Reader::new(&[0x7E, 0x02, 0x00, 0x00, 0x00, 1, 0, 0, 0, 2, 0, 0, 0]);
        let h = take_header(&mut r).expect("0x7E with a mapcount");
        assert_eq!((h.marker, h.extra, h.mapcount), (0x7E, 0, 2));
        assert_eq!(r.remaining(), 8, "the table itself is left for the caller");

        // 0x7D 00: version 0 the long way, mapcount still follows.
        let mut r = Reader::new(&[0x7D, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let h = take_header(&mut r).expect("0x7D version 0");
        assert_eq!((h.marker, h.extra, h.mapcount), (0x7D, 0, 0));
        assert_eq!(r.remaining(), 0, "the mapcount word was consumed");

        // 0x7D 01: version 1, and the next bytes are the **body**.
        let mut r = Reader::new(&[0x7D, 0x01, 0xAA, 0xBB, 0xCC, 0xDD]);
        let h = take_header(&mut r).expect("0x7D version 1");
        assert_eq!((h.marker, h.extra, h.mapcount), (0x7D, 1, 0));
        assert_eq!(
            r.remaining(),
            4,
            "a versioned stream has no mapcount word, so all four body bytes \
             must still be there"
        );
    }

    use super::*;

    fn round_trip(v: PyValue) {
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let got = Decoder::new().load(&mut r).expect("decode");
        assert_eq!(got, v, "round trip differed");
        assert_eq!(r.remaining(), 0, "decoder left bytes behind");
    }

    #[test]
    fn scalars_round_trip() {
        round_trip(PyValue::None);
        round_trip(PyValue::Bool(true));
        round_trip(PyValue::Bool(false));
        round_trip(PyValue::Real(-1.5));
        round_trip(PyValue::Bytes(b"macho".to_vec()));
        round_trip(PyValue::Unicode("Jita IV - Moon 4".to_owned()));
    }

    #[test]
    fn integers_round_trip_at_every_width_boundary() {
        for i in [
            0i64,
            1,
            -1,
            127,
            -128,
            128,
            -129,
            32767,
            -32768,
            32768,
            2147483647,
            -2147483648,
            2147483648,
            i64::MAX,
            i64::MIN,
        ] {
            round_trip(PyValue::Int(i));
        }
    }

    #[test]
    fn integer_encoding_picks_the_narrowest_tag() {
        // -1, 0 and 1 have dedicated cached-constant tags: one byte, no
        // payload. Everything else takes the narrowest fixed width.
        let widths = [
            (-1i64, 1usize),
            (0, 1),
            (1, 1),
            (2, 2),
            (127, 2),
            (128, 3),
            (32768, 5),
            (2147483648, 9),
        ];
        for (v, len) in widths {
            let mut w = Writer::new();
            save(&PyValue::Int(v), &mut w);
            assert_eq!(w.into_bytes().len(), len, "value {v}");
        }
    }

    #[test]
    fn containers_round_trip_including_the_short_forms() {
        round_trip(PyValue::Tuple(vec![]));
        round_trip(PyValue::Tuple(vec![PyValue::Int(1)]));
        round_trip(PyValue::Tuple(vec![PyValue::Int(1), PyValue::None]));
        round_trip(PyValue::Tuple(vec![
            PyValue::Int(1),
            PyValue::None,
            PyValue::Bool(true),
        ]));
        round_trip(PyValue::List(vec![PyValue::Int(7)]));
        round_trip(PyValue::List(vec![]));
        round_trip(PyValue::List(vec![
            PyValue::Bytes(b"a".to_vec()),
            PyValue::Int(2),
        ]));
        round_trip(PyValue::Dict(vec![
            (PyValue::Int(1), PyValue::Bytes(b"k".to_vec())),
            (PyValue::None, PyValue::Real(2.0)),
        ]));
    }

    #[test]
    fn short_tuple_forms_are_actually_used() {
        // The 0/1/2-element forms must emit one tag and no count byte.
        let mut w = Writer::new();
        save(&PyValue::Tuple(vec![]), &mut w);
        assert_eq!(w.into_bytes(), vec![tag::code::TUPLE_EMPTY]);

        let mut w = Writer::new();
        save(&PyValue::Tuple(vec![PyValue::None]), &mut w);
        assert_eq!(w.into_bytes(), vec![tag::code::TUPLE1, tag::code::NONE]);
    }

    #[test]
    fn nested_structure_round_trips() {
        round_trip(PyValue::Tuple(vec![
            PyValue::Int(6),
            PyValue::List(vec![PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)])]),
            PyValue::Dict(vec![(PyValue::Bytes(b"sid".to_vec()), PyValue::Int(5))]),
        ]));
    }

    // --- the shared-object mechanism ------------------------------------
    //
    // These exist because every stream we saw for weeks had `mapcount == 0`,
    // which makes a wrong implementation indistinguishable from a right one.

    /// The flag alone does not register: only eleven handlers call
    /// `RegisterShared`, and `None` is not one of them. Counting it would
    /// desynchronise every later id, so this is asserted rather than assumed.
    #[test]
    fn the_shared_flag_on_a_scalar_registers_nothing() {
        let bytes = [tag::code::NONE | tag::SHARED_FLAG];
        let mut r = Reader::new(&bytes);
        let mut d = Decoder::with_shared(vec![1]);
        assert_eq!(d.load(&mut r).unwrap(), PyValue::None);
        assert_eq!(
            d.shared,
            vec![None],
            "a shared-flagged None must not claim the slot"
        );
    }

    /// A shared-flagged string claims its id, and `0x1B` reads it back.
    #[test]
    fn a_shared_string_can_be_referred_back_to() {
        // (shared "ab", ref 1) -- a 2-tuple whose second element is a
        // back-reference to its first.
        let bytes = [
            tag::code::TUPLE2,
            tag::code::STRING_ALT | tag::SHARED_FLAG,
            2,
            b'a',
            b'b',
            tag::code::SHARED_REF,
            1,
        ];
        let mut r = Reader::new(&bytes);
        let mut d = Decoder::with_shared(vec![1]);
        assert_eq!(
            d.load(&mut r).unwrap(),
            PyValue::Tuple(vec![
                PyValue::Bytes(b"ab".to_vec()),
                PyValue::Bytes(b"ab".to_vec()),
            ])
        );
    }

    /// A shared-flagged 2-tuple (`0x6C`) claims its id, like the 1-tuple
    /// (`0x65`) and 1-list (`0x67`): all three call `RegisterShared` in the
    /// client. Before this was fixed a call holding one object twice failed
    /// to decode ("shared back-reference to an object still being built").
    #[test]
    fn shared_small_containers_claim_their_ids() {
        for (flagged, body, want) in [
            (
                tag::code::TUPLE2,
                vec![tag::code::INT_ONE, tag::code::INT_ONE],
                PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(1)]),
            ),
            (
                tag::code::TUPLE1,
                vec![tag::code::INT_ONE],
                PyValue::Tuple(vec![PyValue::Int(1)]),
            ),
            (
                tag::code::LIST1,
                vec![tag::code::INT_ONE],
                PyValue::List(vec![PyValue::Int(1)]),
            ),
        ] {
            let mut bytes = vec![tag::code::TUPLE2, flagged | tag::SHARED_FLAG];
            bytes.extend_from_slice(&body);
            bytes.extend_from_slice(&[tag::code::SHARED_REF, 1]);
            let mut r = Reader::new(&bytes);
            let mut d = Decoder::with_shared(vec![1]);
            assert_eq!(
                d.load(&mut r).unwrap(),
                PyValue::Tuple(vec![want.clone(), want]),
                "code 0x{flagged:02x}"
            );
        }
    }

    /// The second of three registering objects is a shared 2-tuple, and the
    /// table is out of order (`[3, 1, 2]`): the k-th registering object takes
    /// `table[k]`, so the tuple is id 1 and the back-reference reads it.
    #[test]
    fn a_shared_pair_in_the_middle_of_three_takes_the_kth_id() {
        let bytes = [
            tag::code::TUPLE,
            3,
            tag::code::STRING_ALT | tag::SHARED_FLAG, // k = 0 -> id 3
            1,
            b'a',
            tag::code::TUPLE2 | tag::SHARED_FLAG, // k = 1 -> id 1
            tag::code::INT_ONE,
            tag::code::INT_ONE,
            tag::code::LONG | tag::SHARED_FLAG, // k = 2 -> id 2
            1,
            5,
        ];
        let mut r = Reader::new(&bytes);
        let mut d = Decoder::with_shared(vec![3, 1, 2]);
        let v = d.load(&mut r).unwrap();
        let PyValue::Tuple(items) = v else {
            panic!("a tuple")
        };
        assert_eq!(
            items[1],
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(1)])
        );
        // a later back-reference to id 1 finds the pair
        let more = [tag::code::SHARED_REF, 1];
        let mut r2 = Reader::new(&more);
        assert_eq!(
            d.load(&mut r2).unwrap(),
            PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(1)])
        );
    }

    /// Ids are 1-based and bounded by `mapcount`; the client requires
    /// `1 <= index <= mapcount`.
    #[test]
    fn a_shared_reference_outside_the_table_is_refused() {
        for id in [0u8, 2u8] {
            let bytes = [tag::code::SHARED_REF, id];
            let mut r = Reader::new(&bytes);
            let mut d = Decoder::with_shared(vec![1]);
            assert!(
                d.load(&mut r).is_err(),
                "id {id} is outside 1..=1 and must be refused"
            );
        }
    }

    /// More registering objects than the header promised is the client's
    /// "Shared object table overflow".
    #[test]
    fn more_shared_objects_than_the_header_promised_is_refused() {
        let bytes = [
            tag::code::TUPLE2,
            tag::code::STRING_ALT | tag::SHARED_FLAG,
            1,
            b'a',
            tag::code::STRING_ALT | tag::SHARED_FLAG,
            1,
            b'b',
        ];
        let mut r = Reader::new(&bytes);
        let mut d = Decoder::with_shared(vec![1]);
        assert!(d.load(&mut r).is_err(), "only one id was promised");
    }

    /// The id table is at the **end** of the stream, after the body.
    ///
    /// These are the real first bytes of an `eventLog.LogClientStats` call
    /// payload off the wire -- the first stream we ever saw with
    /// `mapcount > 0`. The object starts immediately after the five-byte
    /// header, which is what proves the table cannot be sitting between
    /// them.
    #[test]
    fn the_shared_id_table_is_at_the_end_of_the_stream() {
        let mut bytes = vec![
            0x7E,
            0x01,
            0x00,
            0x00,
            0x00,              // marker, mapcount = 1
            tag::code::TUPLE2, // the object, immediately
            tag::code::STRING_ALT | tag::SHARED_FLAG,
            2,
            b'h',
            b'i',
            tag::code::SHARED_REF,
            1,
        ];
        bytes.extend_from_slice(&1i32.to_le_bytes()); // the id table, at the end
        let stream = take_stream(&bytes).expect("splits");
        assert_eq!(stream.header.mapcount, 1);
        assert_eq!(stream.shared_ids, vec![1]);
        assert_eq!(
            stream.body[0],
            tag::code::TUPLE2,
            "the body starts right after the header"
        );
        let mut r = Reader::new(stream.body);
        assert_eq!(
            Decoder::with_shared(stream.shared_ids.clone())
                .load(&mut r)
                .expect("decodes"),
            PyValue::Tuple(vec![
                PyValue::Bytes(b"hi".to_vec()),
                PyValue::Bytes(b"hi".to_vec()),
            ])
        );
        assert_eq!(r.remaining(), 0, "the table is not part of the body");
    }

    /// A stream shorter than its own promised table is refused rather than
    /// panicking on the subtraction.
    #[test]
    fn a_stream_too_short_for_its_table_is_refused() {
        let bytes = [0x7E, 0x09, 0x00, 0x00, 0x00, tag::code::NONE];
        assert!(take_stream(&bytes).is_err());
    }

    #[test]
    fn invalid_type_code_is_rejected_like_the_client() {
        for bad in tag::INVALID_TYPE_CODES {
            let bytes = [bad];
            let mut r = Reader::new(&bytes);
            assert_eq!(
                Decoder::new().load(&mut r),
                Err(WireError::UnknownTag { at: 0, tag: bad }),
                "0x{bad:02x}"
            );
        }
    }

    #[test]
    fn unestablished_layout_reports_the_tag_and_offset() {
        // 0x21 (self-delimiting cPickle) is a known type whose payload we
        // have not established; it must say so rather than mis-decode.
        // 0x17 and then 0x2A used to be this test's subject; both are now
        // implemented, which is why the subject keeps moving.
        let bytes = [tag::code::TUPLE1, tag::code::PICKLE, 0, 0];
        let mut r = Reader::new(&bytes);
        let err = Decoder::new().load(&mut r).unwrap_err();
        assert_eq!(
            err,
            WireError::UnsupportedTag {
                at: 1,
                tag: tag::code::PICKLE
            }
        );
    }

    // --- object wrappers (docs/reference/object-wrapper.md) --------------------------

    fn instance(class: &str, state: PyValue) -> PyValue {
        PyValue::Instance {
            class: class.as_bytes().to_vec(),
            state: Box::new(state),
        }
    }

    // --- the Load-callback form, 0x19 (docs/archive/marshal-tags.md Gap 9) ---------

    #[test]
    fn a_callback_object_round_trips() {
        round_trip(PyValue::Callback(Box::new(PyValue::Bytes(vec![
            1, 2, 3, 4,
        ]))));
    }

    /// Tag, then the child, and nothing else: no length prefix, no
    /// terminator, no shared-slot registration.
    #[test]
    fn a_callback_is_tag_then_one_object() {
        let mut w = Writer::new();
        save(&PyValue::Callback(Box::new(PyValue::Int(1))), &mut w);
        assert_eq!(
            w.into_bytes(),
            vec![tag::code::CALLBACK, tag::code::INT_ONE]
        );
    }

    /// The shape a bound-object reference actually takes: the child is a byte
    /// string holding a *second* marshal stream, which the client's
    /// `ParseObjectID` loads to get `(objectID, refID)`.
    #[test]
    fn a_bound_object_reference_nests_a_second_stream() {
        // A whole marshal stream, header and all, as the Save callback
        // returns it.
        let mut iw = Writer::new();
        write_empty_header(&mut iw);
        save(
            &PyValue::Tuple(vec![
                PyValue::Bytes(b"N=1:1".to_vec()),
                PyValue::Int(134_368_857_685_491_816),
            ]),
            &mut iw,
        );
        let inner = iw.into_bytes();

        let mut w = Writer::new();
        save(
            &PyValue::Callback(Box::new(PyValue::Bytes(inner.clone()))),
            &mut w,
        );
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let reloaded = Decoder::new().load(&mut r).expect("decode");

        let PyValue::Callback(child) = &reloaded else {
            panic!("expected a callback, got {reloaded:?}")
        };
        let PyValue::Bytes(b) = child.as_ref() else {
            panic!("the reference child is a byte string, got {child:?}")
        };
        assert_eq!(b, &inner, "the nested stream must survive byte for byte");

        // And that nested stream is itself loadable, which is exactly what
        // the client's ParseObjectID does with it.
        let mut ir = Reader::new(b);
        take_header(&mut ir).expect("inner header");
        let PyValue::Tuple(parts) = Decoder::new().load(&mut ir).expect("inner stream loads")
        else {
            panic!("the reference is a tuple")
        };
        assert_eq!(parts.len(), 2, "(objectID, refID)");
        assert_eq!(parts[0], PyValue::Bytes(b"N=1:1".to_vec()));
    }

    #[test]
    fn an_old_style_instance_round_trips() {
        let v = instance(
            "carbon.common.script.net.machoNetPacket.CallRsp",
            PyValue::Tuple(vec![PyValue::Int(6), PyValue::None]),
        );
        round_trip(v);
    }

    #[test]
    fn an_instance_is_tag_then_name_then_state_and_nothing_else() {
        let v = instance("a.B", PyValue::Int(1));
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        assert_eq!(
            bytes,
            vec![
                tag::code::INSTANCE,
                tag::code::STRING_SHORT,
                3,
                b'a',
                b'.',
                b'B',
                tag::code::INT_ONE,
            ],
            "0x17 must not emit a terminator"
        );
    }

    #[test]
    fn an_instance_name_that_is_not_a_string_is_refused() {
        let bytes = [tag::code::INSTANCE, tag::code::INT_ONE, tag::code::NONE];
        let mut r = Reader::new(&bytes);
        assert!(Decoder::new().load(&mut r).is_err());
    }

    #[test]
    fn both_object_ex_forms_round_trip() {
        for form in [ObjectForm::Call, ObjectForm::New] {
            round_trip(PyValue::ObjectEx {
                form,
                header: Box::new(PyValue::Tuple(vec![
                    PyValue::Bytes(b"cls".to_vec()),
                    PyValue::Tuple(vec![PyValue::Int(2)]),
                ])),
                list: vec![PyValue::Int(7), PyValue::None],
                dict: vec![(PyValue::Bytes(b"k".to_vec()), PyValue::Int(9))],
            });
        }
    }

    #[test]
    fn an_empty_object_ex_still_carries_exactly_two_terminators() {
        let v = PyValue::ObjectEx {
            form: ObjectForm::Call,
            header: Box::new(PyValue::None),
            list: vec![],
            dict: vec![],
        };
        let mut w = Writer::new();
        save(&v, &mut w);
        assert_eq!(
            w.into_bytes(),
            vec![
                tag::code::OBJECT_22,
                tag::code::NONE,
                tag::code::OBJECT_EX_END,
                tag::code::OBJECT_EX_END,
            ]
        );
        round_trip(v);
    }

    #[test]
    fn the_two_forms_differ_only_in_their_tag_byte() {
        let mut call = Writer::new();
        save(
            &PyValue::ObjectEx {
                form: ObjectForm::Call,
                header: Box::new(PyValue::None),
                list: vec![],
                dict: vec![],
            },
            &mut call,
        );
        let mut new = Writer::new();
        save(
            &PyValue::ObjectEx {
                form: ObjectForm::New,
                header: Box::new(PyValue::None),
                list: vec![],
                dict: vec![],
            },
            &mut new,
        );
        let (a, b) = (call.into_bytes(), new.into_bytes());
        assert_eq!(a[0], tag::code::OBJECT_22);
        assert_eq!(b[0], tag::code::OBJECT_23);
        assert_eq!(a[1..], b[1..]);
    }

    #[test]
    fn the_object_ex_dict_stream_is_key_then_value() {
        // The opposite order to tag 0x16, and the easiest bug in the whole
        // format to ship. 0x22's stream calls
        // __setitem__(first, second) = __setitem__(key, value).
        let v = PyValue::ObjectEx {
            form: ObjectForm::Call,
            header: Box::new(PyValue::None),
            list: vec![],
            dict: vec![(PyValue::Int(1), PyValue::Int(0))],
        };
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let i = bytes
            .iter()
            .position(|&b| b == tag::code::OBJECT_EX_END)
            .expect("a terminator");
        // key = 1, value = 0, in that order, between the two terminators.
        assert_eq!(bytes[i + 1], tag::code::INT_ONE, "key comes first");
        assert_eq!(bytes[i + 2], tag::code::INT_ZERO, "value comes second");

        // And the plain dict tag is the other way round, in the same test, so
        // the two conventions cannot drift apart unnoticed.
        let mut dw = Writer::new();
        save(
            &PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(1))]),
            &mut dw,
        );
        let db = dw.into_bytes();
        assert_eq!(db[0], tag::code::DICT);
        assert_eq!(db[2], tag::code::INT_ZERO, "0x16 writes the value first");
        assert_eq!(db[3], tag::code::INT_ONE, "0x16 writes the key second");
    }

    #[test]
    fn a_flagged_terminator_does_not_end_a_stream() {
        // The reader compares the raw byte with no shared-flag strip, so 0x6D
        // is an ordinary (invalid) tag, not a terminator.
        let bytes = [
            tag::code::OBJECT_22,
            tag::code::NONE,
            tag::code::OBJECT_EX_END | tag::SHARED_FLAG,
        ];
        let mut r = Reader::new(&bytes);
        assert!(Decoder::new().load(&mut r).is_err());
    }

    #[test]
    fn an_unterminated_object_ex_stream_is_refused() {
        // Runs off the end rather than silently returning what it has.
        let bytes = [tag::code::OBJECT_22, tag::code::NONE];
        let mut r = Reader::new(&bytes);
        assert!(Decoder::new().load(&mut r).is_err());
    }

    #[test]
    fn nested_instances_round_trip() {
        // A MachoPacket's state holds MachoAddress instances, so this is the
        // real shape, not a contrived one.
        round_trip(instance(
            "carbon.common.script.net.machoNetPacket.CallRsp",
            PyValue::Tuple(vec![
                instance(
                    "carbon.common.script.net.machoNetAddress.MachoAddress",
                    PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)]),
                ),
                PyValue::None,
            ]),
        ));
    }

    #[test]
    fn trace_records_every_tag_in_order() {
        let v = PyValue::Tuple(vec![PyValue::None, PyValue::Int(1)]);
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut quiet = Decoder::new();
        quiet.load(&mut Reader::new(&bytes)).unwrap();
        assert!(quiet.trace.is_empty(), "the trace is opt-in");

        let mut d = Decoder::new().with_trace();
        d.load(&mut Reader::new(&bytes)).unwrap();
        let tags: Vec<u8> = d.trace.iter().map(|&(_, t)| t).collect();
        assert_eq!(
            tags,
            vec![tag::code::TUPLE2, tag::code::NONE, tag::code::INT_ONE]
        );
        assert_eq!(d.trace[0].0, 0);
    }

    #[test]
    fn string_forms_match_the_handlers() {
        // 0x0E empty, 0x0F one raw byte and NO length, 0x10 u8 length,
        // 0x0D escaped length above 255.
        let cases: [(usize, u8, usize); 4] = [
            (0, tag::code::STRING_EMPTY, 1),
            (1, tag::code::STRING_CHAR, 2),
            (255, tag::code::STRING_SHORT, 257),
            (256, tag::code::STRING, 262),
        ];
        for (len, want_tag, want_total) in cases {
            let v = PyValue::Bytes(vec![b'z'; len]);
            let mut w = Writer::new();
            save(&v, &mut w);
            let bytes = w.into_bytes();
            assert_eq!(bytes[0], want_tag, "len {len} tag");
            assert_eq!(bytes.len(), want_total, "len {len} total");
            let mut r = Reader::new(&bytes);
            assert_eq!(Decoder::new().load(&mut r).unwrap(), v, "len {len}");
            assert_eq!(r.remaining(), 0, "len {len}");
        }
    }

    #[test]
    fn one_char_string_has_no_length_byte() {
        let mut w = Writer::new();
        save(&PyValue::Bytes(vec![0x41]), &mut w);
        assert_eq!(w.into_bytes(), vec![tag::code::STRING_CHAR, 0x41]);
    }

    #[test]
    fn empty_unicode_and_empty_list_use_their_own_tags() {
        let mut w = Writer::new();
        save(&PyValue::Unicode(String::new()), &mut w);
        assert_eq!(w.into_bytes(), vec![tag::code::UNICODE_EMPTY]);

        let mut w = Writer::new();
        save(&PyValue::List(vec![]), &mut w);
        assert_eq!(w.into_bytes(), vec![tag::code::LIST_EMPTY]);
    }

    #[test]
    fn utf16_forms_decode() {
        // 0x29: exactly two bytes, one code unit, no length field.
        let b = [tag::code::UNICODE_CHAR, 0x41, 0x00];
        let mut r = Reader::new(&b);
        assert_eq!(
            Decoder::new().load(&mut r).unwrap(),
            PyValue::Unicode("A".into())
        );

        // 0x12: the count is in CODE UNITS, so 2*n bytes follow.
        let b = [tag::code::UNICODE_UTF16, 2, 0x48, 0x00, 0x69, 0x00];
        let mut r = Reader::new(&b);
        assert_eq!(
            Decoder::new().load(&mut r).unwrap(),
            PyValue::Unicode("Hi".into())
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn short_string_tag_decodes_without_an_escape() {
        // 0x10's length byte has no 0xFF escape; a literal 0xFF would be a
        // 255-byte string, not an escape.
        let mut b = vec![tag::code::STRING_SHORT, 3];
        b.extend_from_slice(b"abc");
        let mut r = Reader::new(&b);
        assert_eq!(
            Decoder::new().load(&mut r).unwrap(),
            PyValue::Bytes(b"abc".to_vec())
        );
    }

    #[test]
    fn dict_writes_value_before_key() {
        // Confirmed from PyDict_SetItem(dict, key=second, value=first).
        let d = PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(1))]);
        let mut w = Writer::new();
        save(&d, &mut w);
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], tag::code::DICT);
        assert_eq!(bytes[1], 1, "pair count");
        assert_eq!(bytes[2], tag::code::INT_ZERO, "value first");
        assert_eq!(bytes[3], tag::code::INT_ONE, "key second");
        // and dict_as_kv flips them back
        let kv = d.dict_as_kv().unwrap();
        assert_eq!(*kv[0].0, PyValue::Int(1), "key");
        assert_eq!(*kv[0].1, PyValue::Int(0), "value");
    }

    #[test]
    fn negative_zero_is_not_collapsed_into_the_constant() {
        let mut w = Writer::new();
        save(&PyValue::Real(-0.0), &mut w);
        let bytes = w.into_bytes();
        assert_eq!(
            bytes[0],
            tag::code::REAL,
            "-0.0 must not use the 0.0 constant"
        );
        let mut r = Reader::new(&bytes);
        let PyValue::Real(got) = Decoder::new().load(&mut r).unwrap() else {
            panic!()
        };
        assert!(got.is_sign_negative());
    }

    #[test]
    fn hostile_count_cannot_make_us_allocate() {
        // A huge count with no payload behind it must fail cheaply.
        let bytes = [tag::code::LIST, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
        let mut r = Reader::new(&bytes);
        let err = Decoder::new().load(&mut r).unwrap_err();
        assert!(matches!(err, WireError::Malformed { .. }), "got {err:?}");
    }

    #[test]
    fn truncated_stream_reports_eof() {
        let bytes = [tag::code::INT32, 0x01, 0x02]; // one byte short
        let mut r = Reader::new(&bytes);
        assert_eq!(
            Decoder::new().load(&mut r),
            Err(WireError::UnexpectedEof { at: 1, need: 4 })
        );
    }

    #[test]
    fn header_0x7e_consumes_no_extra_byte() {
        let b = [0x7E, 0, 0, 0, 0];
        let mut r = Reader::new(&b);
        let h = take_header(&mut r).unwrap();
        assert_eq!(
            h,
            Header {
                marker: 0x7E,
                extra: 0,
                mapcount: 0
            }
        );
        assert_eq!(r.remaining(), 0, "0x7E must not consume an extra byte");
    }

    /// **This test used to assert the bug.**
    ///
    /// It read `[0x7D, 0xAB, 0, 0, 0, 0]` and required `remaining() == 0`,
    /// which only held because `take_header` was consuming four bytes as a
    /// mapcount that a versioned stream does not have. `0xAB` is a non-zero
    /// version, so those four bytes are **body** and must survive -- see the
    /// comment on `take_header`.
    #[test]
    fn header_0x7d_consumes_one_extra_byte() {
        let b = [0x7D, 0xAB, 0, 0, 0, 0];
        let mut r = Reader::new(&b);
        let h = take_header(&mut r).unwrap();
        assert_eq!(
            h,
            Header {
                marker: 0x7D,
                extra: 0xAB,
                mapcount: 0
            }
        );
        assert_eq!(
            r.remaining(),
            4,
            "version 0xAB has no mapcount word, so the four body bytes stay"
        );
    }

    #[test]
    fn header_requires_the_map_table_to_fit() {
        let short = [0x7E, 2, 0, 0, 0, 1, 2, 3, 4];
        let mut r = Reader::new(&short);
        assert_eq!(
            take_header(&mut r),
            Err(WireError::Malformed {
                at: 5,
                why: "stream too short for map table"
            })
        );
        let ok = [0x7E, 2, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        let mut r = Reader::new(&ok);
        let h = take_header(&mut r).unwrap();
        assert_eq!(h.mapcount, 2);
        assert_eq!(h.map_table_len(), 8);
    }

    #[test]
    fn header_rejects_a_negative_mapcount() {
        let b = [0x7E, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut r = Reader::new(&b);
        assert_eq!(
            take_header(&mut r),
            Err(WireError::Malformed {
                at: 1,
                why: "invalid mapcount"
            })
        );
    }

    #[test]
    fn header_rejects_a_bad_marker() {
        let b = [0x78, 0x9C, 0, 0, 0];
        let mut r = Reader::new(&b);
        assert_eq!(
            take_header(&mut r),
            Err(WireError::Malformed {
                at: 0,
                why: "invalid marshal header"
            })
        );
    }

    #[test]
    fn empty_header_round_trips() {
        let mut w = Writer::new();
        write_empty_header(&mut w);
        let bytes = w.into_bytes();
        assert_eq!(bytes, vec![0x7E, 0, 0, 0, 0]);
        let mut r = Reader::new(&bytes);
        assert_eq!(take_header(&mut r).unwrap().mapcount, 0);
    }

    #[test]
    fn a_header_plus_a_value_parses_as_a_whole_stream() {
        let mut w = Writer::new();
        write_empty_header(&mut w);
        save(
            &PyValue::Tuple(vec![PyValue::Int(1), PyValue::None]),
            &mut w,
        );
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        take_header(&mut r).unwrap();
        let v = Decoder::new().load(&mut r).unwrap();
        assert_eq!(v, PyValue::Tuple(vec![PyValue::Int(1), PyValue::None]));
        assert_eq!(r.remaining(), 0);
    }

    // --- the decode budget (docs/review/V2-wire.md F2, plan-review.md M4) ---

    fn budget_refused(r: Result<PyValue, WireError>) -> bool {
        matches!(
            r,
            Err(WireError::Malformed {
                why: "decode budget",
                ..
            })
        )
    }

    /// `[shared LIST of k Nones, 0x1B 1 x refs]`: a stream of about
    /// `2 * refs` bytes that builds `k * refs` nodes.
    fn fanout(k: usize, refs: usize) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(tag::code::LIST);
        w.size(refs + 1);
        w.u8(tag::code::LIST | tag::SHARED_FLAG);
        w.size(k);
        for _ in 0..k {
            w.u8(tag::code::NONE);
        }
        for _ in 0..refs {
            w.u8(tag::code::SHARED_REF);
            w.size(1);
        }
        w.into_bytes()
    }

    /// One shared list and a list of references to it: a few kilobytes on
    /// the wire, millions of nodes once every reference is cloned.
    #[test]
    fn a_reference_fanout_past_the_budget_fails() {
        let k = 1000;
        let refs = Budget::INBOUND.nodes / k + 1;
        let bytes = fanout(k, refs);
        assert!(
            bytes.len() * 100 < Budget::INBOUND.nodes,
            "the stream itself is small"
        );

        let mut r = Reader::new(&bytes);
        let mut d = Decoder::with_shared(vec![1]).with_budget(Budget::INBOUND);
        assert!(budget_refused(d.load(&mut r)));
        assert!(
            d.spent().nodes <= Budget::INBOUND.nodes + k + 1,
            "refused at the reference that crossed the line, not after"
        );

        // The same bytes without a budget really do build that many nodes,
        // so it is the budget that refuses them and nothing else.
        let mut r = Reader::new(&bytes);
        let mut free = Decoder::with_shared(vec![1]);
        let PyValue::List(items) = free.load(&mut r).expect("unbudgeted decodes") else {
            panic!("a list");
        };
        assert_eq!(items.len(), refs + 1);
        assert!(free.spent().nodes > Budget::INBOUND.nodes);
    }

    /// A back-reference costs its target's whole cost, and the shared
    /// table's own copy costs it once more.
    #[test]
    fn a_back_reference_is_charged_its_targets_whole_cost() {
        let bytes = fanout(3, 1);
        let mut d = Decoder::with_shared(vec![1]);
        d.load(&mut Reader::new(&bytes)).expect("decodes");
        let list = Budget {
            nodes: 4,
            bytes: 4 * NODE_BYTES,
        };
        let outer = Budget {
            nodes: 1,
            bytes: NODE_BYTES,
        };
        // built + table copy + one clone + the outer list
        assert_eq!(
            d.spent(),
            Budget {
                nodes: 3 * list.nodes + outer.nodes,
                bytes: 3 * list.bytes + outer.bytes,
            }
        );
        // The byte estimate alone also refuses.
        let tight = Budget {
            nodes: usize::MAX,
            bytes: 3 * list.bytes,
        };
        let mut d = Decoder::with_shared(vec![1]).with_budget(tight);
        assert!(budget_refused(d.load(&mut Reader::new(&bytes))));
    }

    /// `machoNet.dropPacketThreshold`, the client's own ceiling on one
    /// payload [V] (`eve-proto/src/frame.rs:33`; this crate cannot see it).
    const CLIENT_PAYLOAD_CEILING: usize = 5_000_000;

    /// Rows of `(int, 20-byte str, float)` until the stream is `len` bytes.
    fn rows_of_at_least(len: usize) -> PyValue {
        let row = |i: usize| {
            PyValue::Tuple(vec![
                PyValue::Int(1_000_000 + i as i64),
                PyValue::Bytes(format!("invented-row-{i:08}").into_bytes()),
                PyValue::Real(i as f64 + 0.5),
            ])
        };
        let mut w = Writer::new();
        save(&row(0), &mut w);
        let per_row = w.into_bytes().len();
        PyValue::List((0..len.div_ceil(per_row)).map(row).collect())
    }

    /// A client payload as large as the client will send, made of ordinary
    /// rows, fits [`Budget::INBOUND`].
    #[test]
    fn a_real_client_blob_decodes_under_the_default_budget() {
        let v = rows_of_at_least(CLIENT_PAYLOAD_CEILING - 64);
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        assert!(bytes.len() >= CLIENT_PAYLOAD_CEILING - 64);
        assert!(bytes.len() <= CLIENT_PAYLOAD_CEILING);

        let mut r = Reader::new(&bytes);
        let mut d = Decoder::new().with_budget(Budget::INBOUND);
        assert_eq!(d.load(&mut r).expect("fits the budget"), v);
        assert_eq!(r.remaining(), 0);
    }

    /// The server's responses (a large rowset, the bulk data) are decoded
    /// by the bots and `eve-probe` with the default decoder, which has no
    /// budget: a response past [`Budget::INBOUND`] still decodes there.
    #[test]
    fn a_large_server_response_decodes_with_the_default_decoder() {
        // Four nodes a row, so this is past the inbound node budget.
        let rows = Budget::INBOUND.nodes / 4 + 1;
        let row = |i: usize| {
            PyValue::Tuple(vec![
                PyValue::Int(i as i64),
                PyValue::Bytes(b"invented".to_vec()),
                PyValue::Real(0.5),
            ])
        };
        let v = PyValue::List((0..rows).map(row).collect());
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();

        let mut d = Decoder::new();
        assert_eq!(d.load(&mut Reader::new(&bytes)).expect("decodes"), v);
        assert!(d.spent().nodes > Budget::INBOUND.nodes);

        let mut inbound = Decoder::new().with_budget(Budget::INBOUND);
        assert!(budget_refused(inbound.load(&mut Reader::new(&bytes))));
    }

    /// How deep a value's tree goes below it: a leaf is 0.
    fn tree_depth(v: &PyValue) -> usize {
        let children: Vec<&PyValue> = match v {
            PyValue::Tuple(items) | PyValue::List(items) => items.iter().collect(),
            PyValue::Dict(pairs) => pairs.iter().flat_map(|(a, b)| [a, b]).collect(),
            PyValue::Callback(c) => vec![c],
            PyValue::Instance { state, .. } => vec![state],
            PyValue::ObjectEx {
                header, list, dict, ..
            } => std::iter::once(header.as_ref())
                .chain(list)
                .chain(dict.iter().flat_map(|(a, b)| [a, b]))
                .collect(),
            _ => Vec::new(),
        };
        children
            .into_iter()
            .map(|c| 1 + tree_depth(c))
            .max()
            .unwrap_or(0)
    }

    /// Every way one value can hold the next, as `(name, prefix, suffix,
    /// slack)`: a level is `prefix`, the child, then `suffix`, and a level
    /// also holds a side child `slack` levels deep (a row's descriptor), so
    /// the deepest such stream has `slack` fewer levels.
    fn nesting_shapes() -> Vec<(&'static str, Vec<u8>, Vec<u8>, usize)> {
        use tag::code as c;
        // A one-column row whose object is the child: the row with a `None`
        // object, minus that last byte.
        let d = DbRowDescriptor::new(vec![dbrow::Column {
            name: b"invented".to_vec(),
            db_type: crate::DbType::Str,
        }])
        .expect("descriptor");
        let descriptor_depth = tree_depth(&d.to_pyvalue());
        let row = DbRow::new(d, vec![dbrow::DbValue::Object(PyValue::None)]).expect("row");
        let mut w = Writer::new();
        save(&PyValue::DbRow(Box::new(row)), &mut w);
        let mut dbrow_prefix = w.into_bytes();
        assert_eq!(dbrow_prefix.pop(), Some(c::NONE));
        vec![
            ("LIST1", vec![c::LIST1], vec![], 0),
            ("TUPLE2", vec![c::TUPLE2], vec![c::NONE], 0),
            ("LIST", vec![c::LIST, 2], vec![c::NONE], 0),
            ("DICT", vec![c::DICT, 1], vec![c::NONE], 0),
            ("CALLBACK", vec![c::CALLBACK], vec![], 0),
            (
                "INSTANCE",
                vec![c::INSTANCE, c::STRING_SHORT, 1, b'a'],
                vec![],
                0,
            ),
            (
                "OBJECT_22",
                vec![c::OBJECT_22],
                vec![c::OBJECT_EX_END, c::OBJECT_EX_END],
                0,
            ),
            ("DBROW", dbrow_prefix, vec![], descriptor_depth),
        ]
    }

    /// `n` levels of one shape around a `None`: the leaf is at depth `n`.
    fn nested(prefix: &[u8], suffix: &[u8], n: usize) -> Vec<u8> {
        let mut b = prefix.repeat(n);
        b.push(tag::code::NONE);
        b.extend(suffix.repeat(n));
        b
    }

    /// The deepest stream the decoder accepts, in every nesting shape,
    /// decodes and drops on a thread with `stack` bytes, and one level deeper
    /// is refused. An overflow aborts the whole test process, not one test.
    fn depth_limit_fits(stack: usize) {
        let worker = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(|| {
                let max = Decoder::new().max_depth as usize;
                for (name, prefix, suffix, slack) in nesting_shapes() {
                    for budget in [None, Some(Budget::INBOUND)] {
                        let fresh = || {
                            let d = Decoder::with_shared(Vec::new());
                            match budget {
                                Some(b) => d.with_budget(b),
                                None => d,
                            }
                        };
                        let deepest = nested(&prefix, &suffix, max - 1 - slack);
                        let mut r = Reader::new(&deepest);
                        let v = fresh().load(&mut r);
                        assert!(v.is_ok(), "{name}: the deepest stream loads");
                        assert_eq!(r.remaining(), 0, "{name}");
                        // Dropping the tree recurses too, on this same stack.
                        drop(v);

                        let too_deep = nested(&prefix, &suffix, max);
                        assert!(
                            matches!(
                                fresh().load(&mut Reader::new(&too_deep)),
                                Err(WireError::Malformed {
                                    why: "recursion limit",
                                    ..
                                })
                            ),
                            "{name}: depth max is refused"
                        );
                    }
                }
            })
            .expect("spawn");
        worker.join().expect("no overflow and no failure");
    }

    /// V2 F8 in a release build: full depth fits the 2 MiB a spawned thread
    /// gets by default. In a debug build it does not (about 6 MB at full
    /// depth, `docs/lanes/ip12.md`), so the test is release-only and the
    /// debug case is the next test's.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "a debug build needs DECODE_STACK, not 2 MiB (V2 F8); run with --release"
    )]
    fn depth_limit_fits_a_two_mib_thread() {
        depth_limit_fits(2 << 20);
    }

    /// V2 F8 in either profile: full depth fits [`DECODE_STACK`].
    #[test]
    fn depth_limit_fits_the_decode_stack() {
        depth_limit_fits(DECODE_STACK);
    }
}
