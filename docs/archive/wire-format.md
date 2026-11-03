# machoNet wire format (client build V24.01, 3619879)

> **Demoted 2026-10-24** — gap 3 is closed and the DBRow gap is superseded. Current truth: [docs/reference/framing.md](../reference/framing.md), [docs/reference/dbrow.md](../reference/dbrow.md). Kept for history; do not brief a lane from this file.

What a server must reproduce to talk to this client. Everything here marked
**confirmed** was read out of this build (decompiled `.pyc` or `blue.dll`
strings); everything marked **gap** is native in `blue.dll`/`carbonio` and
needs a live capture or a disassembly pass to pin. The confirmed parts are
encoded in `server/crates/eve-marshal/src/constants.rs`; the gaps are the
agenda for milestone M1 (the first live capture).

## Layers

```
  TCP stream
   └─ length-prefixed frame            [gap: prefix width/endianness — native carbonio]
       └─ optional zlib                [confirmed: first-byte discriminator + thresholds]
           └─ marshal ("PyRep") blob   [confirmed: structure; gap: numeric opcodes]
               └─ MachoPacket object   [confirmed: field order + command ids]
```

## Marshal blob — structure and tag space confirmed by disassembly

- The codec is 100% native: the Python layer only ever calls
  `blue.marshal.Save(obj, callback)` and `blue.marshal.Load(data,
  skipCrcCheck=True)` (confirmed in `machobase.pyc`: `Dumps`/`Loads`/
  `DumpsSanitized`). No Python reimplementation ships, so the **numeric tag
  byte values are not in the `.pyc` tree** and are compiled constants inside
  `blue.dll`'s `Marshal.cpp` (`Marshal::ReadObject`, `SaveCallback`, ...).
- **Header layout — established** by disassembling `Marshal::Load` at
  `0x1802e1670`:

  ```text
  u8  marker     0x7D or 0x7E, else "invalid marshal header"
  u8  version    ONLY when marker == 0x7D; a 0x7E marker sets this to 0
                 without consuming a byte
  i32 mapcount   little-endian, and ONLY when version == 0; negative ->
                 "invalid mapcount in marshal stream, %d."
  ```

  **The mapcount is conditional, and that was a bug in our reader until
  2026-10-20** -- see the gap list below. `0x7D` is the *versioned* form, not
  an alternate magic.

  The stream must then hold at least `mapcount * 4` further bytes, or Load
  fails with "Too little data in marshal stream … mapcount in header is %d".
  The mapcount width is confirmed: the helper at `0x1802e5f90` requires 4
  bytes and does `mov ecx, dword [r9 + rax]`.

  **Correction: that table is not stream offsets.** Each `u32` is a **1-based
  shared-object ID**, consumed in order: `RegisterShared` (`0x1802dbdf0`) does
  `slot = table[n] - 1` with its own counter `n`. So the k-th shared-flagged
  object in stream order claims ID `table[k]`, and a decoder must keep that
  counter rather than assuming ID equals position. Only eleven type codes
  consume an entry — see `docs/archive/marshal-tags.md`; a shared-flagged `int32` consumes
  nothing.

  **There is no CRC in the header** — an earlier pass inferred one because
  "header crc mismatch" sits near the marshal strings, but that message is
  referenced only from `0x180579a50`, far outside the marshal cluster, while
  `Marshal::Load` — the actual header validator — references no CRC string.
  That matters because the nested call payload is loaded *without*
  `skipCrcCheck` (`ObjectCallGPCS.FromPickle`), which would otherwise have
  forced us to reproduce a header CRC to build any call packet.

  **But there is a CRC in the protocol**, as type code `0x1C`: a `u32`
  checksum followed by one object, verified unless `skipCrcCheck` is set. So
  the keyword does correspond to something — just to that tag, not to the
  header. Details and the two open questions about it are in
  `docs/archive/marshal-tags.md`.

  The `0x7D` byte is the **marshal version**, and the encoder emits the
  `0x7E` form because that is what the client itself emits. See the gap list.
- A **shared-object** mechanism and a **string-table** optimization are both
  in use (`blue.dll`: `stringTable`, `stringTableRev`, `DumpStringTable`).
### The dispatch, read off the instructions

`Marshal::ReadObject` is at `0x1802dd980` in `blue.dll` (self-identified by its
Tracy zone name; `Marshal.cpp` is the source string at `0x18097cbd0`). Its
dispatch is:

