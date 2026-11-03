<!-- Provenance: the `evejs-crosscheck` lane, 2026-10-20. Acts on
     `docs/archive/evejs-261018-reference.md` §9. Read-only: nothing outside this file
     and `docs/lanes/evejs-crosscheck.md` was written; no Rust was changed.
     The EvEJS archive was NOT extracted and nothing was executed from it --
     every verdict below is settled against the client, which is the only
     authority this document recognises. Line numbers for client Python come
     from `co_lnotab` (`tools/call_lines.py` / `tools/pyc27.py`), never from
     uncompyle6's output. Native addresses are VAs in
     `/client/bin64/blue.dll`, ImageBase `0x180000000`, disassembled with
     `tools/pe_functions.py`. Marks: **[V]** read off this build's own bytes,
     **[I]** inferred, **[unknown]** not established. -->

# EvEJS 261018.1 cross-check: what the client says

Client build **V24.01 / 3619879**. EvEJS targets **24.01 / 3396210** — same
version, *different build*; §6 below says what that does to every verdict.

This lane takes the claims in `docs/archive/evejs-261018-reference.md` as hypotheses
and settles each one against the client's own reader and writer. **EvEJS is
nowhere the citation.** Where a verdict says *agree*, it means two independent
readings of the same client arrived at the same answer — a hypothesis that
survived, not a proof.

---

## 1. The verdict table

`claim` is EvEJS's, as reported in `docs/archive/evejs-261018-reference.md`.
`client says` is this build, with the address or `file.py:line` that settles
it.

**31 claims: 19 agree, 8 disagree, 3 partly agree, 1 unknown.**

The one unknown is deliberate and is row #31: this lane did not extract the
archive, so the only opcode assignments compared are the seven the handoff
document names plus the two range endpoints. A full table-against-table
comparison was not done and is not worth doing — ours is read off this
build's dispatch table, and theirs descends from EVEmu's Crucible-era work.

### Framing — GAP 1

| # | claim | our reading | client says | verdict | citation |
|---|---|---|---|---|---|
| 1 | frame is `[uint32 LE payloadLength][payload]` | `word0` = 28-bit length + 4 flag bits | both, and they are compatible: with all flags zero `word0` **is** the payload length | **partly agree** — true of every machoNet frame, false as a description of the field | `0x180388640`, `0x180388701`, `0x18038873e`: `and e{cx,di,ax}, 0xfffffff` |
| 2 | the prefix is 4 bytes, little-endian | same | same | **agree** | three mask sites above; `lea edi, [rax+4]` at `0x180388743` |
| 3 | the prefix does not count itself | same | same: the cap compares `plen + 4` | **agree** | `0x180388743` → `0x180388768 cmp edi, r9d` |
| 4 | "no OOB channel in the framing itself" | bit 28 = `OOB_PRESENT`, which machoNet never sets | an OOB section **exists and is decoded on receive**; machoNet never *sends* one | **disagree** on the field, **agree** on the traffic | `0x1803887aa test dword [rax], 0x10000000`; `machoNetTransport.py:493` is `self.transport.Write(thePickle)`, `CALL_FUNCTION 1` — no `header` arg |
| 5 | cap is `payloadLength > 0 && <= 10_000_000` | `MAX_FRAME = 10_485_760` on `plen + 4` | **10,485,760**, and on the frame *including* the header | **disagree** — 10,000,000 is in neither this client nor ours | `SocketGPS.py:133` `self.MaxPacketSize = 10485760`; installed at `:202` (`Connect`) and `:277` (`Accept`); enforced at `0x180388761` |
| 6 | a cap violation destroys the socket | we return `DeclaredTooLong` and refuse | **the two client paths differ**: carbonio *receive* logs `packet too large on [%p] [%d>%d]` and **discards and continues** (the stream desynchronises); the legacy path raises and `SocketGPS` closes | **disagree** as a description of the client | `0x180388a08`-`0x180388a26` (log, `[rax+8]=0`, continue); `0x180391ff0` owns *"too large a packet detected at %d bytes, max is %d"*, caught by substring at `SocketGPS.py:636` |
| 7 | one accumulation buffer, frames drained strictly in order, partials buffered | same | same: `<4` bytes buffer and return, 1..3 header bytes take only the missing ones, incomplete body appends, complete body consumes and **loops** | **agree** | reader `0x1803885a0`; the three mask sites are the three resume points |

### Marshal — GAP 2

