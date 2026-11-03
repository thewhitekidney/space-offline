# Warp, as an implementation spec

> **Demoted 2026-10-24** — the align-phase section (around line 153) already carries its own SUPERSEDED banner. Current truth: [port-tracker.md](../port-tracker.md) §§47–56. Kept for history; do not brief a lane from this file.

Lane: `warp-destiny`. Research only — this file and `docs/lanes/warp-destiny.md`
are the whole output; no code was written and no other file was touched.

**The question this answers:** when the player right-clicks a planet and picks
"Warp To", what does the server have to send so the ship aligns, warps, and
arrives — and what does it have to send so the ship *stops* warping.

Marking convention, as the brief requires:

- `[V]` verified against **our** client (V24.01 / 3619879): a named `.pyc`
  with `co_lnotab` source lines, or a `bin64/_destiny.dll` virtual address.
- `[V2]` a third party's decode of a different build (EvEJS, 3396210).
- `[I]` inferred; says from what.
- `[A]` an assumption we are choosing to ship; says what falsifies it.

All Python line numbers come from `tools/dis.py` / `tools/call_lines.py`, i.e.
from `co_lnotab`, so they match the client's own tracebacks. All DLL addresses
are **virtual addresses** (image base `0x180000000`), from `.pdata` function
bounds — not raw file offsets.

Contents: 0 headline, 1 minimum viable warp, 2 the inbound call, 3 the destiny
action, 4 the ball state and the WARP tail, 5 the phase model, 6 warp speed
and ranges, 7 arrival, 8 the tunnel and the sound, 9 falsifiable tests,
10 corrections to existing documents, 11 open questions ranked by cost,
12 what to read first next time.

---

## 0. The headline, in six lines

1. **`WarpTo` is a real native `destiny.Ballpark` method and we now have its
   exact signature.** The Python wrapper's `PyArg_ParseTuple` format string is
   **`"Lddd|di"`** — `WarpTo(ballID: i64, x: f64, y: f64, z: f64,
   minimumRange: f64 = 20000.0, warpFactor: i32 = 20)`. Read out of
   `_destiny.dll` at `0x1800acf88` (the format string), with the wrapper at
   `0x18006e6d0` and the implementation at `0x1800425e0` `[V]`. This
   **supersedes** `docs/lanes/movement.md` §3's "there is no native `WarpTo`
   entry point to probe" and §8 item 4.
2. **One message does the whole warp.** The client's native engine runs
   align → activate → accelerate → cruise → decelerate → park, from the single
   `WarpTo` action. There is no per-phase server message, and nothing in the
   client asks for one. `Ballpark::ActivateWarp` (`0x180042863`) is the proof:
   it recomputes the goto target, stamps `effectStamp = ballpark.currentTime`
   and writes `warpDistance` itself `[V]`.
3. **And the warp ends on its own too — the client leaves WARP mode without
   being told.** This was the question I expected to answer the other way, and
   it does not. `Ballpark::Evolve` calls the warp integrator once per tick with
   its "interpolate only" flag **false** (`0x1800357b1`: `mov byte ptr
   [rsp+0x28], 0`, then `call 0x180042b20`) `[V]`; the integrator's arrival
   branch snaps the ball to the destination with zero velocity, restores
   `isMassive`, fires the `DST_WARPEXIT` event, and calls `Ballpark::StopBall`
   (`0x1800430af → 0x180043920`), which zeroes the mode fields and
   **tail-jumps to `SetMode(DSTBALL_STOP)`** (`0x180043a08`) `[V]`. So the
   second server message is a **reconciliation**, not a requirement: the ship
   will stop warping either way. Send it anyway (§7.3) — our authoritative
   position has to agree with the client's, and `Stop` is idempotent.
4. **The WARP mode tail is 52 bytes and is now `[V]`, not `[V2]`** — read
   field by field out of the mode jump table at `0x18009966c`, entry 3 →
   `0x1800994f3`. EvEJS got the sizes and the order right and **the last field
   wrong**: it is not `warpOwnerID`, it is `warpFactor`, the ship's warp speed
   in milli-AU/s (§4.2).
5. **Warp speed is an integer the server computes**, not an attribute the
   client reads. `eve/common/script/util/eveCommonUtils.py:443` (body at `:447`) is
   `AUPerSecondToDestinyWarpSpeed(auPerSecond) = int(auPerSecond /
   const.warpSpeedToAUPerSecond)` = `int(AU/s * 1000)` `[V]` — shipped shared
   code, which is rule zero earning its keep. The AU/s itself is
   `attributeBaseWarpSpeed` (**1281**) × `attributeWarpSpeedMultiplier`
   (**600**) `[V]`.
6. **The client applies the "warp to within X" offset itself.** Send the true
   destination and `minimumRange` separately; `ActivateWarp` moves the target
   back along the line toward the ship by `minimumRange` (`0x1800428ef`–
   `0x180042951`) `[V]`. If we pre-offset *and* pass `minimumRange`, the ship
   lands twice as short.

---

## 1. Minimum viable warp — the thing to implement

State the server needs per warping ship: `target` (the true destination,
metres), `min_range` (metres), `warp_factor` (int, milli-AU/s), the ship's
position at command time, and the tick at which it will declare arrival.

### 1.1 The sequence

| # | when | what the server sends | stamp |
|---|---|---|---|
| 1 | `CmdWarpToStuff` arrives on bound `beyonce` | reply `None` (the client discards it; it must still be *sent*) | — |
| 2 | immediately after | `DoDestinyUpdate([(T, ('WarpTo', (shipID, tx, ty, tz, min_range, warp_factor))), (T, ('SetSpeedFraction', (shipID, 1.0)))], 0)` — **the throttle is not optional**, see the §5.1 correction | `T` = next unused tick |
| 3 | same batch or the next tick | `('OnSpecialFX', (shipID, None, None, None, None, 'effects.Warping', 0, 1, 0))` — §8 | `T` or `T+1` |
| 4 | at the arrival tick `T_a` | one batch, in this order: `('SetBallPosition', (shipID, ax, ay, az))`, `('SetBallVelocity', (shipID, 0.0, 0.0, 0.0))`, `('Stop', (shipID,))` | `T_a` |
| 5 | same stamp or later | `('OnSpecialFX', (shipID, None, None, None, None, 'effects.Warping', 0, 0, 0))` — optional, the client does it itself | `T_a` |

Step 2 and step 3 may share one stamp: a batch may hold more than one entry
provided every stamp is equal and no entry is a `SetState`
(`docs/lanes/movement.md` §2) `[V]`. `OnSpecialFX` is in
`nonDestinyCriticalFunctions`, so it is *not* time-synchronised and is safe in
the same batch (`docs/archive/destiny-and-the-ballpark.md` §4.2) `[V]`.

Step 4 is reconciliation, not animation — the client reaches mode `STOP` by
itself at its own computed arrival tick (§7). Its job is to make the *server's*
position authoritative again and to make a timing disagreement invisible.
`Stop`'s Python implementation is `if ball.mode != 2: StopBall(ball)`
(`0x18009300e`) `[V]`, so it is a **no-op on a ball that has already arrived** —
which is the normal case and why a late step 4 is free. `Stop` is in
`scatterEvents`, so it also lights the HUD's stateful buttons and
`tacticalNavigationService` `[V]`.

**Do not send step 4 early.** An early `Stop` *does* abort a warp in flight,
and an early `SetBallPosition` teleports a ship that is still moving. §1.3 is
about erring late for exactly this reason.

### 1.2 The arrival point

```
dir = normalize(ship_position_at_command_time - target)
arrival = target + dir * min_range
```

