# Inventory: space, destiny and combat (EvEJS)

Everything in EvEJS that happens once a ship is in space: the Destiny wire
protocol (ball serialisation, `SetState`/`AddBalls2`, ~60 action verbs), the
delivery machinery that pushes `DoDestinyUpdate` to a session, the subwarp and
warp movement laws, visibility/bubble membership, the module activation runtime,
the weapon and damage model, and ship destruction. Measured: **303 files,
~149,900 lines** — `space/destiny/**` 116 files / 41,941 LOC, `space/runtime/**`
129 files / 67,912 LOC, `space/modules/**` 28 files / 21,071 LOC,
`space/combat/**` 11 files / 5,220 LOC, loose `space/*.js` 19 files / 13,750 LOC.
Two structural facts dominate everything below. First, `space/destiny/**` is
**not an engine**: it is pure functions with every collaborator injected, and
`destiny/index.js` is the only place the dependencies are actually bound
(`destiny/index.js:995-1035`); all orchestration lives in
`space/runtime/scene/*.js` (18 files, 28,468 LOC) — `authority/README.md` says
so outright ("the scene runtime remains the orchestrator"). Second, the release
ships **zero tests** (`find` for `*.test.js`, `*.spec.js`, `test/`, `__tests__/`
over the whole repo: nothing; 27 files export `_testing` hooks with no runner)
and **zero `TODO`/`FIXME`/`XXX`/`HACK` markers** anywhere in this area, so the
code reads as finished uniformly. Confidence has to come from structure, not
prose.