| # | claim | our reading | client says | verdict | citation |
|---|---|---|---|---|---|
| 8 | payload is `[0x7E][u32 LE mapCount][value][mapCount × u32 LE shared ids]`, ids **after** the value | identical, including the tail position — which we had wrong once and corrected | identical: table pointer is `base + len - mapcount*4` | **agree**, and this is the one place EvEJS corroborates a correction of ours | `0x1802e17d9`-`0x1802e17ec`: `lea r14,[rcx*4]`; `rax = [rdi+0x20] - r14 + [rdi+0x18]`; `[rdi+0x50] = rax` |
| 9 | it emits `0x7E` | we accept `0x7D` and `0x7E`, emit only `0x7E` | the client emits `0x7E` too, because `blue.marshal.Save`'s `version` defaults to 0 | **agree** | see §3 |
| 10 | `OPCODE_MASK 0x3f` | `type = tag & 0x3F` | `and ecx, 0x3f` | **agree** | `0x1802ddab3` |
| 11 | `SAVE_MASK 0x40` | `shared = tag & 0x40` | `shr eax, 6; and al, 1` | **agree** | `0x1802ddac3`-`0x1802ddac6` |
| 12 | `UNKNOWN_MASK 0x80` | bit 7 is **discarded** | bit 7 is never tested anywhere in the dispatch | **partly agree** — the name is fair, but the client *ignores* the bit; a codec that *rejects* on it diverges from the client | `0x1802ddaa4`-`0x1802ddad2`, the whole dispatch: `movsx`, `and 0x3f`, `shr 6`, `and 1`, index |
| 13 | opcodes span `PyNone 0x01` … `PyVarInteger 0x2f` | 42 valid codes in `0x01..0x2F`; `0x00, 0x0C, 0x18, 0x1D, 0x1E, 0x2D` are NULL slots | same range; a NULL slot gives *"Invalid type tag %d in stream."* | **agree** | table `0x180b015f0`; NULL path `0x1802ddafc` → `0x18098…`/`0x18097dd48` |
| 14 | `PyPackedRow 0x2a` | DBRow is `0x2A`, proven from the writer's immediate | `0x2A` → `0x1802fe410` | **agree** | `docs/reference/dbrow.md`; `mov r8d, 0x2a` at `0x1802df84d` |
| 15 | `PySubStream 0x2b` | `0x2B` is a length-prefixed nested marshal stream | shape confirmed; the *name* is ours by inference | **agree** | `docs/archive/marshal-tags.md`; tag `0x2B` → `0x1802db760` |
| 16 | `PyObjectEx1/2 0x22/0x23` | object 2.2 / 2.3 forms | `0x1802de1c0` / `0x1802de430`, both reserve a shared slot before reading any child | **agree** | `docs/reference/object-wrapper.md` |
| 17 | `cPicked 0x21` | `0x21` is a raw, self-delimiting cPickle payload | cached `cPickle.Unpickler` at `stream+0x70`, no length field | **agree** | handler `0x1802dcf60` |
| 18 | `PySavedStreamElement 0x1b` | `0x1B` is the shared back-reference, 1-based, `1 <= i <= mapcount` | *"Invalid TY_REFERENCE in stream"* on a bad index | **agree** | handler `0x1802dce60` |
| 19 | dicts are written **value before key** | same; `pyutil::pydict` exists to encode the inversion once | first `ReadObject` → `r8` (value), second → `rdx` (key), then `PyDict_SetItem(dict, key, value)` | **agree — and independently, which is the point** | `0x1802dd2c6` then `0x1802dd2e1`, then `0x1802dd2ee`-`0x1802dd2f7 call PyDict_SetItem` |
| 20 | packed row is real bit-packing + RLE | size-class regions largest-first, a NULL bitmap region, nibble RLE | as `docs/reference/dbrow.md`, read off `0x1802fe410` | **agree** | `docs/reference/dbrow.md` |
| 21 | object sharing is "restricted to `blue.DBRowDescriptor` by identity" | eleven handlers consume a table entry; no type restriction | the client restricts sharing to **no** type: `0x02, 0x0D, 0x13, 0x14, 0x15, 0x16, 0x17, 0x21, 0x22, 0x23, 0x2F` all consume an entry | **disagree** — that is their implementation's scope, not the client's contract | `RegisterShared 0x1802dbdf0`; per-handler, `docs/archive/marshal-tags.md` |
| 22 | embedded pickle end is found by walking pickle opcodes | `0x21` is self-delimiting (no length); `0x1A` carries a length the client **discards** | same | **agree** | `0x1802dcf60` (`0x21`), `0x1802dd010` (`0x1A`) |
| 31 | the rest of their opcode table (beyond the seven codes the handoff doc names) | 42 codes, 26 named from their handlers' C-API fingerprints | — | **unknown** — not compared; archive not extracted, and see the note above | — |

### `GetHandshakeFunc` — GAP 3

| # | claim | our reading | client says | verdict | citation |
|---|---|---|---|---|---|
| 23 | the client accepts a plain marshaled string containing a Python expression | we wrote `marshal.dumps(<a py2.7 code object>)` | **both work.** `eval(func, globals(), context)` takes a `str` or a code object; the blob is **stdlib** `marshal`, not `blue.marshal` | **agree**, and their shape is the simpler one | `GPS.py:1014` `marshal.loads(marshaled)`; `GPS.py:1026` `eval(func, globals(), context)`; `marshal` is imported plain at `GPS.py:6`, while `blue.marshal` is spelled out at `:144` |
| 24 | "the second element is the verification flag, hardcoded `false` (unsigned)" | we send `0` | the flag is **required**, not disabled: Placebo's `verify` returns `(signedData[0], signedData[1] == 0)`, and `__Execute` raises outside the `try` if that is falsy | **disagree on the reading** — `false` happens to work because `False == 0` in Python 2, but `True`, `1`, `None` or `''` would kill the handshake | `evecrypto/placebo.py:21`; `GPS.py:1010`-`1012` |
| 25 | "an empty-but-valid expression should satisfy the client" | the GAP note already said a *failing* expression is acceptable | right, and smaller still is possible — see §4 | **agree** | `GPS.py:1024`-`1026`, the bare `except:` |

### Bound objects (GAP 6) and service advertisement (the "Bonus")

| # | claim | our reading | client says | verdict | citation |
|---|---|---|---|---|---|
| 28 | `machoNet.GetInitVals` returns a ~145-entry `serviceInfo` dict | we have no `GetInitVals` | **`GetInitVals` does not exist in this client.** Zero hits across all 12,774 `.pyc` files and zero in the native binaries. `serviceInfo` is the entire return value of a *separate* call, `machoNet.GetServiceInfo` | **disagree** — the call is from an older protocol generation | `grep -ra GetInitVals out/pyc/` → nothing (re-run independently in this lane); `machoNet.py:2028` is the real call |
| 29 | `GetServiceInfo` returning a bare `{}` is "consistent with our note that it is only a node-id hint" | `{}` is safe | `{}` is safe and in fact **better** than the real thing — but the "node-id hint" framing is attached to the wrong mechanism | **partly agree** — right conclusion, wrong reason; §8 | `ServiceCallGPCS.py:256`-`260`, `:277` |
| 30 | a call to a dead OID gets a synthetic `UnMachoDestination` so the client's Moniker rebinds and retries **once** | we had the mechanism as a gap | **confirmed**, with three corrections: the retry is only in `util.Moniker`, the exception usually arrives rewritten as `UserError('UnMachoDestination')`, and the class never needs to be named on the wire | **agree** | `moniker.py:256` and `:263` (two `Unresolve` sites in `MonikeredCall`); `cluster/__init__.py:88` `MACHONETERR_UNMACHODESTINATION = 0`; §7 |

### The reference document's own facts

| # | claim | our reading | client says | verdict | citation |
|---|---|---|---|---|---|
| 26 | "client build 3396210 ← SAME TARGET AS OURS" | our build is **3619879** | — | **disagree** — §6 | `docs/archive/recon-v24.01.md:1` |
| 27 | content harvested from *newer* builds `3475087`, `3487903`, `3436472` | — | all three, and 3396210, are **older** than 3619879 | **disagree on direction** — newer than *their* target, older than ours | arithmetic |

---

## 2. Framing: the first word is a length **and** flags

**Answer to the lane's question 1.** `word0` is a little-endian `u32` carrying a
**28-bit payload length in bits 0..27 and four flag bits in bits 28..31**. Two
of the four are implemented:

```text
bits 0..27  plen — bytes that FOLLOW word0 (it does not count itself)
bit  28     OOB_PRESENT  (0x10000000) — a u32 oobLen + oobLen bytes precede the payload
bit  29     ZLIB         (0x20000000) — payload is u32 rawLen + a zlib stream
bits 30..31 unused in this build
```

