# Marshal type codes and payload layouts (client build V24.01, 3619879)

> **Demoted 2026-10-24** — gap 1 was closed elsewhere, and DBRow's tag is `0x2A`, not `0x2B`. Current truth: [docs/reference/object-wrapper.md](../reference/object-wrapper.md), [docs/reference/dbrow.md](../reference/dbrow.md). Kept for history; do not brief a lane from this file.

Companion to `docs/archive/wire-format.md`, which establishes the header, the tag split
(`type = tag & 0x3F`, `shared = tag & 0x40`, bit 7 discarded), the 64-slot
dispatch table at `0x180b015f0` and the 42 valid codes. This file finishes the
table: names for the remaining codes, and byte-level payload layouts.

Markers used throughout:

- **[V]** verified from this build's instructions.
- **[I]** inferred — consistent with the evidence, not read off the
  instructions. A hypothesis.

Canonical/published PyRep values were deliberately not used as a source.

Independently re-verified before any of this was encoded in code: the dict pair
order (`PyDict_SetItem` at `0x1808666e8`, call site `0x1802dd2f7`), and the
`0x0F` layout (`0x1802dca40`: needs 1 byte, reads one byte, `edx = 1`,
`PyString_FromStringAndSize(ptr, 1)` — no length field).

## Corrections to earlier passes

1. **`0x13` is resolved: handler `0x1802dc5a0`, the *same* handler as `0x0D`.**
   The table has exactly one duplicate, which is why a reconstruction whose
   success criterion was "no duplicates" left this slot unexplained. **[V]**
2. **The five 13-byte stubs do not return compile-time constants.** Each is
   `mov rax,[rcx+K]; test; je; inc qword [rax]; ret` — it returns a pre-built
   PyObject cached in the MarshalStream object. "Which constant" is answered by
   the constructor at `0x1802d9b60`, not by the stub.
3. **`0x1B`, not `0x1C`, is the shared back-reference.** `0x1C` is a
   CRC-checked object wrapper.
4. **The header's `mapcount * 4` table is not stream offsets.** It is a table
   of 1-based shared-object IDs.
5. **`0x2A`/`0x2B` do not "read 4".** Both are 5-byte tail jumps; the phantom
   read came from a disassembler scanning past the jump into the neighbouring
   `0x1C` handler.
6. **There is a CRC in the protocol after all** — not in the header (that part
   of `docs/archive/wire-format.md` stands) but as tag `0x1C`, gated by `skipCrcCheck`. So
   `blue.marshal.Load(..., skipCrcCheck=True)` does correspond to something:
   it disables the `0x1C` check.

## Shared conventions

### Count/length fields **[V]**

Every variable-length tag **except `0x10`** uses:

```text
u8  n
if n == 0xFF:  i32 n      ; little-endian, signed
if n < 0:      RuntimeError "Can't read %d elements"
```

Used by `0x02, 0x0D, 0x12, 0x13, 0x14, 0x15, 0x16, 0x1A, 0x1B, 0x2E, 0x2F`.
**`0x10` is the exception: a plain `u8` length with no escape**, so a string of
256+ bytes cannot be encoded as `0x10`.

### Dispatch **[V]**

`handler(ctx + entry.offset, stream, shared)`. **Every entry's `offset` dword
is 0**, so `rcx` is always the MarshalStream object. Per-tag statistics at
`ctx+0x108` (64 × u32); recursion counter at `ctx+0x15c`, limit 1000.

`rcx` — MarshalStream object (ctor `0x1802d9b60`; member table `0x1802d9140`):

| off | contents |
|---|---|
| 0x20 | `stringTable` — `PyDict_New()`, save-side name→index |
| 0x28 | `stringTableRev` — `PyList_New(0)`, load-side index→name |
| 0x38 / 0x40 / 0x48 | `PyInt_FromLong` of -1 / 0 / 1 |
| 0x50 | `PyFloat_FromDouble(0.0)` |
| 0x58 | `PyString_FromString("")` |
| 0x124 | `skipCrcCheck` (read by the `0x1C` handler) |
| 0x15c | recursion depth |

`rdx` — the stream:

| off | contents |
|---|---|
| 0x10 | the Load callback (used by `0x19`) |
| 0x18 / 0x28 / 0x30 | buffer base / end of buffer / current position |
| 0x20 | end of data (used by the CRC) |
| 0x38 / 0x40 / 0x48 | shared-object array: begin / end / capacity |
| 0x50 | pointer to the header's shared-ID table |
| 0x58 / 0x5c | shared-registration counter / mapcount |
| 0x60 / 0x68 / 0x6c | CRC region start / expected CRC / had-CRC flag |
| 0x70 / 0x78 | cached `cPickle.Unpickler` for `0x21` / `0x1A` |
| 0x80 | nonzero in save mode |