All paths below are relative to `$EVEJS_RUN/server/src/`.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---|---|---|
| Ball record encoder | 5: `destiny/stream/ballEncoding.js`, `staticBallTail.js`, `freeBallHeader.js`, `ballConfig.js`, `destiny/constants.js` | 1,240 | functional | `encodeEntityBall` dispatches free vs rigid unconditionally (`stream/ballEncoding.js:580-584`); free path writes 11 header fields then a mode tail for 8 of 13 modes (`:487-576`); rigid writes header + mini-geometry sections (`:468-485`, `stream/staticBallTail.js:114-271`). Cross-checked field-for-field against our own `_destiny.dll` disassembly — 9 of 11 `[I]` fields named, all five sentinel defaults match (`docs/re/destiny-ball-crosscheck.md` §1). |
| `SetState` / `AddBalls2` assembly | 2: `destiny/stream/statePayloads.js`, `destiny/index.js` | 1,448 | functional | `buildAddBalls2Payload` → `["AddBalls2",[[stateBuf, list(extra)]]]` (`stream/statePayloads.js:111-133`); `buildSetStatePayload` builds a 13-key keyval (`:207-229`); state header is `u8 packetType + u32LE stamp`, 0=SetState / 1=AddBalls (`:33-76`). Throws on a missing dependency rather than degrading (`:181-189`). Slim-item dict is ~590 lines of per-kind keys (`destiny/index.js:359-950`). |
| Destiny action verbs | 5: `destiny/stream/actions.js`, `destiny/protocol/{payloads,payloadIdentity,direction,payloadSummary}.js` | 1,670 | functional | Every builder returns `[name, args]` with name-string dispatch and no opcode numbers (`stream/actions.js:28-199`); contract inference matches by payload-name string (`authority/destinyContracts.js:65-94`). |
| Initial ballpark bootstrap (undock / login) | 8: `destiny/bootstrap/initialBallpark.js`, `destiny/presentation/{addBalls,shipPrime,mode,freshAcquire,addBallsTimeline}.js`, `runtime/scene/presentation.js` (`ensureInitialBallpark`), `runtime/firstSightPresentation.js` | ~2,900 (of which `ensureInitialBallpark` is ~1,160) | functional | Three-phase transaction with a named reject code at every boundary (`runtime/scene/presentation.js:2561-2578`, `:3652-3663`); core group order is AddBalls2(+SetBallInteractive) `waitForBubble:true` → undock movement prime → SetState (`:3549-3584`); follow-up is prime → mode → FX replay (`:3367-3388`). Ego is force-appended to both lanes (`destiny/bootstrap/initialBallpark.js:81-95`). |
| Delivery contract + stamp algebra | 17: `destiny/delivery/**` (`deliveryPolicy.js` 2,839, `michelleContract.js`, `stamps.js`, `deliveryTransaction.js`, `sendOptions.js`, …) | 6,460 | functional | `attemptDestinyNotificationDelivery` returns accepted/rejected/ambiguous and the caller destroys the socket on ambiguous (`delivery/michelleContract.js:39-73`, consumed at `runtime/scene/presentation.js:2565-2577`); stamp = `floor(nowMs/1000) & 0x7fffffff` (`delivery/stamps.js:12-24`); lead constants carry client-line citations (`delivery/michelleContract.js:6-17`). |
| Dispatch + notification flush | 8: `destiny/dispatch/**` (`destinyDispatch.js` 3,890, `contractDispatch.js`, `ownerDispatch.js`, `observerReconciliation.js`, `sceneRefresh.js`, `watcherCorrections.js`) + `destiny/batching/**` | 7,529 | functional | Payload tuple built at `dispatch/destinyDispatch.js:1532-1535`, sent as `sendNotification("DoDestinyUpdate","clientID",tuple)` (`delivery/michelleContract.js:3-4,48-52`), retried on a retryable reject (`dispatch/destinyDispatch.js:1551-1565`). Has its own well-formedness assertion pass, `validateDestinyNotificationGroup` (`:1251`). |
| Authority / session Destiny state | 6: `destiny/authority/**` (`destinyAuthority.js` 2,578, `destinySessionState.js`, `scenePositionAuthority.js`, `destinyContracts.js`, `sceneRangeDecision.js`, `destinyJourneyLog.js`) | 4,208 | functional | 8 named delivery contracts with a critical subset (`authority/destinyContracts.js:3-24`) and a 100-line classifier with no fallthrough gaps (`:52-157`); per-session stamp ledger fields are all initialised at attach (`runtime/scene/sessions.js:425-470`). |
| Subwarp movement law | 6: `destiny/simulation/{nativeSubwarp,movement,motionState,carbonIntegration}.js`, `runtime/destinyMovementEngine.js`, `destiny/projection/**` | ~6,400 | functional | `integrateCarbonMotion` is a closed-form exponential-drag integrator with an explicit λ→0 branch (`simulation/carbonIntegration.js:55-110`) and its analytic inverse (`:112-147`); `buildUndockMovement` sets mode GOTO / `targetPoint = pos + dir·1e16` / `velocity = dir·maxVelocity` (`simulation/motionState.js:109-128`). |
| Warp | ~20: `destiny/simulation/warp*.js`, `destiny/commands/warpCommands.js`, `destiny/visibility/{warpHandoff,warpLanding}.js`, `destiny/delivery/postWarpDemotion.js`, `runtime/scene/fleetWarp.js` | ~8,000 | partial | Full state machine, serialisation, interdiction geometry (`simulation/warpInterdiction.js:10-38`) and native-boundary sampling (`simulation/warpNativeBoundary.js:29-60`) are all present and unconditional. Marked partial because the warp-in offset is an admitted parity guess — `MIN_WARP_IN_POINT_RADIUS = 90000`, `JUMP_RADIUS_FACTOR = 130` (`simulation/warpInPointParity.js:7-9`) — and a 280-line divergence logger exists for it (`simulation/warpDiagnostics.js`), which implies it diverges. |
| Visibility / bubble / public grid | ~12: `destiny/visibility/**`, `runtime/scene/visibility.js` (5,664), `runtime/scene/spatialTopology.js`, `runtime/visibility.js`, `runtime/visibilityMembership.js` | ~10,600 | partial | Behaviour works, but the *extracted lane is a skeleton*: `destiny/visibility/sceneTickCoordinator.js` is a 3-line re-export, and the thing it re-exports does nothing itself — every reconcile step is an injected callback defaulting to `null` (`visibility/bubbleUpdater.js:12-35`). `visibility/README.md` is written in the future tense ("will own"). The grid constants are tuned to their own captures by their own admission (`visibility/constants.js:22-32`). |
| Pilot commands | 19: `destiny/commands/**` (`warpCommands.js` 1,397, `subwarpCommands.js` 846, `stopSpeedCommands.js`, `tractorBeam.js`, `fighter.js`) + `runtime/scene/pilotCommands.js` | ~4,300 | functional | Real command handlers for warp/subwarp/stop/orbit/follow. Note that 8 of the 19 files are 9-25 line single-field setters (`commands/npc.js`, `commands/mining.js`, `commands/docking.js` are each one assignment) — the file count overstates the subsystem. |
| Targeting | 4: `runtime/targeting.js` (1,532), `runtime/scene/targeting.js`, `runtime/spaceRuntime/targeting.js`, `modules/targetingModuleRuntime.js` | 2,023 | functional | Canonical lock-time formula `(4e7/scanRes)/asinh(sigRadius)²` with finite guards (`runtime/targeting.js:106-115`); loss reasons enumerated with TQ-recording counts (`:136-143`); derived-attribute invalidation set (`:145-151`). |
| Module activation runtime | ~35: `space/modules/**` (`superweapons/superweaponRuntime.js` 4,555, `assistanceModuleRuntime.js`, `hostileModuleRuntime.js`, `salvagerRuntime.js`, `commandBurstRuntime.js`, `localCycleRuntime.js`, `jammerModuleRuntime.js`, `entosisLinkRuntime.js`, …) + `runtime/scene/modules.js` (4,227) + `runtime/module*.js` | ~29,400 | functional | Every runtime I sampled has substantive logic and a full export surface; `moduleClientParityAuthority.js:21-60` encodes per-family hardpoint/bootstrap policy and a 6-profile space-attach matrix. No stub files in the tree. Caveat: 4,555 of those lines are superweapons, which is not a path we need. |
| Weapons + damage | 11: `space/combat/**` (`weaponDogma.js` 1,701, `damage.js`, `laserTurrets.js`, `precursorTurrets.js`, `vortonProjectors.js`, `breacherPods.js`, `missiles/*`) | 5,220 | functional | `applyDamageToEntity` walks shield→armour→structure with per-type resonances (`combat/damage.js:291-...`); `buildDamageState` returns the exact `[[shieldRatio, shieldTau, long(ft)], armorRatio, structureRatio]` the client reads (`:262-285`); turret quality bands, vorton arc-chain target selection and breacher DoT ticks are all implemented. `weaponDogma.js` exports 25 symbols, no placeholders. |
| Missiles | ~6: `runtime/missileEntity.js`, `runtime/missileClientBall.js`, `runtime/autoTargetingMissiles.js`, `combat/missiles/{missileSolver,missileReloads}.js`, `modules/defenderMissileRuntime.js` | 2,432 | functional | Missiles are real Destiny balls: `AddBalls2` creates the ball and `LaunchMissile` activates native flight as a dependent action on the same stamp (`destiny/presentation/addBalls.js:243-250`); `isTargetedLiveMissile` gates MISSILE mode and silently downgrades to GOTO when targetless (`stream/ballEncoding.js:90-118`). |
| Destruction / wrecks / killmails | 5: `space/shipDestruction.js` (1,968), `space/wreckUtils.js`, `space/corpseUtils.js`, `runtime/combatDestruction.js`, `combat/killmailTracker.js` (1,132) | 4,582 | functional | Killmail tracker has a damage ledger, attacker/victim resolution, final-blow selection and crash-recovery of pending work (`combat/killmailTracker.js` exports, incl. `recoverPendingKillmailWork`). Corpse is its own ball kind with a client-source citation (`destiny/constants.js:48-52`). |
| Scene tick + clock | 4: `runtime/scene/tick.js` (3,185), `runtime/scene/clock.js` (941), `runtime/runtimeClock.js`, `runtime/destinyStamps.js` | ~4,600 | partial | `tick(wallclockNow)` is one 2,700-line function that advances the clock, opens a presentation batch, and fans out to ~15 subsystem `tickScene` hooks each wrapped in its own try/catch (`runtime/scene/tick.js:406-560`). Marked partial on one named regression they did not fix: periodic `DoSimClockRebase` "made Michelle's ball time run backwards", so rebases are limited to scene entry and explicit TiDi changes "until the native TiDi update path is reproduced" (`:428-431`). |
| Transitions (dock/undock/jump/eject) | 1: `space/transitions.js` (5,653, 107 functions) | 5,653 | functional | The undock arm computes a spawn point, moves the item, emits the session change, and calls `attachSession` with `undockBootstrapMovementPrime: true` (`transitions.js:2685-2781`); four separate `attachSession` call sites for the four transition kinds (`:2248`, `:2769`, `:5224`, `:5542`). |
| Native Destiny oracle | 1: `destiny/native/README.md` | 11 | stub | The directory contains a README and nothing else. It describes a boundary to "pinned CCP Carbon source" that does not exist in the tree. |
| Collision / bumping | 0 | 0 | stub | Absent by explicit design, stated three times: `destiny/simulation/README.md` ("No general ship/static contact solver, bumping, collision geometry … belongs here"), `destiny/authority/README.md`, `destiny/native/README.md`. Confirmed in code — undock actively sends `SetBallMassive(false)` on the ego ball (`destiny/presentation/shipPrime.js:94-98` → `stream/actions.js:185-187`). |