```text
movsx ecx, byte [stream]          ; the tag byte
and   ecx, 0x3f                   ; type index = tag & 0x3F
inc   dword [stats + idx*4]       ; a 64-slot per-type counter
shr   eax, 6 / and al, 1          ; bit 6 = the shared/save flag
movups xmm0, [0x180b015f0 + idx*16]
movq  r9, xmm0 ; test r9,r9 ; je  -> "Invalid type tag %d in stream."
call  r9                          ; handler(ctx, stream, shared_flag)
```

**Confirmed, and now encoded in `eve-marshal::tag`:**

- A tag byte is a **6-bit type code plus one flag bit**: `type = tag & 0x3F`,
  `shared = tag & 0x40`. **Bit 7 is discarded.** The 64-slot statistics array
  corroborates the 6-bit space.
- Dispatch is a **function-pointer table** of 64 × 16-byte entries
  `{handler: u64, offset: i32}` at `0x180b015f0`, populated at init by
  `0x1802d9570` (it lives in the uninitialized tail of `.data`, so it is empty
  on disk).
- **42 type codes are valid**, spanning `0x01..0x2F`. The six codes
  `0x00, 0x0C, 0x18, 0x1D, 0x1E, 0x2D` have an explicitly NULL slot and are
  rejected with "Invalid type tag %d in stream."
- Reconstruction method: strictly local backward pairing of each
  `movaps [rip+slot], xmm0` store to the handler staged for it (the initializer
  is software-pipelined, so naive FIFO zipping drifts — it produced a bogus
  `.rdata` "handler" and a duplicate). The local method resolved 41 of 42 with
  **no duplicates**; code `0x13` stages its pointer through `rcx` and is
  unresolved. Tooling: `tools/re/table_local.py`.

### Type codes identified from their handlers

Each handler was fingerprinted by the Python C-API it calls, the singletons it
references, its read widths, and whether it recurses into `ReadObject` (which
separates containers from scalars). **26 of the 42 codes are named on that
evidence** and are encoded in `eve-marshal::tag::code`:

| code | evidence | meaning |
|---|---|---|
| 0x01 | references `_Py_NoneStruct` | None |
| 0x03 | reads 8, `PyLong_FromLongLong` | int64 |
| 0x04 | reads 4, `PyInt_FromLong` | int32 |
| 0x05 | reads 2, `PyInt_FromLong` | int16 |
| 0x06 | reads 1, `PyInt_FromLong` | int8 |
| 0x0A | reads 8, `PyFloat_FromDouble` | f64 |
| 0x0F | reads 1, `PyString_FromStringAndSize` | string |
| 0x10 | `PyString_FromStringAndSize` | string, second form |
| 0x12 | `PyUnicodeUCS2_DecodeUTF16` | unicode |
| 0x14 | `PyTuple_New`, recurses | tuple |
| 0x15 | `PyList_New`, recurses | list |
| 0x16 | `PyDict_New`, recurses twice per entry | dict |
| 0x17 | `PyClass_Type`/`PyObject_Type`/`PyObject_Repr` | old-style instance |

| 0x1F | references `_Py_TrueStruct` | True |
| 0x20 | references `_Py_ZeroStruct` | False |
| 0x22 | `PyObject_CallObject`/`GetAttr` | object, 2.2 form |
| 0x23 | `PyObject_CallObject`/`GetAttr` | object, 2.3 form |
| 0x24 | `PyTuple_New`, no recursion | empty tuple |
| 0x25 | `PyTuple_New`, one recursion | 1-tuple |
| 0x26 | `PyList_New` | list, second form |
| 0x27 | `PyList_New`, one recursion | 1-list |
| 0x28 | `PyUnicodeUCS2_FromString` | unicode |
| 0x29 | `PyUnicodeUCS2_DecodeUTF16`, reads 2 | unicode |
| 0x2C | `PyTuple_New`, reads 2, recurses | 2-tuple |
| 0x2E | `PyUnicodeUCS2_DecodeUTF8` | unicode, UTF-8 |
| 0x2F | `PyLong_FromLong`, variable-length read | long |

**The "reads 2" / "reads 1" once recorded for `0x22`/`0x23` were a misreading.**
Those constants are `PyTuple_GetItem` *indices*, not stream byte counts:
neither code reads a single raw byte beyond its tag. The figures are removed
from the table above rather than left to mislead.

This reproduces the canonical PyRep assignment where the two overlap — so the
canonical table is *this build's* table, established from the binary rather
than assumed.