### The shared-object mechanism **[V]**

**Correction (lane ci-fix-d5): fourteen handlers register, not eleven.** `0x25`, `0x27` and `0x2C` also call `RegisterShared` when flagged (`0x1802dd8b0`, `0x1802dd550`, `0x1802dd7d0` each call `0x1802dbdf0` before reading the element). Missing them made the decoder refuse any call holding one 2-tuple twice (`shared back-reference to an object still being built`).

`RegisterShared` = `0x1802dbdf0`, load path:

```text
n = stream[0x58]
if n >= stream[0x5c]:  RuntimeError "Shared object table overflow"
slot = ((u32*)stream[0x50])[n] - 1
stream[0x58] = n + 1
shared_array[slot] = obj
```

So the header's `mapcount * 4` bytes are **1-based shared-object IDs**, and the
k-th shared-flagged object in stream order claims ID `table[k]`. A decoder must
keep its own counter and look the ID up — the table is not offsets, and ID is
not position.

#### Correction: the table is at the **END** of the stream **[V]**

This document used to say the table sits between the header and the first
type byte. It does not, and the wording above is where that reading came
from: `Marshal::Load`'s check is that the stream *holds at least*
`mapcount * 4` further bytes, which is a **length validation**, not a
position. The writer cannot know the IDs until the body is written, so it
appends the table afterwards and `Load` seeks to the end for it.

The first real payload with `mapcount > 0` settled it — an
`eventLog.LogClientStats` call:

```text
7e 01 00 00 00   marker 0x7E, mapcount = 1
14 04            TUPLE of 4   <- the object starts here, immediately
09               1
13 0e            "LogClientStats"
14 03            TUPLE of 3   (the args)
6f 08 ...        0x2F|0x40 -- a shared-flagged LONG, which claims the one ID
...
<4 bytes>        the ID table, at the very end
```

Every stream we had handled until then had `mapcount == 0`, which makes the
two readings **indistinguishable**. Worth remembering as a shape of bug: a
format detail that only one payload in a thousand exercises will read as
settled for as long as that payload does not arrive.

`eve_marshal::take_stream` splits a stream into header, IDs and body, and is
the only place that layout is encoded. Both `eve-server::wire::loads` and
`eve-proto`'s `from_pickle` use it — the latter had been skipping the table
entirely, which is what made this call undecodable rather than merely
mis-shared.

**Only these handlers consume a table entry**: `0x02, 0x0D, 0x13, 0x14, 0x15,
0x16, 0x17, 0x21, 0x22, 0x23, 0x2F`. Scalars, the constant stubs, `0x19`,
`0x1A`, `0x1B` and `0x1C` ignore the flag — a shared-flagged int32 does **not**
consume an entry. **[V]**

`0x17/0x22/0x23` register **before** reading children (reserving the slot so a
child can refer back); containers register the empty container; the rest
register after building. **[V]**

## The table

### Constant stubs — all five proven **[V]**

Zero payload bytes. Identity from the constructor at `0x1802d9b60`.

| code | handler | field | value | evidence |
|---|---|---|---|---|
| 0x07 | 0x1802dcba0 | ctx+0x38 | **int -1** | `lea ecx,[rsi-1]` (rsi=0) → `PyInt_FromLong` |
| 0x08 | 0x1802dcb90 | ctx+0x40 | **int 0** | `xor ecx,ecx` → `PyInt_FromLong` |
| 0x09 | 0x1802dcb80 | ctx+0x48 | **int 1** | `lea ecx,[rsi+1]` → `PyInt_FromLong` |
| 0x0B | 0x1802dcb00 | ctx+0x50 | **float 0.0** | `xorps xmm0,xmm0` → `PyFloat_FromDouble` |
| 0x0E | 0x1802dcab0 | ctx+0x58 | **empty str** | `PyString_FromString("")` |

The folklore order turns out right — but is now read off the constructor.

