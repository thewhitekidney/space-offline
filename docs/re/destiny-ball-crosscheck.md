# The ball record, cross-checked against a second implementation

`docs/archive/destiny-and-the-ballpark.md` §4.4 reads the ball record out of
`_destiny.dll` and marks **eleven fields `[I]`** — sizes and order from the
disassembly, semantics a best fit against the `destiny.Ball` attribute table.
That file also says, correctly, that there is "no Python source to port and no
marshal structure to mirror — this is a C++ serialiser, and it is the hard wall
`docs/archive/roadmap.md` M3 names."

EvEJS has to feed that same C++ serialiser. So it contains an independent
reading of the same wall. This file puts the two side by side.

**What was done, exactly.** Read their encoder, wrote down the field order and
the default values, compared against our disassembly table, kept the
disagreements. **No code was copied and none will be** — see §5. The output is
a corrected field list we implement from `_destiny.dll`.

Build difference applies throughout: theirs targets client **3396210**, ours is
**3619879**. Where the two disagree, the client wins
(`docs/archive/evejs-261018-reference.md` §0.3).

Contents: 1 the common header, 2 what changed in our doc, 3 flags and modes,
4 the mode tails, 5 the licence line, 6 what is still unknown.

---

## 1. The common header, both readings

Ours is byte offsets and register destinations from `0x1800989e0`. Theirs is a
sequence of typed writes. They line up field for field:

| # | our disasm | their write | verdict |
|---|---|---|---|
| 1 | 8 B → `rbp+0x00`, **itemID** `[V]` | `int64 itemID` | **agree** |
| 2 | 1 B → `rbp-0x7c`, **mode** `[V]` | `u8 mode` | **agree** |
| 3 | 4 B → `rbp-0x54`, `[I]` radius f32 | `f32 radius`, default **1** | **`[I]`→ confirmed** |
| 4 | 24 B → `rbp+0x38`, **position** 3×f64 `[V]` | `f64 x, y, z` | **agree** |
| 5 | 1 B → `rbp+0x158`, **flags**, bit 0 gates block 7 `[V]` | `u8 flags`, `IS_FREE = 0x01` | **agree, and named** |
| 6 | *if mode != RIGID* | rigid encoder stops here; free continues | **agree structurally** |
| 6a | 8 B → `rbp-0x20`, `[I]` mass f64 "pre-seeded from a constant" | `f64 mass`, default **1 000 000** (10 000 for container/wreck) | **`[I]`→ confirmed, constant supplied** |
| 6b | 1 B → `rbp-0x7b`, `[I]` a bool, init 0 | `u8 cloakMode` | **`[I]`→ named** |
| 6c | 8 B → `rbp+0x08`, `[I]` **followId** int64, init −1 | `int64 harmonic`, default **−1** | **disagree — ours was wrong, §2.1** |
| 6d | 4 B → `rbp-0x50`, `[I]` int32, init −1 | `int32 corporationID`, default **−1** | **`[I]`→ named** |
| 6e | 4 B → `rbp-0x4c`, `[I]` int32, init −1 | `int32 allianceID`, default **−1** | **`[I]`→ named** |
| 7 | *if flags & 1* | free ball always writes this block | **agree: bit 0 = `IS_FREE`** |
| 7a | 4 B → `rbp-0x60`, `[I]` int32, init 0 | `f32 maxVelocity` | **ours mistyped, §2.2** |
| 7b | 24 B → `rbp+0x20`, **velocity** 3×f64 `[I]` | `f64 vx, vy, vz` | **`[I]`→ confirmed** |
| 7c | 4 B → `rbp-0x64`, `[I]` f32 init `0x3f800000` = 1.0, "probably speedFraction" | `f32 inertia`, default **1** | **ours was wrong, §2.2** |
| 7d | 4 B → `rbp-0x68`, `[I]` int32, init 0 | `f32 speedFraction`, default **0** | **ours mistyped, §2.2** |
| 8 | gated on a config byte at `0x1800d42d1` | `u8 0xff` written unconditionally | **unresolved, §6** |