This is exactly what `Ballpark::ActivateWarp` computes, except that it uses the
ship's position at the *activation* tick rather than at the command tick
(`0x18004287e`–`0x180042951`) `[V]`. For the default right-click warp
`min_range == 0` and the two agree exactly. For a "warp to within 100 km" over
several AU the align drift is a few km on a 100 km offset — invisible, and
step 4's `SetBallPosition` reconciles it anyway.

### 1.3 How long the warp takes

The engine's own model, read from `Ballpark::OnDeactivatingWarp`
(`0x180042b20`–`0x180042bcf` for the coefficients, `0x180042c04`–`0x180042cc9`
for the times) `[V]`. Constants read from `.rdata` at the cited addresses:

```
k_accel = warp_factor * 0.001                      # AU/s   [0x1800ad7f0 = 0.001]
k_decel = min(2.0, warp_factor * 1/3000)           # AU/s   [0x1800ad8a0 = 2.0, 0x1800ad7e8 = 1/3000]
                                                   #  i.e. min(k_accel/3, 2)
v_cruise = min((d + 1.0) * k_accel*k_decel/(k_accel+k_decel),
               k_accel * AU)                       # m/s    [0x1800ad940 = 149597870700.0]
t_accel  = ln(v_cruise / k_accel) / k_accel
t_decel  = ln(v_cruise / k_decel) / k_decel
t_cruise = d/v_cruise - 1/k_accel - 1/k_decel + 1/v_cruise
```

where `d` is the straight-line distance from the ship's position at activation
to the offset arrival point, in metres. This is CCP's published warp model and
our DLL agrees with it coefficient for coefficient. **Total flight time is
`t_accel + t_cruise + t_decel`, plus the align phase.**

The align phase is not predictable server-side from first principles (§5.1),
so for the first implementation:

> **SUPERSEDED, 2026-10-22 — the align phase *is* predictable, and the number
> was already in our hull table.** The client activates the warp at
> `speed² > (0.75 * maxVelocity)²` (`docs/port-tracker.md` §34) and a ship
> from rest reaches ¾ of its commanded speed at `-ln(0.25) * tau`, which is
> the definition of the published align time. The gate and the fitting
> window's figure are the same quantity, asserted over all 19 hulls by
> `the_published_align_time_is_when_the_warp_gate_opens`. `destiny::warp_flight_seconds`
> therefore budgets `align_seconds * 3 + 2 s` rather than a flat fifteen; the
> factor covers a ship that must reverse an existing velocity before it can
> point at the target. §36.6 has the whole correction. The original text
> follows, because the reasoning about *which direction to err in* is still
> exactly right:
>
> `[A]` **Schedule `T_a` at `ceil(t_align_budget + t_accel + t_cruise +
> t_decel)` ticks with `t_align_budget = 15 s`, and always round up.**
> Destiny ticks at 1 Hz (`simulationTimeStep = 1000`), so one tick of slack is
> one second of slack.
>
> *Why late is safe and early is not:* the client arrives and stops on its own
> (§7), so a late step 4 lands on a ball already in mode `STOP` at the right
> position and is a no-op. An **early** `Stop` aborts the warp wherever the
> ship happens to be, which looks like a teleport and leaves the server's and
> the client's positions disagreeing.
>
> *Falsified by:* a human test where the tunnel visibly lingers for more than
> ~3 s after the ship is clearly stationary at the destination, which means
> the align budget is too generous and should be modelled instead (§5.1).

---

## 2. The inbound call

### 2.1 It is on `beyonce`, as a bound call

`michelle.GetRemotePark()` returns `self.__bp.remoteBallpark` — the bound
`beyonce` object itself, not a wrapper (`michelle.py:511-516`) `[V]`. So warp
arrives exactly where `CmdGotoDirection` already does:
`dispatch::handle_bound_call` → `Registry::dispatch` with
`service = "beyonce"`, `bind_params = Some(solarsystemID)`. **No new transport
work.** `findings/client-calls.tsv` agrees (`beyonce CmdWarpToStuff …
michelle.py:949 … 2 args`).

### 2.2 The two methods

| inbound call | positional | keyword | types | call site `[V]` |
|---|---|---|---|---|
| `CmdWarpToStuff(subject, subjectID, **kw)` | 2 | `minRange`, `fleet`, plus whatever `michelle.SetFleetFormationSettings` consumes | `subject`: py2 `str`; `subjectID`: i64; `minRange`: float **or int**; `fleet`: bool | `michelle.py:949`, forwarded verbatim with `CALL_FUNCTION_KW` |
| `CmdWarpToStuffAutopilot(destinationID)` | 1 | — | i64 | `autopilot.py:575`; wrapped in `try/except UserError` |

`michelle.CmdWarpToStuff` (`michelle.py:945-949`) is a pure forwarder: it calls
`self.SetFleetFormationSettings(kwargs)` at `:946`, then
`bp.CmdWarpToStuff(subject, subjectID, **kwargs)` at `:949` and discards the
result `[V]`. `CmdWarpToExternalDungeon(dungeonInstanceIdentifier, **kwargs)`
(`:951-956`) just calls `CmdWarpToStuff(WarpSubjects.EXTERNAL_DUNGEON, …)`
`[V]`. **There is no local mutation and no local prediction** — same finding
as every other `Cmd*`.

Both return values are discarded, so `None` is the right reply — but it must be
*sent*, or the calling tasklet blocks to `machoTimeout`.

### 2.3 `subject` is a short closed vocabulary

`ballparkCommon/parkhelpers/warpSubjects.py` is **shipped shared code** and
defines `WarpSubjects` exhaustively `[V]`:

```
'epinstance' 'item' 'bookmark' 'char' 'scan' 'coords' 'spawnpoint'
'launch' 'district' 'operationDungeon' 'moonMiningPoint'
'externalDungeon' 'mercenaryDen'
```

`WARP_ABLE_SUBJECTS` is the list of all thirteen. **For tonight, `'item'` and
`'bookmark'` are the whole surface a human tester can reach** (§2.4); every
other subject should be answered with a user error rather than a silent `None`,
so a mistake shows up.

### 2.4 Exactly what each UI path sends

Every row `[V]`, read from the call site's bytecode:

| UI action | call |
|---|---|
| right-click a celestial → **Warp To** | `CmdWarpToStuff('item', itemID, minRange=<default>)` — `movementFunctions.WarpToItem`, call at `:494` |
| right-click → **Warp to within N km** | the same, with `minRange = float(ranges[i])` — `menusvc.WarpToMenu` (`:2763`) builds the submenu from `movementFunctions.GetWarpToRanges()` (`:380`) |
| right-click a **bookmark** → Warp To | `CmdWarpToStuff('bookmark', bookmarkID, minRange=warpRange, fleet=fleet)` — `movementFunctions.WarpToBookmark` (`:452`), call at `:462` |
| bookmark in the **People & Places** window | `CmdWarpToStuff('bookmark', bookmarkID)` — **no kwargs at all** (`addressBookEntries.py:163`) |
| **scan result** | `CmdWarpToStuff(subject, subjectID, minRange=minRange, fleet=fleet)` — `menusvc._WarpXToScanResult`, call at `:1704` |
| fleet **Warp Fleet** / **Warp To Member** | `menusvc.WarpFleet` `:3256`, `WarpToMember` `:3265`, `WarpFleetToMember` `:3274`, all with `minRange=` and some with `fleet=True` |
| **autopilot** | `CmdWarpToStuffAutopilot(destID)` — `autopilot.py:575` |
| mission journal "warp to launch" | `CmdWarpToStuff('launch', launchID)` (`journal.py:555`) |
| moon mining point | `CmdWarpToStuff('moonMiningPoint', moonID, minRange=…, fleet=…)` (`movementFunctions.py:524`) |
| operation site | `CmdWarpToStuff('operationDungeon', siteID)` (`movementFunctions.py:505`) |
| dev profiling helper | `CmdWarpToStuff('item', location)` (`profiling.py:344`) — a one-liner test hook |

