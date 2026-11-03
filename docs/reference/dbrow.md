# `blue.DBRow` packed-row layout (client build V24.01, 3619879)

Recovered by disassembling `/client/bin64/blue.dll`. Supersedes the "DBRow
packed-byte layout" gap in `docs/archive/wire-format.md`. Confidence markers: **[V]**
verified from instructions, **[I]** inferred, **[?]** open.

Two findings here **correct earlier documents in this repo**, and both were
re-verified independently before any code changed:

1. **The marshal tag for DBRow is `0x2A`, not `0x2B`.** `docs/archive/marshal-tags.md`
   previously recorded `0x2B` as "the likeliest DBRow". Wrong.
2. **`DBTYPE_BYTES` is 128, not 508.** `const.py` says 508, which the client
   *rejects*. Our `DbType::Bytes` was 508 — a real bug, now fixed.

## 1. The tag is `0x2A`; `0x2B` is a SubStream

**[V]** Both table slots are 5-byte `jmp` thunks, not handlers:

```text
0x1802dcd90:  jmp 0x1802fe410      ; slot 0x2A -> the DBRow reader
0x1802dcd80:  jmp 0x1802db760      ; slot 0x2B -> a length-prefixed byte blob
```

**[V]** `0x1802fe410` type-checks the object it reads against the
`blue.DBRowDescriptor` type and otherwise raises
`"Hacker Warning! Invalid type in marshal data, expected blue.DBRowDescriptor."`
(`0x1809813e0`).

**[V]** Confirmed from the **writer**, re-verified here:
`Marshal::WriteObject` is at `0x1802dee00`, and its DBRow branch is

```text
0x1802df843  cmp  [r15+8], rax      ; obj->ob_type == blue.DBRow?
0x1802df84d  mov  r8d, 0x2a         ; <<< the tag
0x1802df859  call 0x1802e2310       ; WriteTag
```

with `mov r8d, 0x2b` at `0x1802df93a` for the other type, whose shape is one
length-prefixed byte blob with no descriptor and no sub-objects — a SubStream.
(The *name* SubStream is **[I]**; the shape is **[V]**.)

Worth recording: `0x1802dee00` is the function an earlier pass dismissed as "a
C-API format-character parser, wrong function". It is `Marshal::WriteObject`.
It is not Tracy-instrumented, which is why zone-name mapping did not find it.

## 2. The descriptor

**[V]** Constructor `0x1802f9a20`, parsing each element with
`"si|O:DBRowDescriptor"` — so a descriptor is built from a **tuple of
`(name: str, dbtype: int)` tuples**. The dbtype is stored as a `u16`.
Names beginning with `__` are rejected. `__reduce__` (`0x1802fa790`) re-emits
`(type, (cols_tuple,))`.

**Upgraded to [V] on the reader side.** The `0x22` handler literally does
`callable = header[0]; args = header[1]; PyObject_CallObject(callable, args)`
(`docs/reference/object-wrapper.md` §2), so the shape below is what the reader will
accept. **Two bytes are missing from the sketch as originally written: every
`0x22` carries exactly two `0x2D` terminators**, one closing the list-append
stream and one closing the `__setitem__` stream, and they are emitted even
when both streams are empty. A descriptor without them is unreadable.

A server should emit the descriptor as an **ObjectEx (tag
`0x22`)** whose header tuple is
`(<global 'blue.DBRowDescriptor'>, ((name, type), ...))`: the `0x22` handler
does `callable = header[0]; args = header[1]; PyObject_CallObject(callable, args)`,
which is exactly that call. **[V]** The DBRow reader only type-checks the
result, so re-emitting a full descriptor per row is legal (wasteful, but legal).

Shape from the client's own Python (`eveCfg.GetOwnersRowDescriptor`):

```python
blue.DBRowDescriptor((('ownerID', const.DBTYPE_I4), ('ownerName', const.DBTYPE_WSTR),
                      ('typeID', const.DBTYPE_I4), ('gender', const.DBTYPE_I2),
                      ('ownerNameID', const.DBTYPE_I4)))
```

> **Correction, and not a cosmetic one.** This example previously had
> `ownerNameID` and `gender` the other way round. The real order — re-read
> here from `GetOwnersRowDescriptor`'s constant pool, which is
> `ownerID, ownerName, typeID, gender, ownerNameID` — matters because
> `eveCfg.LoadEveOwners` **pre-fills** `Recordset.data` with DBRows packed in
> that order and pre-sets the header to match. `_Prime` then replaces the
> header with the one *we* send while leaving those pre-filled rows in place.
> So a server using the wrong order does not merely mislabel its own rows: it
> corrupts the client's own NPC data. `docs/reference/login-path.md` §8 had it right.