Re-verified in this lane, not taken from `docs/reference/framing.md`: the mask
`and e{cx,di,ax}, 0xfffffff` at the **three** reader resume points
`0x180388640`, `0x180388701`, `0x18038873e`, and the OOB test
`test dword [rax], 0x10000000` at `0x1803887aa`. **[V]** Bit 29 and the writer
at `0x180382d70` stand on `docs/reference/framing.md`'s pass and were **not** re-read
here.

So EvEJS's `[uint32 LE payloadLength][payload]` is a correct description of
**every frame machoNet ever emits**, because machoNet never sets a flag:
`machoNetTransport.py:493` is `self.transport.Write(thePickle)` with
`CALL_FUNCTION 1` — one positional argument, no `header` — and `header` is the
*only* way bit 28 gets set (`SocketGPS.py:555`, reached only
`if header and usingIOCP`; the plain call is `:557`). **[V], read from the
bytecode, not the decompiler.** It is not a correct description of the *field*,
and "no OOB channel in the framing itself" is wrong: the receive side decodes
one.

### What the client does with high bits set

**The two implementations disagree, and that is the whole reason never to set a
flag bit.** `_slsocket` carries both, selected by `use_carbonio`
(`0x180b01a3c`); this install runs **carbonio** (`socketIO = iocp`).

- **carbonio** masks, so it reads bits 28/29 as flags and the low 28 bits as
  the length. A frame with bit 28 set is parsed as an OOB frame.
- **legacy stacklessio does not mask.** `0x180391dd0`-`0x180391e00`:
  `edx = [rdi+0xc0]` — the length word used **whole** — then
  `cmp edx, [rax+0xa4]; ja 0x180391ff0`. Any flag bit makes `edx >= 0x10000000`
  = 268,435,456, far over the cap, so it jumps to the function that owns
  *"too large a packet detected at %d bytes, max is %d"* and raises;
  `SocketGPS.py:636` catches it by substring and turns it into
  `GPSTransportClosed`. **On the legacy path, setting any flag bit kills the
  connection.** **[V]**

All-zero flags is the only setting both implementations read identically.

### The cap — and it is ours that the client justifies

**The client does enforce a cap, and it is 10,485,760 on the whole frame
including the 4-byte header.** **[V]**

| where | value | address / line |
|---|---|---|
| carbonio default, set lazily when the field is 0 | **1,000,000** (`0xF4240`) | `cmp dword [rdi+0x11d0], 0` at `0x18038781d`, `mov dword [rdi+0x11d0], 0xf4240` at `0x180387826` |
| legacy default | **1,048,576** (`0x100000`) | `0x18038e6b0`, `0x18038e97b` (field `+0xa4`) |
| what `SocketGPS` actually installs, on **both** `Connect` and `Accept` | **10,485,760** | `SocketGPS.py:133` (`SocketTransportFactory.__init__`, consts `[10, 1024, 10240, 10485760]`); `s.setmaxpacketsize(...)` at `:202` and `:277` |
| carbonio receive comparison | `plen + 4` vs the field | `lea edi, [rax+4]` `0x180388743`; `mov r9d, [rbx+0x11d0]` / `cmp edi, r9d` `0x180388761`-`0x180388768` |
| legacy receive comparison | the **payload length alone** vs the field | `0x180391dfa` |
| carbonio send comparison | whole frame | `0x180385f3f` |
| Python-side send ceiling on the marshal blob | **5,000,000**, from `prefs.GetValue('machoNet.dropPacketThreshold', 5000000)` | `machoNetTransport.py:157` |

Three things follow.

1. **Our `MAX_FRAME = 10_485_760` is the client's own number**, and our
   `write_frame` check (`payload.len() + 4 > MAX_FRAME`) yields a maximum
   payload of 10,485,756 — which is exactly right, because carbonio compares
   `plen + 4` while legacy compares `plen`, so only `payload ≤ cap - 4` is
   accepted by both. **The brief's worry — "a number neither of us can justify
   from the client" — does not apply to us.** It applies to EvEJS: 10,000,000
   matches none of 1,000,000 / 1,048,576 / 10,485,760 / 5,000,000. It is a
   *safe* choice (above the 5,000,000 the client will ever send, below the
   10,485,760 the client itself refuses) but it is invented.
2. **`MAX_PAYLOAD = 5_000_000` is a default, not a guarantee.**
   `dropPacketThreshold` is a `prefs` value, so a client with that pref raised
   can legitimately send more. Treat 5,000,000 as the expected ceiling and
   10,485,756 as the enforced one — which is what we already do. **[V]** on
   the pref lookup, **[I]** that nothing else re-clamps it.
3. **Never emit an oversize frame.** Carbonio *receive* oversize does not
   close: it logs and resets `[pkt+8] = 0` and continues
   (`0x180388a08`-`0x180388a26`), so a bad length silently desynchronises the
   stream and every later frame is garbage. EvEJS's "destroy the socket" is a
   fine *server* policy — it is just not what the client does, and the failure
   mode to design against is the silent one.

---

## 3. `0x7D` vs `0x7E`: closed. `0x7D` is marshal **version 1**

**Answer to the lane's question 2, and it closes `docs/archive/wire-format.md` gap 4 and
`docs/archive/marshal-tags.md` gap 10.** Both markers are read off this build's reader
*and* writer, and the Python-visible signature names the field.

### The signature

`blue.marshal.Save` parses `O|OiiO:Save` with the keyword list
`['object', 'callback', 'useChecksum', 'version', 'stringMap']`:

```python
blue.marshal.Save(object, callback=None, useChecksum=0, version=0, stringMap=None)
blue.marshal.Load(buffer, callback=None, skipCrcCheck=0, offset=0, stringTable=None)
```

**[V]** — format string `0x18097df10` (`'O|OiiO:Save'`), kwlist `0x180aed560`;
`Load`'s are `0x18097df68` (`'O|OinO:Load'`) and `0x180aed590`. Bindings at
`0x1802e0d40` and `0x1802e0ec0`. `version` is range-checked
`cmp r9d, 1; jbe` and anything above 1 raises `ValueError("invalid version")`
(`0x1802e0e31`-`0x1802e0e50`, string `0x18097df40`). `stringMap` must be a
dict or `ValueError("stringMap must be a dict")` (`0x18097df20`), and
`callback` must be callable or `ValueError("callback argument must be
callable")` (`0x18097dee8`).

### The writer, `0x1802da7b0`

`version` arrives as the 4th argument; the branch on it is the whole answer:

```text
0x1802da8f8  test r14d, r14d          ; r14d = version
0x1802da8fb  jne  0x1802da92f
0x1802da8fd  or   ebx, 0x7e           ; version == 0
0x1802da915  mov  byte [rcx+rax], bl  ; emit 0x7E
0x1802da91c  xor  edx, edx
0x1802da921  call 0x1802e5da0         ; write u32 0 -- the mapcount placeholder
...
0x1802da92f  or   ebx, 0x7d           ; version != 0
0x1802da947  mov  byte [rcx+rax], bl  ; emit 0x7D
0x1802da94e  movzx edx, r14b
0x1802da955  call 0x1802e5df0         ; write u8 version -- and NO mapcount word
0x1802da962  test r15b, r15b          ; useChecksum
0x1802da96f  call 0x1802daac0 (edx=0x1c) ; emit tag 0x1C, then a u32 CRC placeholder
```

### The reader, `0x1802e1670` (`Marshal::Load`)

```text
0x1802e16de  cmp cl, 0x7e / je 0x1802e174c  -> extra := 0 (no byte consumed), read mapcount
0x1802e16e3  cmp cl, 0x7d / je 0x1802e1703  -> consume one byte into [rdi+0x80]
0x1802e16e8  else  ValueError "invalid marshal header"
0x1802e1728  test cl, cl / je 0x1802e1754   -> extra == 0: read the mapcount
0x1802e172c  mov [rdi+0x5c], 0              -> extra != 0: mapcount := 0, header ENDS, return
```

### So

| form | bytes | shared-object map | emitted by |
|---|---|---|---|
| **version 0** | `7E` + `i32 mapcount` | yes, `mapcount × u32` 1-based ids **at the tail** | `Save(...)` with `version` omitted or 0 — i.e. **everything on the machoNet path** |
| **version 1** | `7D 01` | **none**, and no mapcount word at all | `Save(..., version=1)` |
| `7D 00` + `i32 mapcount` | 6 | yes | **nothing** — accepted by the reader, never produced by the writer |

- **What the client emits:** `0x7E`. `machobase.Dumps` is
  `blue.marshal.Save(packet, None)` (`machobase.py:29`) and
  `DumpsSanitized` is `Save(what, SanitizedGetObjectID)` (`:54`) — neither
  passes `version`, so both take the default 0. **[V]**
- **What the client accepts:** `0x7E`, `0x7D 00 <mapcount>`, and
  `0x7D <v != 0>`. **[V]**
- **What the other marker is for:** `0x7D` is the **no-shared-map compact
  header**, selected by `version=1`. The "extra byte" is the version number,
  and a non-zero version is precisely what tells the reader there is no map.
  **[V]** Why a second form exists at all — presumably for streams that cannot
  have shared objects, where a 2-byte header beats a 5-byte one — is **[I]**.

EvEJS emitting `0x7E` **agrees** with what the client emits.

### This is a bug in our reader, and the lane's highest-value find

`server/crates/eve-marshal/src/value.rs:905`-`907`:

```rust
let extra = if marker == 0x7D { r.u8()? } else { 0 };
...
let mapcount = r.i32()?;
```

We read the mapcount **unconditionally**. For a version-1 stream (`7D 01`) the
client reads no mapcount and starts the body at offset 2; we would swallow four
body bytes as a mapcount and then mis-decode everything. See §9 for the
proposal. Nothing we have seen on the wire is version 1 — this client never
writes one — so this is latent, not live.

### Two things that fell out of the same pass

- **`useChecksum` is the switch that emits tag `0x1C`.** `0x1802da962`-`:96f`:
  when the flag is set the writer emits tag `0x1C` and then a `u32` placeholder
  before the body. `Load`'s `skipCrcCheck` is the matching read-side gate.
  This confirms `docs/archive/marshal-tags.md`'s `0x1C` reading **from the writer**, which
  it previously had only from the reader. **[V]**
- **The header's id table really is at the tail**, re-confirmed here from the
  pointer arithmetic (`0x1802e17d9`: `lea r14,[rcx*4]`;
  `rax = [rdi+0x20] - r14 + [rdi+0x18]`; `[rdi+0x50] = rax`), and ids are
  validated `1 <= id <= mapcount` or `RuntimeError("Bogus map data in marshal
  stream.")` (`0x1802e1813`-`0x1802e182d`). EvEJS's layout line puts the ids
  after the value too — **the one place in the archive that corroborates a
  correction of ours rather than a claim of ours.** **[V]**

---

## 4. `GetHandshakeFunc`: the minimum, and why the doc's reading is wrong

**Answer to the lane's question 4.**

### The shape, confirmed from the client

H8 is a 4-tuple; element 2 is `signedFunc` and element 3 is the `eval` locals
dict (`GPS.py:962`):

```python
serverChallenge, signedFunc, context, response = macho.Loads(...)   # GPS.py:962
...
outputBuffer, funcResult = self.__Execute(signedFunc, context)      # GPS.py:977

def __Execute(self, signedFunc, context):                           # GPS.py:1009
    marshaled, verified = Crypto.Verify(signedFunc)                 # :1010
    if not verified:
        raise RuntimeError('Failed to verify initial function blobs')  # :1012
    func = marshal.loads(marshaled)                                 # :1014
    ...
    try:
        try:
            ...
            funcResult = eval(func, globals(), context)             # :1026
        except:
            funcResult = {}
            <traceback printed into the captured WriteBuffer>
    finally:
        <stdout/stderr restored>
    return (output.buffer, funcResult)
```

Under the Placebo crypto pack — which is this install's default,
`cryptoPack = boot.GetValue('cryptoPack', 'Placebo')` in
`evecrypto/settings.py` — `Verify` is:

```python
def verify(signedData):
    return (signedData[0], signedData[1] == 0)      # evecrypto/placebo.py:21
```

**[V]** on all of it.

So `signedFunc` is a 2-tuple `(blob, flag)` where:

- `blob` is handed to the **standard library's** `marshal.loads`, not
  `blue.marshal` — `marshal` is imported plain at `GPS.py:6`, and where the
  client means the other one it writes `blue.marshal.Load` (`GPS.py:144`).
  **[V]** This is the detail that makes the whole thing cheap: the inner blob
  is a CPython 2.7 marshal blob, which is trivial to emit by hand.
- `flag` must satisfy `flag == 0`.

### Correction to `docs/archive/evejs-261018-reference.md` §2 GAP 3

The doc says the second element is *"the **verification flag, hardcoded
`false`** (unsigned)"* and draws the conclusion *"the client accepts … with
verification off."* **That reading is wrong, and it is the dangerous kind of
wrong — the conclusion it reaches is right by accident.**

