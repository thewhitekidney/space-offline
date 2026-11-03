# Lane: EvEJS `server/src/space/` Destiny model — facts extracted

Source repo: `$EVEJS` (AGPL-3.0). **Facts only** below: constants,
formulas as mathematics, state machines, field names, behavioural rules. No source copied.
All citations `path:line` relative to the fork root.

Note on their own provenance claims: `destiny/simulation/README.md`, `destiny/batching/README.md`,
`destiny/visibility/README.md` each carry an **MIT notice for `carbonengine/destiny` v3.1.1
(c) 2026 CCP Games**, asserting the integrator, warp-factor behaviour, Park packaging and the
BubbleUpdater law are adapted from that MIT-licensed source. The surrounding repo is still AGPL;
treat everything here as facts-only regardless.

---

## 1. Inventory of `server/src/space/destiny/` (12 subdirs, ~41k LOC)

| Dir | Concern | Files that matter |
|---|---|---|
| `constants.js` | Wire enums + global Destiny constants | single file, 64 lines |
| `authority/` | Package contracts, per-session Destiny generation state, position/range authority, journey log | `destinyAuthority.js` (2578), `destinySessionState.js` (880), `scenePositionAuthority.js`, `sceneRangeDecision.js`, `destinyContracts.js` |
| `batching/` | Park update batching, `PackagedAction` encode/decode, per-system history | `packagedAction.js`, `parkUpdateBatcher.js` (695), `systemHistory.js` |
| `bootstrap/` | Initial ballpark / static bootstrap on login | `initialBallpark.js`, `staticBootstrap.js` |
| `commands/` | Player & NPC command entry points (one file per command family) | `warpCommands.js` (1397), `subwarpCommands.js` (846), `stopSpeedCommands.js`, `teleport.js`, `docking.js`, `drone.js`, `fighter.js`, `missile.js`, `mining.js`, `probe.js`, `tractorBeam.js`, `superweapon.js`, `npc.js`, `npcWarpCompletion.js`, `starbaseForceField.js`, `mobileTractorUnit.js`, `shipDerivedMotion.js`, `warpTick.js` (19) |
| `delivery/` | Stamp algebra, send-option policy, delivery transaction, session windows, post-warp demotion, handoff | `deliveryPolicy.js` (2839), `stamps.js` (343), `sendOptions.js`, `sessionStamps.js`, `postWarpDemotion.js` (605), `sameSceneShipHandoff.js` (771), `michelleContract.js`, `sessionWindows.js`, `transitionStamps.js`, `deliveryTransaction.js`, `coordinateFrame.js`, `fallbackStampAllocator.js`, `sync.js` |
| `dispatch/` | Per-tick dispatch to owner / observers / contracts, watcher corrections | `destinyDispatch.js` (3890), `contractDispatch.js` (829), `ownerDispatch.js` (617), `observerReconciliation.js` (490), `watcherCorrections.js`, `sceneRefresh.js`, `ticker.js` |
| `identity/` | int64 ball-ID normalization + interaction scope | `entityID.js` (233), `entityIDFastPath.mts`, `interactionScope.js` (645) |
| `native/` | README only — boundary note to pinned Carbon source; **no code** | `README.md` |
| `presentation/` | Mode→payload translation, AddBalls/RemoveBalls, ship prime/boarding, SpecialFX replay | `mode.js` (306), `addBalls.js`, `addBallsTimeline.js`, `removeBalls.js`, `shipPrime.js`, `shipBoarding.js`, `freshAcquire.js`, `siteEnvironment.js`, `specialFxPayloads.js`, `activeSpecialFxReplay.js` |
| `projection/` | Deterministic forward projection of the entity graph (read path) | `kinematics.js` (595), `entityProjection.js`, `range.js` |
| `protocol/` | Payload identity/summary/direction helpers | `payloads.js`, `payloadIdentity.js`, `payloadSummary.js`, `direction.js` |
| `simulation/` | **Movement laws**: integrator, modes, warp profile/progress, missile kernel | `movement.js` (1404), `nativeSubwarp.js` (1870), `warpState.js` (933), `warpBuilders.js` (778), `carbonIntegration.js`, `warp.js`, `warpContract.js`, `warpRamp.js`, `warpPhase.js`, `warpSerialization.js`, `warpTargets.js`, `warpInterdiction.js`, `warpDiagnostics.js`, `warpNativeBoundary.js`, `activeWarpTick.js`, `pilotWarpPhase.js`, `motionState.js` |
| `stream/` | Byte-level ball encoding + state packets + action payload builders | `ballEncoding.js` (760), `statePayloads.js` (396), `staticBallTail.js` (271), `actions.js` (425), `primitives.js`, `freeBallHeader.js`, `ballConfig.js` |
| `visibility/` | Once-per-tick acquisition/removal planning, bubble + public-grid membership, warp handoff/landing, quiet window | `membershipState.js` (1091), `controlState.js` (818), `warpHandoff.js` (725), `warpLanding.js` (370), `acquisition.js` (526), `delta.js`, `bubbleUpdater.js`, `constants.js`, `referencePosition.js`, `sceneTickCoordinator.js` (3-line re-export) |

Related code that lives **outside** `destiny/` but owns numbers the model needs:
`space/runtime/destinyMovementEngine.js` (all warp/correction tunables),
`space/runtime/shipMotionKinematics.js` (agility derivation),
`space/runtime/scene/spatialTopology.js` (bubble + grid-cluster assignment),
`space/runtime/scene/visibility.js` (the actual see/no-see predicates),
`space/runtime/visibility.js` (public-grid key math),
`space/runtime/runtimeClock.js` (tick cadence),
`space/runtime/scene/clock.js` (pilot-perceived warp trail),
`space/runtime/scene/fleetWarp.js` (fleet-warp formation).

---

## 2. Global Destiny constants (`destiny/constants.js`)

```
BALL_MODE: GOTO 0, FOLLOW 1, STOP 2, WARP 3, ORBIT 4, MISSILE 5, MUSHROOM 6,
           BOID 7, TROLL 8, MINIBALL 9, FIELD 10, RIGID 11, FORMATION 12
BALL_FLAG: IS_FREE 0x01, IS_GLOBAL 0x02, IS_MASSIVE 0x04, IS_INTERACTIVE 0x08,
           IS_SPACEJUNK 0x10, HAS_MINIBOXES 0x20, HAS_MINIBALLS 0x40, HAS_MINICAPSULES 0x80
DESTINY_STAMP_INTERVAL_MS                      = 1000
DESTINY_STAMP_MAX_LEAD                         = 1
DESTINY_ORBITAL_PRECESSION_PER_TICK            = 0.001      (Carbon Ballpark.cpp ORBITAL_PRECESSION)
DESTINY_SPACE_FRICTION                         = 1_000_000
CLIENT_WARP_VISUAL_ACTIVATION_ACCELERATION_MS2 = 310_000    (client WarpFlash.py gate)
```
`constants.js:5-52`. Free-ball kinds of their own: `"unanchoredStructureHull"`, `"corpse"`
(`constants.js:47,52`).

Host tick: `RUNTIME_TICK_INTERVAL_MS = 100` (`runtime/runtimeClock.js:22`);
`SIM_CLOCK_REBASE_INTERVAL_MS = 250`, time dilation clamped to `[0.1, 1.0]`
(`runtime/runtimeClock.js:28-32`). So **host pulse 100 ms, Destiny stamp 1000 ms** — the
simulation advances on the 1 s stamp, not the pulse.

FILETIME: ticks/ms `10000`, epoch offset `116444736000000000` (`runtimeClock.js:24,26`).

---

## 3. Movement / physics

### 3.1 The analytic integrator (this is their core law)
`simulation/carbonIntegration.js:55-110`, declared adapted from Carbon v3.1.1.

Let `m = inertialMass`, `b = friction` (default `DESTINY_SPACE_FRICTION = 1e6`),
`a` = force acceleration, `λ = b/m`, step `t`.

- If `λ ≤ 1e-12`: plain ballistic — `x' = x + v t + ½ a t²`, `v' = v + a t`.
- Otherwise, with `v_eq = a/λ`, `Δv = v - v_eq`, `f = e^{-λ t}`:
  - `v' = v_eq + Δv · f`
  - `x' = x + v_eq · t + Δv · (1 - f)/λ`

Inverse (used to recover an intrinsic acceleration from a velocity pair),
`carbonIntegration.js:112-147`: `a = (v_end - v_start·f) · λ / max(1 - f, 1e-9)`.
Mass floor / epsilon `CARBON_INTEGRATION_EPSILON = 1e-9`.

**Inertial mass fed to it** (`movement.js:436-448`): `m = mass × inertia`; if that is 0,
fallback `m = agilitySeconds × DESTINY_SPACE_FRICTION`. Note `m/b = mass·inertia/1e6 = τ`, i.e.
`λ = 1/τ` — the drag time constant *is* the agility τ.