## Dependency order

Layered. Each edge says why it is an edge, not just that it is one.

**L0 — bytes and identity.** `destiny/stream/primitives.js`,
`destiny/identity/entityID.js`, `destiny/constants.js`.
- *Everything → identity.* `pushBigInt64` refuses an unsafe-integer Number and
  throws (`stream/primitives.js:89-113`). A ship's `itemID` exceeds 2^53, so a
  float path here corrupts every later payload silently rather than loudly. This
  must be exact before anything is sent.
- *Encoder and every mode decision → `constants.js`.* `BALL_MODE` is read by the
  encoder, by the mode-update builder, and by the config resolver. This is also
  the file we already know disagrees with our client (below).

**L1 — the two things a ball needs.**
- `ball encoder → L0`. Nothing can draw without it.
- `stamp algebra → constants`. `delivery/stamps.js:12-24`. The encoder embeds
  the stamp *inside* the state header (`stream/statePayloads.js:33-38`), and the
  client discards a batch whose first entry predates `latestSetStateTime`, so
  the encoder's bytes are useless without the stamp rule. These two are
  co-equal, not sequential.

**L2 — payloads.**
- `SetState/AddBalls2 → encoder + stamps + slim-item dict + damage state`. The
  slim-item builder (`destiny/index.js:359-950`) and `buildDamageState`
  (`combat/damage.js:262-285`) are injected dependencies of
  `buildSetStatePayload`, which **throws** without them
  (`stream/statePayloads.js:181-189`). They are not later work; they ship with
  the payload builder.
