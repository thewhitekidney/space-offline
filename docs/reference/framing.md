# machoNet TCP framing (client build V24.01, 3619879)

How a payload is delimited on the wire. Closes gap 3 of `docs/archive/wire-format.md`.
Established by disassembly of `/client/bin64/blue.dll` (ImageBase
`0x180000000`), not by capture; every claim names an address.

Independently re-verified here before implementing: the length mask and the
`+4` at all three reader sites (`and ecx, 0xfffffff` / `add rcx, 4` at
`0x180388640`, `0x180388701`, `0x18038873e`) and the OOB flag test
(`test dword [rax], 0x10000000` at `0x1803887aa`).

## Which path is live

`_slsocket` carries **two** packet implementations, selected per call by a
global at `0x180b01a3c` (`use_carbonio`). `autoexec.py` sets it from
`iocp.UsingIOCP()`, which is true when `socketIO = iocp` — **which this
install has**, so the **CarbonIO path is live**. The legacy stacklessio path is
documented too, because it is an independent implementation of the same format
and therefore the best cross-check available short of a capture. They agree.

| method | dispatcher | legacy | carbonio |
|---|---|---|---|
| `sendpacket` | `0x18036a5c0` | `0x180395150` | `0x180382d70` |
| `recvpacket` | `0x18036a770` | `0x180395550` | `0x180387620` |
| `recvpacketoob` | `0x18036a810` | `0x1803955a0` | `0x180387620` |
| `setmaxpacketsize` | `0x18036aa40` | `sock+0xa4` | `sock+0x11d0` |

## The frame

```text
off  size      field
---  --------  ------------------------------------------------------------
0    4         word0, u32 LITTLE-ENDIAN
                 bits 0..27  plen — bytes that FOLLOW word0 (excludes itself)
                 bit  28     OOB_PRESENT  (0x10000000)
                 bit  29     ZLIB         (0x20000000)
                 bits 30..31 unused in this build
--- only if bit 28 ---
4    4         oobLen, u32 LE
8    oobLen    oob bytes
--- payload ---
P    plen-(P-4)   where P = 4 + (bit28 ? 4 + oobLen : 0)
```

- **Total on the wire = `4 + plen`.** The prefix does not count itself.
- **No magic, no version, no sequence number, no checksum.** `plen` plus four
  flag bits is the whole header. (An FNV-1a hash is computed at `0x18038999f`
  but only feeds a debug string; it is not transmitted.)
- With no flags — the normal machoNet case — the frame is literally
  `u32 len` followed by `len` bytes.
- Bit 29 set: the payload is `u32 rawLen` + a **zlib** stream. `plen` describes
  the *compressed* payload, so **framing is unaffected by compression**.
  *Still unimplemented, and still never observed:* the first real client sent
  compressed payloads with this bit **clear**, compressing the marshal stream
  itself instead (`docs/reference/login-path.md` §8). `eve-inflate` now exists, so closing
  this is only an API change — `try_split_frame` hands out borrowed slices and
  an inflated frame needs an owned buffer. `FrameError::Compressed` until then,
  rather than a silent misread.

### Writer, `0x180382d70`

```text
0x180382dd4  mov  dword ptr [rdi + 4], esi ; word1 = oobLen
0x180382dd7  bts  eax, 0x1c                ; set bit 28
0x180382de2  mov  dword ptr [rdi], eax     ; word0
...
0x180382df1  mov  dword ptr [rdi], r14d    ; no-oob: word0 = dataLen, no flags
0x180382df4  mov  esi, 4                   ; payload offset = 4
```

A zero-length oob takes the no-oob branch (`test esi, esi`), so bit 28 is never
set with an empty section.

### Reader, `0x1803885a0` — streaming reassembly

The mask/`+4` appears at three identical sites. The complementary nibble is the
flag field, confirmed by the header rewrite at `0x1803898d5`
(`and dword ptr [rax], 0xf0000000` — exactly four flag bits preserved).

Flag decode at `0x1803887aa`: bit 28 set → oob ptr = `buf + 8`, oob len =
`word1`, payload offset = `oobLen + 8`; clear → payload offset = `4`.

**Partial and pipelined frames are both handled.** Fewer than 4 bytes →
buffer and return; 1..3 header bytes → take exactly the missing ones; body
incomplete → append and return; body complete → consume and **loop**, so
multiple frames in one TCP read are fine.

Framing only applies once the socket is in *packet mode*, a latch at
`socket+0x11d9` set lazily on the first `recvpacket` (`0x180387816`). Mixing
`recv()` and `recvpacket()` is refused outright.

### The legacy path agrees, and is stricter

Legacy writer `0x180395150` writes a bare `u32` payload length with **no flag
bits**; `0x180390df0` contains the narrowing guard proving the field is 32-bit
(`mov eax, ecx; cmp rax, rcx; jne throw`). Legacy reader `0x180391db0` takes
the **whole u32** as the length — it does not mask — so it cannot read a frame
with bit 28 or 29 set.

**Therefore: never set a flag bit.** All-zero flags is valid for both
implementations.

## Size caps

| where | address | compares | limit |
|---|---|---|---|
| carbonio send | `0x180385f3f` | whole frame | `socket+0x11d0` |
| carbonio recv | `0x180388761` | `plen + 4` | `socket+0x11d0` |
| legacy send/recv | `0x18039524f` / `0x180391dfa` | payload only | `socket+0xa4` |
| zlib inflate/deflate | `0x18038b015` / `0x18038ae20` | raw size | **20,000,000** |
| field width | — | `plen` | `2^28 - 1` |