### 3.2 Agility / τ
`runtime/shipMotionKinematics.js:52-78`, in priority order:
1. **Official**: `τ = mass × inertia / 1_000_000` (if > 0), floored at 0.05 s.
2. else `τ = maxAccelerationTime / ln(10000)`, floored 0.05.
3. else `τ = alignTime / ln(4)`, floored 0.05.
4. else `τ = 1`.

Constants: `DESTINY_ACCEL_LOG_DENOMINATOR = ln(10000)`,
`DESTINY_ALIGN_LOG_DENOMINATOR = ln(4)` (`shipMotionKinematics.js:29,31`).
`MAX_SUBWARP_SPEED_FRACTION = 1.0` (`:27`).
`getCarbonAgilitySeconds` re-derives `mass·inertia/friction` first (`movement.js:311-335`).

### 3.3 Legacy (non-Carbon) velocity response — still used for diagnostics/heading
`movement.js:76-100`: `τ = max(responseSeconds, 0.05)`, `decay = e^{-Δt/τ}`,
`v' = v_desired + (v - v_desired)·decay`,
`Δx = v_desired·Δt + (v - v_desired)·τ·(1 - decay)`.
Velocity is snapped to zero below **0.05 m/s**, and also when
`desiredSpeed ≤ 0.001 ∧ |v| < 0.1` (`movement.js:256-262`).
"Changed" test for dispatch: position moved > **1 m** or velocity changed > **0.5 m/s**
(`movement.js:304-308`, repeated at `:473-475`, `:1349-1352`).

### 3.4 Heading / turning (separate from translation)
`movement.js:102-204`, `shipMotionKinematics.js:35-46`, `:80-93`.

- `TURN_ALIGNMENT_RADIANS = 4° = 4·π/180` — inside this the heading **snaps** to target.
- Turn metrics: `alignment = clamp(dot(cur,tgt), -1, 1)`, `radians = acos(alignment)`,
  `turnFraction = sqrt((alignment+1)/2)`.
- `degPerTick = clamp(75 / max(τ, 0.05), 0.75, 12)` — bounded inverse curve, explicitly
  replacing an earlier linear falloff that stalled capitals past ~60 s agility.
- `tickScale = max(Δt/0.1, 0.05)`; `maxStepDegrees = degPerTick · tickScale`;
  `turnPercent = clamp(maxStepDegrees / max(degrees, 0.001), 0.001, 1)`.
- `turnSeconds = max(τ/2.2, 0.05)` (diagnostic only).
- **From near rest the heading snaps**: if `currentSpeedFraction ≤ 0.1`, jump straight to target.
- Interpolation is a true slerp with weights `sin((1-f)·θ)/sin θ` and `sin(f·θ)/sin θ`,
  degenerating to normalized lerp when `|sin θ| < 1e-6` (`movement.js:111-144`).
- Carbon path keeps translation and orientation **separate**: position/velocity come from the
  analytic integrator, heading from the torque model (`movement.js:449-453`).
- Alignment reference direction: velocity if `|v| > max(0.5, 0.01·maxVelocity)`, else the stored
  heading (`shipMotionKinematics.js:80-93`).

### 3.5 GOTO
`movement.js:346-392` (Carbon thrust) and `:633-638` (legacy).
- `cruiseVelocity = maxVelocity · clamp(speedFraction, 0, 1)` (`movement.js:337-344`).
- `a_max = cruiseVelocity / τ`.
- `stepDistance = cruiseVelocity · Δt`.
- **Homing damping** (only inside one step, and never for missiles):
  if `d² < stepDistance²` then `k = (d²/stepDistance²)²` else `k = 1`.
- `a = (separation / d) · a_max · k`, zero when `d ≤ 1e-9`.
- Comment states the client runs this exact law locally, so a server-side change shows up as a jolt.
- Undock: `targetPoint = position + direction·1.0e16`, mode GOTO (`motionState.js:109-128`).

### 3.6 FOLLOW
Two implementations.

**Carbon plan** (`movement.js:479-527`): stand-off point is on the *target→follower* line, **no
pursuit lead**:
`desiredDistance = followRange + selfRadius + targetRadius`;
`gotoPoint = targetPos + (pos - targetPos)·desiredDistance/|pos - targetPos|`
(or `targetPos + RIGHT·desiredDistance` when coincident, threshold 1e-9); then GOTO thrust at
that point. Explicit comment: client solves the same plan, so no lead is allowed.

**Legacy plan** (`movement.js:1022-1077`):
- `gap = |targetPoint - pos| - desiredRange`, `desiredRange = followRange + selfRadius + targetRadius`.
- If `gap > 50 m`: head radially; `desiredSpeed = min(maxVelocity, max(0, v_target·r̂ + max(0.5·gap, 0.25·maxVelocity)))`.
- Else: head along the target's velocity; `desiredSpeed = min(maxVelocity, |v_target|)`.
- Target point for stations/structures is a docking-approach position, otherwise the raw position
  (`runtime/destinyMovementEngine.js:355-367`).
- No target ⇒ mode forced to STOP, speedFraction 0, velocity 0, targetPoint = own position.

Follow/orbit command validity gates (`destinyMovementEngine.js:369-400`): rejected if target is
moribund/destroyed/pendingRemoval, cloaked, in a **different bubbleID** (both non-zero), or the
range is not finite.

### 3.7 ORBIT
Two implementations.

**Carbon plan** (`movement.js:529-631`) — this is the interesting one:
- Orbit plane seed: `seed = itemID & 0xffff` (`:529-534`).
- Default orbit normal from the tick phase, `phase = currentStamp · 0.001`,
  `seededPhase = seed + phase`, components truncated to 7 decimals:
  `n = (cos(phase)·cos(seededPhase), sin(seededPhase), sin(phase)·cos(seededPhase))`.
- `desiredDistance = orbitDistance + selfRadius + targetRadius`.
- `t̂ = normalize(n × d̂)` where `d̂` points at the target; `r = |to target|`.
- If `r² - desiredDistance² ≥ 0`: re-aim
  `d̂ ← normalize( d̂·(r² - D²)/r + t̂·(D·sqrt(r² - D²))/r )`.
- **Radial factor** (Gaussian with a hard 40 000 denominator, truncated to 7 decimals):
  `g = e^{-(D - r)² / 40000}`.
- `c = -dot(d̂, t̂)`; `det = 1 + g²(c² - 1)`;
  `transverse = det > 0 ? g·c + sqrt(det) : g·c`; then `transverse ·= sign(r - D)`.
- `a = (t̂·g + d̂·transverse) · a_max`.
- Reported `gotoPoint = pos + a · 10 · 149_597_870_700` (10 AU along the acceleration).

**Legacy plan** (`movement.js:1079-1125`):
- `desiredDistance = max(orbitDistance + selfRadius + targetRadius, selfRadius + targetRadius + 500)`.
- Orbit normal rebuilt perpendicular whenever `|dot(normal, r̂)| > 0.95`.
- `radialError = r - desiredDistance`;
  `correction = r̂ · clamp(-radialError / max(desiredDistance, 1), -0.75, 0.75)`;
  heading `= normalize(t̂ + correction)`, `t̂` signed by `entity.orbitSign`.
- `desiredSpeed = clamp(max(0.35·maxVelocity, 0.5·|radialError|), 0, maxVelocity)`.
- No target ⇒ STOP.

### 3.8 STOP
- `advanceMovement` STOP branch = `applyDesiredVelocity(dir, 0, Δt)` (`movement.js:1129-1130`).
- Native STOP adds a one-shot **old-orientation Y damping** before the friction integrator:
  `v.y ·= (1 - 0.07)·(1/1.07) ≈ 0.869158878…` , applied once per frame
  (`nativeSubwarp.js:63-66`, `:701-711`). Attributed to `Ballpark::EvolveStop` when dynamical
  orientation is disabled.
- `resetEntityMotion`: mode STOP, speedFraction 0, velocity 0, targetPoint = position, all
  tracking fields cleared (`motionState.js:96-102`).
- Tracking fields cleared on any mode change (`motionState.js:11-26`):
  `speedFractionBeforeStop, targetEntityID, followRange, orbitDistance, warpState, pendingWarp,
   dockingTargetID, lastPilotWarpStartupGuidanceStamp, lastPilotWarpVelocityStamp,
   lastPilotWarpEffectStamp, lastPilotWarpCruiseBumpStamp,
   lastPilotWarpMaxSpeedRampIndex(= -1), lastWarpDiagnosticStamp`.
- A Stop that resumes picks up `previousSpeedFraction > 0 ? previousSpeedFraction : 0.75`
  (`commands/subwarpCommands.js:506`); speed-fraction epsilon `1e-6` (`:53`, `:508`).