- `action verbs → L0 only`. `stream/actions.js` needs identity and nothing else.
  **Parallelisable with the whole ball-encoder line** — this is the cheapest
  independent lane in the area.

**L3 — can the client be told anything.**
- `delivery contract → notification name`. 108 lines
  (`delivery/michelleContract.js`). Port it *before* testing L2, because it is
  the only thing that reports whether a send was accepted, rejected or ambiguous
  — and ambiguous is the case that must kill the socket.
- `delivery transaction → delivery contract`. Visibility and the
  `initialBallparkVisualsSent` latch commit only inside `onDeliveryCommit`
  (`runtime/scene/presentation.js:2944-2952`), and `initialStateSent` only in
  the final-truth commit it schedules (`:3209`). Skip this and the server believes
  things the client never received, which is the failure mode that is hardest to
  debug later.

**L4 — something to encode.**
- `encoder → entity model`. A ball needs `position, velocity, mass, inertia,
  radius, maxVelocity, speedFraction, mode, direction, targetPoint`
  (`runtime/shipEntity.js`, written at `runtime/scene/sessions.js:425-470`), and
  a slim item needs `typeID, ownerID, charID, corpID`. **This, not the encoder,
  is the prerequisite our side actually lacks.**
- `motion state → entity model`. `simulation/motionState.js:109-128` is what
  makes the undocked ego ball a GOTO ball rather than a STOP ball. One file, 141
  lines, and it changes the wire bytes.

**L5 — the undock.**
- `initial ballpark bootstrap → all of L1..L4`. `ensureInitialBallpark`
  (`runtime/scene/presentation.js:2507`) touches the encoder, the stamps, both
  payload builders, four action verbs, the delivery transaction, the entity model
  and the motion state. It is the convergence point and cannot be partially
  ported.
- `transitions (undock arm) → bootstrap`. `transitions.js:2769-2781` is the
  caller that sets the flag the bootstrap reads. Port the bootstrap first and
  drive it by hand; the transition is the last mile, not the first.

**L6 — making it live.**
- `scene tick → bootstrap`. Without a tick the ball is drawn once and frozen.
  `runtime/scene/tick.js:406` advances the clock, then opens the presentation
  batch that every later subsystem writes into.
- `movement projection → tick + motion state`. The integrator needs a clock to
  integrate over. This is what makes the ship *move* after undock.
- `visibility/bubble → bootstrap + tick`. **Deliberately late.** A single-client
  undock needs none of it: the bootstrap already force-appends ego regardless of
  visibility (`destiny/bootstrap/initialBallpark.js:95`). Do not let 10,600 LOC
  of bubble logic block one ball.

**L7 — gameplay, mostly parallel.**
- `pilot commands → action verbs + movement + tick`.
- `warp → pilot commands + visibility`. Warp handoff moves an observer between
  grids, so it genuinely needs bubble membership
  (`destiny/visibility/warpHandoff.js`). Warp is the first thing that actually
  requires L6's visibility work.
- `targeting → entity model + tick`. **Independent of movement** — can run in
  parallel with the movement lane.
- `module runtime → targeting + tick`. Most modules need a locked target and a
  cycle schedule.
- `weapons/damage → module runtime + targeting`. Note the asymmetry: the
  *damage-state wire shape* is an L2 dependency (`SetState` needs it), while
  *damage application* is L7. Port the shape early, the model late.
- `destruction/wrecks/killmails → weapons`.
- `missiles → encoder (MISSILE tail) + weapons + tick`. Missiles are the one
  gameplay subsystem that reaches back into L1, because they need the MISSILE
  mode tail and the `LaunchMissile`-after-`AddBalls2` ordering
  (`destiny/presentation/addBalls.js:243-250`).

## What this unblocks for us

Our blocker is that nothing pushes a Destiny state containing the ego ball.
Their tree has that path end to end. In call order:

**1. Spawn point.** `space/transitions.js:2685` calls
`spaceRuntime.getStationUndockSpawnState(...)` →
`services/station/stationLocatorGeometry.js:386-399`
`buildStationUndockSpawnState`, returning `{position, direction}` from the
station's docking-locator geometry.

**2. Attach.** `space/transitions.js:2769-2781`
`spaceRuntime.attachSession(session, shipItem, {systemID, undockDirection,
speedFraction: 1, undockBootstrapMovementPrime: true, ...})`.

**3. Ego ball becomes a moving ball.** `runtime/scene/sessions.js:406-412`
calls `buildUndockMovement(shipEntity, undockDirection, 1)` →
`destiny/simulation/motionState.js:109-128`, which sets `mode = "GOTO"`,
`direction`, `targetPoint = position + direction × 1e16`, `speedFraction = 1`,
`velocity = direction × maxVelocity`. Then `runtime/scene/sessions.js:425-470`
writes `session._space` with `initialStateSent: false`,
`initialBallparkVisualsSent: false`, `undockBootstrapMovementPrime: true`.

**4. The client binds `beyonce`, which triggers the push.**
`services/ship/beyonceService.js:453` `bootstrapSessionBallpark` →
`:520` `spaceRuntime.ensureInitialBallpark(session, options)`.