**Still unnamed, deliberately.** `0x07, 0x08, 0x09, 0x0B, 0x0E` are 13-byte
stubs that read nothing and return a constant; canonically minus-one / zero /
one / real-zero / empty-string, consistent with the evidence but not
individually proven, so they are `CONSTANT_STUB_CODES`. `0x02, 0x0D, 0x11,
0x19, 0x1A, 0x1B, 0x1C, 0x21` build objects through CCP's own helpers rather
than the C-API, so fingerprinting is weak. `0x13`'s handler address is
unresolved (staged via `rcx`). All are recorded with their evidence in
`eve-marshal::tag::HANDLERS`; one live capture names them by example.

**Correction.** An earlier revision of this section listed `0x2A` and `0x2B`
among the unidentified codes and called `0x2B` "the likeliest DBRow", on the
strength of it calling two internal helpers. That was inference, and it was
wrong. **DBRow is `0x2A`**, proven by `mov r8d, 0x2a` at `0x1802df84d` inside
`Marshal::WriteObject`; `0x2B` is a SubStream. See `docs/reference/dbrow.md`. The wrong
guess is left on the record here because it is the third time this build has
punished a plausible fingerprint — the rule that came out of it is in
`docs/archive/marshal-tags.md`: read the writer's immediate operand, do not reason
from the reader's call shape.

### String table — confirmed, build-specific

`eve/common/lib/marshalstrings.py` `rawstrings`: **210 entries**, 1-based. The
string-table-indexed opcode carries an index `1..210`; the string is that
entry. Extracted verbatim into `server/crates/eve-marshal/src/string_table.rs`
(`tools/extract_string_table.py`). Entry 1 = `*fleetid`, entry 210 = `wingid`.
A different build ships a different table; regenerate.

## DBTYPE column codes — confirmed

From `const.py`. Validated by `blue.dll`: `DBRowDescriptor doesn't support data
type %d`.

| name | value | | name | value |
|---|---|---|---|---|
| EMPTY | 0 | | UI1 | 17 |
| I2 | 2 | | UI2 | 18 |
| I4 | 3 | | UI4 | 19 |
| R4 | 4 | | I8 | 20 |
| R5 (8-byte real) | 5 | | UI8 | 21 |
| CY (currency i64) | 6 | | FILETIME | 64 |
| BOOL (bit-packed) | 11 | | DBTIMESTAMP | 135 |
| I1 | 16 | | STR | 129 |
| | | | WSTR (utf-16) | 130 |
| | | | BYTES (varlen) | 508 |

**Gap:** the DBRow *packed-byte* layout (columns emitted largest-scalar-first,
bools bit-packed, varlen values appended, trailing-zero compression on the
fixed area) is native and consistent with these codes but not byte-confirmed
from this build. Confirm at M1. `CRowset`/`CIndexedRowset`/`CFilterRowset`
(`dbutil.*`, `__passbyvalue__`) wrap a DBRowDescriptor header + list of DBRow;
`ROWSETVERSION = 3` (`crowset.pyc`).

## Packet envelope — confirmed

`MachoPacket.__getstate__` is a 14-tuple in this exact order
(`machoNetPacket.pyc`): `command, source, destination, userID, body, oob,
contextKey, journeyID, trace_id, parent_span_id, sampled, ingress_id,
sample_rate, cut_parent`. `body` is built from the subclass's `params`; a param
named with a trailing `?` is optional and, if absent, is dropped from the end
of `body` (so an optional param must be last). `oob` is `None` on the wire
unless `compressedPart` is set, when it becomes `{'compressedPart': ...}`.
These are old-style classes, and they go on the wire as marshal tag **`0x17`**:
a class-name **string**, then the state object. No `__slots__`, no terminator,
no append streams.

**Correction — this said `(guid-object, state)`, and that was wrong.** The name
is not a guid object. The `0x17` writer at `0x1802dff60` looks `__module__` up
in the class dict, and if it is missing fails with *"Instance %s has no
`__module__` attribute."*; otherwise it takes the `ob_sval` of `__module__` and
of the class's `cl_name` (`+0x20` on each) and formats them with
`"%s.%s"` — the literal at `0x180963a14` — before emitting tag `0x17` at
`0x1802e016c`. **[V]**, read off the bytes here and not taken on report.

So the name of a `CallRsp` on the wire is the full dotted path
`carbon.common.script.net.machoNetPacket.CallRsp`, and **it must appear in
`globalsWhitelist`** or the reader raises `HACKER WARNING! object %s.%s is not
in whitelist`. The whitelist is **169 unique names** for a client build, and
the one thing to notice about it is that
**`MachoPacket` itself is absent** — only its eleven subclasses are listed, so
the base class can never be named on the wire.