### 3.9 Native subwarp frame (behaviour-phase freeze)
`simulation/nativeSubwarp.js`. Native subwarp modes are exactly `{STOP, GOTO, FOLLOW, ORBIT}`
(`:13-18`) — MISSILE and WARP are excluded (`:130-137`).
- Frame index = `floor(simTimeMs / 1000)`; a cadence anchor can shift the boundary
  (`:98-128`).
- One control per ball is computed at the frame boundary and **frozen** for the whole 1 s frame:
  `{kind: ACCELERATION|STOP|MISSING_TARGET, mode, acceleration, targetPoint}` (`:238-315`).
  Control plans are built with `nativeTickSeconds = 1.0`.
- A ball inserted after the behaviour phase starts **evolves only at the next native boundary**
  — insertion order into the host map does not matter (`:655-663`).
- `MISSING_TARGET` ⇒ if the command is still current, force STOP/0/targetPoint=pos and clear
  `dockingTargetID`; then integrate with zero acceleration (`:680-700`).
- A same-mode re-aim mid-frame survives until the next boundary (`:712-721`).
- Projection step defaults: `DEFAULT_MAX_PROJECTION_STEP_MS = 250`,
  `DEFAULT_MAX_PROJECTION_STEPS = 240`, hard cap `ABSOLUTE_MAX_PROJECTION_STEPS = 4096`,
  with a 64-iteration bisection to fit the step budget (`projection/kinematics.js:7-11,109-121`);
  command-side mirror `range.js:10-11`.

### 3.10 Missile kernel (their only collision exception)
`movement.js:640-1020`. Explicitly flagged as the *only* sanctioned contact solver; never to be
reused for ship/static contact (`:681-682`, and all four READMEs).
- `resolveMissileFollowRange = -(missileRadius + targetRadius)` (`:640-644`).
- **Impact distance uses the target radius only**, floored at 1 m:
  `max(0, |pos - targetPos| - max(1, targetRadius))` — using `missileRadius + targetRadius`
  removed the missile a tick early and produced visible "fizzle short of target" (`:646-658`).
- Swept impact (`:683-806`): relative start `p = p_m - p_t`, relative velocity
  `v = v_m - (p_t,end - p_t,start)/Δt`, impact radius `R = max(1, targetRadius)`.
  - closest approach `t_c = clamp(-dot(p,v)/|v|², 0, Δt)` when `|v|² > 1e-6`.
  - quadratic `|v|² t² + 2·dot(p,v) t + (|p|² - R²) = 0`; if `|p|² - R² ≤ 0` ⇒ impact at t=0
    reason `"already-inside-radius"`; else the smallest root in `[-1e-6, Δt+1e-6]` ⇒
    reason `"relative-sweep"`.
- Launch-tick partial delta: the active step starts at `max(stepStartMs, launchedAtMs)` so a
  newly launched missile does not consume the whole tick (`:884-900`).
- Mode dispatch is a plain switch on `entity.mode` string: STOP / GOTO / FOLLOW / MISSILE /
  ORBIT / WARP, default no-op (`movement.js:1127-1373`).

### 3.11 Explicitly absent from their model (confirmed absences)
- **No general ship/static contact, bumping, collision geometry, response cadence or landing
  receipt**: stated in `destiny/simulation/README.md`, `destiny/authority/README.md`,
  `destiny/native/README.md`. Missile sweep is the only exception.
- **No angular/rotational dynamics in the sim** — `SetBallAngularVelocity`,
  `SetMaxAngularVelocity`, `SetMaxAngularSpeed`, `SetBallAngularAgility`, `SetBallRotation` exist
  only as outbound payload builders (`stream/actions.js:138-170,201-202`); nothing integrates them.
- **No `native/` implementation** — that directory is a README only.
- `destiny/visibility/sceneTickCoordinator.js` is a 3-line re-export; the real coordinator is
  `bubbleUpdater.js`.

---

## 4. Warp

### 4.1 Warp-factor constants (`simulation/warp.js:10-18`)
Attributed to Carbon `Ballpark.cpp` `SetupWarpConstants`.
```
WARP_FACTOR_TO_AU_PER_SECOND  = 0.001
WARP_FACTOR_TO_ACCELERATION   = 1/1000
WARP_FACTOR_TO_DECELERATION   = 1/3000
NATIVE_WARP_DECEL_RATE_MAX    = 2.0
NATIVE_WARP_TO_GOTO_DISTANCE_METERS         = 100_000
NATIVE_WARP_TO_GOTO_DISTANCE_SQUARED_METERS = 10_000_000_000
```
- `warpFactor = warpSpeedAU / 0.001` (i.e. AU/s × 1000).
- `k_a = max(warpFactor/1000, 0.001)`  ← **0.001 floor** (their wrapper's, not Carbon's).
- `k_d = min(max(warpFactor/3000, 0.001), 2.0)` ← the 0.001 floor is applied to *decel too*, and
  the comment says Carbon does **not** apply that low-factor floor to deceleration. This is a
  deliberate divergence worth noting.
- `cruiseSpeed = warpFactor · 0.001 · ONE_AU_IN_METERS`.
- Dropout speed: `min(maxSubwarpVelocity/2, maxDropoutSpeed)`, default max **100 m/s**
  (`warp.js:77-88`; `WARP_DROPOUT_SPEED_MAX_MS = 100` at
  `runtime/destinyMovementEngine.js:289`).
- **WarpTo falls back to GotoPoint below 100 km** (squared test), but the player-command service
  validation requires **150 km**: `MIN_WARP_DISTANCE_METERS = 150000`
  (`destinyMovementEngine.js:251`, enforced `commands/warpCommands.js:377-380`).
- `ONE_AU_IN_METERS = 149_597_870_700` (`destinyMovementEngine.js:249`).

### 4.2 Warp profile construction (`simulation/warpState.js:122-225`)
Given `D = totalDistance` (to the stop-distance-adjusted target point), `k_a`, `k_d`,
`v_cruise = warpSpeedAU · AU`:
```
v_limit = ((D + 1) · k_a · k_d) / max(k_a + k_d, 0.001)      # the +1 is "part of the native geometry"
v_max   = max(0, min(v_cruise, v_limit))
profileType = v_max < v_cruise ? "short" : "long"
accelDistance  = max(v_max / k_a, 0)
decelDistance  = max(v_max / k_d - 1, 0)                      # note the -1
cruiseDistance = max(D - accelDistance - decelDistance, 0)
accelTimeMs  = (ln(max(v_max / k_a, 1)) / k_a) · 1000
cruiseTimeMs = v_max > 0 ? (cruiseDistance / v_max) · 1000 : 0
decelTimeMs  = (ln(max(v_max / k_d, 1)) / k_d) · 1000
durationMs   = accelTimeMs + cruiseTimeMs + decelTimeMs
```
`targetPoint = rawDestination - direction·stopDistance` (unless `applyStopDistance === false`).
`warpSpeed` (wire) `= max(1, round(warpSpeedAU·1000))`.
`warpSpeedAU` resolution: explicit → entity → **default 3**, then rounded to 1/1000 with a floor
of 1/1000 (`warpState.js:70-82`, mirrored in `warpSerialization.js:64-70`).
`followID` default **15000**, `followRangeMarker` default **-1** (`warpState.js:213-214`).
Serialized `warpSpeed` must never be 0 — a persisted zero crashes the client through integer
division; the fallback is **3000** (`warpSerialization.js:126-129`, `ballEncoding.js:321-333`).
Serialized `accelExponent`/`decelExponent` default **5** each
(`warpSerialization.js:16-17`) and double as the accel/decel rate fallbacks.

### 4.3 Warp progress / phase (`warpState.js:779-882`)
Elapsed `t` from `startTimeMs`, phase boundaries `accelMs`, `accelMs+cruiseMs`:

- **Accel** (`t < accelMs`), with `s = t/1000`:
  `v_base = min(v_max, k_a · e^{k_a s})`
  `traveled = min(accelDistance, max(v_base / k_a, 0))`
  reported `speed = max(v_base, activationSpeedMs)` — the *reported* speed is floored by the
  activation speed but the *distance* is not.
- **Cruise**: `traveled = accelDistance + v_max·s`, `speed = v_max`, with `s` measured from
  `accelMs`.
- **Decel**, `s` from `decelStart = accelMs + cruiseMs`:
  `v = max(0, v_max · e^{-k_d s})`
  `traveled = accelDistance + cruiseDistance + max((v_max - v)/k_d, 0)`
- **Completion predicate** (strict, speed-based not time-based):
  - if `dropoutSpeed > 0 ∧ v < dropoutSpeed` ⇒ complete, `nativeDropout = true`.
  - if `dropoutSpeed ≤ 0 ∧ t ≥ durationMs` ⇒ complete, `nativeDropout = false`.