### Singletons and fixed-width scalars

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x01 | 0x1802dcd60 | None | none | `_Py_NoneStruct` **[V]** |
| 0x03 | 0x1802dccf0 | int64 | 8 bytes | needs 8, `PyLong_FromLongLong` **[V]**; LE two's complement **[I]** |
| 0x04 | 0x1802dcc80 | int32 | 4 bytes | needs 4, `PyInt_FromLong` **[V]** |
| 0x05 | 0x1802dcc10 | int16 | 2 bytes | needs 2, `PyInt_FromLong` **[V]** |
| 0x06 | 0x1802dcbb0 | int8 | 1 byte | needs 1, `PyInt_FromLong` **[V]** |
| 0x0A | 0x1802dcb10 | f64 | 8 bytes | `movsd`, `PyFloat_FromDouble` **[V]** |
| 0x1F | 0x1802dcae0 | True | none | `_Py_TrueStruct` **[V]** |
| 0x20 | 0x1802dcac0 | False | none | `_Py_ZeroStruct` **[V]** |

### Strings — the `0x0F`/`0x10`/`0x0D` question answered

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x0F | 0x1802dca40 | **1-character string** | exactly **1 byte**, the character | needs 1; `edx = 1`, `PyString_FromStringAndSize(&b, 1)`. **No length field** **[V]** |
| 0x10 | 0x1802dc9b0 | **short string** | `u8 len` (**no escape**), then `len` bytes | no `cmp al,0xff` anywhere **[V]** |
| 0x0D | 0x1802dc5a0 | **string, escaped length, interned** | `u8`/`0xFF`+`i32` len, then bytes | helper `0x1802ddc00`; `PyString_InternInPlace` when `len <= 30`; registers shared **[V]** |
| 0x13 | 0x1802dc5a0 | **same decoder as `0x0D`** | identical | same handler address **[V]** |
| 0x0E | 0x1802dcab0 | empty string | none | see stubs **[V]** |
| 0x11 | 0x1802dc910 | string-table index | `u8 index`, 1-based | see below **[V]** |

Encoder consequence: a string of length ≥ 256 **must** use `0x0D`/`0x13`,
never `0x10`.

### The four unicode forms

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x12 | 0x1802dc7e0 | UTF-16 | escaped count `n` = **UTF-16 code units**, then `2*n` bytes | check `n <= remaining>>1`; `DecodeUTF16(p, 2*n, NULL, &bo=-1)` → LE, no BOM **[V]** |
| 0x29 | 0x1802dc740 | 1-character unicode | exactly **2 bytes** | needs 2, `DecodeUTF16(p, 2, …)`. No length field **[V]** |
| 0x28 | 0x1802dc7d0 | **empty unicode** | **none** | whole handler is `lea rcx,[zero byte]; jmp PyUnicodeUCS2_FromString` **[V]** |
| 0x2E | 0x1802dc620 | UTF-8 | escaped `len` = **bytes**, then `len` bytes | `DecodeUTF8(p, len, NULL)` **[V]** |

The trap: `0x12`'s count is code units (×2 bytes), `0x2E`'s is bytes.

### Containers

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x14 | 0x1802dd610 | tuple | escaped `n`, then `n` objects in order | `PyTuple_New(n)`, register-if-shared, loop filling `ob_item`. A self-referencing child raises "Self referencing tuple" **[V]** |
| 0x15 | 0x1802dd3b0 | list | same as `0x14` | `PyList_New(n)` **[V]** |
| 0x16 | 0x1802dd1d0 | dict | escaped `n` = **pair count**, then `n` × (**value, then key**) | `A=ReadObject(); B=ReadObject(); PyDict_SetItem(dict, key=B, value=A)` at `0x1802dd2f7` — `rdx`(key) is the second read, `r8`(value) the first **[V]** |
| 0x24 | 0x1802dc590 | **empty tuple** | none | `xor ecx,ecx; jmp PyTuple_New` **[V]** |
| 0x26 | 0x1802dd600 | **empty list** | none | `xor ecx,ecx; jmp PyList_New` **[V]** |
| 0x25 | 0x1802dd8b0 | 1-tuple | 1 object, no count | one `ReadObject` call site **[I]**; calls `RegisterShared` when flagged **[V]** (disassembled, lane ci-fix-d5) |
| 0x27 | 0x1802dd550 | 1-list | 1 object, no count | one `ReadObject` call site **[I]**; calls `RegisterShared` when flagged **[V]** |
| 0x2C | 0x1802dd7d0 | 2-tuple | 2 objects, no count | loop bound of 2 **[I]**; calls `RegisterShared` when flagged **[V]**, which a real PI `SubmitChanges` stream proved |

### String table

`0x11` (`0x1802dc910`) **[V]**:

```text
u8 index
if index < 1 or index > ob_size(ctx[0x28]):  RuntimeError "Invalid string table index %d"
return incref(ob_item(ctx[0x28])[index - 1])
```

`ctx+0x28` is `stringTableRev` (member table at `0x1802d9140` pairs the name
with offset `0x28`). **1-based**, confirming the indexing in
`server/crates/eve-marshal/src/string_table.rs`.

**Resolved by the first real client. [V]** This was an open risk, recorded
here as: *`blue.dll` contains no built-in table, so the active table may be
empty and `0x11` may never arrive.* Half of that was right and the important
half was wrong.

Right: `eve/common/lib/marshalstrings.pyc`'s `DoIt()` really does
`oldTables = SwapTables((smap, strings))` then `SwapTables(oldTables)`, so the
Python table it carries is installed and then immediately swapped back out.
That module is a no-op and is **not** the source of the live table.

Wrong: the table it swaps back in is not empty. `blue.dll` *does* carry a
built-in table — an array of 195 string pointers at VA `0x180953b00` in this
build, walked by an initialisation loop that ends

```text
inc  edi
add  rsi, 8
cmp  edi, 0xc3        ; 195
jl   <loop top>
```

and the `0x11` handler bounds-checks against the same 195 (`cmp eax, 0xc3`).

And `0x11` does arrive: the first real client's very first service call
carries `{machoVersion: 1}`, with the key encoded as `11 87` — index 135.
`machoVersion` is entry 135 of the native table, which confirms the table, the
1-based convention, and that this tag is routine rather than exotic, all from
one byte of real traffic.

`server/crates/eve-marshal/src/string_table.rs` is regenerated from the DLL by
`tools/extract_string_table.sh`, which locates both the array and the count
structurally and refuses to emit a table that does not put `machoVersion` at
135. The earlier table, taken from `marshalstrings.py`, had 210 entries in a
different order and put `machoVersion` at 148.

### Shared references and CRC

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x1B | 0x1802dce60 | **shared back-reference** | escaped `index`, **1-based** | load path requires `1 <= index <= mapcount` and a non-NULL slot; else `ValueError "Invalid TY_REFERENCE in stream"` **[V]**. **Implemented.** A reference to a slot that is still empty is a cycle, and we refuse it: the client can build one because it registers a mutable container before filling it, while we return owned values. No real payload has needed it. |
| 0x1C | 0x1802dcda0 | **CRC-checked object** | `u32 checksum` (LE), then **one object** | sets region start/expected; when not saving and `skipCrcCheck == 0`, calls `0x1802dbfd0` → `0x180576ee0(1, base+start, stream[0x20]-start)`, compares, `ValueError "Bad CRC"` on mismatch; then tail-jumps `ReadObject` **[V]** |