`__module__` can be overridden in a class body, so a name is not always
derivable from the source file's path. Take names from the whitelist text.

Two further things this makes true, both verified independently here:
`class MachoPacket():` compiles with `BUILD_TUPLE 0` — an empty bases tuple,
hence old-style in Python 2.7 — and so does `class MachoAddress():`, which also
defines `__getstate__`/`__setstate__`. A packet's `source` and `destination`
fields therefore hold **wrapped `0x17` instances**, not bare state tuples.

Full layouts for `0x17`, `0x22`, `0x23` and the `0x2D` terminator are in
`docs/reference/object-wrapper.md`. All three are implemented in `eve-marshal`, and
`eve-proto::MachoPacket::to_payload` emits a complete packet.

Command ids (`MACHONETMSG_TYPE_*`, from `cluster/__init__.py`) and per-subclass
`body` params:

| class | guid | command | params |
|---|---|---|---|
| AuthenticationReq | — | 0 | (native/early) |
| AuthenticationRsp | — | 1 | (native/early) |
| IdentificationReq | macho.IdentificationReq | 2 | `[2,3,4,5,6,7]` |
| IdentificationRsp | macho.IdentificationRsp | 3 | `[2,3,4,5,6,7]` |
| CallReq | macho.CallReq | 6 | `['payload?']` |
| CallRsp | macho.CallRsp | 7 | `['payload']` |
| TransportClosed | macho.TransportClosed | 8 | `['clientID','isRemote']` |
| ResolveReq | — | 10 | |
| ResolveRsp | — | 11 | |
| Notification | macho.Notification | 12 | `['payload?']` |
| ErrorResponse | macho.ErrorResponse | 15 | `['originalCommand','code','payload']` |
| SessionChangeNotification | macho.SessionChangeNotification | 16 | `['sid','change','nodesOfInterest?']` |
| SessionInitialStateNotification | macho.SessionInitialStateNotification | 18 | `['sid','sessionType','initialstate']` |
| PingReq | macho.PingReq | 20 | `['times']` |
| PingRsp | macho.PingRsp | 21 | `['times']` |

The Identification params are the literal integers `2..7` used as attribute
names via `getattr`/`setattr`, not strings — reproduce as-is.

`MachoAddress` (`macho.MachoAddress`) `__getstate__` shape depends on
`addressType` (NODE=1, CLIENT=2, BROADCAST=4, ANY=8):

- CLIENT: `(2, clientID, callID, service)`
- BROADCAST: `(4, broadcastID, narrowcast_list, idtype)`
- ANY: `(8, service, callID)`
- NODE: `(1, nodeID, service, callID)`

## Compression — confirmed (Python path)

From `machoNetTransport.pyc`, effective thresholds for this build:

- `minimumBytesToCompress` = 200
- `maxPercentagePreCompressedToCompress` = 75
- `dropPacketThreshold` = 5_000_000 (packet dropped above this)