So: **`minRange` can be absent, an `int`, or a `float`.** Coerce to `f64`
before it reaches an action, and treat a missing kwarg as `0.0`, not as the
native's `20000.0` default (§6.3).

### 2.5 Client-side gates the human tester will hit first

All local, all `[V]`, none of them our server's business — but they decide
whether the menu entry even appears:

- `celestialCheckers._CanWarpToItem` (`:2229`): not the active ship, same
  system, **not already warping**, and `session.isInWarpRange(distance)`.
- `sessionChecker.isInWarpRange(distance)` (`:93`) is
  `distance is not None and distance > appConst.minWarpDistance` — **150 km**
  (`appConst.py:1747`, `minWarpDistance = 150000`).
- `sessionChecker.isPilotWarping` (`:169`) is `ball.mode == DSTBALL_WARP`.
- `michelle.IsPositionWithinWarpDistance(position)` (`:896-908`) is the same
  150 km test against `michelle.GetMyPosition()`.

**Therefore the human test needs a celestial more than 150 km from the undock
point.** And if warp menu entries stay greyed out with the hint
`UI/Menusvc/MenuHints/YouAreInWarp` after the ship has visibly stopped, the
ball is stuck in mode WARP — which, given §7, means the client never reached
its arrival condition at all (a bad `warpFactor`, or a `maxVelocity` of zero
making the dropout threshold unreachable), not that we forgot a message.

---

## 3. The destiny action

### 3.1 The shape

```python
(stamp, ('WarpTo', (ballID, x, y, z, minimumRange, warpFactor)))
```

- `ballID`: Python `long` / `PyValue::Int`
- `x`, `y`, `z`: `PyValue::Real`, the **true** destination in metres
- `minimumRange`: `PyValue::Real`, metres; optional, native default `20000.0`
- `warpFactor`: `PyValue::Int`, **int32**; optional, native default `20`

`[V]`, three ways:

1. **The format string.** `destiny.Ballpark.WarpTo`'s Python wrapper is
   `0x18006e6d0`; it calls the arg-parsing shim `0x180092830`, whose
   `PyArg_ParseTuple` format string lives at `0x1800acf88` and reads
   **`Lddd|di`**. The two optional slots are pre-seeded before the parse:
   the `d` slot with the double at `0x1800ad908` = **20000.0** and the `i` slot
   with the immediate **`0x14` = 20** (`0x180092851`).
2. **The call into the implementation.** `0x1800928d2` calls `0x1800425e0`
   (`Ballpark::WarpTo`) passing, in Microsoft x64 order: `rcx` = the ballpark,
   `rdx` = a pointer to the parsed `int64` ballID, `xmm2`/`xmm3` = x/y,
   `[rsp+0x20]` = z, `[rsp+0x28]` = the double, `[rsp+0x30]` = the int.
3. **The implementation's own use of each argument.** `0x1800425e0` NaN-checks
   the three coordinates with three calls to the same validator
   (`0x18004260c`, `0x18004261d`, `0x180042637`) — which is exactly the
   `NaN in WarpTo. Ignored` string `docs/lanes/movement.md` §3.1 already
   found — then at `0x1800427bc`–`0x18004281a` writes
   `gotoX/Y/Z = (x,y,z)`, `effectStamp = -1`, `followId_slot = max(0.0, arg6)`,
   `ownerId_slot = (int64)max(arg7, 1)`, `followRange = 0`, and calls
   `SetMode(3)`.

`WarpTo` is in `Park.scatterEvents` (`docs/archive/destiny-and-the-ballpark.md` §4.2)
`[V]`, so `RealFlushState` calls the native `_parent_WarpTo(*args)` **and** then
`sm.ScatterEvent('OnBallparkCall', 'WarpTo', args)`. Two consumers read that
scatter, and they confirm the argument order independently `[V]`:

- `spaceMgr.OnBallparkCall` (`:524`): `if functionName == 'WarpTo' and
  args[0] == session.shipid:` then `x, y, z = args[1], args[2], args[3]` and
  stores them in `warpDestinationCache[3]`. **This is where the HUD's "Warp
  destination: <name>" text and the warp-distance readout come from**, so
  getting `args[1..3]` wrong costs the overlay, not just the physics.
- `positionalControl.OnBallparkCall` (`:234`) only checks `args[0]`.

### 3.2 The decode site, named

`michelle.py` `Park.RealFlushState` (`:1507`) → `scatterEvents` branch →
`self._parent_WarpTo(*args)` → `destiny.Ballpark.WarpTo` wrapper
`0x18006e6d0` → `Ballpark::WarpTo` `0x1800425e0`. The chunked `.pdata` bounds
for that implementation are `0x1800425e0-0x180042658`,
`0x180042658-0x18004276e`, `0x18004276e-0x18004278e`,
`0x18004278e-0x180042824`, `0x180042824-0x180042844` — one function in five
chunks, which is why a single `--disasm` of the entry address stops early.

### 3.3 Hazards

- **Non-finite arguments are discarded silently.** Three NaN checks, no Python
  exception, no log on the default channel. Guard in the encoder.
- **`warpFactor <= 0` is silently replaced by 1** (`0x180042660`:
  `mov esi, 1; cmovg esi, eax`) `[V]` = 0.001 AU/s. A warp at one
  thousandth of an AU per second looks exactly like a ship that is stuck, so a
  zero `warpFactor` is the most expensive single mistake available here.
- **A negative `minimumRange` is clamped to 0** (`0x180042672`:
  `xorps xmm9,xmm9; maxsd xmm9, [rsp+0xd8]`) `[V]`.
- **`WarpTo` on a ball that is already in WARP mode with the same target is a
  no-op**, including no mode-change event (`0x18004278e`–`0x1800427ba`
  compares `mode == 3` and all three target components) `[V]`. A re-send with
  a *different* target does restart the warp.

---

## 4. The ball state: the WARP mode tail

### 4.1 Where it goes

Exactly where `Mode::Goto`'s 24-byte tail goes today, i.e. after the
`formation_id` byte that `Park::encode` writes following the `isFree`-gated
physics block. The mode byte is **3**.

The mode dispatch is a jump table at `0x18009966c`, 13 `uint32` RVAs indexed by
the mode byte, reached via `cmp edx, 0xc; ja <error>` at `0x180099037` `[V]`.
Resolved:

| mode | target | tail |
|---|---|---|
| 0 GOTO | `0x1800994ce` | 24 B target |
| 1 FOLLOW | `0x180099054` | i64 followId, f32 followRange |
| 2 STOP | `0x1800990fc` | none |
| **3 WARP** | **`0x1800994f3`** | **§4.2** |
| 4 ORBIT | `0x180099054` | identical to FOLLOW |
| 5 MISSILE | `0x180099448` | i64 followId, f32 followRange, i64 ownerId, i32 effectStamp, 24 B target |
| 6 MUSHROOM | `0x18009959e` | — |
| 7 BOID | `0x180099633` | **the error path** |
| 8 TROLL | `0x1800990d6` | i32 effectStamp |
| 9 FIELD | `0x180099633` | **the error path** |
| 10 MINIBALL | `0x1800990fc` | none |
| 11 RIGID | `0x1800990fc` | none |
| 12 FORMATION | `0x180099096` | i64 followId, f32 followRange, i32 effectStamp |

(Modes 7 and 9 landing on the same block as `mode > 12` is a free cross-check
on `docs/re/destiny-ball-crosscheck.md` §2.3 that neither reading had: in
**this** build, two of the thirteen modes have no reader at all.)