**5. `ensureInitialBallpark`** — `runtime/scene/presentation.js:2507`. Order:
- `:2510-2555` guards and the in-progress / already-sent latch.
- `:2582` `egoEntity = this.getShipEntityForSession(session)`;
  `:2590` `refreshShipPresentationFields(egoEntity)`.
- `:2606` `buildInitialBallparkEntityPlan({...})` →
  `destiny/bootstrap/initialBallpark.js:58-103`. Returns `bootstrapEntities`
  (dynamic entities, **ego appended if missing**, `:95`) and
  `stateRefreshVisibleEntities` (all visible, **ego appended if missing**,
  `:81-84`). The load-bearing comment is `:55-57`: the full-state lane must
  retain ego.
- `:2652-2700` `syncClockOnce()` — TiDi notification then sim-clock rebase,
  gated *before* any state goes out.
- `:2801-2810` `addBallsStamp = getCurrentDestinyStamp(rawNow)`;
  `:3052` `setStateRawStamp = addBallsStamp + 1`; prime and mode share the
  SetState stamp.
- `:3549-3584` **core delivery groups, in this order**:
  1. `AddBalls2` for `bootstrapEntities` with `waitForBubble: true`, plus one
     `SetBallInteractive` per entity →
     `destiny/presentation/addBalls.js:74-90` →
     `destiny/index.js:995-1008` → `destiny/stream/statePayloads.js:111-133`.
     Shape: `["AddBalls2", [[<binary state, packetType 1>, <list of slim dicts
     or (slimDict, damageState) pairs>]]]`.
  2. Undock movement prime (undock only) →
     `destiny/presentation/shipPrime.js:72-116`: `OnSpecialFX("effects.Uncloak",
     6000 ms)`, `SetBallPosition`, `SetBallMassive(false)`, `SetBallMass`,
     `SetBallVelocity` if non-zero, `GotoDirection`.
  3. `SetState` at stamp+1 → `destiny/presentation/addBalls.js:92-116` →
     `destiny/index.js:1009-1035` → `destiny/stream/statePayloads.js:169-232`.
     Keyval keys in wire order: `stamp, state (packetType 0), ego,
     industryLevel, researchLevel, damageState, dbuffState, aggressors,
     droneState, slims, solItem, effectStates, allianceBridges`.
- `:3367-3388` **follow-up groups** after the core commits: ship prime
  (`SetBallAgility`, `SetBallMass`, `SetMaxSpeed`), then mode updates
  (`destiny/presentation/mode.js`), then active-FX replay.
- Every group leaves through `destiny/dispatch/destinyDispatch.js:1532`
  `buildDestinyUpdatePayload(updates, waitForBubble, delayedTargetEvents)` →
  `destiny/stream/statePayloads.js:363-378`, producing
  `[list_of_(stamp, payload), waitForBubble]`, sent by
  `destiny/delivery/michelleContract.js:48-52` as
  `sendNotification("DoDestinyUpdate", "clientID", tuple)`.

**Three things this gives us that `docs/archive/destiny-and-the-ballpark.md` does not.**

- **The bootstrap must be split across notifications.**
  `runtime/scene/presentation.js:2615-2618`: "V23.02 expects the initial
  bootstrap as a split AddBalls2 -> SetState -> prime/mode sequence. Collapsing
  everything into one waitForBubble batch leaves Michelle stuck in 'state
  waiting: yes' on login." Our §8 Step 4 plans a single `SetState`. Theirs sends
  `AddBalls2` first with `waitForBubble: true`, then `SetState` one stamp later
  in a separate notification. This is an empirical claim about build 3396210 and
  I cannot verify it, but it costs one extra notification to honour.
- **The undocked ego ball is GOTO, not STOP.**
  `destiny/simulation/motionState.js:109-128`. That means a 24-byte target-point
  tail, with `targetPoint = position + direction × 1e16` — a "straight line
  forever" sentinel, not a real destination. `docs/re/destiny-ball-crosscheck.md`
  §4.1 concluded "the minimum viable ball has no tail at all — header only",
  which is right for a parked ship and **wrong for the ship the client expects
  immediately after an undock**, which it animates flying out of the station.
- **Two `SetState` keys our §4.3 bag table does not list:** `aggressors`
  (empty dict) and `allianceBridges` (empty list),
  `destiny/stream/statePayloads.js:218,228`. Our decompile of `Park.SetState`
  does not read either, so they are probably harmless padding, but a real server
  sends them.

Also directly reusable: `damageState` entry shape is
`[[shieldRatio, shieldTau, long(simFileTime)], armorRatio, structureRatio]`,
with the shield triple replaced by `null` for a reinforced starbase
(`combat/damage.js:262-285`); `solItem` is a packed row of 11 columns with
literal `typeID 5 / groupID 5 / categoryID 2 / quantity -1`
(`destiny/index.js:41-52`, `:979-996`); `droneState` is a `Rowset` whose class
name must be `"eve.common.script.sys.rowset.Rowset"` (`destiny/index.js:65`,
`:961-968`).