There is no "off". `verified` is computed as `signedData[1] == 0`, and
`__Execute` raises `RuntimeError` on a falsy `verified` **outside** the
try/except, so the handshake dies. The flag is a **required value**: it must be
`0`-equal. In Python 2, `False == 0` is `True`, which is why EvEJS's `false`
works. But `True`, `1`, `None`, `''` and `()` all give `verified = False` and
kill the connection. A server author who read "verification is off" and sent
`True` there — the obvious choice for a flag you think is ignored — would get
an unexplainable `GPSTransportClosed`.

Our own `docs/reference/login-path.md` §4 H8 note 3 already says to send `0`, and is
correct.

### The minimum we can emit

Three gates, in order, and only the first two are mandatory:

1. `signedFunc[1] == 0` must hold. Send the integer `0`.
2. `marshal.loads(signedFunc[0])` must not raise. It is **not** inside the
   try/except, so a malformed blob is fatal.
3. `eval(func, globals(), context)` may do anything, **including raise** — the
   bare `except:` swallows it, sets `funcResult = {}`, prints the traceback
   into the captured buffer, and the handshake continues. The client never
   inspects `funcResult`; it just hands it back to us in the next message
   (`GPS.py` writes `(CryptoHash(serverChallenge), outputBuffer, funcResult)`),
   and we ignore it.

**So the minimum that satisfies the client is a single byte: `'N'`**, CPython
2.7 marshal's `TYPE_NONE`. `marshal.loads('N')` → `None`; `eval(None, ...)`
raises; the exception is swallowed; login proceeds. `signedFunc = ('N', 0)`.
That emits **no executable code to the client at all**, which is the strongest
possible answer to finding S3. **[V]** on the swallow and on `marshal.loads`;
**[I]** that `eval(None, …)` raises `TypeError` specifically — the handler does
not care which exception it is.

**What we should actually emit is one step up from the minimum**, because the
minimum leaves a traceback in the client's log for no reason:

```text
signedFunc = ( "s" + u32le(2) + "{}" , 0 )      # 7 bytes + the integer 0
context    = {}                                  # element 3 of H8
```

`marshal.loads` reads `TYPE_STRING` as `'s'` + a little-endian `i32` length +
that many bytes, giving the string `"{}"`; `eval("{}", globals(), {})` returns
an empty dict, touches nothing in the client's namespace, raises nothing,
leaves `output.buffer` empty, and sets `funcResult = {}` — the type a real
server's `GetHandshakeFunc` result is expected to have (the *server* side
checks `isinstance(funcResult, types.DictType)` at `GPS.py:448`; the client
does not). **[I]** on the exact `TYPE_STRING` byte layout, which was not read
out of this client's `python27.dll`; **[V]** on everything the client's own
Python does with the result.

**Either way we never send a code object and never send a statement.** EvEJS's
`'eval(compile("…","<tidi>","exec"))'` string exists because they want to run
*statements*, which `eval` of an expression cannot do. We want to run nothing,
so we need neither the `compile` nor the inner `eval`. The right design rule,
and it is the doc's own: **the handshake function is a hole, and the only safe
size for it is empty.** Putting anything in it means every client that connects
to us runs it, and the next person to need a feature will reach for the hole
that is already there.

---

## 5. The other markers: what else the client's `Save`/`Load` surface tells us

Small, and worth recording because this lane had the signatures open:

- `Save`'s `stringMap` and `Load`'s `stringTable` are **per-call** overrides of
  the 195-entry native string table. Nothing on the machoNet path passes one
  (`machobase.py:29`, `:54`, `:33`, `:58`), so the live table is the native one
  — which `docs/archive/marshal-tags.md` already established and
  `tools/extract_string_table.sh` already pins. **[V]**
- `Load` takes an `offset`, so the client can decode a stream that does not
  start at byte 0 of the buffer. We never need to emit anything for this; it
  only means a nested stream need not be copied. **[V]**
- `machoNetTransport.Read` inflates with a bare `zlib.decompress(thePickle)`
  (`machoNetTransport.py:522`) with no output cap — the client has the same
  zlib-bomb shape as EvEJS's S11. Irrelevant to us as a server except as a
  reminder that **we must not rely on the client refusing anything**; our own
  `eve-inflate::MAX_OUTPUT` (64 MiB) and `eve-marshal`'s depth cap of 1000
  (matching the client's own `0x3e8` limit at `0x1802dda56`, string
  *"Maximum recursion depth of %d reached"*) already cover S11 and S12 on our
  side. **No action.** **[V]**

---

## 6. What the build difference does to every verdict above

**Answer to the lane's question 5.** `docs/archive/evejs-261018-reference.md` §1 says
*"target EVE 24.01, client build 3396210 ← SAME TARGET AS OURS"*. **Our build
is 3619879** (`docs/archive/recon-v24.01.md:1`). Same *version*, different *build*,
223,669 apart.

Two corrections to §1 at once:

1. **Not the same target.** The line should read *"same version, 223k builds
   earlier"*.
2. **"Also harvested from *newer* clients … `3475087`, `3487903`, `3436472`"
   is newer than *their* target and older than ours.** Ordered:
   `3396210 < 3436472 < 3475087 < 3487903 < 3619879`. **Relative to
   eve-offline, every build named anywhere in that archive is older than the
   one we target.** The §1 warning should be inverted for our purposes: the
   risk is not post-24.01 content, it is *pre*-3619879 content, i.e. content
   that may since have changed or been removed.

### Which claims are build-independent, and which are not

**Build-independent, or near enough that agreement is real corroboration:**