### 4.2 The WARP tail, 52 bytes, in order

```
f64  targetX          -> ball+0x1a8   (ball.gotoX)
f64  targetY          -> ball+0x1b0   (ball.gotoY)
f64  targetZ          -> ball+0x1b8   (ball.gotoZ)
i32  effectStamp      -> ball+0x88    (ball.effectStamp)
f64  warpDistance     -> ball+0x110   (no Python member)
f64  minimumRange     -> ball+0x90    (ball.followId's slot, read as a double)
i64  warpFactor       -> ball+0x98    (ball.ownerId's slot)
```

`[V]`, from the five stream reads at `0x1800994f3`, `0x180099513`,
`0x180099539`, `0x180099559`, `0x180099579` — sizes `0x18, 4, 8, 8, 8` and
destinations as above.

**The field names are not guesses.** The offsets were cross-checked against the
`PyMemberDef` tables in `.rdata`, decoded by matching each member-name string
pointer and reading the `type`/`offset` pair that follows it `[V]`:

```
effectStamp   T_INT        0x88     mode          T_INT       0x238
followId      T_LONGLONG   0x90     speedFraction T_FLOAT     0xf4
ownerId       T_LONGLONG   0x98     followRange   T_FLOAT     0xf8
harmonic      T_LONGLONG   0xa0     mass          T_DOUBLE    0x100
allianceID    T_INT        0xc0     gotoX/Y/Z     T_DOUBLE    0x1a8/0x1b0/0x1b8
corporationID T_INT        0xc4     radius        T_FLOAT     0xdc
isFree        T_BOOL       0xd0     maxVelocity   T_FLOAT     0xe0
```

and `Ballpark.currentTime` is `T_INT` at `0x1c`, which is where the blob's
4-byte header lands (`0x180097913`) — an independent confirmation of
`docs/archive/destiny-and-the-ballpark.md` §4.4's `[I]` on that field.

Then the two semantic names that no member table could give:

- **`ball+0x110` is `warpDistance`** and **`ball+0x98` is `warpFactor`**, both
  `[V]` from the client's own format string: `Ballpark::OnDeactivatingWarp`
  logs `"Ship stuck in extended warp %I64d: warpFactor:%f warpDistance:%f
  time:%f interpolating:%d"` (string at `0x1800a77e0`) and the call at
  `0x180042fbb` loads its arguments from `[rdi+0x18]` (ballID),
  `(double)[rdi+0x98]` (warpFactor), `[rdi+0x110]` (warpDistance). The
  `cvtsi2sd` at `0x180042bbb` proves `+0x98` is an **integer** read
  numerically, not an owner id.
- **`ball+0x90` is `minimumRange`**, `[V]` because `Ballpark::WarpTo` writes
  its 6th argument there as a double (`0x1800427f7`) and
  `Ballpark::ActivateWarp` reads it back with `movsd xmm0, [rbx+0x90]`
  (`0x1800428ef`) and multiplies a unit vector by it.

**Nothing in the client's Python reads `ball.ownerId`** (the only `ownerId` hits
are a local variable in `state.py:1179`) `[V]`, and `ball.followId` is read
only under modes FOLLOW/ORBIT (`movementFunctions.py:251-255`) and resolved to
a pointer only for the mode bitmask `0x1032` = {FOLLOW, ORBIT, MISSILE,
FORMATION} `[V]`. So the slot reuse is safe: no consumer will read a double as
a ball id.

### 4.3 The Rust shape

Drops into `destiny.rs`'s existing `Mode` with no other change:

```rust
/// `DSTBALL_WARP`, 52-byte tail. Reader at `_destiny.dll` 0x1800994f3.
Warp {
    /// The target as the ball currently holds it. After the client's own
    /// `ActivateWarp` this is ALREADY offset by `minimum_range` -- see §4.4.
    target: [f64; 3],
    /// `ballpark.currentTime` at activation; <= 0 means "still aligning".
    effect_stamp: i32,
    /// Metres remaining at activation. 0.0 before activation.
    warp_distance: f64,
    /// Metres to stop short of `target`. Applied once, at activation.
    minimum_range: f64,
    /// Warp speed in milli-AU/s. `int(AU_per_s * 1000)`. Must be >= 1.
    warp_factor: i64,
}
```

`code()` returns `3`. Encode order is the table in §4.2; `effect_stamp` is a
**signed** 4-byte field (the client stores `-1` and counts down), so
`i32::to_le_bytes`, not `u32` as `Mode::Troll` uses.

### 4.4 The trap in serialising a ball that is *already* warping

`ReadFullStateFromStream` **does not call `ActivateWarp`** — it only assigns
fields, and `ActivateWarp` runs only on the align→warp transition. So for a
full-state blob (or `AddBalls2`) describing a ship mid-warp, the server must
write the **post-activation** values:

- `target` = the already-offset arrival point, **not** the celestial's centre
- `effect_stamp` = the tick at which the warp activated (positive)
- `warp_distance` = the distance from the ship's position *at activation* to
  that arrival point

If the server instead writes the unoffset target with a positive
`effect_stamp`, the offset is never applied and the ship flies to the
celestial's centre — for **every observer**, because this is the shared park
state, which is exactly the failure mode the brief warns about.

For a ball that has been commanded to warp but has not activated yet, write
`effect_stamp = -1`, `warp_distance = 0.0`, and the unoffset target; the
client's own align path will then do the right thing. `[I]` from the fact that
`WarpTo` leaves the ball in precisely that state.

**Cheapest safe option for tonight:** do not put a warping ball in a full
`SetState` at all. Encode it as `Mode::Stop` at its last known position until
the warp ends. An observer who enters the grid mid-warp then sees the ship
stationary rather than desynced, and nothing in the park is mis-framed. `[A]`,
falsified the moment a second client needs to *see* a third ship warp.

---

## 5. The phase model — one message, and the answer is not ambiguous

**The server sends one `WarpTo` and the client animates every phase locally.**

### 5.1 Align, in the client

Each tick, for a ball in mode WARP with `effectStamp <= 0`, the engine runs
`AlignForWarp` (the chunk `0x180037357-0x180037605`) `[V]`:

1. `dir = normalize(gotoTarget - position)`; `vdir = normalize(velocity)`;
   `err = 1.0 - dot(dir, vdir)` (`0x180037368`–`0x1800374bd`). Note that a
   **zero velocity does not normalise** — the `ucomisd`/`je` at
   `0x18003744e` skips the division — so a stationary ball has `dot = 0`
   and `err = 1`.
2. If `|err| >= 0.01` (not yet aligned; the tolerance is the `f64` at
   `0x1800ad808`, loaded at `0x1800374d7`) **or**
   `speed² <= (0.75 * maxVelocity)²`
   — the constant at `0x1800ad848` is **0.5625 = 0.75²**
   (`0x1800374ed`–`0x180037507`) — then: `effectStamp -= 1`
   (`0x1800375ce`: `lea eax,[rsi-1]; mov [rbx+0x88], eax`) and the ball is
   steered toward `&ball->gotoX` by the ordinary sub-warp goto path
   (`0x1800375ec`, with `r9 = rbx+0x1a8`).

   **CORRECTION, 2026-10-22.** An earlier revision of this line said "this is
   why a warping ship accelerates to 75% of max speed on its own with no
   `SetSpeedFraction` from us". **That is wrong, and it shipped a defect**
   (`docs/port-tracker.md` §34): the "ordinary sub-warp goto path" is
   throttle-scaled. `0x180038870` opens with `movss xmm3, [r8+0xf4]`
   (`speedFraction`), `movss xmm8, [r8+0xe0]` (`maxVelocity`) and
   `mulsd xmm8, xmm3` **[V]**, so a ball with `speedFraction == 0` is commanded
   to zero, never accelerates, never passes either gate, and warps only when
   the 180-tick watchdog in step 3 force-activates it — three minutes of
   "Aligning" for every hull equally. **The server must send
   `SetSpeedFraction 1.0` with the `WarpTo`**, and 1.0 exactly: the gate is
   strictly greater than 0.75 and a goto approaches its commanded speed from
   below, so any throttle ≤ 0.75 can never cross it.