**Write:** compress only if the transport is not `tcp:packet:machoNet`, the
pre-compressed fraction is < 75%, the uncompressed remainder is > 200 bytes,
and compression isn't globally disabled. When eligible, `zlib.compress(data,
1)` — **level 1** on this software path (the pref'd level 6 is IOCP-only). Then
send compressed only if it helped: not if it grew, and not if the gain was
≤ 5%.

**Read — the concrete discriminator:** the first byte decides. If
`data[0] not in '}~'` (i.e. not `0x7D` and not `0x7E`) the stream is
zlib-compressed and inflated before Load. So an **uncompressed marshal stream
begins with `0x7D` or `0x7E`** in this build; a zlib stream begins `0x78…`.
Two valid leading markers — the `}` vs `~` distinction is a **gap** (likely
with-map vs without-map, or a version flag).

## See also

- `docs/archive/marshal-tags.md` — the full type-code table: all 42 codes, 26 with
  encodable payload layouts, the shared-object mechanism, the gaps.
- `docs/reference/framing.md` — the TCP frame, the size caps, and the corrected layer stack
  (there is a symmetric-encryption layer between frame and marshal blob).
- `docs/reference/dbrow.md` — the DBRow packed-row layout and the descriptor form.

Note on the `header crc mismatch` string: the 6.5 KB function referencing it,
`0x180579a50`, **is zlib's `inflate`** — it also references `incorrect header
check` and `unknown compression method`. The message is zlib's own
gzip-header-CRC text, which confirms there is no CCP header CRC. The earlier
conclusion was right; this is the reason.

## Gaps → M1 agenda

1. ~~Numeric marshal opcode byte values~~ — **done by disassembly**: the tag
   space, the mask/flag split, the valid set and six proven codes are settled.
   What remains is *naming* the other 31 codes, for which a capture is the
   cheap oracle.
2. ~~DBRow packed-byte layout~~ — **done by disassembly**, see `docs/reference/dbrow.md`.
   The tag is `0x2A` (not `0x2B`), columns are partitioned into size-class
   regions largest-first while keeping declaration order within a region,
   there is a third NULL-bitmap region, and the blob is nibble RLE rather than
   trailing-zero trimmed.
3. ~~On-wire length-prefix framing~~ — **done by disassembly**, see
   `docs/reference/framing.md`: `u32` little-endian, low 28 bits the length (excluding
   itself), bits 28/29 OOB and zlib, which machoNet never sets. Implemented in
   `eve-proto::frame`.
4. ~~`0x7D` vs `0x7E` marker meaning~~ — **CLOSED, 2026-10-20.** `0x7D` is
   not an alternate magic; it is the **versioned** form.
   `blue.marshal.Save(object, callback=None, useChecksum=0, version=0,
   stringMap=None)` writes `0x7E` plus a `uint32` shared-object mapcount when
   `version == 0`, and `0x7D` plus a **one-byte version and no mapcount word**
   otherwise (writer `0x1802da8f8`, reader `0x1802e1728`). So the three
   streams the client accepts are `0x7E …`, `0x7D 00 <mapcount> …` and
   `0x7D <v≠0> …`, and **the mapcount is present exactly when the version is
   zero**.

   This client only ever *writes* `0x7E`, because `machobase.Dumps` passes no
   `version` — which is why the gap sat open so long and why the bug it hid
   was latent. `take_header` read the mapcount unconditionally, so a
   version-1 stream would have had four bytes of its **body** consumed and
   failed somewhere unrelated. Fixed, with a test; the test that was there
   asserted the broken behaviour.

   Found by the `evejs-crosscheck` lane. `useChecksum` is the same function's
   writer-side switch for tag `0x1C`.
5. Nested call payloads are complete marshal streams carried as bytes
   (`ToPickle` = `blue.marshal.Save(value, GetObjectID)`), so the codec
   recurses into them with no new mechanism — but the save callback that
   encodes bound-object references is native, and which tag it emits is
   unknown. `0x2B` is now known to be a SubStream (`docs/reference/dbrow.md`), which
   makes it the obvious candidate, but that is still an inference.

**Where this stands now.** Gaps 1, 3, 4 and the object wrapper are closed by
disassembly; gap 5 is narrowed. `docs/reference/object-wrapper.md` has the three layouts
and the `0x2D` terminator, `eve-marshal` implements all three codes, and
`eve-proto::MachoPacket::to_payload` emits a complete packet — a thing that
could not be written at all a few hours ago.

What remains is not a *format* gap but an unverified one: nothing here has been
checked against the real client. Every layout is read off the client's own
reader and writer, which is strong evidence, but it is still our reading.

**Softened rather than struck, 2026-10-20.** A third-party emulator for the
same client *version* was read statically as an independent second reading
(`re/evejs-crosscheck.md`), and on framing and the marshal header it agrees
with us: `uint32` LE length with flags in the high bits, `0x7E` payload magic,
value-before-key dict ordering. On the frame cap it agrees with the *client*
and not with itself — `MAX_FRAME = 10_485_760` turns out to be
`SocketGPS.MaxPacketSize`, which we had copied correctly and it had not.

That is **evidence the format is stable across builds, not a capture.** The
lane that produced it argued for this wording against its own result, and the
reason is worth keeping: of 31 claims checked, 19 agreed — and a run of
agreements is exactly the condition under which the twentieth gets waved
through. Two of its eight disagreements read like protocol facts and were not.
So the sentence above stands; it is merely better supported than it was.

A second practical point: the **GPS handshake carries no objects at all**. H1..H10 (`docs/reference/login-path.md` §4) are plain tuples, dicts,
ints, reals and strings, so the server can complete a client's entire login
without the wrapper — see `docs/reference/server.md`. The wrapper blocks the traffic
*after* authentication, and a client that gets that far will name the tag
itself: `eve-server` decodes post-handshake payloads purely to report which tag
it could not read.