- **Framing** (#1-#7). The format is compiled into `blue.dll`'s carbonio and
  stacklessio implementations. We read it from **3619879 directly**, so our
  claim does not depend on theirs at all; what their agreement adds is
  evidence that the format did not move across 223k builds — which is more
  useful than same-build agreement would have been, because it is evidence of
  *stability*, the property we actually care about for M1.
- **The tag split and the opcode space** (#10-#18). Same argument: read from
  our binary's dispatch table, corroborated across builds.
- **Dict value-before-key** (#19). Same.
- **The header layout and the tail id table** (#8). Same.
- **`machoVersion = 496` and the handshake secret key `170472`.** EvEJS pins
  both and we read both from 3619879. Agreement across builds says these are
  24.01-wide, not build-wide. **[V]** on ours
  (`eve-marshal::constants::handshake`).

**Build-dependent, and a trap:**

- **The marshal string table.** This is the sharpest one. Our build carries
  **195** native entries at `0x180953b00` with `machoVersion` at index 135,
  while `eve/common/lib/marshalstrings.py` in the *same* build has 210 in a
  *different* order with `machoVersion` at 148
  (`docs/archive/marshal-tags.md`). If two tables inside one build disagree, a table
  from a build 223k earlier is worthless to us. **Nothing involving tag `0x11`
  may be taken from EvEJS, at any confidence.** We regenerate from the DLL;
  keep doing exactly that. **[V]**
- **`boot_build`.** By construction. Note the direction, which is benign:
  the client rejects only a server build **newer** than its own
  (`boot.build < r[4]` → `HANDSHAKE_INCOMPATIBLEBUILD`,
  `docs/reference/login-path.md` §4), so EvEJS's pinned 3396210 would be *accepted* by
  our 3619879 client. The build mismatch is therefore not a reason their
  handshake observations would not transfer. **[V]**
- **`serviceInfo`, service names, every content-derived claim.** 3396210's
  service list is not 3619879's, and the archive's bulk data is from builds
  older still. §8 shows this is worse than build-dependence: the *call itself*
  (`GetInitVals`) does not exist in our build. **This is the clearest example
  in the whole cross-check of a claim that is not merely stale but structurally
  inapplicable**, and it was invisible until someone grepped our build for the
  name. The lesson for the rest of the archive: before taking any
  *call-level* claim, grep this build for the call name first — it costs one
  command and it is the only check that catches a whole protocol generation
  having moved.
- **`tickAlignedSends`' 60 ms / 2,381-of-2,682 measurement.** Measured against
  retail at an unknown build. The doc already says to re-measure from our own
  captures; that remains the right call and this lane did not do it.

### The trap the brief named, stated plainly

Of 27 claims, 19 agreed. That is a high rate, and a high rate is exactly the
condition under which the next claim gets waved through. Two of the six
disagreements were found only because they were checked after a run of
agreements: #21 (sharing "restricted to DBRowDescriptor") reads like a
protocol fact and is an implementation detail, and #24 ("verification off")
reads like a conclusion and is a misreading whose conclusion happens to hold.
**Both would have been adopted by anyone who stopped checking once the pattern
looked good.** Treat the 19 as hypotheses that survived.

---

## 7. `UnMachoDestination`: the rebind contract, confirmed and narrowed

**GAP 6, the item the handoff doc says is "worth knowing before M2".** The
claim — *"a call to a dead OID gets a synthetic `UnMachoDestination` so the
client's Moniker rebinds and retries once"* — **is correct.** Three corrections
change how we would implement against it.

### The mechanism, as a server has to use it

**Reply to the `CallReq` with an `ErrorResponse` (machoNet command 15) whose
body is `(originalCommand, 0, payload)`.** That is the whole thing:

- `code = 0` is `MACHONETERR_UNMACHODESTINATION`
  (`cluster/__init__.py:88`; `MACHONETMSG_TYPE_ERRORRESPONSE = 15` at `:83`).
  **[V]**, re-verified in this lane.
- `machoNet._BlockingCall` tests `retval.code == MACHONETERR_UNMACHODESTINATION`
  and **constructs the exception client-side** (`machoNet.py:4308`-`4309`).
  **[V]**
- `ExceptionMappingGPCS` then rewrites it to
  `UserError('UnMachoDestination', {'what': e.payload})`
  (`ExceptionMappingGPCS.py:77`-`80`). **[V]**
- `Moniker.MonikeredCall` catches **both** forms — the mapped `UserError`
  (`moniker.py:254`) and the bare class (`:260`) — and on each, if `retry` is
  still 0, sets it, calls `Unresolve()` and `continue`s the `while 1`. Two
  `Unresolve` call sites at `moniker.py:256` and `:263`, **[V]**, re-verified.
  `retry` is initialised at `:238`, once per call, so it is **one original
  attempt plus one retry, then the exception propagates**. "Retries once" is
  exactly right.

### Correction 1 — nothing named `UnMachoDestination` need ever cross the wire

**This is the correction that matters for implementation.** The numeric route
needs no class name, no pickle, and no whitelist entry. The class *is*
whitelisted (`whitelist.py:126`), and there is a second route where the server
pickles an instance inside a `MachoWrappedException` (code 2), but we should
never use it: `code = 0` plus any marshalable payload is strictly less
machinery. CCP's own proxy sends a plain string there, and it only ever reaches
`{'what': …}` and a log line.

### Correction 2 — the retry lives in `util.Moniker` and **nowhere else**

- **A `util.Moniker`** re-resolves and re-binds: `Unresolve` unbinds, nulls the
  node id, and calls `SetNodeOfAddress(service, bindParams, None)`
  (`moniker.py:331`, `:340`), then `Bind` issues
  `MachoResolveObject(bindParams)` followed by
  `MachoBindObject(bindParams, call)` — **with the original call tuple
  piggybacked on the bind, so the bind's return value *is* the original call's
  result** (`moniker.py:209`). Our server must be ready for that two-call
  sequence, and the second attempt must actually succeed: a second code-0 reply
  gives up and the error reaches the UI.
- **A raw `MachoObjectConnection`** (what `ParseObjectID` builds from an
  object id in a payload, reprs as `<RemoteObject:N=…:…>`) has **no retry
  logic at all**. The exception goes straight to application code.
- **A plain service call** does not retry either: `ServiceCallGPCS.py:354`-`355`
  clears the whole client-side service→node map and re-raises.

**Design consequence: hand out a `util.Moniker` for anything whose hosting node
can change.** If we hand out a bare bound object, we have given up the rebind
primitive for that reference, permanently.

### Correction 3 — `UpdateMoniker` is the better tool for a *move*, and it has a hole

If the goal is to point a client at a different service / bindParams / node
rather than refresh a stale id, `UpdateMoniker` (whitelisted,
`__passbyvalue__ = 1`, carries all three) does it directly. But
`MonikeredCall`'s `UpdateMoniker` branch (`moniker.py:247`) has **no
same-target guard**, unlike `Bind`'s, which refuses a second referral to the
same location (`moniker.py:219`, `:222`). **A server that answers
`UpdateMoniker` with the same target twice spins the client in a tight loop.**
Worth knowing before we emit one.

### One trap on a sibling exception

`SessionUnavailable` is installed into the `exceptions` module by
`builtinmangler` but is **absent from the whitelist string**. CCP's own server
downgrades it to a `RuntimeError` before it crosses the wire. **Never send it
as itself.**

*§7 rests on a dedicated pass over the client's bytecode delegated within this
lane. The four claims this lane re-verified itself are marked "re-verified":
the two error codes, the two `Unresolve` call sites, and `GetInitVals`'
absence in §8. The rest is **[V]** on that pass's reading and has not been
read twice.*

---

## 8. `GetInitVals` does not exist, and `serviceInfo` is advisory

**The handoff doc's "Bonus — service advertisement" is the one section that
does not transfer at all.**

### `machoNet.GetInitVals` is not in this client

**Zero hits for `GetInitVals` across all 12,774 `.pyc` files**, and none in the
native binaries. **[V]**, re-run independently in this lane. It is a
pre-Crucible-era call. In V24.01 its job is split in two:

1. The **native** handshake (`rawTransport.Authenticate`) returns the dict from
   which the client reads exactly three things — `session_init['userid']`,
   `proxy_nodeid`, and `config_vals` — which is precisely what
   `docs/reference/login-path.md` §4 H10 already documents.
2. The service map is fetched by a **separate, explicit call**,
   `machoNet.GetServiceInfo`, at `machoNet.py:2028` inside `ConnectToServer`.
   **[V]**, re-verified.

So the cross-check the doc proposed — "~145 entries vs our 175" — is comparing
a list from an older protocol against a census of a different population. **It
should not be attempted.**

### `serviceInfo` is advisory, and `{}` is the better answer

The call **must be answered** — `machoNet.py:2028` sits in a `try/finally` with
no `except`, so an exception there aborts login. Its **content is free**.

Only three places in the client read it, all re-verified in this lane as the
only consumers (`ServiceCallGPCS.py:256`-`260` and `:388`-`395`,
`eveServiceCallGPCS`): **[V]**

- The authoritative routing cache is `self.services`, populated by the
  **server-pushed `OnUpdateServiceMapping`** notification and by
  `WrongMachoNode` redirects. `serviceInfo` is consulted **only** when
  `self.services` has no entry for the service.
- Its only use is **co-location**: "service X resolves the same way as service
  Y, and I already know Y's node, so try X there." It never yields a node id by
  itself.
- Missing, empty, or unknown names are all harmless. The code falls through to
  `destination = MachoAddress(service=service)` (`ServiceCallGPCS.py:277`) —
  `ADDRESS_TYPE_ANY` with a service name and no node id — and the call goes
  out anyway for the proxy to resolve. The "I haven't got a clue where … should
  run" branch is a `LogInfo` and nothing else.

**So `{}` is safe — and it is *better* than a populated answer**, because the
real server returns a `util.CachedObject` (`__passbyvalue__ = 1`) whose state
on the wire is only `(objectID, nodeID, objectVersion)`; the client then
lazily fetches the body through
`objectCaching.GetCachableObject('machoNet.serviceInfo', …)`. A plain dict
satisfies every operation the client performs on it (`in`, `[]`,
`.iteritems()`, `.get()`) and removes that dependency entirely. A
`CachedObject` stub whose backing fetch fails raises
`ReferenceError('The specified object is no longer available in cache')` on
**every** later unresolved service call. **[V]** on the mechanics, **[I]** on
"better".

### If we ever do populate it

Values are **strings or `None`, never ints or tuples** — the client does
`v.startswith(where)` at `ServiceCallGPCS.py:260`, so an int there is an
`AttributeError` on the call path. The vocabulary, from the server-side
`CoreService._MachoResolveAdditional`, is `None`, `'station'`,
`'solarsystem'`, `'solarsystem2'`, `'location'`, `'locationPreferred'`,
`'character'`, `'corporation'`, `'user'`, `'clustersingleton[_<n>]'`. The only
literals the **client** itself compares against are `'solarsystem'`,
`'location'` and `'station'` (`eveServiceCallGPCS.py:32`, `:35`). **[V]**

### Our 175 is the right number, for the wrong question

`findings/remote-services.txt` holds **175** names and was reproduced exactly
by re-running `tools/scan_remote.py`'s logic. **[V]** But it counts *service
names the client statically mentions at a `RemoteSvc`-family call site*, which
is neither EvEJS's ~145 (their server's own roster) nor
`findings/client-services.json`'s 300 (client-side service classes). **Three
different populations; no two are comparable.** The live `serviceInfo` size is
whatever *our* service roster is — our choice, and `{}` until we want the
first-call optimisation.

Two census gaps the same pass turned up, **not mine to fix** but worth handing
to whoever owns those files:

- `tools/scan_remote.py`'s idiom set omits the `ConnectTo*Service` family;
  adding it yields 184 names, the 9 extras being mostly server-internal code
  that merely ships in the client archive. Excluding them is defensible, but it
  should be a decision rather than an omission.
- It also misses `RemoteServiceCall(sess, nodeID, 'svc', 'method')`, where the
  service literal is argument 3. **Consequence: the login-critical
  `machoNet.GetServiceInfo` call at `machoNet.py:2028` is absent from
  `findings/client-calls.tsv`.** That is a real hole in the login-path
  inventory — a call our server must answer or login aborts, which our own
  census does not list.

*§8 rests on the same delegated pass. `GetInitVals`' absence, the
`GetServiceInfo` call site, and the set of `serviceInfo` consumers were
re-verified in this lane; the rest is **[V]** on that pass's reading.*

---

## 9. Proposals — what should change in our docs and our code

Nothing below was done by this lane. **No Rust was written and no file outside
this document and `docs/lanes/evejs-crosscheck.md` was touched.**

### Code (proposals only — this lane writes no Rust)

**P1 — `eve-marshal::take_header` mis-parses a version-1 header.** `value.rs:905`-`907`
reads the mapcount unconditionally. The client does not, when the marker is
`0x7D` and the version byte is non-zero. Two acceptable fixes:

```rust
// (a) parse it the way the client does
let extra = if marker == 0x7D { r.u8()? } else { 0 };
let mapcount = if extra != 0 { 0 } else { r.i32()? };
```

or **(b) reject it explicitly** — `WireError` on `0x7D` with a non-zero
version, with a comment pointing at `0x1802e172c`. (b) is defensible: this
client never writes version 1, so the branch would be untested code on a path
that only hostile or foreign input reaches. (a) is better if any tool of ours
ever reads a non-machoNet marshal blob. Either beats silently consuming four
body bytes. Whichever is chosen, the doc comment at `value.rs:851`-`852` —
*"What the `0x7D` `extra` byte means is still undetermined"* — should be
replaced with the version semantics and the two addresses.

**P2 — name `MAX_FRAME`'s derivation and the 4-byte asymmetry.** `frame.rs:35`
says *"the native per-socket cap `SocketGPS` installs"*; add that it is
`SocketGPS.py:133` installed at `:202`/`:277`, that **carbonio compares
`plen + 4` and legacy compares `plen`**, and that this is why the payload
ceiling is `MAX_FRAME - 4`. The constant is right; the reason is worth
writing down, since this is exactly the sort of number a later pass "tidies".

**P3 — `MAX_PAYLOAD` is a pref default.** `frame.rs:33` calls 5,000,000 "the
practical payload limit". It is `prefs.GetValue('machoNet.dropPacketThreshold',
5000000)` (`machoNetTransport.py:157`), so a client can be configured above it.
Worth one sentence so nobody later treats it as enforced by the client.

**P4 — the `UNKNOWN_MASK` question, for whoever owns `tag.rs`.** The client
**discards** bit 7 of a tag byte. If we reject on it we are stricter than the
client; if we discard it we accept streams the client accepts. Neither is
wrong for a server, but the choice should be deliberate and commented, because
"the client ignores this" is not obvious from a constant named `UNKNOWN_MASK`
in someone else's codebase.

### Docs (proposals — these files are not mine)

**P5 — `docs/archive/wire-format.md`.** Three edits:

- **gap 4 can close.** Replace *"What the `0x7D` `extra` byte means is still
  undetermined"* with: `0x7D` is marshal **version 1**, the no-shared-map
  header; the extra byte is the version; a non-zero version means no mapcount
  word follows. Writer `0x1802da8f8`-`0x1802da955`, reader
  `0x1802e1728`-`0x1802e172c`, signature `blue.marshal.Save(object,
  callback=None, useChecksum=0, version=0, stringMap=None)` from
  `0x18097df10` / `0x180aed560`.
- **line 345 — "it is still our reading" — can be softened, carefully.** The
  honest wording is not "verified" but something like: *"Still our reading of
  the client — but for the framing, the header, the tag split, the opcode space
  and the dict pair order, a second independent implementation targeting an
  earlier 24.01 build arrives at the same answers (`docs/re/evejs-crosscheck.md`).
  Agreement across two builds is evidence the format is stable; it is not a
  capture."* Do **not** write "confirmed": nothing here was checked against a
  live server, and the two readings share an ancestor in the published PyRep
  folklore even where neither copied it.
- the **`useChecksum`** flag is the writer-side switch for tag `0x1C`, which
  gives `docs/archive/marshal-tags.md`'s `0x1C` entry its writer-side half.

**P6 — `docs/archive/marshal-tags.md` gap 10 can close**, same content as P5.

**P7 — `docs/reference/login-path.md` §4 H8 note 3.** The note is correct but understates
what it knows. Add: (a) the inner blob goes to the **stdlib** `marshal`, not
`blue.marshal` (`GPS.py:6` vs `:144`), so a hand-built CPython blob is enough
and no code object is needed; (b) element 2 of the tuple is not a flag that is
ignored but a value that **must** be `== 0` (`evecrypto/placebo.py:21`), and a
truthy value raises at `GPS.py:1012`, *outside* the try; (c) the recommended
payload is `("s" + u32le(2) + "{}", 0)` with context `{}`, and the absolute
minimum is `("N", 0)`. Cite `GPS.py:962`, `:977`, `:1009`-`:1014`, `:1026`.

**P8 — `docs/archive/evejs-261018-reference.md`** (not mine to edit; corrections for
the coordinator to fold in):

| section | correction |
|---|---|
| §1 | *"client build 3396210 ← SAME TARGET AS OURS"* — ours is **3619879**. Same version, 223,669 builds earlier. |
| §1 | *"Also harvested from **newer** clients"* — `3436472`, `3475087`, `3487903` are newer than **their** 3396210 and **all older than our 3619879**. Relative to us the risk is stale content, not post-24.01 content. |
| §2 GAP 1 | *"no OOB channel in the framing itself"* — there is one, bit 28, and the client decodes it on receive (`0x1803887aa`). machoNet never *sets* it (`machoNetTransport.py:493`), which is why their framing works. |
| §2 GAP 1 | the cap `10_000_000` is **not** the client's. The client's is **10,485,760 on `plen + 4`** (`SocketGPS.py:133`, enforced `0x180388761`). Their number is safe but invented. |
| §2 GAP 1 | *"violation ⇒ destroy socket"* describes their server. The **client**, on carbonio receive, logs and **continues** (`0x180388a08`), silently desynchronising. |
| §2 GAP 2 | *"object share … restricted to `blue.DBRowDescriptor` by identity"* is their implementation's scope. The client lets **eleven** tag handlers consume a shared-id entry. |
| §2 GAP 3 | **the important one.** *"the verification flag, hardcoded `false`"* / *"with verification off"* is a misreading. Placebo's `verify` is `(signedData[0], signedData[1] == 0)` (`evecrypto/placebo.py:21`) and `GPS.py:1012` raises on a falsy result **outside** the try. The element is a **required** `0`-equal value, not a disabled check; `false` works only because `False == 0` in Python 2. Sending `True` would be fatal and inexplicable. |
| §2 GAP 2 | add, as the one thing their codec told us that we did not have: **`0x7E` means marshal version 0** and `0x7D` version 1. They emit `0x7E`, which is what the client emits. |
| §2 GAP 6 | the rebind claim is **confirmed** — add that the retry lives only in `util.Moniker` (a raw bound object gets none), that `code = 0` in an `ErrorResponse` is sufficient and the class name never needs to cross the wire, and that the rebind arrives as `MachoResolveObject` then `MachoBindObject(bindParams, call)` with the original call piggybacked. |
| §2 Bonus | **`machoNet.GetInitVals` does not exist in V24.01** — zero hits in 12,774 `.pyc` files. `serviceInfo` is the whole return of `machoNet.GetServiceInfo` (`machoNet.py:2028`). The proposed "~145 vs our 175" cross-check compares three non-comparable populations and should be struck. |

**P9 — `GetServiceInfo` must be answered, and `{}` is the answer.** For
whoever owns the machoNet service module: `machoNet.py:2028` has no `except`
around it, so an exception aborts login, but the content is free and a **plain
empty dict** is both sufficient and preferable to the `util.CachedObject` the
real server returns. This belongs in `docs/reference/login-path.md` beside H10, because
it is a second mandatory call on the login path and the one our own census
does not list. If it is ever populated, values must be strings or `None` —
never ints — because the client calls `.startswith()` on them.

**P10 — `tools/scan_remote.py` has two blind spots**, and one of them hides a
login-critical call. Not my file; see §8 for the two idioms and the
consequence (`machoNet.GetServiceInfo` at `machoNet.py:2028` missing from
`findings/client-calls.tsv`). Whoever owns the census should decide the
`ConnectTo*Service` question explicitly rather than inherit it.

### Not a proposal: what agreement does *not* buy

Nothing here was tested against a live server, and six of our own claims above
rest on `docs/reference/framing.md`'s earlier pass rather than on bytes re-read in this
lane (bit 29, the carbonio writer at `0x180382d70`, the legacy writer, the
`20,000,000` native inflate cap, `use_carbonio`'s selection, and the
`recvpacketoob` sequence-number analysis). Those are flagged where they appear.
The M1 capture remains the oracle.