3. Otherwise, or once `|effectStamp| > 180` ticks — `cmp eax, 0xb4`
   at `0x180037522`, which is the
   `AlignForWarp: Forcing warp as %I64d has been aligning for %d ticks.`
   string — the engine fires `DST_WARPACTIVATION` and calls
   `ActivateWarp` (`0x1800375b3 → 0x180042850`).

So **the three-minute align timeout is the client's**, and `effectStamp`
counting *down* from `-1` is the align tick counter. There is nothing for us to
send during align.

### 5.2 Activation, in the client

`Ballpark::ActivateWarp`, `0x180042863` `[V]`, in one pass:

```
dir      = normalize(position - gotoTarget)
gotoXYZ  = gotoTarget + dir * minimumRange     # the offset, applied HERE
effectStamp   = ballpark.currentTime           # 0x180042959-0x180042960
warpDistance  = |gotoXYZ - position|           # 0x18004296d-0x180042982
isMassive     = 0                              # 0x18004298a  (+0xd2)
ball+0x478    = 0                              # 0x180042991
```

Three consequences worth writing down:

- **The offset is the client's job** (§0.6).
- **`effectStamp` becomes positive exactly at activation**, which is the gate
  `Warping.WarpLoop` uses: `if ball.effectStamp > 0 and not
  hasSentActiveWarpEvent:` (`Warp.py:576`) `[V]`. That is the moment
  `OnWarpActive` scatters and the tunnel progress bar starts.
- **A warping ball is non-massive**, i.e. collisionless, and
  `OnDeactivatingWarp` sets `isMassive = 1` again on arrival
  (`0x18004309b`) `[V]`. If our server ever does its own collision work, warp
  is the exemption.

Python sees the activation: the native dispatches it to
`Park.OnActivatingWarp(srcID)` (`michelle.py:2146-2151`), which calls
`ball.EnterWarp()` (`ship.py:470-479`) to raise `boosters.warpIntensity` and
set the `IsWarping` controller variable `[V]`.

### 5.3 Accelerate, cruise, decelerate, park

All inside `Ballpark::OnDeactivatingWarp` (`0x180042b20`, 1555 bytes) `[V]`,
which despite its name is the per-tick warp integrator: it recomputes
`k_accel`, `k_decel`, `v_cruise`, `t_accel`, `t_cruise`, `t_decel` from
`warpFactor` and `warpDistance` every tick, picks the phase, and writes the
interpolated position into `*r15` and velocity into `*r14` (the caller's output
vectors). The arrival branch writes position = `gotoXYZ` exactly and velocity =
`(0,0,0)` (`0x18004300b`–`0x18004302b`).

The Python side is a pure observer. `Warping.WarpLoop` (`Warp.py:555`) polls
every 500–1000 ms (`blue.synchro.SleepSim`) and derives everything it shows
from the ball and the ship model `[V]`:

- `OnWarpStarted` at `:561`, before the loop
- `OnWarpActive` + `warp_signals.on_warp_activated` when `effectStamp > 0`
  (`:576`)
- `OnWarpStarted2` and the `ship_warp_explosion_play` / `ship_warp_wind_play`
  audio when the model's speed fraction crosses `0.005` (`:607`)
- `OnWarpDecelerate` when it falls back under `1e-06`
- `OnWarpArrived` when the model's speed stops increasing and
  `space.GetWarpDistance() < 10 000 000` (`:631`)
- `OnWarpFinished` + `StopWarpIndication` + `ship_warp_wind_stop` after the
  loop (`:658`)

**Every one of those is derived locally. None of them is a server message, and
none of them is waiting for one.**

### 5.4 What the server must know versus what the client will fake

| quantity | server | client |
|---|---|---|
| the destination | **must know** — it is in the action | reads it off the ball |
| the align phase | need not model it | runs it, with its own 180-tick timeout |
| the 75%-speed-to-warp rule | **must open the throttle** (§5.1 correction) | enforces it |
| position during warp | **may treat as undefined**; reconcile at arrival | integrates it analytically |
| the offset arrival point | **must know**, to send at arrival (§1.2) | computes the same thing at activation |
| total flight time | **must know**, to pick `T_a` (§1.3) | computes it every tick |
| the arrival tick | **must know**, to reconcile (§1.3) | detects it itself and stops |
| collisions during warp | exempt | sets `isMassive = 0` |
| tunnel, sound, camera shake, HUD text | nothing | all of it |

---

## 6. Warp speed and ranges

### 6.1 Warp speed: an integer we compute, from two dogma attributes

`eve/common/script/util/eveCommonUtils.py:443`, body at `:447` `[V]`:

```python
def AUPerSecondToDestinyWarpSpeed(auPerSecond):
    return int(auPerSecond / const.warpSpeedToAUPerSecond)
```

with `appConst.warpSpeedToAUPerSecond = 0.001` (`appConst.py:1815`) `[V]`. So
**`warpFactor = int(au_per_second * 1000)`**, which the DLL confirms from the
other side: `k_accel = warpFactor * 0.001`.

The AU/s comes from two attributes, and the client's own fitting window shows
the formula `[V]`:
`navigationPanel.SetWarpSpeedText` (`:95`) is
`baseWarpSpeed.value * warpSpeedMultiplier.value * appConst.AU`.

| attribute | id | `dogma/const.py` |
|---|---|---|
| `attributeBaseWarpSpeed` | **1281** | `:1116` |
| `attributeWarpSpeedMultiplier` | **600** | `:1109` |
| `attributeWarpScrambleStatus` | **104** | `:1107` — autopilot reads it to explain a refusal (`autopilot.py`) |

So: `warp_factor = round(attr(1281) * attr(600) * 1000)`, clamped to `>= 1`.
We already serve godma attributes as `{attributeID: float}`, so this costs us
nothing new. If a type is missing either attribute, `[A]` use **1.0 × 3.0 =
3.0 AU/s → `warp_factor = 3000`**, a cruiser-ish speed; falsified by a ship
that visibly warps at a speed unrelated to its fitting-window figure.

`attributeWarpSpeedAdd` (1950) and `attributeWarpSpeedBonus` (624) also exist
and are presumably modifiers; the fitting panel reads neither, and nothing read
here needs them.

### 6.2 Range constants, all from `appConst.py` `[V]`

| name | value | line |
|---|---|---|
| `minWarpDistance` | **150 000** m | 1747 |
| `minWarpEndDistance` | **0** | 1748 |
| `maxWarpEndDistance` | **100 000** m | 1736 |
| `minAutoPilotWarpInDistance` | **10 000** m | 1740 |
| `warpJitterRadius` | **2 500** m | 1814 |
| `warpSpeedToAUPerSecond` | **0.001** | 1815 |
| `invulnerabilityWarpingIn` / `Out` | 10 000 / 5 000 | 1716 |
| `shipNotWarping` / `shipWarping` | 0 / 1 | 1857 / 1858 |
| `warpTypeNone` / `Regular` / `Forced` | 0 / 1 / 2 | 1862–1864 |

### 6.3 The "warp to within X" offsets

`movementFunctions.GetWarpToRanges()` (`:380-388`) `[V]`:

```python
[const.minWarpEndDistance,
 (const.minWarpEndDistance / 10000 + 1) * 10000,
 (… + 2) * 10000, (… + 3) * 10000, (… + 5) * 10000, (… + 7) * 10000,
 const.maxWarpEndDistance]
```

With `minWarpEndDistance = 0` that is **`[0, 10000, 20000, 30000, 50000,
70000, 100000]`** — the seven submenu entries, and the exact set of
`minRange` values a right-click can produce.

The plain "Warp To" entry passes
`GetDefaultActionDistance('WarpTo')` →
`defaultRangeUtils.FetchRangeSetting('WarpTo')`, whose default is
`DEFAULT_RANGES[WARP_TO_SETTING] = const.minWarpEndDistance` = **0**
(`defaultRangeUtils.py:13-15`) `[V]`. It is a per-character client setting, so
a player who has set a default warp range will send that instead.

Note the **two different defaults**: the client's plain Warp To sends
`minRange = 0.0`, while `movementFunctions.WarpToBookmark`'s Python default and
the native `WarpTo`'s own optional-argument default are both `20000.0`. Treat a
missing `minRange` kwarg as **`0.0`**, since the only call sites that omit it
(`addressBookEntries.py:163`, `journal.py:444`, the dungeon warps) are
"go there", not "stop 20 km short". `[A]`; falsified by a bookmark warp that
lands 20 km off when the player expected to land on top of the beacon.

For `CmdWarpToStuffAutopilot` the server picks the range: `[A]`
`minAutoPilotWarpInDistance = 10000`, which is what the constant exists for and
what autopilot behaviour requires (you must land near enough to a gate to
jump). Falsified by an autopilot that lands too far from the gate to jump.

---

## 7. Arrival

**What ends a warp: the client, by itself, on a condition it computes — and
then the server, to reconcile.**

### 7.1 The client's own arrival

The warp integrator is `Ballpark::OnDeactivatingWarp`, `0x180042b20`. Despite
the name it runs every tick for a ball in mode WARP, and its sixth argument is
an **interpolate-only** flag. It has exactly two callers `[V]`:

| caller | flag | what it is |
|---|---|---|
| `0x1800357cc`, inside `Ballpark::Evolve` (`0x180035590`, the function that does `inc dword ptr [r14+0x1c]` on `currentTime`) | `0` at `0x1800357b1` | **the authoritative tick** |
| `0x18000ae91` | `1` at `0x18000ae67` | a position query between ticks, for rendering |

`Evolve` first reads `ecx = ball->effectStamp` and does
`test ecx, ecx; js <align path>` (`0x18003578c`–`0x180035794`) — so a
**negative** `effectStamp` means "still aligning" and a non-negative one means
"warping", and the elapsed warp time it passes in is
`(currentTime - effectStamp) * tickInterval` (`0x18003579a`–`0x1800357b1`)
`[V]`. That is the whole reason `effectStamp` exists in the WARP tail.

**The arrival condition** (`0x180042f4b`–`0x180042f73`) `[V]`:

```
dropout_speed = min(maxVelocity * 0.5, 100.0)      # m/s
arrive when   current_warp_speed < dropout_speed
```

— the `0.5` and the `100.0` are the doubles at `0x1800ad840` and
`0x1800ad8e8`, the `mulsd`/`movsd` operands at `0x180042f56` and
`0x180042f5e`. This is CCP's
published warp-dropout rule, confirmed coefficient for coefficient, and it
means **a ball with `maxVelocity == 0` can never arrive.** Serve a real
`maxVelocity` or the ship warps forever.

On arrival, with the interpolate-only flag false, the engine (`0x180043008`
onward) `[V]`:

1. writes position = the (already offset) `gotoX/Y/Z` and velocity = `(0,0,0)`
   into the caller's output vectors (`0x18004300b`–`0x18004302b`);
2. dispatches `DST_WARPEXIT`, which reaches
   `Park.OnDeactivatingWarp(srcID)` (`michelle.py:2154-2159`) →
   `ball.ExitWarp()` (`ship.py:483-492`), dropping `boosters.warpIntensity`
   and clearing the `IsWarping` controller variable;
3. sets `isMassive = 1` again (`0x18004309b`);
4. calls `Ballpark::StopBall` (`0x1800430af → 0x180043920`), which unregisters
   followers for the mode bitmask `{FOLLOW, ORBIT, MISSILE, FORMATION}`, zeroes
   `effectStamp`, `followId`, `ownerId` and `followRange` — i.e. **the whole
   WARP tail** — and **tail-jumps to `Ball::SetMode(DSTBALL_STOP)`**
   (`0x1800439f6`–`0x180043a08`).

`SetMode` then sees `old == 3, new != 3` and fires `OnExitWarp`
(`0x18000a152`–`0x18000a1bc`) `[V]`, `Warping.WarpLoop`’s
`while ball.mode == destiny.DSTBALL_WARP` (`Warp.py:572`) falls through, and
the Python tail runs: `OnWarpFinished`, `effects.Warping` stopped
(`Warp.py:654`), `ship_warp_wind_stop`, `space.StopWarpIndication()`
(`Warp.py:658`) `[V]`.

That `SetMode` is reached by a **tail `jmp`**, not a `call`, which is why a
naive enumeration of `call` sites concludes that no tick path changes a ball’s
mode. There are exactly two such tail jumps in the DLL, `0x180043a08`
(`StopBall`) and `0x180043887`; both go to `SetMode`. **If you enumerate
callers of anything in this DLL, search `e9` as well as `e8`.**

### 7.2 Where the ship ends up

At `target + normalize(pos_at_activation - target) * minRange`, mode `STOP`,
velocity zero. For the default right-click warp (`minRange = 0`) that is the
celestial’s exact centre, which is not what TQ does — TQ scatters the warp-in
with `warpJitterRadius = 2500` (`appConst.py:1814`). Adding a random offset of
up to 2.5 km to the target we send is one line and it is the difference between
ships stacking on a single point and ships landing on a grid. `[A]` do it;
nothing falsifies it, it is a choice.

### 7.3 So why send anything at all

Because **the server is the authority on position and it has just lost track of
one.** Three reasons to send step 4 of §1.1 anyway:

1. **Reconciliation.** Our server must end up believing the ship is at the
   arrival point with zero velocity, and the cheapest way to be sure the client
   agrees is to say so. `Stop` is `if ball.mode != 2: StopBall(ball)`
   (`0x18009300e`) `[V]` — a no-op on a ball that has already arrived — and
   `SetBallPosition`/`SetBallVelocity` on a stationary ball at the position it
   is already at are no-ops in effect.
2. **The observers.** Every other client in the park has its own copy of the
   warping ball and will arrive at its own tick; a stamped batch is what makes
   them agree.
3. **It is the only lever we have if the client *doesn’t* arrive.** A zero
   `maxVelocity`, a `warpFactor` of 1, or a warp whose target the ship can
   never reach all leave the ball in mode WARP indefinitely (§7.1), and the
   client has no recovery of its own: its only timeout is the 180-tick align
   forcing (§5.1), and `"Ship stuck in extended warp"` is a log line, not a
   fix.

**Nothing bad happens if step 4 is late. Something visible happens if it is
early** (§1.1).

---

## 8. The tunnel and the sound

The warp tunnel is `effects.Warping`, class `Warping(ShipEffect)` with
`__guid__ = 'effects.Warping'` in
`eve/client/script/environment/effects/Warp.py` `[V]`. It is reached only
through `OnSpecialFX`: the guid is const index 31 of `fxSequencer`'s module
const table and a member of `FX_PROTECTED_EFFECT_GUIDS` `[V]`, and `grep` over
all 12,774 modules finds `'effects.Warping'` as a *trigger* in exactly two
places, both of which only ever **stop** it or re-dispatch it:

- `spaceMgr.DoBallsAdded` (`:267-268`): when a ball arrives **already in**
  `DSTBALL_WARP` and it is our own ship, the client fakes
  `OnBallparkCall('WarpTo', (ball.id, ball.gotoX, ball.gotoY, ball.gotoZ))` and
  then calls `OnSpecialFX(ball.id, None, None, None, None, 'effects.Warping',
  0, 1, 0)` itself `[V]`. That is the reconnect-into-warp path.
- `Warping.Prepare` (`Warp.py:43-48`): if the effect's ship is **not** the
  player's, it immediately stops `effects.Warping` and starts
  `effects.WarpOut` instead `[V]`.

The second one is the load-bearing observation: the client is written to
receive `effects.Warping` for *every* warping ship and to convert the remote
case into a WarpOut flash itself. So:

> `[A]` **The server sends, for every ship entering warp:**
> `(stamp, ('OnSpecialFX', (shipID, None, None, None, None, 'effects.Warping',
> 0, 1, 0)))`
>
> and, optionally, the `start = 0` form at arrival (`WarpLoop` already does
> this itself at `Warp.py:654`).
>
> *Falsified by* finding a dogma effect whose guid resolves to
> `effects.Warping` through `fsdBuiltData.client.effects.GetEffectGuids`, which
> would mean the trigger is an `OnDogmaEffect`/effectID rather than a guid
> string. That check was not run; it is open question 11.3.

**ARITY, SETTLED, 2026-10-22.** `Park.OnSpecialFX` is `argcount 15` (`self`
plus fourteen) and the `Park` class body builds it with `MAKE_FUNCTION 5`,
whose five defaults are `-1, None, None, 0, None` **[V]** — so **fourteen
parameters, nine required**. `docs/re/jump-gates.md` §6's "fourteen
positionals" and this section's "nine" were never in conflict, and the
implementation withheld the tunnel for a round on the belief that they were.
`docs/port-tracker.md` §35 has the whole episode. Also checked, because it is
the guard that could have swallowed the event in silence:
`res:/staticdata/traveleffects.fsdbinary` holds only the jump family, so
`IsTravelEffect('effects.Warping')` is **false** and
`fxSequencer.OnSpecialFX:480` does not return early **[V]**.

`Park.OnSpecialFX` (`michelle.py:2087`) takes
`(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid, isOffensive,
start, active, duration=-1, repeat=None, startTime=None, timeFromStart=0,
graphicInfo=None)` and forwards all of it to
`sm.ScatterEvent('OnSpecialFX', …)` `[V]`; `fxSequencer.OnSpecialFX` (`:441`)
has the same signature `[V]`. Nine positional arguments is the minimum.

The remote ships' flashes the client raises by itself, for free:
`ship._SetInitialState` (`:337-353`) fires `effects.WarpIn` for a non-ego ball
that is added in WARP mode `[V]`, and `Warping.Prepare` fires
`effects.WarpOut` as above.

Sounds — all client-local, all driven off the ball and the ship model, none of
them ours: `ship_warp_explosion_play`, `ship_warp_wind_play`,
`ship_warp_wind_stop` in `WarpLoop` `[V]`; `msg_AutoPilotWarpingTo_play` and
`msg_AutoPilotWarpingToStation_play` from `autopilot.AP_WarpToDestID` `[V]`.

---

## 9. Falsifiable tests

Written so they assert the client's contract, not our implementation.

**T1 — the WARP tail round-trips at 52 bytes.** `Park::encode` of a one-ball
park with `Mode::Warp { target: [1.0,2.0,3.0], effect_stamp: -1,
warp_distance: 0.0, minimum_range: 20000.0, warp_factor: 3000 }`, then
`Park::decode`, is the identity; and the encoded length is exactly 52 bytes
longer than the same park with `Mode::Stop`. Fails if a field is mistyped or
the order is wrong.

**T2 — `effect_stamp` is signed.** A park with `effect_stamp: -1` encodes the
four bytes `ff ff ff ff` and decodes back to `-1`, not `4294967295`. This is
the one field where copying `Mode::Troll`'s `u32` would compile and be wrong.

**T3 — the action's argument types.** The `PyValue` list produced for
`Action::WarpTo` is `[Int, Real, Real, Real, Real, Int]` in that order, and the
method name marshals as a Python 2 `str` (`pyutil::bstr`), matching
`Bootstrap::to_push`'s `bstr("SetState")`. A `Real` where the format string
wants `i`, or an `Int` where it wants `d`, is a `TypeError` inside
`RealFlushState`'s `try/except` — a logged traceback and a silently skipped
action.

**T4 — no non-finite coordinate can leave the encoder.** For every
`Action::WarpTo` the encoder rejects or clamps NaN and ±inf in `x`, `y`, `z`,
`minimum_range`. The client discards these with no exception and no log, so an
unguarded NaN is an invisible no-op.

**T5 — `warp_factor >= 1`.** No `Action::WarpTo` is ever emitted with
`warp_factor <= 0`. The client would substitute 1 (= 0.001 AU/s) and the ship
would appear frozen.

**T6 — every warp has a terminator, and it is not early.** A loopback test
that issues `CmdWarpToStuff('item', id)` and then drains the push queue asserts
that the transcript contains a `WarpTo` action **and**, at a strictly later
stamp, a `Stop` (or a `SetState` whose ball is not in `Mode::Warp`) for the
same ball, **and** that the gap between the two stamps is at least
`t_accel + t_cruise + t_decel` for the `warp_factor` and distance used. The
client would recover from a missing terminator (§7.1) but our own position
would stay wrong; it cannot recover from an early one.

**T6b — `max_velocity` is never zero on a ball we tell to warp.** The client's
arrival condition is `speed < min(maxVelocity * 0.5, 100)`, so a ball with
`maxVelocity == 0` warps forever (§7.1). Assert it in the encoder, not in a
test on one path.

**T7 — stamps.** The `WarpTo` batch and the arrival batch carry distinct,
strictly increasing stamps, and neither is below the last `SetState`'s stamp.
One distinct stamp per notification or the client calls `OnFatalDesync`
(`docs/archive/destiny-and-the-ballpark.md` §4.2).

**T8 — the arrival point matches the client's formula.** Given a ship at `p`
and a target `t` with `min_range = r`, the position the server sends in step 4
equals `t + normalize(p - t) * r` to within 1 m, and equals `t` exactly when
`r == 0`. Fails if we pre-offset the target as well as passing `min_range`.

**T9 — flight time is monotone in the right direction.** For fixed
`warp_factor`, the computed `T_a - T` grows with distance; for fixed distance
it shrinks as `warp_factor` grows; and it is never zero. A sanity harness, not
a precision claim.

**T10 — `minRange` arrives as an int and survives.** A `CmdWarpToStuff` whose
`minRange` kwarg is the Python int `20000` produces the same action as the
float `20000.0`. `journal.py:550` already proves the client sends bare ints for
the analogous `CmdFollowBall` range.

**T11 — the subject vocabulary is closed.** Every string in
`WarpSubjects.WARP_ABLE_SUBJECTS` is either handled or answered with a user
error; an unknown subject is never answered with a bare `None`.

**One test that needs the client, not a loopback:** warp, and confirm from the
client log that `OnWarpStarted`, `OnWarpActive`, `OnWarpDecelerate`,
`OnWarpArrived` and `OnWarpFinished` all scatter, in that order. All five come
from `WarpLoop` reading only the ball, so seeing all five is proof the tail and
the action are both right.

---

## 10. Corrections to existing documents