Phase names (`simulation/warpDiagnostics.js:182-207`):
`accel` while `t < accelTimeMs`; `cruise` while `t < accelTimeMs + cruiseTimeMs`;
`decel` while `t < durationMs`; else `complete`.
`WARP_COMPLETION_DISTANCE_RATIO = 0`, `MIN = 1`, `MAX = 1` metres — i.e. the distance-based
completion gate is effectively neutered to a 1 m constant
(`destinyMovementEngine.js:295-299`, consumed `warpState.js:110-120`).

### 4.4 Ramp / activation (the align-then-activate gate)
`warpState.js:500-554`:
- `alignedByDot = |1 - dot(alignmentDir, desiredDir)| < 0.01`  (≈ within 8.1°, but it is a dot
  test, not an angle test)
- `alignedBySpeed = actualSpeedFraction > WARP_ENTRY_SPEED_FRACTION`, with
  `actualSpeedFraction = clamp(|v| / max(maxVelocity, 0.001), 0, 1)`
  (`shipMotionKinematics.js:95-102`) and `WARP_ENTRY_SPEED_FRACTION = 0.75`
  (`destinyMovementEngine.js:281`). **This is a linear-fraction test, not a squared one.**
- `ready = aligned || forced`, where `forced = floor(elapsedMs/1000) > 180` — a **180-tick /
  180-second** hard timeout.
- `WARP_NATIVE_ACTIVATION_SPEED_FRACTION = 0.75`,
  `WARP_NATIVE_ACTIVATION_SPEED_MARGIN_MS = 1` (`destinyMovementEngine.js:283-285`).

Activation (`warpState.js:576-656`):
- If the pending warp's `nativeWarpCommand === "GOTO"`: clear tracking, set targetPoint to the raw
  destination, resume `speedFraction = 1` if it was 0 or mode was STOP, mode `GOTO`, return null.
- Otherwise: mode `WARP`, `speedFraction = 1`, direction = toward the adjusted target point,
  `startupGuidanceVelocity` = current velocity, then position/velocity are immediately set from
  `getWarpProgress(warpState, startTimeMs)`.
- `lastWarpPositionBroadcastStamp = -1`, all pilot-ramp stamps reset to null, ramp index -1.
- Activation velocity for the pilot's own ball: direction × `|startupGuidanceVelocity|`, zeroed
  if that is `≤ 0.5 m/s` (`warpState.js:556-574`).

Pilot max-speed ramp (`simulation/warpRamp.js`, `pilotWarpPhase.js`):
- Ramp entries `{atMs, stamp, speed, label}`, filtered to `atMs > 0 ∧ speed > 0`.
- `PILOT_WARP_SPEED_RAMP_FRACTIONS = [0.2, 0.45, 0.7, 1.0]` (profile-time fractions)
  and `PILOT_WARP_SPEED_RAMP_SCALES = [0.6, 0.75, 0.9, 0.95]` (speed scales)
  (`destinyMovementEngine.js:327-329`).
- The whole ramp is **disabled**: `ENABLE_PILOT_WARP_MAX_SPEED_RAMP = false` (`:321`).
- A due ramp entry is *folded into* the cruise bump rather than sent separately when both are due
  in the same pass (`pilotWarpPhase.js:117-138`).
- Pilot-phase updates are sent with `minimumLead = maximumLead = 4` ticks
  (`pilotWarpPhase.js:164-171`).

### 4.5 Timing contract (`simulation/warpContract.js`)
```
PILOT_WARP_ACTIVATION_DELAY_DESTINY_TICKS           = 4   # client WarpTo is authored 4 ticks ahead
PILOT_WARP_HISTORY_SAFE_DESTINY_LEAD                = 4   # == above
PILOT_WARP_LANDING_STAMP_LEAD_DESTINY_TICKS         = 3   # landing stamped 3 ticks past server dropout
PILOT_WARP_SOURCE_REMOVAL_SETTLE_MS                 = 0   # no second wall-clock grace
PILOT_WARP_DESTINATION_ACQUIRE_LEAD_DESTINY_TICKS   = 0
PILOT_WARP_DESTINATION_STATIC_PRELOAD_DISTANCE_METERS = 10_000_000   # 10 Mm
```
Derived: `PILOT_WARP_LANDING_DESTINATION_UNREACHABLE_GRACE_MS = (4-2)·1000 = 2000`
(`destinyMovementEngine.js:253-254`).

### 4.6 Native stamp sampling for active warp (`simulation/warpNativeBoundary.js`)
Active warp advances on the **canonical 1 s stamp, not on each host pulse**:
- `activationStamp = stamp(startTimeMs)`, `currentStamp = stamp(now)`.
- If `previousSampleStamp === currentStamp` ⇒ `{ready: false, reason: "SAME_NATIVE_STAMP"}` and the
  tick is skipped entirely (`movement.js:1183-1194` returns
  `nativeWarpSampleDeferred: true`).
- `elapsedNativeTicks = forwardDistance(activationStamp, currentStamp)`,
  `sampleAtMs = activationAtMs + elapsedNativeTicks·1000`.
- `nativeBoundaryAtMs = floor(now/1000)·1000`.
- Transient cursor fields written onto `warpState`: `nativeSampleStamp`, `nativeSampleAtMs`,
  `nativeBoundaryAtMs` — deliberately not persisted.

**Dropout recovery when the host pulse skipped stamps** (`movement.js:1202-1236`): if progress is
complete and `firstUnsampledNativeTick < sampledNativeTick`, a **binary search over whole native
ticks** finds the *first* tick at which progress was complete, and that tick becomes the modeled
dropout. Prevents overshooting the landing point after a host stall.

### 4.7 Landing / exit
`movement.js:1255-1360`:
1. Record `dropoutPosition`, `dropoutVelocity`, `nativeDropout`, `dropoutSpeedMs`,
   `remainingDistance = totalDistance - traveled`.
2. Mode ← `STOP`, `speedFraction` ← 0, `warpState` ← null.
3. `nativeStopEvolutionTicks = max(1, trunc(elapsedNativeTicks - sampledNativeTick) + 1)`.
4. **One same-tick STOP evolution** through the analytic integrator with zero acceleration over
   `nativeStopEvolutionTicks · 1 s`, inertial mass `mass·inertia` or fallback
   `max(τ, 0.05)·1e6`. The result is the exit position/velocity.
5. `exitPosition`, `exitVelocity`, `nativeCompletionStamp`, `nativeCompletionAtMs`,
   `nativeModeledCompletionAtMs`, `nativeStopEvolutionTicks` recorded on the completed state.
6. `lastMotionDebug` is rebased to a zero-length step so the STOP evolution is not mistaken for a
   swept segment.
Result flags: `warpCompleted`, `completedWarpState`, `suppressCompletionStopUpdates`,
`nativeWarpBoundaryAtMs`, `nativeWarpModeledAtMs`.

Warp-in / landing-point geometry other than the parity file (`simulation/warpTargets.js`):
- `WARP_EXIT_VARIANCE_RADIUS_METERS = 2500` (`destinyMovementEngine.js:256`);
  `MAX_STARGATE_JUMP_DISTANCE_METERS` default 2500 (`warpTargets.js:78`).
- Stargate exit point: along gate→ship, at `max(gateRadius + shipRadius + 500, minimumRange)`
  (`:110-132`).
- Stargate envelope point: `gateRadius + (minimumRange > 2500 ? minimumRange : 0)` (`:134-159`).
- Landing point: envelope point + a uniform point in a **hemisphere** of radius 2500 oriented along
  gate→ship; `stopDistance = max(1, U(0,1)·2500)`; `rawDestination = landing + travelDir·stopDistance`
  (`:187-223`). Scatter only applies when `minimumRange > 2500` for the simpler
  `getStargateWarpLandingPoint` (`:161-171`).
- Uniform-in-sphere sampling: `θ = U·2π`, `vertical = 2U-1`, `scale = cbrt(U)` (`:90-108`).
- Warp stop distance by target kind (`warpState.js:884-903`), all `+ 2·shipRadius`:
  planet/moon `max(r + 1_000_000, range)`; sun `max(r + 5_000_000, range)`;
  station `r + range`; stargate `max(max(2500, 0.3·r), range)`;
  asteroidBelt `max(2500, range)`; default `max(max(1000, r), range)`.

### 4.8 Demotion (post-warp, two exact stamps)
`delivery/postWarpDemotion.js:275-384`:
- Landing stamp `S` must equal `handoff.landingDestinyStamp`.
- **Bridge demotion** is authored at `S - MICHELLE_HELD_FUTURE_DESTINY_LEAD = S - 1`.
- **Final demotion** is authored at `S`.
- Both must be exactly `SetBallMassive(entityID, 0)` for the pilot's own ball, frozen plain wire
  objects (no functions, symbols, cycles, or non-Object/Array prototypes) and verified by payload
  identity, not value (`:54-121`, `:172-180`).
- Capability objects are branded with private Symbols and bound to the live session generation +
  handoff; a stale generation invalidates them.
- Bridge update builder: `presentation/mode.js:100-105`
  (`buildPostWarpDemotionBridgeUpdate`).