**Nine of eleven `[I]` rows now have a name, and every default value matches
an init value we read independently out of the DLL** — −1, −1, −1, 1.0, 0. Two
readings of one C++ serialiser agreeing on five arbitrary sentinels is not
coincidence.

---

## 2. Three corrections to `docs/archive/destiny-and-the-ballpark.md` §4.4

### 2.1 Row 6c is not `followId`

We guessed `followId` because the field is an int64 initialised to −1 and
because the same function has a fixup pass resolving `followId` to a pointer.
Their header says **`harmonic`**, and `followID` appears in their
**mode-specific tail**, not the common header.

Their reading is the right one, and *our own disassembly already said so*. The
fixup pass is gated on mode bitmask **`0x1032`** — bits 1, 4, 5, 12 =
FOLLOW, ORBIT, MISSILE, FORMATION. Those are **exactly** the four modes whose
tails push a `followID` (§4). A field in the common header would need no
mode gate. We had the evidence and drew the wrong conclusion from it.

### 2.2 Rows 7a / 7c / 7d: two mistypes and one swap

Our note on 7c reasons: "float32, init `0x3f800000` = 1.0 — a fraction,
probably `speedFraction`". The init value is right and the inference is
backwards. **A speed *fraction* defaults to 0** (a ship at rest); **inertia
defaults to 1**. So:

```
7a   f32 maxVelocity      (we read int32 — same 4 bytes, wrong type)
7c   f32 inertia      = 1 (we read speedFraction)
7d   f32 speedFraction= 0 (we read int32)
```

Three 4-byte slots we had as `int32, f32, int32` are **`f32, f32, f32`**. The
sizes were never wrong; only the types and one name.

### 2.3 Where *they* are wrong: `DstBallMode` 9 and 10

Their constants table:

```
… BOID 7, TROLL 8, MINIBALL 9,  FIELD 10,   RIGID 11, FORMATION 12
```

Ours, which §0's verified list cross-checked **three ways** against this
client:

```
… BOID 7, TROLL 8, FIELD 9,     MINIBALL 10, RIGID 11, FORMATION 12
```

**Positions 9 and 10 are swapped.** Ours stands: RIGID = 11 is independently
confirmed by `cmp byte ptr [rbp-0x7c], 0x0b` at `0x180098b1d`, and the mode
bound `cmp eax, 0xc; ja` fixes the range, so both ends of their table agree
with ours and only the middle differs. Their file cites "Carbon
`IDstConstants.h`" for a build three revisions older than ours.

Do not take their constants on trust. This is the §0.3 rule earning its keep
on the first real test: an agreement is worth something, a disagreement is
worth more, and the client decides.

---

## 3. Flags — our `[V]` bit 0, now with the whole byte

```
IS_FREE          0x01    gates the physics block (our block 7)  [V both]
IS_GLOBAL        0x02
IS_MASSIVE       0x04
IS_INTERACTIVE   0x08
IS_SPACEJUNK     0x10
HAS_MINIBOXES    0x20  ─┐
HAS_MINIBALLS    0x40   ├─ each appends a tail section (§4.2)
HAS_MINICAPSULES 0x80  ─┘
```

Our disassembly established "bit 0 gates block 7" as `[V]` without a name.
`IS_FREE = 0x01` names it and the semantics are self-consistent: a free ball
has velocity, inertia and a speed fraction; a rigid one does not.

Their comment notes that TQ's own inbound AddBalls records use flags **`0x09`**
for a proper WARP ball — `IS_FREE | IS_INTERACTIVE`. That is a claim about
CCP's server, unverifiable from our side, and recorded as a hypothesis worth
testing once we can send one.

---

## 4. The tails

### 4.1 Mode-specific, free balls

Written after the `0xff` byte, keyed on mode:

```
GOTO       f64 target{x,y,z}
FOLLOW     i64 followID, f32 followRange
FORMATION  i64 followID, f32 followRange, i32 effectStamp
MISSILE    i64 followID, f32 followRange, i64 modeOwnerID, i32 effectStamp,
           f64 target{x,y,z}
WARP       f64 target{x,y,z}, i32 effectStamp, f64 totalDistance,
           f64 minimumRange, i64 warpOwnerID
ORBIT      i64 followID, f32 followRange
MUSHROOM   f32 followRange, f64 span, i32 effectStamp, i64 modeOwnerID
TROLL      i32 effectStamp
STOP       (nothing)
```

Note which four push a `followID`: **FOLLOW, FORMATION, MISSILE, ORBIT** — the
`0x1032` bitmask from our own disassembly, independently derived. §2.1's
correction and this list confirm each other.

**STOP writing nothing is the important row.** A docked-to-undocked ship
sitting still is mode STOP, so the minimum viable ball has **no tail at all**
— header only. That is the smallest thing that can draw a ship, and it is now
fully specified except for §6.

### 4.2 Mini-geometry, gated on flag bits 5–7

Each present section is `u16 count` then that many records:

```
HAS_MINIBALLS     f64 center{x,y,z}, f32 radius
HAS_MINICAPSULES  f64 hemisphereA{x,y,z}, f64 hemisphereB{x,y,z}, …
HAS_MINIBOXES     f64 corner{x,y,z}, localX{x,y,z}, localY{x,y,z}, localZ{x,y,z}
```

Counts are `u16` (their array clamp is `0xffff`, consistent). **A ship needs
none of these** — they are collision geometry for stations and structures.
This is very likely our unresolved block 8, and §6 says how to settle it.

---

## 5. The licence line, and why this file is the whole of what we take

EvEJS is **AGPL-3.0-only**; our tree is under its own licence (`LICENSE.md`)
and holds no EvEJS code. Their
codec is itself ported from EVEmu, so the chain runs further back than the
archive. Copying their encoder — or transcribing it closely enough to be a
derivative work — would relicense our tree by accident.

It is also unnecessary, which is the better argument. **What we needed was
never the code; it was the names of eleven fields.** Those are facts about
CCP's serialiser, and a fact learned from a second reading is ours to use.
This document is that extraction, complete: the layout above is everything
their 23 KB encoder knows that we did not.

One trap to name explicitly. Their `visibility/README.md` carries a **"Carbon
license notice"** claiming `carbonengine/destiny` v3.1.1 is MIT © CCP Games,
which would make that part freely portable. `docs/archive/evejs-261018-reference.md`
§8 already lists this claim as uncorroborated. CCP does not publish
carbonengine under MIT. **Treat the notice as false and the code as AGPL**;
an MIT header inside an AGPL archive asserting a licence for someone else's
proprietary engine is the single most expensive thing in the tree to believe.

---

## 6. Still unknown, and the one experiment that closes it

1. **Block 8 / the `0xff` byte.** Our disassembly has block 8 gated on a config
   byte at `0x1800d42d1` (`[I]` the `destiny.settings` orientation flag). Their
   encoder writes a bare `0xff` unconditionally in that position and no
   orientation. Both cannot be right. Candidates: a section terminator, an
   orientation-present sentinel, or a build difference.
2. **Whether the header is complete for 3619879.** Nine named fields is not a
   proof of no tenth.
3. **Flags `0x09` for WARP** — their claim about TQ, untested.

All three are settled by the experiment `docs/archive/destiny-and-the-ballpark.md` §8
already specifies, and this cross-check does not replace it: drop a loose `.py`
override into the client, build a `destiny.Ballpark`, `AddBall` one ball with
known field values, `WriteFullStateToStream`, write the bytes to
`C:\eve-logs\`. That produces a **byte-exact reference blob from the client's
own serialiser**, which outranks both readings on this page.

What the cross-check changes is the *cost* of that experiment. We now know
which values to set and what to expect, so one dump confirms or refutes
nineteen fields at once instead of hunting for them. **The blocker was never
the experiment; it was not knowing what to look for.**