## 3. DBTYPEs and size classes

**[V]** Two independent switch tables agree (`0x1802f9530`, and inline in the
layout builder `0x1802f9c60`). Both cover `0x00..0x87` only; above 135 →
`"DBRowDescriptor doesn't support data type %d"`.

| DBTYPE | value | class | blob bytes | read as |
|---|---|---|---|---|
| EMPTY | 0 | −1 | 0 | virtual column |
| I2 | 2 | 2 | 2 | signed |
| I4 | 3 | 3 | 4 | signed |
| R4 | 4 | 3 | 4 | f32 → float |
| R5 (=R8) | 5 | 4 | 8 | f64 |
| CY | 6 | 4 | 8 | i64 **divided by 10000.0** |
| BOOL | 11 | 0 | **1 bit** | `bt` → True/False |
| I1 | 16 | 1 | 1 | signed |
| UI1 | 17 | 1 | 1 | unsigned |
| UI2 | 18 | 2 | 2 | unsigned |
| UI4 | 19 | 3 | 4 | **read SIGNED** (same handler as I4) |
| I8 | 20 | 4 | 8 | `PyLong_FromLongLong` |
| UI8 | 21 | 4 | 8 | `PyLong_FromUnsignedLongLong` |
| FILETIME | 64 | 4 | 8 | as UI8 |
| **BYTES** | **128** | 5 | 0 (separate object) | object slot |
| STR | 129 | 5 | 0 (separate object) | object slot |
| WSTR | 130 | 5 | 0 (separate object) | object slot |
| DBTIMESTAMP | 135 | 4 | 8 | **unreadable — see below** |

Class → size: `1→1, 2→2, 3→4, 4→8, 5→object slot, 0→one bit, −1→nothing`.

### Traps **[V]**

- **`DBTYPE_BYTES` is 128.** The native `blue.db` init does
  `PyModule_AddIntConstant(m, "DBTYPE_BYTES", 0x80)` at `0x1802fe989`, with
  neighbours `0x81`/`0x82` for STR/WSTR. `const.py`'s 508 exceeds `0x87` and is
  **rejected by the constructor**. Re-verified here.
- **A `DBTIMESTAMP` (135) column can be declared but never read.** The getter's
  jump table covers only `0x00..0x82`
  (`0x1802fc119: cmp r8d, 0x82; ja` → `"Unexpected db column type encountered: %d"`).
  **Never emit one; use FILETIME.**
- **`UI4` is sign-extended on read** — values ≥ 2^31 come back negative.
- **`CY` is divided by 10000.0** — store scaled integers.

## 4. The layout

**[V]** From the builder at `0x1802f9c60`. With `n8/n4/n2/n1` the counts of
8/4/2/1-byte columns, `nB` BOOLs, `nO` object columns, `nE` EMPTY:

```text
offset[8-byte] = 0
offset[4-byte] = 8*n8
offset[2-byte] = 8*n8 + 4*n4
offset[1-byte] = 8*n8 + 4*n4 + 2*n2
F              = 8*n8 + 4*n4 + 2*n2 + n1        ; fixed-scalar area, bytes

boolBitCursor  = 8*F
nullBitBase    = 8*F + nB
blobSize       = ceil((nullBitBase + nNonEmpty) / 8)
objBase        = roundup8(blobSize)              ; if nO != 0
```

Then, **in declaration order**, each column takes the next slot in its class's
region; BOOLs take the next bit; object columns take the next 8-byte slot after
the blob; EMPTY takes nothing.

**[V] Columns are not reordered.** The blob is partitioned into size-class
*regions*, largest first, and **within a region columns keep declaration
order**. The usual "largest first" phrasing is right as a region ordering and
wrong if read as a column permutation.

```text
[ 8-byte cols ][ 4-byte ][ 2-byte ][ 1-byte ]   = F bytes
bit 8*F + k            BOOL column k (declaration order)
bit 8*F + nB + i       NULL flag for column index i
blobSize = ceil((8*F + nB + nNonEmpty)/8)
```

- Multi-byte scalars are **little-endian** (plain loads, no byte swap). **[V]**
- **Bit numbering is LSB-first**: `byte = idx >> 3`, `bit = idx & 7`, `bt`. **[V]**
- STR/WSTR/BYTES occupy **no blob bytes** — only a NULL bit; values are
  appended to the stream as separate objects. **[V]**