1. **`docs/lanes/movement.md` §3, the `WarpTo` row, and §8 item 4.** "There is
   no native `WarpTo` entry point to probe — `destiny.Ballpark` exposes none"
   is **wrong**. `WarpTo` is in the Ballpark method table — its name string is
   at `0x1800aa518`, registered at `0x18006beda` with wrapper `0x18006e6d0` —
   between `MissileFollow` and `GotoPoint`. The same registration run gives
   `FollowBall` → `0x18006e610`, `MissileFollow` → `0x18006e670`, `GotoPoint`
   → `0x18006e730`. The method table is built at runtime by `lea`, not stored
   as pointers in `.data`, which is why a search for pointers to the name
   string finds nothing and why the earlier probe concluded it was absent.
   **`WarpTo` is directly probeable** with the existing
   `tools/client-overrides/destiny_dump.py` harness, which would turn §4.2 from
   `[V]`-by-disassembly into `[V]`-by-readback in one run.
2. **`docs/re/destiny-ball-crosscheck.md` §4.1, the WARP row.** The last field
   is **not** `warpOwnerID`. It is `warpFactor`, an int64 in milli-AU/s, read
   with `cvtsi2sd` and multiplied by `0.001` to get AU/s. The sizes and the
   order are right; the name is wrong, and it is the one field whose wrong
   value silently destroys the warp. `totalDistance` is the client's
   `warpDistance` and `minimumRange` is confirmed.
3. **`docs/re/destiny-ball-crosscheck.md` §6 item 3**, "Flags `0x09` for WARP,
   their claim about TQ, untested" — still untested here, but note that the
   flags byte is not mode-dependent in the reader, and a ship that is warping
   must have `IS_FREE` set because the physics block carries its velocity. The
   `IS_INTERACTIVE` half of `0x09` is unrelated to warp.
4. **`docs/archive/destiny-and-the-ballpark.md` §4.4 row 1 of the stream header**, the
   `[I]` on "it is the simulation tick": `Ballpark.currentTime` is `T_INT` at
   offset `0x1c` in the `PyMemberDef` table, and the header's four bytes are
   written to `ballpark+0x1c`. That `[I]` can be promoted to `[V]`.
5. **`docs/archive/destiny-and-the-ballpark.md` §4.4 table**, the mode bound: modes 7
   (BOID) and 9 (FIELD) share the jump-table slot with `mode > 12`, i.e. they
   have **no reader** in this build. Worth a line, because "mode is 0..12" is
   not the same as "eleven modes have tails".
6. **A method note for anyone reading `_destiny.dll` after me.** The claim
   "no tick path changes a ball's mode" is false and I believed it for an hour,
   because `Ballpark::StopBall` reaches `Ball::SetMode` by a **tail `jmp`**
   (`0x180043a08`) rather than a `call`. An `e8`-only caller enumeration misses
   it, and the whole of §7 depends on not missing it. Search `e9 rel32`
   alongside `e8 rel32`; in this DLL there are exactly two tail jumps to
   `SetMode` and both matter.

---

## 11. Open questions, ranked by what they cost if wrong

**11.1 The align budget (§1.3). Cost: a stale server-side position, or a
teleport.** Now that §7.1 is settled this is cheaper than it looked: the client
arrives on its own, so an over-long budget costs us only the window in which
the server's idea of the ship's position is stale. The server cannot predict the client's align duration, and the
client will not tell it. A generous budget is safe but ugly; a modelled one
needs the ship's `maxVelocity`, agility and starting velocity and will still
drift. The honest fix is a server-side align model that mirrors
`AlignForWarp`'s two conditions — and the real fix, later, is for the server to
run the same analytic trajectory and treat the client as a renderer. Cheapest
mitigation tonight: over-budget, and include `SetBallPosition` +
`SetBallVelocity` in the arrival batch so a late terminator is invisible.

**11.2 `warpFactor` for a ship whose attributes we do not serve. Cost: a ship
that looks frozen in warp, reported as a hang.** Mitigated by the clamp in T5
and the 3000 default in §6.1. Check `attributeBaseWarpSpeed` (1281) is actually
present in `findings/corpus-typedogma.tsv` for the starter hulls before
shipping; if it is missing, the fallback is load-bearing rather than defensive.

**11.3 Whether `effects.Warping` is really guid-triggered from the server
(§8). Cost: a silent warp — exactly the "reported as a bug in the morning"
case.** The reasoning is strong (`Warping.Prepare`'s remote-ship branch only
makes sense if the server sends the guid for every ship) but it is `[A]`. One
grep of `fsdBuiltData.client.effects` for an effectID mapping to
`effects.Warping` settles it. If we send it and something else also sends it,
the worst case is `fxSequencer` merging two activations of the same guid on the
same ship, which its `Trigger.__eq__` is built to handle.

**11.4 Fleet warp (`fleet=True`) and `fleetFormationSettings`. Cost: nothing
tonight.** Out of scope: answer `fleet=True` by warping only the caller's ship.
`spaceMgr.OnRemoteMessage` handles an `'FleetWarp'` message carrying a
`warpInfoDict` with a `celestialID` (`:418-422`), so there is a second,
separate mechanism for telling fleet members where they are going; it is not
needed for one ship.

**11.5 The `warpDistance` the server should write for a mid-warp full state
(§4.4). Cost: a park-wide desync — but only once an observer needs to see a
third party warp.** Avoidable tonight by encoding warping balls as
`Mode::Stop` in `SetState`.

**11.6 Whether `Ballpark.warpSpeed` (a `T_DOUBLE` at ballpark offset `0x50`,
documented as "Warp speed in AU/s") is used at all when a ball carries its own
`warpFactor`. Cost: none that I can see** — `warpFactor` is per ball and the
integrator reads only `ball+0x98`. It is probably a default for balls created
without one. Noted so nobody hunts for a park-level warp speed to send.

**11.7 `EntityWarpIn` (`0x180043140`, docstring "Warps an entity into a
position without having to enter warp first"). Cost: none, but it is a free
feature.** It is a Ballpark method that teleports a ball to a position with the
warp-in visuals and no align, logging
`EntityWarpIn getting called for %I64d to arrive at %.12f, %.12f, %.12f`. That
is almost certainly how NPC spawns and GM "bring here" work, and it may be the
cheapest possible warp for bots. Its argument list was not read.

---

## 12. What to read first next time

1. §1 of this file, then §4.2, then §4.4's trap.
2. `docs/lanes/movement.md` §§1–3 for the transport and the action envelope —
   warp changes none of it, and §10 item 1 of this file is the only correction.
3. `_destiny.dll` `0x180042863` (`ActivateWarp`, 300 bytes) is the single most
   informative function about warp in the client. `0x180042b20`
   (`OnDeactivatingWarp`, the integrator) is the second.
4. The disassembly recipe, because `tools/pe_functions.py` cannot be run the
   obvious way: `/work/tools/dis.py` shadows the stdlib `dis` module that
   `capstone` imports transitively, so
   `tools/analysis.sh python3 /work/tools/pe_functions.py …` dies in
   `inspect.py`. Run it off stdin instead, where `sys.path[0]` is `/work`:

   ```
   tools/analysis.sh python3 - <<'EOF'
   import sys
   sys.argv = ['x', '/client/bin64/_destiny.dll', '--disasm', '0x180042863', '--limit', '200']
   exec(open('/work/tools/pe_functions.py').read())
   EOF
   ```

   Note also that a function split across several `.pdata` chunks disassembles
   only to the end of the chunk you name — `Ballpark::WarpTo` is five chunks.
5. The `PyMemberDef` decode in §4.2 is reusable for any field in any of these
   extensions: find the member-name string's VA, search the image for an
   8-aligned pointer to it, and read `(int type, int64 offset)` at `+8` and
   `+16`. It named nine fields in one pass and it outranks every inference in
   `docs/archive/destiny-and-the-ballpark.md` §4.4.