### 4.9 Handoff (source-grid exit → destination-grid entry)
`visibility/warpHandoff.js`:
- `beginPilotWarpVisibilityHandoff` records `{shipID, sourcePublicGridKey, sourceClusterKey,
  destinationPublicGridKey, destinationClusterKey, currentPublicGridKey,
  currentPublicGridClusterKey, destinationStaticInstanceID, sourceRemoved,
  destinationStaticJoined, destinationJoined, destinationPrewarmed}` (`:84-100`).
- **Effect gate**: nothing happens until `effectStamp >= 0` and (`effectAtMs <= 0` or
  `now >= effectAtMs`) (`:132-149`). A **negative `effectStamp` means "preparing/aligning"**.
- Client visual activation time gate: `now + 0.001 >= clientVisualActivationAtMs`; source removal
  additionally waits `+ sourceRemovalSettleMs` (which is **0**) (`:187-201`).
- `sourceHandoffReady` ⇔ client visual settled **and** (live grid key ≠ source grid key **or**
  live cluster key ≠ source cluster key) — a live-grid test, no wall-clock grace (`:213-227`).
- `destinationHandoffReady` ⇔ `phase === "decel"` **and** (destination grid entered or destination
  cluster entered) (`:289-295`).
- `destinationStaticPreloadReady` ⇔ `phase === "decel"` **and** distance to destination
  `≤ 10_000_000 m`.
- `destinationControllerPrewarmReady` ⇔ a destination cluster key exists **and** `phase === "decel"`
  — controllers warm through the whole deceleration without exposing balls; the comment says TQ
  assembles one complete destination snapshot in that window and then acquires statics and
  dynamics together near landing (`:277-282`).
- `destinationDedicatedStaticCompleteReady` requires `phase === "complete"` plus a dedicated
  instance ID and grid/cluster entry or the 10 Mm preload distance.

### 4.10 Subwarp / sessionless warp ingress
- `SESSIONLESS_WARP_INGRESS_DURATION_MS = 1500` (`destinyMovementEngine.js:301`).
- Ingress state (`warpState.js:658-693`): `durationMs = max(250, …)`;
  with the native profile it takes `warpState.durationMs` and
  `completionHoldMs = 0`; without it, `completionHoldMs = 1000` (one stamp).
  `completeAtMs = startTimeMs + durationMs + completionHoldMs`.
- An entity with `sessionlessWarpIngress.useNativeWarpProfile !== true` is advanced by
  `advanceSessionlessWarpIngress` instead of the warp profile (`movement.js:1158-1163`).
- `isEntityInActiveWarp` ⇔ mode `WARP` ∧ no `pendingWarp` ∧
  `warpState.nativeWarpCommand !== "GOTO"` (`simulation/warpPhase.js:6-15`).

### 4.11 Warp disruption / interdiction (`simulation/warpInterdiction.js`)
- `BUBBLE_DRAG_DISTANCE_METERS = 500_000`.
- Pure command-time ray/sphere geometry, no field ownership, no collision solver:
  for each field `{position, rangeMeters, disruptorID}`,
  `proj = dot(center - origin, d̂)`; skip if `proj < 0`;
  `perp = |(center - origin) - proj·d̂|`; skip if `perp > radius`;
  `entry = max(0, proj - sqrt(radius² - perp²))`;
  skip if `|entry - targetDistance| > 500_000`.
- Tie-break: smallest `entry`, then smallest `disruptorID` as a BigInt.
- The landing becomes `origin + d̂·entry`, the warp is **re-requested** to that point with
  `stopDistance = 0` and all destination identity cleared, carrying the original request in an
  `interdiction.request` sub-object (`commands/warpCommands.js:382-399`).
- `options.ignoreWarpDisruptionField === true` or a GOTO-class command bypasses it.

### 4.12 Fleet warp
`runtime/scene/fleetWarp.js` + `services/fleets/fleetWarpCommandAssociation.js`.
- Formation identity is a **command token** shared by leader and follower, scoped to the scene
  system ID; both results must be `success && !redundant` to associate (`:37-80`).
- `isFleetWarpFormationCompanionForSession` requires: same association, the companion already in
  the session's committed `visibleDynamicEntityIDs`, and a fleet warp in progress on either side
  (`:111-134`).
- Settled peer completion batches require, for both the owner and the observer ego:
  mode `STOP`, no `pendingWarp`, no `warpState`, no `fleetWarpFormationQueuedWarp`, matching
  active formation token, no pending landing handoff, the peer committed-but-not-fresh, not
  pending removal (`:167-240`).
- A fleet-warp association is retired whenever the ship has a `pendingWarp`, a
  `fleetWarpFormationQueuedWarp`, or (`mode === "WARP"` ∧ `warpState`)
  (`simulation/motionState.js:28-37`).
- `BALL_MODE.FORMATION = 12` exists on the wire with tail `followID:i64, followRange:f32,
  effectStamp:i32` (`stream/ballEncoding.js:533-537`).

### 4.13 Pilot-perceived warp trail (client/server skew)
`runtime/scene/clock.js:256-284`:
```
prepareExecMs      = pilotPrepareVisibleStamp·1000 - sessionClockOffsetMs
entrySpeedMs       = max(0.75·maxVelocity, 1)
accelAdvantageMs   = entrySpeedMs > k_a ? (ln(entrySpeedMs / k_a)/k_a)·1000 : 0
trailMs            = clamp(prepareExecMs - warpRequestedAtMs - accelAdvantageMs, 0, 2·4·1000)
```
i.e. clamped to `[0, 8000] ms`. Fallback when the anchors are missing:
`PILOT_WARP_PERCEIVED_TRAIL_FALLBACK_MS = 3000` (`destinyMovementEngine.js:325`).

### 4.14 Feature flags that are OFF (so their shipped behaviour excludes these)
`destinyMovementEngine.js:264,307,311,317,319,321,323,277`:
```
ENABLE_ACTIVE_SUBWARP_WATCHER_POSITION_ANCHORS   = false
ENABLE_PILOT_WARP_FACTOR_OPTION_A                = false   (scale 1.15)
ENABLE_PILOT_WARP_SOLVER_ASSIST_OPTION_B         = false   (scale 1.5, lead 1000 ms)
ENABLE_PILOT_PRE_WARP_ADDBALL_REBASE             = true
ENABLE_PILOT_WARP_EGO_STATE_REFRESH              = false
ENABLE_PILOT_WARP_MAX_SPEED_RAMP                 = false
ENABLE_PILOT_WARP_ACTIVE_CORRECTIONS             = false
OBSERVER_STATIONARY_ANCHOR_MODE                  = "off"
WARP_START_ACTIVATION_SEED_SCALE                 = 1.1     (read only by dead paths)
WARP_MEDIUM_DISTANCE_AU = 12, WARP_LONG_DISTANCE_AU = 24   (classification only)
```
`OBSERVER_STATIONARY_ANCHOR_MODE = "off"` justification recorded in-source: Tranquility sends a
parked ship nothing (5 `SetBallPosition` in 46 recordings, none periodic); the anchors moved the
pilot's own ball ≤ 0.05 m in 29 of them; an anchor arriving behind the park's tick made the client
step back a tick 7 of the 11 times it went back across six recordings.

---

## 5. Visibility / grid model

### 5.1 Constants (`destiny/visibility/constants.js`)
```
BUBBLE_RADIUS_METERS                            = 250_000      # internal ownership unit
BUBBLE_HYSTERESIS_METERS                        = 5_000
BUBBLE_RETENTION_RADIUS_METERS                  = 255_000      # radius + hysteresis
BUBBLE_CENTER_MIN_DISTANCE_METERS               = 500_000      # 2 × radius
PUBLIC_GRID_BOX_METERS                          = 7_864_320    # CCP Nullarbor's post-BiaB public grid
PUBLIC_GRID_HALF_BOX_METERS                     = 3_932_160
PUBLIC_GRID_NEARBY_VISIBILITY_RADIUS_METERS     = 12_500_000   # 12.5 Mm, non-transitive
MICHELLE_TRANSIENT_WARP_VISIBILITY_RANGE_METERS = 50_000_000   # 50 Mm
```
All comparisons are done on the **squared** values (`BUBBLE_RADIUS_SQUARED`,
`BUBBLE_RETENTION_RADIUS_SQUARED`, `BUBBLE_CENTER_MIN_DISTANCE_SQUARED`).
Stated evidence for 12.5 Mm: current-client Jita captures acquire the diagonal gate group
`50001250` at 11.2 Mm centre-to-centre / 8.6 Mm from the landing point, while excluding the next
gate at 20.3 Mm. Stated evidence for 50 Mm: TQ's stationary-observer golden first materializes the
inbound ship at the first one-second sample below the 50 Mm boundary.