## Effort to port, per subsystem

| subsystem | effort | why | risk |
|---|---|---|---|
| Ball record encoder | S | ~1,240 LOC of straight-line byte writes, and our cross-check already names the fields. We write it from `_destiny.dll`, not from them. | Their `BALL_MODE` 9/10 is wrong for us; `IS_FREE` must gate the physics block the same way; the unconditional `0xff` at `stream/ballEncoding.js:519` sits where our disassembly has a config-gated block 8. A wrong byte here is a mis-render, not a parse error — it will fail silently. |
| `SetState`/`AddBalls2` assembly | M | The envelope is small, but the slim-item dict is ~590 lines of per-entity-kind keys and we need the ship branch at minimum. | `foo.SlimItem` tolerance means a wrong key is ignored rather than refused, so a missing one shows up as a missing HUD field three steps later. The `Rowset` class-name string and the packed-row column type codes are build-coupled. |
| Destiny action verbs | S | Name-string dispatch, no opcode table. ~60 one-line builders. | Low. The main risk is a verb name our build renamed; the client logs and skips a bad action (our §4.2), so this self-reports. |
| Delivery contract + stamp algebra | S | 108 + 343 lines, pure arithmetic and one notification call. | Their lead of 1 is justified against *their* `michelle.py` `DoPreTick` line 906. If our 3619879 `DoPreTick` window differs, every batch lands in the wrong tick and the client desyncs rather than erroring. Check our own `michelle.py` first. |
| Initial ballpark bootstrap | L | 1,160 lines in one function, but most of it is their transaction/rollback discipline. The *sequence* is ~40 lines of real content; the rest is correctness machinery we may not need for single-player. | Porting only the happy path means the server's `initialStateSent` latch can get ahead of the client. Their design kills the socket on an ambiguous send for exactly this reason (`runtime/scene/presentation.js:2565-2577`). |
| Delivery transaction / dispatch | L | 14,000 LOC combined, and most of it is multi-session fanout we do not need yet. | Cherry-picking leaves the commit/rollback boundary half-built, which produces state the server thinks the client has. Prefer reimplementing the boundary small than porting it large. |
| Subwarp movement law | M | The integrator is 150 lines of closed-form maths we can derive independently; `nativeSubwarp.js` (1,870) is their scheduling around it. | `DESTINY_SPACE_FRICTION = 1_000_000` (`destiny/constants.js:41`) is load-bearing for every trajectory and is attributed to Carbon, not read from our client. Wrong friction gives plausible-but-wrong speeds that nobody notices for weeks. |
| Warp | L | ~8,000 LOC, a real state machine, plus handoff and landing. | `warpInPointParity.js:7-9` constants are parity guesses. A wrong warp-in offset puts the ship in the wrong place on landing and is invisible until someone warps to a gate and lands 90 km off. |
| Visibility / bubble | XL | 10,600 LOC and the extracted lane is a skeleton, so the real logic is a 5,664-line scene file. | `visibility/constants.js:22-32` admits two of its four range constants are fitted to their captures. Porting them means inheriting their guesses as our constants. |
| Pilot commands | M | Real logic, but 8 of 19 files are one-line setters. | Low; mostly depends on movement being right first. |
| Targeting | S | One formula plus bookkeeping; the formula is public knowledge. | `TARGETING_CLIENT_FALLBACK_LOCK_MS = 2000` and `TARGETING_MAX_LOCK_MS = 180000` (`runtime/targeting.js:131-133`) are their choices. Fallback paths that silently return a plausible number are the dangerous kind. |
| Module activation runtime | XL | ~29,400 LOC across 35 files, each a different module family. | Every file carries hardcoded SDE group/effect/attribute IDs with no name lookup (`runtime/moduleAttributes.js:88-120`). A shifted ID applies the wrong modifier to the right module. |
| Weapons + damage | L | 5,220 LOC, well structured, and the damage model is closed-form. | `combat/damage.js:19-44` uses `getAttributeIDByNames(...) || N` — the literal fallback fires whenever the name lookup misses, so a wrong fallback is undetectable from the outside. |
| Missiles | M | 2,432 LOC; needs the MISSILE mode tail and the stamp-ordering rule. | The `AddBalls2`-then-`LaunchMissile`-on-the-same-stamp ordering is a client requirement they discovered (`destiny/presentation/addBalls.js:243-245`). Get it wrong and missiles appear but never fly. |
| Destruction / wrecks / killmails | L | 4,582 LOC, lots of inventory interaction outside this area. | Low wire risk; high data-model risk. |
| Scene tick + clock | M | The tick is one long function but the structure is simple: advance clock, batch, fan out. | Their own named regression: periodic `DoSimClockRebase` ran Michelle's ball time backwards (`runtime/scene/tick.js:428-431`). If we emit rebases on a timer we will reproduce it exactly. |
| Transitions | L | 5,653 LOC covering dock, undock, jump, eject, board, with four `attachSession` sites. | Mostly our own session machinery; low build-delta risk. |

## Build-delta hazards