- EMPTY columns occupy nothing, not even a NULL bit. **[V]**

### The NULL bitmap — a third region usually omitted **[V]**

`0x1802fc0b0` opens, before any type dispatch:

```text
cmp  r8, [r10+0x94]        ; columnIndex >= nullBitCount -> no NULL bit
movsxd rax, [r10+0x80]     ; nullBitBase
add  rax, r8               ; + columnIndex
bt   ecx, eax              ; set -> return Py_None
```

**A set bit means the column reads as `None` whatever the bytes say.** An
encoder must size the blob to include this region; leaving the bits clear falls
out of the zero-fill, so the practical requirement is just the **size**.

## 5. Blob compression is nibble RLE **[V]**

Decoder `0x1802fe210`/`0x1802fe0f0`, encoder `0x1802fdf60`. Control bytes;
**low nibble first**, then high:

```text
n < 8   -> copy 8-n literal bytes
n >= 8  -> emit n-7 zero bytes
```

Overflow past `blobSize` → `"Invalid RLE string"`. **The reader takes the true
length from `descr->blobSize`, not from the stream**: a short blob is simply
zero-filled to size. Trailing zero-nibble runs *are* trimmed by the encoder
(whole output bytes only), which is why that works.

A conforming encoder may skip the trim entirely — it is pure size optimisation —
but must never decode to more than `blobSize` bytes.

## 6. The wire form **[V]**

Reader `0x1802fe410`, writer branch `0x1802df83e`. They agree field for field.

```text
u8      tag          0x2A (| 0x40 for the shared flag)
...     descriptor   one complete marshal object; must be a blue.DBRowDescriptor
u8/u32  blobLen      1 byte if <= 0xFE, else 0xFF + u32 LE
u8[]    blob         nibble-RLE of the packed row
...     values       one marshal object per class-5 column, declaration order
```

## 7. Rowsets are pure Python **[V]**

`crowset.py`: `CRowset(list)` with `.header`, `CIndexedRowset(dict)`,
`CFilterRowset(dict)`, all `__passbyvalue__`. `ROWSETVERSION = 3` appears only
in the *disk* format, **not** on the network. `CRowset` indexes rows by
position via `header.Index(col)`, so the column order the server declares is
the order the client uses.

### `0x2D` is the ObjectEx stream terminator **[V]**

This resolves a standing puzzle: `0x2D` has a NULL handler slot because **it is
not a type** — it terminates ObjectEx's append and setitem streams.
`0x1802de6b0` loops `ReadObject` → `obj.append(item)` until it *peeks* `0x2D`
(`0x1802de701: cmp cl, 0x2d`), advancing the cursor only then. So a `CRowset`
goes out as:

```text
22  <header: (global 'dbutil.CRowset', (), {'header': descr})>
    <DBRow> <DBRow> ... 2D      ; the list stream
    2D                          ; the (empty) dict stream
```

## 8. Gaps

1. ~~**[?] No capture validates any of this.**~~ **CLOSED [V].** A real
   client read one of our packed rows back, field for field. The character's
   `invItem` in the dogma prime, as the client's own log renders it:

   ```text
   UpdateItem: <DBRow object [90000001L, 1373, 90000001, 60000004L, 0, 1, 1, 1, 1, 1]>
   Godma:UpdateItem - Inserting item 90000001
   ```

   against the descriptor it had asked us for a moment earlier:

   ```text
   invbroker::GetItemDescriptor args=(), retval=<DBRowDescriptor object
     [('itemID', 20, 8), ('typeID', 3, 4), ('ownerID', 3, 4),
      ('locationID', 20, 8), ('flagID', 2, 2), ('quantity', 3, 4),
      ('groupID', 3, 4), ('categoryID', 3, 4), ('stacksize', 3, 4),
      ('singleton', 3, 4)] 48>
   ```

   Every value came back as sent, the two `I8` columns as Python longs, and
   the row survived `UpdateItem`'s category filter — which it only does when
   `groupID` reads as exactly `groupCharacter`, so the field offsets are
   right and not merely plausible. The descriptor's own byte total (48) and
   size classes match what we computed. So the packed layout, the size
   classes and the column ordering are all confirmed against the real thing.

   Still open within this: the **NULL bitmap** (gap 3) was not exercised —
   that row has no nulls.