Defaults: carbonio **1,000,000** (`0x180387826`), legacy **1,048,576**. Both
are overridden by `SocketGPS.SocketTransportFactory.MaxPacketSize` =
**10,485,760**, written through `setmaxpacketsize`. So the effective client cap
is **10 MiB including the header**.

`machoNet.dropPacketThreshold` = 5,000,000 is a *separate, smaller* Python-side
limit on the marshal blob, applied before `sendpacket`. Treat 5,000,000 as the
practical payload ceiling.

Behaviour on violation differs and matters: carbonio **receive** oversize only
logs and resets `[pkt+8] = 0` (`0x180388a22`) — it **discards and continues**,
without closing or resynchronising. A bad length silently desynchronises the
stream. Carbonio **send** oversize raises; legacy **receive** oversize raises
`too large a packet detected at %d bytes, max is %d`, which
`SocketGPS` catches by substring.

## Compression is native here, and off

`machoNetTransport.py` enables the native path only when
`iocp.UsingCompression()`, which needs `/iocpCompress` or a pref. **This
install has neither**, so the Python zlib path (level 1, `}`/`~` first-byte
discriminator) is the live one, exactly as `docs/archive/wire-format.md` says. The native
path is documented because the receiver honours bit 29 regardless of what the
sender does.

Compressed payload = `u32 rawLen` (LE) + a zlib stream (`deflateInit2_` with
the default wrapper, not raw deflate). The header rewrite at `0x1803898d5`
preserves the flags and rewrites only `plen`.

**This resolves a loose end in `docs/archive/wire-format.md`:** the 6.5 KB function at
`0x180579a50` that owns the `header crc mismatch` string, previously noted only
as "far outside the marshal cluster", **is zlib's `inflate`** — it also
references `incorrect header check` and `unknown compression method`
(re-verified here). The string is zlib's own gzip-header-CRC error text, which
confirms there is no CCP header CRC anywhere. The earlier conclusion was right;
this is the reason.

## Out-of-band data: three unrelated things

1. **`oob`, field 6 of the `MachoPacket` 14-tuple** — inside the marshal blob,
   nothing to do with framing.
2. **The frame's OOB section (bit 28)** — real, but exposed to Python only as
   the `header` argument of `SocketPacketTransport.Write`, and
   `machoNetTransport.Write` never passes one. **So machoNet never sets bit
   28.** The receive side does decode it.
3. **`MSG_OOB`/`SO_OOBINLINE`** — TCP urgent data, present as constants,
   unused by the packet path.

**`recvpacketoob` is not a separate channel.** `0x18036a810` is byte-for-byte
`recvpacket` except one instruction (`mov byte [rsp+0x20], 1` vs `bl` = 0); the
flag only selects whether the result is a bare string or a
`(packet, oob, sequence)` tuple. On the legacy path that `oob` is hard-wired to
`None` (`0x1803955d5`).

## The sequence number is local

`recvpacketoob`'s third element is a per-socket ordinal the client assigns
itself (`inc qword [rbx+0x38]` at `0x180388908`), used only to re-order packets
delivered by CarbonIO's completion workers. **It is not a wire field; a server
must not emit one.**

## Corrected layer stack

```text
TCP stream
 └─ frame: u32 word0 (plen:28 | oob:1 | zlib:1 | unused:2) [+ u32 oobLen + oob]
     └─ [bit 29] u32 rawLen + zlib            <- native; OFF on this install
         └─ symmetric encryption               <- see below
             └─ [Python] optional zlib, first byte not in {0x7D, 0x7E}
                 └─ marshal blob
                     └─ MachoPacket object
```

**The encryption layer sits between the frame and the marshal blob.**
`machoNet.get_gps_map()` maps `tcp:packet:server` to
`SecureSocketPacketTransportFactory`, whose `Read`/`Write` are
`EncryptedRead`/`EncryptedWrite`:

```python
def EncryptedRead(self):
    return self.cryptoContext.SymmetricDecryption(self.UnEncryptedRead())
def EncryptedWrite(self, packet, header=None):
    return self.UnEncryptedWrite(self.cryptoContext.SymmetricEncryption(packet), header)
```

Framing is unaffected — the prefix measures whatever bytes reach `sendpacket` —
but the `}`/`~` discriminator applies to the **decrypted** payload. Under the
Placebo pack (see `docs/archive/session-flow.md`) `SymmetricEncryption`/`Decryption` are
identity functions, so for us frame payload *is* the marshal blob. Under
CryptoAPI it is not.

## What a server must do

- **Write:** `u32_le(len(blob)) + blob`.
- **Read:** 4 bytes, `plen = u32le & 0x0FFFFFFF`, then `plen` bytes. If
  `plen != u32le` the peer set a flag bit.
- Never set bits 28..31. Never emit a sequence number.
- Keep payloads under 5,000,000; hard cap 10,485,760 total.
- Accept pipelined frames and arbitrary segmentation on read.

## Gaps

1. **Bits 30/31.** The flag field is four bits wide, but nothing in this build
   reads or writes them. Strings `snappy` and `Snappy compression is no longer
   supported` suggest a retired second compression type; inference, and
   irrelevant since this build can neither emit nor decode it.
2. **Whether CCP's live server sets bit 29** toward this client. Nothing
   negotiates it; we simply never set it.
3. **CarbonIO's `WSARecv` buffer sizing** — does not affect the format.
4. **Behaviour after a bad length** beyond "discards and continues".