Ordered by how silently each one fails for build 3619879.

1. **`BALL_MODE` positions 9 and 10** — `destiny/constants.js:15-16` has
   `MINIBALL: 9, FIELD: 10`; our client says FIELD 9, MINIBALL 10. Already
   known, but note the blast radius: `FIELD` is a *supported free mode* in their
   encoder (`stream/ballEncoding.js:59`, `:63`), and the mode-tail switch has no
   `FIELD` case (`:521-574`), so a force-field ball writes mode byte 10 with an
   empty tail where our client expects 9 with an empty tail. **It parses fine
   and renders wrong.** Their range clamp `[GOTO=0, FORMATION=12]`
   (`stream/ballConfig.js:29-45`) happens to match ours, so the clamp will not
   catch it.
2. **The unconditional `0xff`** — `stream/ballEncoding.js:519`, written between
   `speedFraction` and the mode tail. Our disassembly has block 8 gated on a
   config byte at `0x1800d42d1`. Both cannot be right (crosscheck §6.1 open).
3. **Michelle tick-lead constants** — `destiny/delivery/michelleContract.js:6-17`.
   `MICHELLE_HELD_FUTURE_DESTINY_LEAD = 1` and
   `MICHELLE_CLIENT_IMMEDIATE_FUTURE_DELTA = 3`, both cited to
   *their* `michelle.py` `DoPreTick` line 906 (`0 < stamp - currentTime < 3`).
   Check our own build's `DoPreTick` before trusting either number.
4. **Marshal packed-row column type codes** — `destiny/index.js:41-52`:
   `0x14, 0x03, 0x02, 0x0b, 0x81` for the `solItem` row. If 3619879's packed-row
   encoding differs, this is silent corruption inside a field the client only
   stores.
5. **The accepted `Rowset` class name** — `destiny/index.js:65`
   `"eve.common.script.sys.rowset.Rowset"`, with the note at `:962` that "V23.02
   rejects `util.Rowset` here during remote SetState unmarshal". The accepted
   class name lives in the client's unmarshal whitelist and is build-coupled.
6. **`effects.Uncloak` with a 6000 ms duration** —
   `destiny/presentation/shipPrime.js:23-24`. An effect GUID string plus a magic
   duration, sent on every undock.
7. **Dogma attribute-ID literals with no name lookup** —
   `runtime/moduleAttributes.js:88-120`: 20, 51, 73, 147, 554, 567, 763, 796,
   669, 713, 714.
8. **Attribute-ID literals used as `|| fallback`** — `combat/damage.js:19-44`:
   hp 9, armorHP 265, shieldCapacity 263, armour resonances 267-270, shield
   resonances 271-274, structure resonances 109-113. The `||` means the literal
   fires whenever the name lookup misses, so a wrong literal never surfaces as
   an error.
9. **SDE group / type / effect literals** — `combat/weaponDogma.js:78-82`
   (53, 55, 74, 1986, 4060); `modules/targetingModuleRuntime.js:28-32` (groups
   96, 82; effects 54, 55); `modules/hostileModuleRuntime.js:113-114` (effects
   6848, 6849); `modules/moduleClientParityAuthority.js:18-19` (481, 1226);
   `modules/projectedWeaponModifiers.js:11` (3300) and `:31` (55033);
   `runtime/abyssalRoomEntity.js:85-93` (366, 49689, 47951, 49661, 49662);
   `runtime/visibility.js:102` (type 58745);
   `runtime/shipActionRestrictions.js:86` (515);
   `modules/commandBurstRuntime.js:68` (4905). These track the SDE edition
   rather than the client build, but they still move.
10. **Bubble / grid geometry** — `destiny/visibility/constants.js:6-32`.
    `BUBBLE_RADIUS 250 km` and `PUBLIC_GRID_BOX 7,864,320 m` are at least
    attributed; `MICHELLE_TRANSIENT_WARP_VISIBILITY 50 Mm` (`:27`) and
    `PUBLIC_GRID_NEARBY 12.5 Mm` (`:32`) are, by their own comments, fitted to
    their captures of Jita.
11. **`CLIENT_WARP_VISUAL_ACTIVATION_ACCELERATION_MS2 = 310_000`** —
    `destiny/constants.js:42-44`, cited to a specific client `WarpFlash.py`.
    Build-specific by construction.
12. **Carbon-attributed physics constants** — `DESTINY_SPACE_FRICTION = 1_000_000`
    and `DESTINY_ORBITAL_PRECESSION_PER_TICK = 0.001`
    (`destiny/constants.js:37-41`), both attributed to `Carbon Ballpark.cpp`, not
    read from our client.
13. **Warp-in parity guesses** — `destiny/simulation/warpInPointParity.js:7-9`:
    `GROUP_WRECK 186`, `MIN_WARP_IN_POINT_RADIUS 90000`,
    `JUMP_RADIUS_FACTOR 130`.
14. **Stamp domain convention** — `DESTINY_STAMP_MODULUS = 0x80000000`
    (`destiny/delivery/stamps.js:12`) keeps stamps in the non-negative half of
    int32, while the state header writes `writeUInt32LE`
    (`stream/statePayloads.js:36`). The agreement is probably real but it is a
    convention they chose, not a value they read.