Two `0x1C` details are **[I]**: the checksum function is called with seed 1
(adler32's signature) but was not disassembled; and the region uses
`stream[0x20]` (end of *data*), so it appears to cover everything after the
checksum field to the end of data, not just the nested object. Confirm both
before emitting a `0x1C`; reading one is safe.

### Callback, pickle and global forms

| code | handler | name | payload | evidence |
|---|---|---|---|---|
| 0x19 | 0x1802dd0d0 | **custom data via the Load callback** | **one object**, no length | requires `stream[0x10]`, else "Unmarshal stream contains custom data but I have no callback method"; returns `PyObject_CallFunctionObjArgs(callback, child, NULL)`. No shared registration **[V]** |
| 0x21 | 0x1802dcf60 | **cPickle payload** | raw cPickle, self-delimiting, **no length** | cached `cPickle.Unpickler` at `stream[0x70]`, `find_global` copied from the MarshalStream object; registers shared **[V]** |
| 0x1A | 0x1802dd010 | **length-prefixed cPickle** | escaped `len` — **read and discarded** — then raw cPickle | the length is never used; unpickler at `stream[0x78]`, `memo` cleared on reuse **[V]** |
| 0x02 | 0x1802ddd60 | **global by name** | escaped `len` + `len` bytes = the name | resolved by `0x1802deb60` via `globalsWhitelist`; owns "TY_GLOBAL: %s" and "HACKER WARNING! object %s.%s is not in whitelist" **[V]** |
| 0x2F | 0x1802de9a0 | **arbitrary-precision int** | escaped `len` + `len` bytes | `len == 0` → 0; else `_PyLong_FromByteArray(p, len, little_endian=1, is_signed=1)` **[V]** |

`0x19` is the counterpart of `blue.marshal.Save(obj, callback)` and therefore
the likely carrier of bound-object references (`GetObjectID`), which would make
gap 5 of `docs/archive/wire-format.md` `0x19` rather than `0x2B`. **[I]** — the handler
proves only "calls the load callback with one decoded object".

### Object forms — named, layouts incomplete

| code | handler | name | established | status |
|---|---|---|---|---|
| 0x17 | 0x1802dde40 | old-style instance | reserves its shared slot first, reads one child and resolves it as a global; "global object %s should be of class type but is of %s"; uses `PyInstance_NewRaw`, `__setstate__`/`__dict__`, and a second `ReadObject` | **[V]** prologue; **field order not established** |
| 0x22 | 0x1802de1c0 | object, 2.2 form | reserves slot first; reads one child; `PyTuple_GetItem(child, 0)` then 1; `PyObject_CallObject`, `PyObject_GetAttr` | **[V]** prologue; layout **gap** |
| 0x23 | 0x1802de430 | object, 2.3 form | same shape, `PyTuple_GetItem(child, 0)` | **[V]** prologue; layout **gap** |
| 0x2A | → 0x1802fe410 | **DBRow** | 5-byte tail jump; see `docs/reference/dbrow.md` | **[V]** |
| 0x2B | → 0x1802db760 | **SubStream** (length-prefixed byte blob) | 5-byte tail jump | shape **[V]**, name **[I]** |

For `0x22`/`0x23`, the thing to record now is that the **shared-slot
reservation happens before any child is read** — a decoder resolving references
lazily must reserve the ID in the same place or the numbering drifts.

## Gaps

1. **`0x17`/`0x22`/`0x23` payload field order** — blocks the MachoPacket
   envelope, the highest-value remaining item.
2. **`0x25`/`0x27`/`0x2C` element counts** are **[I]**, from call-site counts
   and a loop bound.
3. **`0x03`–`0x06` endianness/signedness** not re-read; only widths and the
   C-API target are verified.
4. ~~**Is `stringTableRev` ever populated?**~~ **Closed.** Yes: 195 native
   entries in `blue.dll`, and a real client sent index 135 (`machoVersion`).
5. **`0x1C`'s checksum algorithm and exact extent.**
6. **`0x0D` vs `0x13`** — identical at decode time; what makes the encoder pick
   one is unknown.
7. **`0x1A`'s discarded length** — presumably a skip hint. Unconfirmed.
8. **`0x2A`** body — other track. ~~`0x2B`~~ **closed**: it is the carrier for
   a `CallReq`/`CallRsp` pickle, an escaped length then a complete nested
   marshal stream, seen from the first real client.
9. **`0x19` as the bound-object carrier** — inferred.
10. ~~**The `0x7D` extra byte**~~ — **closed, 2026-10-20**: it is the marshal
    **version**, and a non-zero version means the header carries **no
    mapcount word**. `docs/archive/wire-format.md` gap 4 has the detail and the latent
    reader bug it uncovered.

## Tooling notes

- `tools/re/table_local.py` resolves 41 of 42 slots; the one it cannot
  (`0x13`) is a *duplicate* handler the compiler kept live in `rcx` across two
  stores — so its "no duplicates" success criterion is exactly what hid the
  answer.
- `tools/pe_functions.py --disasm` relies on `.pdata`, which **does not cover
  leaf functions**. For the 13-byte stubs, the 5-byte tail jumps (`0x2A`,
  `0x2B`) and the 7-byte `0x24`/`0x26`/`0x28` handlers it reports a
  *neighbouring* function's bounds, and `handler_profile.py` then attributes
  the neighbour's reads and imports to the handler. **Every "reads N" figure
  for a sub-16-byte handler in earlier notes should be treated as suspect** —
  that is precisely how `0x2A`/`0x2B` acquired their phantom 4-byte read.

---

## Corrections from the object-wrapper pass

`docs/reference/object-wrapper.md` settled `0x17`, `0x22`, `0x23` and `0x2D`. Working
through the two handlers turned up four things this document had wrong or
incomplete. They are recorded here with their evidence and, importantly, with
**how far each was checked** — the three marked *reported* come from that pass's
disassembly and have not been re-read independently, unlike the object-wrapper
layouts themselves, which were.

### Object forms — layouts no longer incomplete **[V]**

The section above is titled "layouts incomplete". They are not any more:

```text
0x17  <class-name string>  <state>                      — no terminator
0x22  <header tuple>  <append items...> 0x2D  <key,value...> 0x2D
0x23  <header tuple>  <append items...> 0x2D  <key,value...> 0x2D
```

The two `0x2D` bytes are **unconditional**. An exhaustive `.pdata` caller scan
found the two stream helpers (`0x1802de6b0`, `0x1802de800`) called only from the
`0x22` and `0x23` handlers, and on every success path — so `0x17` never emits or
consumes one, and `0x22`/`0x23` always carry exactly two, even when both streams
are empty.

The **dict stream is `key, value`** — the opposite of tag `0x16`, which is
`value, key`. From `PyObject_CallMethodObjArgs(obj, "__setitem__", first,
second)`. This is the easiest bug in the whole format to ship, and
`eve-marshal` has a test that pins both orders in one place so they cannot
drift apart unnoticed.

### The `0x17` class name **[V]**, re-read here

`"%s.%s"` of the class dict's `__module__` and the class's `cl_name`, formatted
at `0x1802e0142` (the literal is at `0x180963a14`), tag emitted at
`0x1802e016c`. A missing `__module__` gives *"Instance %s has no `__module__`
attribute."* The result must be in `globalsWhitelist`.

**Correction to an earlier figure here: the whitelist is 169 names, not 92.**
`carbon/common/lib/whitelist.py` builds it by `+=` from four string literals —
base 92, common 72, server 88 and client 6 — and the server blob is added only
when `boot.role == 'server'`. This build is `client`, so the real list is
92 + 72 + 6 = 170 entries, **169 unique**
(`carbon.common.script.net.moniker.Moniker` is listed twice), plus the
`exceptions.*` names `InitWhitelist` adds. The "92" was the first literal, read
and reported as if it were the whole thing. The full 169 are in
`eve-rowset::whitelist::GLOBALS`.

### `ctx+0x108` is a pointer, not the array — *reported*

`0x1802ddab6: mov rax, [r14+0x108]; inc dword [rax+rcx*4]`. So `ctx+0x108`
holds a **pointer to** the 64-entry `u32` stats array rather than being the
array itself. The save-side equivalent is `ctx+0xf0`.

### The stream member at `+0x70` — *reported, and unresolved*

On the **save** side `stream+0x70` is the **Save callback** (`0x1802dffce`,
`0x1802e0023`) and `stream+0x68` is a cached `cPickle.Pickler` (`0x1802dfa79`,
`0x1802dfad3`). This document records `0x70` as the `0x21` Unpickler. Either
the field is reused by direction, or one of the two offsets is wrong. **Left
open deliberately** — guessing which is exactly the move that has cost this
project time three times already.

### Gap 6 (`0x0D` vs `0x13`) — probably answerable — *reported* **[I]**

The writer's string branch emits `0x0E`, `0x0F`, `0x11` and `0x13`, and there
is no `mov r8d, 0xd` anywhere in `Marshal::WriteObject`. On that evidence the
client **never emits `0x0D`**, and `0x13` is the general string tag. Marked
**[I]** rather than **[V]** because only immediate tag constants inside
`WriteObject` itself were enumerated; a tag staged through a register would not
have been seen. It costs us nothing either way — the two decode through the
same handler.

### Gap 9 (`0x19`, the callback form) — mechanism settled — **implemented**

The Save callback is consulted at exactly **one** site in the entire writer:
the `0x17` branch. A non-`None` return there makes the writer emit tag `0x19`
followed by the returned object instead of the instance. So `0x19` is reachable
*only* for old-style instances. The mechanism is solid; the long-standing guess
that the returned object is a **bound-object reference** remains **[I]**.

**Now implemented** as `PyValue::Callback`, read and written: tag, then one
child, no length and no terminator, no shared registration.

**And confirmed end to end against the real client [V].** Its own log prints
the object it built from one of ours:

```text
invbroker::MachoBindObject args=(...), retval=(<RemoteObject:N=1:4>, <RemoteObject:N=1:3>)
```

Two `MachoObjectConnection`s, constructed by `ParseObjectID` from two `0x19`
objects we emitted, naming the ids we minted — and the client went on to call
methods on them. So the *reader* side of this tag is settled, which is the
side we have to satisfy. What remains strictly inferred is only whether CCP's
writer also chooses it. `eve_proto::object_reference`
builds the bound-object form; see `docs/reference/login-path.md` for the three constraints
`ParseObjectID` puts on the id and what each one breaks.