### 5.2 Public-grid key and cluster
`runtime/visibility.js:83-131`:
- `axisIndex = floor(coord / 7_864_320)` per axis; key is the string `"x:y:z"` of the three
  indices. `parsePublicGridKey` splits on `":"`, defaults `"0:0:0"`.
- A box's occupancy is noted from **both** static and dynamic entities
  (`spatialTopology.js:434-462`).
- Cluster composition (`spatialTopology.js:464-535`): flood fill over **face-connected** boxes
  only — the 6 `±1` neighbours. Diagonal/corner joins are explicitly rejected because they
  over-composed dense systems like Jita and leaked login/bootstrap visibility across nearby but
  distinct gates. Cluster key = `"cluster:" + lexicographically smallest member box key`.
- Recomposition is deferred while a Destiny presentation batch is live for the tick unless
  `allowDuringActiveSceneTick === true` (`:537-550`).
- For a ship in `mode === "WARP"` with a `warpState`, the visibility grid/cluster key is the
  **live** key from its current position, not the committed `entity.publicGridKey`
  (`spatialTopology.js:163-171`, `:210-218`).

### 5.3 Bubble assignment (hysteresis)
`spatialTopology.js:299-345`:
1. If the current bubble is within `BUBBLE_RETENTION_RADIUS_SQUARED` (255 km), **keep it** — this
   is the hysteresis.
2. Else take the nearest bubble within `BUBBLE_RADIUS_SQUARED` (250 km).
3. Else create a bubble; its centre is `position + motionDirection · (250_000/2)` = 125 km ahead
   along the direction of travel (`:~290`).
4. New bubble centres are nudged apart: up to **8 iterations**, any centre closer than 500 km to an
   existing centre is pushed to exactly 500 km from it along the offset direction
   (`:49-78`).
Entities in `mode === "WARP"` are **skipped** by bubble reconciliation entirely
(`spatialTopology.js:~575`, `bubbleUpdater.js:51-58`).

`hasPendingEntityVisibilityTopologyChange` (`spatialTopology.js:~585-600`): true if the grid key
changed; for a warping entity that is the only test; otherwise also true when the bubble is gone or
the entity is outside `BUBBLE_RETENTION_RADIUS_SQUARED`.

### 5.4 Who sees whom
`runtime/scene/visibility.js:2610-2636` (dynamic) and `staticVisibilityGate.mts` (static):
1. Both ego and candidate must resolve a public-grid **cluster** key.
2. Same cluster key ⇒ visible.
3. `requireExactPublicGridCluster === true` ⇒ otherwise not visible.
4. Else visible iff `|egoPosition - entityPosition| ≤ 12_500_000 m`. This augments clusters and is
   deliberately **non-transitive** — it does not merge clusters.
Static gate adds: scene-key match, dungeon lifecycle/privacy/ownership, and the same nearby-sphere
test (`staticVisibilityGate.mts:12-15, 295`).

Warping-ship departure envelope (`runtime/scene/visibility.js:1480-1540`, `1663-1680`): while the
departure grace is live, a warping ship stays visible to the old grid iff its *presentation*
surface distance is `< 50_000_000 m`; beyond that the reason is
`warp-player-departure-transient-envelope-exceeded`.

### 5.5 Departure grace window (the one real ms window)
`runtime/scene/visibility.js:196-211` → `visibility/controlState.js:155-182`:
```
departureBubbleVisibleUntilMs = now + max(2000, warpState.durationMs + 1000)
```
i.e. **at least two Destiny stamps, otherwise the whole warp duration plus one stamp**.
`departureBubbleID` is the bubble the ship was removed from. Reset sets both to null/0.

### 5.6 Quiet window (stamp-based, not ms)
`visibility/controlState.js:322-364`, `:480-515`; consumers
`dispatch/ownerDispatch.js:185-204,297,353`, `dispatch/contractDispatch.js:499-552`,
`delivery/deliveryPolicy.js:1898,2305-2308`, `runtime/scene/clock.js:689-705`.
- On an **accepted** pilot-warp landing, `generation.pilotWarpQuietUntilStamp` is set to the
  accepted landing stamp (taking the furthest of the existing value and that stamp), the handoff is
  cleared, and `landingPending` goes false.
- The window is *active* while `isDestinyStampAfter(currentSessionStamp, quietUntilStamp, maxLead)`
  — i.e. the quiet stamp is still in the future. Once passed it is cleared and reported `expired`.
- While active, the owner's presented maximum future lead is narrowed and a
  `quietWindowMinimumStamp` floor is injected into delivery.
- `bypassPilotWarpQuietWindow: true` is passed by the landing reconcile path, fleet-warp peer
  batches, and three other scene-visibility call sites.
There is **no millisecond quiet window** anywhere — only this stamp-ring window plus the departure
grace above.