15. **A dead config hook worth not reviving** —
    `destiny/simulation/motionState.js:5-9` reads
    `destinyConstants.MAX_SUBWARP_SPEED_FRACTION`, which `destiny/constants.js`
    **does not export**, so the clamp silently defaults to 1. Harmless as
    shipped; a trap if someone ports the lookup and then supplies the constant.

**Licensing hazard, restated with the file list.** Four files assert that
`carbonengine/destiny v3.1.1` is MIT © CCP Games 2026:
`destiny/simulation/README.md`, `destiny/batching/README.md`,
`destiny/visibility/README.md`, and inline at
`destiny/simulation/carbonIntegration.js:3-4`. Three more cite Carbon internals
as sources without the MIT claim: `destiny/constants.js:3` ("Wire values from
Carbon `IDstConstants.h`"), `destiny/constants.js:37-40` ("Carbon
`Ballpark.cpp` ORBITAL_PRECESSION"), and
`destiny/bootstrap/initialBallpark.js:55` ("CCP
`Ballpark::PyWriteFullStateToStream`"). `docs/re/destiny-ball-crosscheck.md` §5
already rules the MIT notice false; this is the complete set of files a reviewer
must treat as AGPL-with-a-false-MIT-header. Note also that `constants.js:3` is
the provenance claim for the very table that is wrong at positions 9/10 — which
is the best available evidence that the claim is not a direct read of real
Carbon source.

## Notes for the coordinator

**Surprising.**

- **Zero tests and zero `TODO` markers across ~150k LOC.** There is no textual
  signal anywhere in this area distinguishing finished code from aspirational
  code. The only two honest stub signals I found are `destiny/native/` (a
  README and nothing else) and `destiny/visibility/sceneTickCoordinator.js` (a
  3-line re-export of a file whose every step is an injected callback defaulting
  to `null`). Treat uniformly confident prose as uninformative.
- **`destiny/**` is a payload library, not an engine.** 41,941 LOC of pure
  functions with everything injected; `destiny/index.js:995-1035` is the only
  wiring point. The behaviour is 28,468 LOC in `runtime/scene/`. Porting
  "`destiny/`" buys payload builders and no behaviour, which is a trap if the
  directory name is used to scope a round.
- **There is no collision and no bumping, anywhere, by design.** Stated in three
  READMEs and enforced in code: undock sends `SetBallMassive(false)` on the ego
  ball (`destiny/presentation/shipPrime.js:94-98`). For the co-op/PvP end goal
  this is a hole they have not dug, so there is nothing here to port for it.
- **Their `BALL_MODE` table is wrong at 9/10 and cites Carbon as its source.**
  That combination is worth remembering as a general prior about their citations:
  the attribution is confident and the value is wrong.
- **`npcAuthoredStartupEnabled` defaults to `false`**
  (`config/schema/npc.js:302-304`), while `miningEnabled`
  (`config/schema/mining.js:362`) and `wormholesEnabled`
  (`config/schema/world.js:5`) default to `true`. So "they have NPCs" is not
  evidenced by the shipped default config.

**Could not determine.**

- Whether the `AddBalls2`-then-`SetState` split is genuinely required by the
  client or a workaround for something else in their own stack. Their comment at
  `runtime/scene/presentation.js:2615-2618` is the only evidence and it names
  V23.02.
- What `aggressors` and `allianceBridges` in the `SetState` bag are for. Both
  are built empty (`destiny/stream/statePayloads.js:218,228`) and nothing in
  their tree populates either.
- Whether their warp-in offsets are right. The existence of a 280-line
  divergence logger (`destiny/simulation/warpDiagnostics.js`) next to admitted
  parity-guess constants suggests they are not, but I have no capture to check
  against.
- Whether `destiny/bootstrap/staticBootstrap.js` matters for our first undock. I
  read `initialBallpark.js` closely and only skimmed its sibling.

**What I would look at next, in priority order.**

1. **`destiny/dispatch/destinyDispatch.js:1251` `validateDestinyNotificationGroup`.**
   This is their own assertion list for a well-formed `DoDestinyUpdate` — a free
   test suite for our encoder, written by someone who had a client to fail
   against. Highest value per line in the whole area and I only saw its call
   site.
2. **`destiny/delivery/deliveryPolicy.js`, 2,839 lines.** The single largest
   file I did not open. It decides which session receives which update; I read
   only the contract surface around it.
3. **`runtime/scene/clock.js` (941) plus `runtime/destinyStamps.js`.** Their
   comment at `runtime/scene/tick.js:428-431` says broadcasting
   `DoSimClockRebase` as a keepalive made Michelle's ball time run backwards.
   That is a trap we would otherwise walk straight into, and the fix they
   settled on is in these files.
4. **`destiny/index.js:359-950`, the slim-item dict, ship branch only.** We need
   it for `SetState` and I have only confirmed its shape, not its ship-specific
   key set.