2. **[?] Which tag the client's own writer uses for a descriptor.** The reader
   accepts `0x22` unambiguously, so a server can emit it safely; what the
   client emits is unresolved (several writer branches load the tag through a
   register, and there is a cPickle fallback at `0x21`).
3. **[?] The setter side of the NULL bitmap** — "set bit means None" is
   verified from the getter only.
4. **[V/I] EMPTY/virtual columns** (lane DBROW-SINGLETON, read from
   `blue.dll` 3619879). What is established:
   - A virtual column is read through `VirtualGet` (`0x1802fc257`, the
     `DBTYPE_EMPTY` arm of the getter): it needs the descriptor's
     `mVirtualGetSet` (`[descr+0x98]`) to be a *list*, takes the entry at the
     column's slot, requires it to be a tuple of two or more, and calls
     **`entry[1](row)`**. Unset, it raises "Internal error in VirtualGet; no
     mVirtualGetSet" (`0x180980fc0`). So an EMPTY column declared without
     `.virtual` makes `item.singleton` raise, which is worse than today.
   - The constructor (`0x1802f9a20`, `"si|O"` per column) **ignores** the
     optional third element; nothing in the client's Python assigns
     `.virtual` except `godma.sublocrd`, which copies it from `itemrd`.
   - `__reduce__` (`0x1802fa790`) builds `(type, (cols,), virtual)`
     (`"O(O)O"`), with `cols` holding the columns *minus* `len(virtual)`:
     the virtual list travels as the **state** (third header element) and
     the virtual columns are re-created from it. This is the likely wire
     route [I]; the setstate half and the entry format were not read.
   - The EMPTY-last case is safe for the NULL bitmap: the bitmap is sized by
     non-EMPTY columns and the getter skips the bit when
     `columnIndex >= nullBitCount`, so trailing EMPTY columns (CCP's 9 and 10)
     do not trip the hazard in `DbRowError::EmptyColumn`.
   - Why it matters: `godma.sublocrd`/`subloc_internalrd` drop columns with
     `dbtype == 0 and size == 0`, so CCP's `itemrd` gives a **nine**-column
     sublocrd, matching godma's nine-value `blue.DBRow(...)` calls. Ours has
     eleven physical columns, so each charge in a module logs
     "sequence is too short" (about 800 per run). The row is still made.
   - Open: the entry format (likely `('stacksize', eveCfg.StackSize)` and
     `('singleton', eveCfg.Singleton)`, the client's own legacy helpers, sent
     as marshalled globals) and whether the reader accepts it. Needs a
     real-client experiment; until then keep eleven physical columns.
   - **Live-ammo (2026-10-29): the descriptor half is now sent.**
     `invbroker.GetItemDescriptor` answers CCP's shape: nine physical columns
     and the header's third element `[(stacksize, eveCfg.StackSize),
     (singleton, eveCfg.Singleton)]` (both in the client's marshal
     whitelist). Reason: with eleven physical columns godma's
     `subloc_internalrd` kept a physical `stacksize` it never fills, so every
     HUD charge read `stacksize = None` (label 0, Ctrl+R `TypeError` in
     `GetChargeReloadInfo`), and `sublocrd` rows logged "sequence is too
     short". **Rows stay on eleven physical columns**: a virtual `Singleton`
     would read 0 for modules (retail encodes singleton as a negative
     quantity). Still `[A]`, pending the client CI scenario `fci_ammo_count`:
     A1 the third element is the setstate that recreates the virtual columns
     (falsifier: login hang at `invCache.GetItemHeader`, "VirtualGet" or a
     `godma.sublocrd` traceback); A2 `len(itemrd)` counts the virtual columns,
     so `invCache.List`'s length check still accepts eleven-column rows
     (falsifier: empty hangar, 'Invalid Item'); A3 `(name, function)` is the
     whole entry format (same symptoms as A1).
   `service::item_row_descriptor()` is the one descriptor all item rows use,
   so the switch is a one-place change; `service::tests::item_row_shape`
   pins the shape and the nine-value mismatch.
5. **[?] FILETIME epoch** — returned as a raw u64 with no conversion. Windows
   FILETIME (100 ns since 1601) is the obvious reading, unevidenced here.
6. **[?] The exact ObjectEx header triple** for a `__guid__`-bearing
   `list`/`dict` subclass, and whether `0x22` or `0x23` carries it.
7. **[?] Shared-object interaction** — re-emitting the descriptor per row is
   believed safe but unconfirmed against a real client.