### 5.7 Once-per-tick coordinator ordering
`visibility/bubbleUpdater.js:7-118` (the "final scene-tick visibility coordinator", described as
Carbon's once-per-`currentTime` BubbleUpdater law adapted to the 100 ms scene tick):
1. Dedup topology entities → `reconcilePublicGrid` each.
2. Dedup bubble entities, **skipping `mode === "WARP"`** → `reconcileBubble` each.
3. `composePublicGrids()` once.
4. Landing reconciles first, one per session; a stale/rejected landing must **not** fall through to
   the generic path (it would disturb the quiet/retry boundary).
5. Remaining sessions: warp-handoff path if a handoff entity resolves, else the generic path.
Only ephemeral local `Set`s; no retained dictionaries, timers, tick caches or membership stores.

### 5.8 Watcher correction cadence
`destinyMovementEngine.js:258-279`, consumed `dispatch/watcherCorrections.js:70-79`:
```
WATCHER_CORRECTION_INTERVAL_MS                            = 500
WATCHER_POSITION_CORRECTION_INTERVAL_MS                   = 1000
ACTIVE_SUBWARP_WATCHER_CORRECTION_INTERVAL_MS             = 250
ACTIVE_SUBWARP_WATCHER_POSITION_CORRECTION_INTERVAL_MS    = 5_000
ACTIVE_SUBWARP_WATCHER_POSITION_ANCHOR_DISTANCE_METERS    = 1_000
WARP_POSITION_CORRECTION_INTERVAL_MS                      = 250
```

### 5.9 Membership bookkeeping
`visibility/membershipState.js:19-29`: `DEFAULT_JOURNAL_MAX_ENTRIES = 200`,
`DEFAULT_JOURNAL_MAX_IDS = 64`, plus private Symbols
`REMOVAL_RESERVATION_BRAND`, `REMOVAL_PRESENTATION_BRAND`.
Pending acquisition/removal keys on the session: `pendingNativeVisibilityAcquisitionsByID`,
`pendingNativeVisibilityRemovalsByID` (`visibility/acquisition.js:7-8`).
Committed membership fields on `session._space`: `visibleDynamicEntityIDs`,
`freshlyVisibleDynamicEntityIDs`, `visibleStaticEntityIDs`, `pilotWarpVisibilityHandoff`,
`pilotWarpQuietUntilStamp`, `shipID`, `sceneKey`, `destinyPositionOrigin`.
Visibility and authority changes commit **only** with the accepted delivery transaction and stay
retry-eligible on rejection (all three READMEs).

---

## 6. Client-facing protocol details (cross-check material)

### 6.1 Stamp algebra (`delivery/stamps.js`)
```
DESTINY_STAMP_MODULUS            = 0x80000000      # 31-bit ring, signed-int32 wire
DESTINY_STAMP_MASK               = 0x7fffffff
DESTINY_STAMP_MAX_FORWARD_LEAD   = 16
DESTINY_STAMP_MAX_ORDERING_DISTANCE = 0x40000000 - 1 = 1_073_741_823
```
- `stamp(now) = floor(now / 1000) & 0x7fffffff`; a ceil variant exists for one-tick-up allocation.
- Ordering is **modular**, never `<` / `>`:
  `forwardDistance(a,b) = ((b - a) + 2^31) mod 2^31`;
  `compare(a,b)` picks whichever direction is shorter; `isDestinyStampAfter(ref, cand, maxLead)`
  is `0 < forwardDistance(ref, cand) ≤ maxLead`.
- `null`/`undefined` are the canonical *absence* value and must not coerce to a real stamp 0
  (`:54-67`). `hasDestinyStamp` requires an integer in `[0, 0x7fffffff]`.
- `getDestinyStampForwardBoundaryDeltaMs = forwardDistance·1000 - (now mod 1000)` within the lead
  window, else 0.
- `advanceNextDestinyStamp`: a previous lead beyond the lane ceiling is allocated *at* the ceiling
  if it is still `≤ 16`, otherwise treated as stale/behind and reset to the current stamp. Comment
  notes a specialized "acceleration gate" lane authors **+5**.
- `advanceHistorySafeDestinyStamp` clamps `max(previousLead, minimumLead)` into `[0, maxLead≤16]`.

### 6.2 Michelle (client) contract (`delivery/michelleContract.js`)
```
DESTINY_NOTIFICATION_NAME                  = "DoDestinyUpdate"
DESTINY_NOTIFICATION_ID_TYPE               = "clientID"
MICHELLE_DIRECT_CRITICAL_ECHO_DESTINY_LEAD = 1
MICHELLE_HELD_FUTURE_DESTINY_LEAD          = 1
MICHELLE_POST_HELD_FUTURE_DESTINY_LEAD     = 2
MICHELLE_CLIENT_IMMEDIATE_FUTURE_DELTA     = 3
```
Recorded evidence: an update's stamp minus the client park's tick on arrival is `-1`, `0` or `+1`,
never `+2`, across 46 recordings / 6,203 updates; michelle's own Action log applies each at `+0`.
The **client's** own rule (`michelle.py DoPreTick`, line 906) holds an entry while
`0 < stamp - currentTime < 3`; at `≥ 3` it applies immediately and steps the park up to it.
Delivery result taxonomy: `accepted` / `rejected(session-closed, non-retryable)` /
`rejected(explicit-false, retryable)` / `ambiguous(delivery-threw)`; an ambiguous delivery may
destroy the socket (`:39-91`).

### 6.3 State packet framing (`stream/statePayloads.js`)
- 5-byte header: `uint8 packetType`, then `uint32LE stamp`.
  `packetType = 1` for **AddBalls**, `0` for **SetState** (`:33-38`, `:64-76`).
- `AddBalls2` payload = `["AddBalls2", [[stateBuffer, list(extraBallData)]]]`; each extra entry is
  the slim-item dict, or `[slimItem, damageState]` when the entity forces damage state, is a
  station, or has damageable health (`:78-133`).
- `SetState` dict key order (`:206-231`):
  `stamp, state, ego, industryLevel(0), researchLevel(0), damageState, dbuffState, aggressors(
  empty), droneState, slims, solItem, effectStates, allianceBridges(empty)`.
- Re-stamping rewrites bytes 1..4 of the embedded state buffer in place; `AddBalls2`, `SetState`
  and `PackagedAction` each have their own restamp path (`:234-362`).
- `entity.omitDestinyBall === true` excludes a ball from the state buffer;
  `omitSlimItem === true` excludes it from extra ball data.

### 6.4 Free-ball record layout (AddBalls / SetState) — `stream/ballEncoding.js:487-578`
Fixed head, little-endian throughout:
```
int64  itemID
uint8  mode
float  radius                        (default 1)
double position.x, .y, .z
uint8  flags
double mass                          (fallback 10_000 for container/wreck, else 1_000_000)
uint8  cloakMode
int64  harmonic                      (-1 when absent)
int32  corporationID                 (-1 when absent)
int32  allianceID                     (-1 when absent)
float  maxVelocity
double velocity.x, .y, .z
float  inertia                       (default 1)
float  speedFraction
uint8  0xff                          (fixed sentinel)
```
Ownership triple from `stream/freeBallHeader.js:58-71`; the int32 IDs are range-checked to signed
int32 and throw otherwise.

Mode-specific tails:
| mode | tail |
|---|---|
| GOTO (0) | `double targetPoint.x,.y,.z` → **24 B** |
| FOLLOW (1) | `int64 followID`, `float followRange` → **12 B** |
| FORMATION (12) | `int64 followID`, `float followRange`, `int32 effectStamp` → **16 B** |
| MISSILE (5) | `int64 followID`, `float followRange`, `int64 ownerID`, `int32 effectStamp`, `double target.x,.y,.z` → **48 B** |
| **WARP (3)** | `double target.x,.y,.z`, `int32 effectStamp`, `double totalDistance`, `double minimumRange`, `int64 ownerID` → **52 B** |
| ORBIT (4) | `int64 followID`, `float followRange` → **12 B** |
| MUSHROOM (6) | `float followRange`, `double span`, `int32 effectStamp`, `int64 ownerID` → **24 B** |
| TROLL (8) | `int32 effectStamp` → **4 B** |
| STOP (2) and others | no tail |

**This confirms the 52-byte WARP tail and its field order:
target(3×f64) → effectStamp(i32) → totalDistance(f64) → minimumRange(f64) → ownerID(i64).**

Rigid/static record (`:468-485`): `int64 itemID, uint8 mode, float radius, double pos×3,
uint8 flags`, then the static collision tail (mini-boxes / mini-balls / mini-capsules, each
prefixed by a `uint16LE` count, `staticBallTail.js:114-118`). An explicit
`entity.destinyCollisionTail` (Buffer, byte array, or hex string) overrides generation and also
suppresses the generated mini-geometry flag bits (`staticBallTail.js:25-47`, `:97-112`).

### 6.5 Flag rules
`ballEncoding.js:231-262`:
- Missile: `IS_FREE` alone, plus `IS_MASSIVE` only for a *targeted live* missile.
- Free balls: `IS_FREE | (configured & (IS_GLOBAL|IS_MASSIVE|IS_INTERACTIVE|IS_SPACEJUNK))`, then
  `IS_INTERACTIVE` forced on/off by `isFreeBallInteractive`.
- For ships: `IS_MASSIVE` is set **only** for a warp-alignment ball, defined as
  `kind === "ship" ∧ mode === WARP ∧ pendingWarp ∧ integer effectStamp < 0` (`:217-229`).
  So TQ's inbound AddBalls records use **`0x09`** for proper WARP and **`0x0d`** for the
  negative-effect-stamp alignment presentation. Rationale given: Carbon aliases a WARP record's
  total distance through `mLastCollision`, so a proper WARP record must stay out of the massive
  collision path or that phase input is overwritten before the warp solver reads it.
- Static fallbacks (`staticBallTail.js:77-88`): station/stargate `IS_GLOBAL|IS_MASSIVE`;
  orbital `IS_GLOBAL|IS_INTERACTIVE`; container/wreck `IS_INTERACTIVE`; otherwise `IS_GLOBAL`.
  `IS_FREE` is always cleared for statics.
- All flag bytes masked to `0xff`; configured modes clamped to `[GOTO(0), FORMATION(12)]`
  (`ballConfig.js:28-45`).

### 6.6 `effectStamp` sign semantics (confirms your reading)
- `< 0` ⇒ **preparing / aligning**; normalized to exactly `-1`
  (`warpSerialization.js:56-62`, `warpState.js:25-39`).
- `≥ 0` ⇒ a real stamp; handoff only arms when `effectStamp >= 0` (`warpHandoff.js:132-141`).
- Target point selection follows the sign (`ballEncoding.js:284-305`): while preparing (not in
  active warp, **or** `effectStamp < 0`) the encoder sends `warpState.rawDestination` plus the
  minimum range, because the client still executes RealWarp and would otherwise apply the stop
  range twice; an active proper warp sends the already-adjusted `targetPoint` so a reconnect
  resumes the modeled trajectory.
- Mode presentation (`presentation/mode.js:240-271`): WARP with `pendingWarp ∧ effectStamp < 0`
  emits the **warp-prepare** command update; otherwise the in-flight acquire updates (different
  builders for a piloted vs. sessionless ball).
- For non-WARP modes, `SetSpeedFraction` is emitted when `speedFraction > 0` and
  `SetBallVelocity` when `|v| > 0` — **never for WARP** (`mode.js:276-293`).
- Fallback target point when none is stored: `position + direction·1.0e16`
  (`ballEncoding.js:268-282`).

### 6.7 Action payload inventory (`stream/actions.js`)
`GotoDirection, GotoPoint, FollowBall, WarpTo, AddBall, EntityWarpIn, Orbit, SetSpeedFraction,
Stop, SetBallVelocity, SetBallPosition, SetBallAngularVelocity, SetMaxAngularVelocity,
SetBallRotation, SetBallAgility, SetBallAngularAgility, SetBallMass, SetMaxSpeed, SetBallMassive,
SetBallFree, SetBallInteractive, SetBallRadius, SetMaxAngularSpeed, SetBallTroll, SetBallHarmonic,
CloakBall, UncloakBall, LaunchMissile, BallNotGlobal, RemoveGlobalBall, OnSpecialFX,
OnDamageStateChange, OnSlimItemChange, OnClientControllerEvent, OnDbuffUpdated,
TerminalPlayDestructionEffect, RemoveBall, RemoveBalls`.
Vectors go out as three bare numbers; scalars as marshal `{type:"real"}` wrappers
(`primitives.js:40-54`); booleans as `1`/`0`. `BallNotGlobal` takes a **bubbleID int32**, not an
entity ID.

### 6.8 `PackagedAction` (`batching/packagedAction.js`)
- Payload name `"PackagedAction"`; body is a marshal stream decoding to a list of exactly
  `[stamp, payload]` pairs, where `stamp` is an integer (Number or BigInt) in `[0, 0xffffffff]`
  and `payload` is a 2-element array whose first element is a non-empty string or Buffer
  (`:33-80`). Anything else is rejected as malformed.
- Stamps are normalized with `>>> 0` (unsigned 32-bit), unlike the 31-bit ring used for ordering.

### 6.9 Entity identity (`identity/entityID.js`)
- Ball IDs are **signed int64**: `[-(2^63), 2^63 - 1]`. Negative IDs are valid wire/local
  identities; only *persistent database* entities are positive (`:5-11`).
- Python boundary wrappers `{type: "long"|"PyLong", value}` are unwrapped before parsing (`:13-34`).
- A Number that is not a safe integer has already lost identity bits and is **rejected**, not
  repaired via BigInt (`:43-48`). Strings must match `/^-?\d+$/`.
- `"invalid-persistent-identity"` is the sentinel written when a present-but-unparseable
  persistent ID is serialized (`warpSerialization.js:18`, `warpState.js:19`).
- Orbit plane seed uses the low 16 bits: `itemID & 0xffff` (`movement.js:529-534`).

### 6.10 Coordinate frame (`delivery/coordinateFrame.js`)
`ACTIVE_DESTINY_COORDINATE_FRAME = "world"`. `shouldRebaseDestinyPackets()` is therefore always
false and `worldToClientPosition` / `clientToWorldPosition` are identity. The rebase machinery
(`session._space.destinyPositionOrigin`) exists but is inert. **Confirmed absence of any
client-relative coordinate rebase.**

### 6.11 Delivery contracts (`authority/destinyContracts.js`)
Eight contracts: `state_reset, bootstrap_acquire, owner_pilot_command,
owner_missile_lifecycle, observer_missile_lifecycle, critical_movement_or_shipprime,
combat_noncritical, destruction_teardown`. All but `combat_noncritical` are **critical**.
Inference order (`:52-157`): explicit option → `SetState` ⇒ state_reset →
explodingRemovalGroup / destructionTeardown ⇒ destruction_teardown →
ownerMissileLifecycleGroup → missileLifecycleGroup →
`destinyAuthorityOwnerPilotCommand` or commandSource `CmdGotoDirection`/`CmdSteerDirection` ⇒
owner_pilot_command → movement payloads (`GotoDirection, GotoPoint, LaunchMissile, Orbit,
FollowBall, Stop, WarpTo`) or ship-prime setters (`CloakBall, UncloakBall, SetBallAgility,
SetBallMass, SetMaxSpeed, SetBallMassive, SetSpeedFraction, SetBallVelocity, SetBallPosition`)
⇒ critical_movement_or_shipprime → `AddBalls2` or freshAcquireLifecycleGroup ⇒
bootstrap_acquire → `RemoveBalls` ⇒ destruction_teardown → damage/SpecialFX/ProjectileFired ⇒
combat_noncritical → default combat_noncritical.
Delivery lead ceilings in `deliveryPolicy.js` are mostly `MICHELLE_HELD_FUTURE_DESTINY_LEAD = 1`
(`:2236,2244,2252,2510,2518,2536`); `OWNER_PENDING_GOTO_DUPLICATE_ALIGNMENT = 0.9998` is the
dot-product threshold for suppressing a duplicate owner GOTO (`:35`).

---

## 7. Oracles: files matching parity / golden / fixture / vector

Repo-wide search (excluding `node_modules`):

| File | What it is |
|---|---|
| `server/src/space/destiny/simulation/warpInPointParity.js` (284) | **skipped per instructions** |
| `server/src/space/modules/moduleClientParityAuthority.js` | Module-state client parity authority — not Destiny motion; module attribute presentation |
| `server/src/space/modules/moduleLoadParity.js` | Module load/charge parity |
| `server/src/services/industry/industryParityHelpers.js` | Industry, unrelated |
| `server/src/services/sovereignty/sovParityReset.js` | Sovereignty, unrelated |
| `server/src/common/vector.js` | Plain vector math helpers (`DEFAULT_RIGHT`, `DEFAULT_UP`, `distanceSquared`, `buildPerpendicular`, …) |
| `server/src/services/activity/warpVectorMgrService.js` | Warp "vector manager" *service* (naming collision, not a test vector) |
| `server/src/services/chat/commands/spaceVectors.js` | GM chat command |
| `server/src/abyssal/catalog/fixtures/t0TranquilRetailCapturedSceneryFixture.json` | A captured retail abyssal scenery snapshot |
| `bots/scenarios/fixture/golden-world.json`, `brain/scenarios/golden-world.overrides.json` | Bot/brain golden-world scenario seed, not a Destiny oracle |
| `bots/src/teststack/golden.mts` (+ `.test.mts`) | Bot test-stack golden harness |
| `bots/src/roles/fixture.mts`, `bots/src/queue/page/testFixtures.mts`, `brain/src/harness/fixture.mts`, `brain/src/industry/orders.fixture.mts`, `brain/src/scheduler/tickExposition.fixture.mts` | Bot/brain fixtures |
| `server/src/services/corporation/coreFixtureSeeder.js`, `server/src/services/_shared/fixtureIdentities.js` | Seed data |

**Confirmed absence: there is no golden/parity test-vector corpus for the Destiny simulation
itself.** Nothing under `server/src/space/destiny/` matches those names except
`warpInPointParity.js`, and `destiny/native/README.md` says the lane "may hold fixtures or
optional oracle adapters" — it holds neither; the directory contains only that README.
Likewise `doc/PARITY_WARP_IN_MATERIALIZATION.md`, cited by name in
`delivery/michelleContract.js:9`, **is not present in this checkout** (`doc/` has no `*PARITY*`
file). The captured-recording claims quoted throughout the comments (46 recordings, 6,203
updates, Jita gate group `50001250`, salvage golden tick 62674, Rouvenor tick 49989) are therefore
**assertions in comments with no checked-in evidence** — usable as hypotheses, not as oracles.

Tests that do exist nearby, and that double as behavioural specs:
`destiny/identity/entityIDFastPath.test.mts`,
`runtime/scene/staticVisibilityGate.test.mts` (asserts e.g. a structure's
`publicGridKey === "0:0:2"` and exercises the box/nearby-radius boundaries),
`runtime/scene/dynamicVisibilityPassMemo.test.mts`,
`runtime/scene/targetLockValidationMemo.test.mts` (pulls `TURN_ALIGNMENT_RADIANS` out of the
kinematics module).

---

## 8. Divergences / cross-check notes for our Rust port

1. **`k_d` floor.** They apply the `0.001` floor to deceleration as well as acceleration and say
   outright that Carbon does not. Our `k_d = min(wf/3000, 2)` matches Carbon, not them. If we ever
   compare against their numbers at tiny warp factors, expect disagreement.
2. **Align gate shape.** Ours is `speed² > (0.75·maxVelocity)²`; theirs is
   `actualSpeedFraction > 0.75` where `actualSpeedFraction = clamp(|v|/maxVelocity, 0, 1)`.
   Algebraically the same for non-negative values, but their clamp means a ship above `maxVelocity`
   can never exceed 1.0 — immaterial for the `> 0.75` test. Their *dot* gate is
   `|1 - dot| < 0.01`, plus a **180-tick forced activation** we may not have.
3. **Accel distance uses `v_base/k_a`, not the time integral.** Their accel-phase traveled distance
   is `min(accelDistance, v_base/k_a)` — a closed form that falls out of `v = k_a e^{k_a s}`, so
   `x = v/k_a`. Worth matching exactly; the naive `∫v dt = (e^{k_a s} - 1)` form differs by the
   constant `1` (which is exactly the `e^{k_a t} - 1` vs `e^{k_a t}` difference in our curve).
4. **The `+1` / `-1` pair in the short-warp geometry** (`(D+1)·k_a·k_d/(k_a+k_d)` and
   `decelDistance = v_max/k_d - 1`) is asserted to be part of native geometry, not a fudge.
5. **Completion is speed-based**, `v < dropoutSpeed`, with `dropoutSpeed = min(maxVelocity/2, 100)`
   — not a duration timer. The duration timer is only the `dropoutSpeed ≤ 0` fallback.
6. **One STOP evolution tick after dropout** moves the ship further than the dropout point; the
   final landing position is `integrate(dropoutPos, dropoutVel, a=0, Δt = ticks·1 s)`.
7. **The WARP ball is not massive.** `0x09` for proper warp, `0x0d` for alignment presentation —
   check our AddBalls flag byte.
8. **The pilot never gets a `SetMaxSpeed` for a warp** in their shipped config (ramp disabled,
   `buildWarpPrepareDispatch` noted as sending none).
9. **No `SetSpeedFraction` / `SetBallVelocity` while in WARP mode.**
10. **`nativeWarpCommand` is a per-warp field with values `"WARP"` / `"GOTO"` / null**, decided by
    the 100 km squared test at build time and re-decided on rehydration; a `"GOTO"` pending warp
    degrades to an ordinary GOTO on activation and leaves `speedFraction = 1`.
