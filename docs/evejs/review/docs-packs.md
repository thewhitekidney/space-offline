# Lane: EvEJS docs + content packs (read-only RE, facts only)

Target repo: `$EVEJS` (AGPL-3.0 — facts extracted, no code copied).
All paths below are relative to that repo unless absolute.

## 0. Build-number reality check (read this first)

Three different build numbers appear, and they are not interchangeable:

| Number | What it is | Where |
|---|---|---|
| **3396210** | the **client/server build EvEJS targets**; `CLIENT_COMPATIBILITY_PROFILE` | `README.md:3`, `AGENT.md:130-131` |
| **496** | `machoVersion` | `AGENT.md:131` |
| **`V24.01@ccp`** | `projectVersion` | `AGENT.md:131` |
| **170472** | `birthday` (handshake constant) | `AGENT.md:131` |
| **3475087** | the **client build the dungeon data was extracted from** (`clientDungeonBuild`) | every client-derived pack manifest |
| **3503375** | client build behind the homefront pack | `evejs-homefront-1.0.5.json` manifest |
| **3487903** | `donorGraphBuild` for the AIR tutorial node graphs | `evejs-air-tutorial-*.json` manifest |

We target **V24.01 / 3619879**. `projectVersion` matches (`V24.01@ccp`); the build number does not.
`AGENT.md:132` states explicitly that **changing one of clientBuild / machoVersion / projectVersion /
birthday alone breaks handshakes and cache CRCs** — so the four are a tuple we must get right for
3619879, and EvEJS's values are a template, not an answer.

---

# JOB 1 — The documentation

## 1.1 Inventory

`AGENT.md` (42,847 B) + `doc/` (31 files, ~1.0 MB). Full list with coverage:

| File | KB | What it covers |
|---|---|---|
| `AGENT.md` | 43 | **The single most valuable document.** 13 sections: repo layout, ports/processes, boot sequence, network/protocol, service framework, gameStore, space sim, edge processes, HTTP proxy, config, market, content packs, Linux client build, conventions, gotchas, index of planning docs, "where to look first" table |
| `doc/CommandGuide.txt` | 60 | ~200 GM slash commands with parameters/examples; lists the 23 source files that define them |
| `doc/LIVE_TEST_QUEUE.md` | 124 | Live-test release queue design + PR series Q0–Q8 (EvEJS process, not CCP) |
| `doc/LIVING_UNIVERSE_ROADMAP.md` | 60 | Milestones M0–M9 for bot personas/corps/threats; `brain.sqlite` data model; statistical encounter rule |
| `doc/UPDATING.md` | 56 | **Per-feature migration notes = de-facto game-rule specs** (see 1.2) + the Windows updater |
| `doc/FRESH_START.md` | 56 | "every pilot begins at a new character's zero": measurements, design, PR series |
| `doc/SETUP.md` | 52 | Docker + native Windows setup, ops, monitoring, content packs, troubleshooting |
| `doc/OPTIMIZATION_OPPORTUNITIES.md` | 48 | 18 evidence-backed perf items; **§1 visibility, §2 Destiny broadcast fan-out** are implementation-grade |
| `doc/PERSISTENT_UNIVERSE_ARCHITECTURE.md` | 44 | Measured capacity (~20 bots / 13 systems, tick p95 58.7 ms), options, agency gaps |
| `doc/BOT_ECONOMY.md` | 40 | Markets restocked only by player+bots: measurements, design, PR series |
| `doc/LIVE_TEST_FLOW_PLAN.md` | 40 | PRs A–E for faster live scenario runs |
| `doc/LIVE_INSTANCE.md` | 36 | Runbook for the second compose project (`evejs-live`, `127.0.0.2`) |
| `doc/OFFLINE.md` | 36 | What offline mode enforces, install of controls, firewall blocks, live proof, historical flag-serving record |
| `doc/BOT_TEST_PLATFORM_PLAN.md` | 32 | `tools/bots` → test suite; **723 role→subsystem pairs in `evejsWebGatewayRuntime.js:103–3988`** |
| `doc/CAPACITY_AND_AI_BRAIN.md` | 32 | Per-pilot cost; persistent "brain" as separate TS process over the gateway |
| `doc/TWO_INSTANCES.md` | 28 | Two worlds on one client install; `/server:` + second loopback |
| `doc/TYPESCRIPT_ADOPTION_PLAN.md` | 28 | tsconfig/eslint, `.d.ts` seams, migration + do-not-touch lists |
| `doc/LINT_BASELINE_ANALYSIS.md` | 24 | 3,963 legacy lint warnings characterised (67% dead `require` bindings) |
| `doc/LIVE_TEST_FLOW_RESEARCH.md` | 24 | Evidence behind the flow plan |
| `doc/ROADMAP_ASSESSMENT.md` | 24 | Feature effort, bots on the gateway, no TS migration of old code, no rewrite |
| `doc/CONTENT_PACKS.md` | 20 | **Pack install/removal + per-pack content and spawn rules** (see Job 2) |
| `doc/MARKET_SEEDER.md` | 20 | v1 synthetic / v2 TQ snapshot / v3 Jita index seeders; reseed runbook |
| `doc/OFFLINE_CLIENT_FLAGS.md` | 20 | **67 CCP client feature-flag keys + code defaults + `path:line` citations** |
| `doc/PAUSE_PROFILING.md` | 12 | Capturing long server pauses |
| `doc/TICK_STALL_SCENE_LOAD.md` | 12 | **World-thread stall on first session entry**, with a per-stage ms breakdown |
| `doc/UPSTREAM_MERGE_261018.1.md` | 12 | Vendor-branch procedure; what release 261018.1 changed |
| `doc/NON_DOCKER_SETUP_AUDIT.md` | 12 | Native Windows setup audit |
| `doc/MARKET_SETUP.md` | 8 | Which market to pick |
| `doc/TROUBLESHOOTING.md` | 8 | Player-facing troubleshooting |
| `doc/LAUNCHERS.md` | 8 | What each `.bat` launcher does |
| `doc/TOOLS.md` | 4 | Admin tool basics |
| `doc/FUTURE_FEATURES.md` | 8 | Owner wishlist: sov influence maps, killboard, doctrines, gate camps, leader-bot wars |

## 1.2 Documents that pin down **CCP's** protocol / data / behaviour (high value)

Ranked. "CCP facts" = things true of the real client or CCP's static data, as opposed to EvEJS design.

1. **`doc/OFFLINE_CLIENT_FLAGS.md`** — *Highest value, pure CCP data.*
   67 distinct `featuremanagement.client.flag` keys read out of the decompiled client, each with
   type, **code default**, feature, and a `tools/ClientCodeGrabber/Latest/<path>.py#L<n>` citation.
   68 call sites found; 2 dynamic callers. Pins down what the client does with **no flag server**:
   - `eve-local-chat-authority` = `false` → client uses the **old XMPP Local chat path**, not the public gateway (`chat/client/chat_service.py:42`). This decides which chat stack we must implement.
   - `enable_new_character_creation` = `false`, `achievements_feature_enabled` = `false`, `character_titles_feature_enabled` = `false`, `eve_ag_guidance_enabled` = `false`.
   - `market-transaction-tax` = `8.0` (float) — the client's own tax display math (`marketsvc.py:100`).
   - `client-secure-url-string-flag` = `https://secure.eveonline.com`, `client-store-url-string-flag` = `https://store.eveonline.com` (`eve/common/script/util/urlUtil.py:10-11`) — must be rewritten to loopback.
   - `client-notice-consumer-amount-of-notices-to-consume-at-once` = `100`; `eve-desktop-client-request-tracing-sample-rate` = `1000`; `campaigns_client_live_updates_polling_frequency` = `30`; `freelance_projects_client_live_updates_polling_frequency` = `30`; `heraldry-minimum-alliance-membership-duration-for-em-donations` = `30` days.
   - `machonet_transport_metrics_enabled` = `false` (`carbon/common/script/net/machoNet.py:493`).
   - 13 `job-board-*-enabled-feature-flag` keys, all `true` except `job-board-open-on-startup-feature-flag` = `false` — tells us which AIR Opportunities categories the client will ask about.
   - Default JSON for `eve_client_hostile_network_exception_reporting`: `{"exception_FQDNs":["requests.exceptions.ConnectionError","socket.error"],"ASNs":[5..13],"country_ISOs":["CN","RU"]}`.
   - "Verification boundary": the LaunchDarkly SDK patch rewrites **six** native endpoint URL strings to `https://localhost`, **padded with trailing slashes to preserve original length** — NUL padding is unsafe because native constructors use explicit original lengths.

2. **`AGENT.md` §6 "Network / protocol"** (`AGENT.md:126-151`) — *the protocol spec in 25 lines.*
   - Compatibility tuple: clientBuild 3396210, machoVersion 496, projectVersion `V24.01@ccp`, birthday 170472.
   - Framing: **4-byte little-endian length prefix, 10 MB max**; codec is EVE Python marshal; **zlib-inflate when the first byte is `0x78`**; crypto = `"Placebo"` only.
   - Handshake order (`tcp/handshake.js`, `EVEHandshake`): version exchange → queue check → **`VK`** → **`OK CC`** → auth → `CryptoServerHandshake` (carries a **signed Python func** + `config_vals`) → ack → **`SessionInitialStateNotification`**.
   - Auth is **inside the handshake**, not a service.
   - Dispatch path: `packetDispatcher` → `serviceManager.lookup(name)` → `service.callMethod(method, args, session, kwargs, {callID})`.
   - **A non-Macho exception in a handler only logs; the client receives `None`.** Unknown services also return `None`.
   - Marshal value tags used: `dict` (as `entries:[[k,v]]`), `list`, `tuple`, `object`, `objectex1`, `objectex2`, `substream`, `packedrow`.
   - Client-visible error path: `throwWrappedUserError("CustomNotify", {notify: "..."})` — **688 uses**.
   - Session API shape: `sendNotification(type, idType, tuple)`, `sendSessionChange({key:[old,new]})`, `sendServiceNotification`; outbound queue is serialized.
   - **32 services implement `Handle_MachoBindObject`** (bound objects).

3. **`AGENT.md` §7 "The Linux client setup"** (`AGENT.md:244-308`) — *client-side ground truth.*
   - Client argv the real client accepts: `/noCrashReportUpload /resfileserver=http://127.0.0.1:26002/resfiles/ /port:26000 /login:<user>:<pass>` (and `/server:<ip>`).
   - `start.ini` must say `server = 127.0.0.1`, `cryptoPack = Placebo`, `build = 3396210`.
   - `EO_REMOTEFILECACHEFOLDER` points at `…/EVE Online - 3396210/ResFiles`.
   - Binary patches (recipes in `tools/ClientSETUP/*_patch_recipe.json`): `blue.dll` one-byte **JE→JNE at `0x1F4DE9`** (purpose undocumented upstream); `launchdarkly_stackless_client_sdk.pyd` six URLs → `https://localhost///…`; `exefile.exe` minidump URL → loopback.
   - Client **settings folder is per server name** (`…_127.0.0.1`, `…_127.0.0.2`) at `drive_c/users/<u>/AppData/Local/CCP/EVE/<path-key>_<server>/settings/prefs.ini`.
   - **Wine/crypt32 gotcha**: the client CA's name constraints exclude an empty directoryName while leaves have empty subjects, so the CA must be re-signed with that one exclusion removed. LaunchDarkly SDK **rejects IP SANs**, hence a DNS SAN for `localhost`.
   - Decompile toolchain: uncompyle6/xdis; see also `tools/ClientCodeGrabber`.

4. **`AGENT.md` §13 last row — client-side Python push** (`AGENT.md:559`).
   The server appends per-feature `.py` sources to the handshake's signed func
   (`network/tcp/clientPython/*`, assembled by `buildTidiSignedFuncSource` in `handshake.js`), and
   the client logs what it printed as `signedFunc output`. This is the mechanism for pushing
   client-side UI/behaviour, and it pins the handshake's signed-func payload as *arbitrary Python*.

5. **`tools/DatabaseCreator/staticTables/dungeonAuthority/data.json`** (68 MB, not a doc but is the
   provenance record) — *names CCP's own static resources exactly.* See Job 2 §2.6. It lists the
   **17 client `res:/staticdata/*.fsdbinary|.static` resources**, each with CCP `resourceHash`,
   `sizeBytes`, and the **ResFiles hashed cache path layout `<2 hex>/<16 hex>_<md5>`**.

6. **`doc/UPDATING.md:77-357`** — *de-facto game-rule specs per feature*, written as migration notes.
   Each section states the rule, the persisted field names, and what resumes after restart:
   - **Planetary launch IDs**: the client is sent a launch ID as a **32-bit** number, so IDs counted from `910000000000` could not be delivered; they now start at `910000000` and keep `legacyLaunchID`.
   - Corp industry output now files to **Corp Deliveries**, not the corp office.
   - **AIR tutorial**: consumer implements **chapters 1.1–1.5 and 2.1–2.5**; `dungeonRuntimeState.spawnState.airMining` holds extraction intents; `miningRockRemainingByID` holds depletion; `birth_station_id` / `birth_system_id` aliases; a 350,000 ISK settlement in chapter 2.2; failure code `AIR_ENTRY_CAPSULE_REQUIRED`.
   - Engine-owned static tables added over time: `clientPvpFilaments`, `clientTowComponents`, `clientMicroJumpComponents`, `clientShipLinkComponents`.
   - **Breacher pods**: SQLite table `breacherPodDeaths`; `spaceState.breacherPodRuntime`; killmarks gain `awardIDs`.
   - **Warp bubble drag**: `spaceState.warpState.interdiction` on ship rows.
   - **Sov structure self-destruct**: `selfDestructAt` + `selfDestructGeneration`; receipts in `structures._meta.structureSelfDestructs`.
   - **Skyhook reagent raids**: raid containers keep a **two-hour decay deadline**.
   - **Customs inspection**: `customsInspectionCases` table; `customsInspectionScanDelayMs` default **3000**, `customsInspectionPromptTimeoutSeconds` default **60**; CPF gate groups at the **Maurasi and Muvolailen** gates; `npcDefaultConcordStartupEnabled`.
   - Tow components: "build 3396210's original resource and matching loader verify the **two unpublished types**".
   - Election replay: four SQLite tables `campaignTrailProgress`, `eventLifecycle`, `fwOpenReplay`, `homefrontReplay`; config `events.campaignTrailEnabled`, `events.eventSchedules`.

7. **`AGENT.md` §6 "Space simulation"** (`AGENT.md:181-196`) — *Destiny facts.*
   - **Destiny = EVE's ballpark physics/replication**, delivered as **`DoDestinyUpdate`** notifications through a transactional delivery layer (`space/destiny/delivery/`).
   - EvEJS's simulation is "adapted from CCP's `carbonengine/destiny` **v3.1.1 (MIT)**" — i.e. there is a permissively-licensed CCP reference for the physics; worth locating independently.
   - Tick: **`setInterval` at 100 ms (10 Hz)**. Scenes with no sessions and no deadline sleep; empty scenes GC after **60 s**.
   - **No ship collision/bumping except missile impact.**
   - TiDi: per-scene factor **0.1–1.0**.
   - Preloaded systems here: Jita **30000142**, New Caldari **30000145**, **30100032**.

8. **`doc/OPTIMIZATION_OPPORTUNITIES.md` §2** (`:93-135`) — *the Destiny fan-out call graph*, with
   function names and line numbers: `presentation.js:4344 broadcastMovementUpdates` (called from
   `tick.js:3092` shared and `:3143` per-watcher), `filterMovementUpdatesForSession (:4745)`,
   `canSessionSeeDynamicEntity (:4832)`, `clampQueuedSubwarpUpdates`, `sendDestinyUpdates`
   (`destinyDispatch.js:2955`), `flushCollectedDestinyGroups (:1383-1620)` which deep-clones
   **20 authority keys per transaction** (`delivery/deliveryTransaction.js:29-66,132`), plus
   `broadcastSpecialFx (:916)`, `broadcastAddBalls (:3645)`, `broadcastRemoveBall (:3815)`,
   `shipHealthState.js:480 broadcastDamageStateChange`. Also: **`clientSession.js:829-863` encodes
   the inner payload, then `sendPacket` encodes the outer packet again — two marshals per
   notification** (`michelleContract.js:40`). Directly relevant to our Destiny stamp allocator.

9. **`doc/TICK_STALL_SCENE_LOAD.md`** — *measured client-facing latency budget.*
   First-session attach into an unloaded system costs ~200 ms on the world thread, broken down:
   `attachSession` 201 ms → `prepareUniverseSitesForPlayerEntry` 119 ms (of which
   `reconcileUniverseSeededInstances` 58 ms, `listDesiredUniverseDungeonSiteDefinitions` 42 ms,
   `cleanupInvalidGeneratedIceAuthority` 17 ms), `wakeSceneForImmediateUse` 37 ms,
   `ensureScene` 36 ms, attach proper 10 ms. Jita grid = **107 entities**. Also: `Profiler.start`
   itself blocks a 2–4 GB Node process ~900 ms (so don't mistake that for a server stall).

10. **`AGENT.md` §6 "HTTP proxy / gateway"** (`:204-215`) — *what the real client phones home to.*
    Intercepted hosts: **`sentry.io`, `public-gateway.evetech.net`, `dev-public-gateway.evetech.net`**
    (CONNECT tunnelled to local 26003; everything else 403/204). **LaunchDarkly paths must return
    404** so the client falls back to built-in defaults. **`/resfiles` always 404** (the client's
    ResFiles cache must be complete). **24 gRPC "gateway services"** under
    `express/gatewayServices/` (skill plans, corp goals, evermarks, New Eden Store, sov, …).
    Client env for the proxy: `HTTP(S)_PROXY=http://127.0.0.1:26002/`,
    `EVE_CLIENT_SENTRY_DSN=http://evejs@127.0.0.1:26002/1`.

11. **`doc/OFFLINE.md`** — companion to #1/#3: what offline mode enforces, that the native endpoint
    patch is **mandatory** while the two firewall blocks are only recommended, and a historical
    record of the earlier flag-serving implementation (replaced 2026-09-20 by client defaults only).

12. **`doc/CommandGuide.txt`** — mostly EvEJS's own GM surface, but useful as a **client-call
    inventory**: it lists ~200 commands and the 23 server files behind them, exposing which
    client-visible effects exist (`/effect <name>`, `/celestial <typeID>`, `/lco <typeID>`,
    `/jetcan`, `/spawncontainer`, `/tidi`, `/tr … pos=x,y,z|offset=x,y,z`).

13. **`doc/CONTENT_PACKS.md`** — contains **CCP game rules** stated plainly (security bands per site
    tier, Odyssey-2013 removal of pirate-exploration spawns, wormhole class gating, mining-mission
    rocks holding exactly the mission quantity). See Job 2 §2.7.

### Documents that are EvEJS architecture only (low CCP value)
`SETUP.md`, `UPDATING.md`'s updater half, `LAUNCHERS.md`, `TOOLS.md`, `TROUBLESHOOTING.md`,
`MARKET_*.md`, `NON_DOCKER_SETUP_AUDIT.md`, `LINT_BASELINE_ANALYSIS.md`,
`TYPESCRIPT_ADOPTION_PLAN.md`, `UPSTREAM_MERGE_261018.1.md`, `PAUSE_PROFILING.md`,
`LIVE_*`, `BOT_*`, `FRESH_START.md`, `CAPACITY_AND_AI_BRAIN.md`,
`PERSISTENT_UNIVERSE_ARCHITECTURE.md`, `LIVING_UNIVERSE_ROADMAP.md`, `FUTURE_FEATURES.md`,
`TWO_INSTANCES.md`, `ROADMAP_ASSESSMENT.md`. Useful for *our* design (capacity numbers, bot
architecture), not for client compatibility.

## 1.3 Confirmed absences (findings)

**21 documents are referenced by `doc/*.md`, `README.md` or `AGENT.md` but do not exist in this
checkout.** Almost all of them are the per-feature rule specs we would want most:

```
AIR_COMBAT.md                            NODEGRAPH_EXECUTION.md
AIR_ECONOMY.md                           pvp-filament-events-and-queue.md
breacher-pods.md                         ship-self-destruct.md
customs-inspection-cases.md              SKINR.md
customs-operator-authority.md            skyhook-raiding.md
election/OPERATIONS_AND_ACCEPTANCE.md    sovereignty-structure-self-destruct.md
generic-ship-links.md                    TEST_BASELINE.md
HOMEFRONT_OPERATIONS_IMPLEMENTATION_STATUS.md  tow-objective-component.md
home-station-clone-self-destruct.md      warp-bubble-drag.md
JITA_INDEX_SEED_PLAN.md (x2 paths)       micro-jump-components.md
```

`AGENT.md:515` acknowledges one of them ("`README.md` links `doc/JITA_INDEX_SEED_PLAN.md`, which is
missing from this checkout"). The pattern suggests the release ships only a subset of upstream's
`doc/`. **`doc/UPDATING.md:77-357` is therefore the only surviving summary of those feature rules**
— and it is a summary, so the field names it gives are reliable while the behavioural detail is thin.

Also absent: `tools/ClientCodeGrabber/Latest/**` (the decompiled client that
`OFFLINE_CLIENT_FLAGS.md` cites `path:line` into) — only the `tools/ClientCodeGrabber` directory
shell is present. And `_local/**` is gitignored, so the Linux client, decompile output and certs
described in `AGENT.md` §7 are not here.

---

# JOB 2 — The content packs

`content-packs/` = 18 files, **331 MB** (`du`) / 347 MB apparent. Sizes:

| File | Size | templates | spawn profiles |
|---|---|---|---|
| `evejs-ded-rated-sites-1.0.2.json` | 86 M | 50 | 1 |
| `evejs-combat-anomalies-1.4.4.json` | 68 M | 176 | 1 |
| `evejs-pirate-exploration-1.0.0.json` | 64 M | 204 | 2 |
| `evejs-ore-anomalies-1.1.0.json` | 30 M | 199 | 1 |
| `evejs-wormhole-sites-1.2.0.json` | 30 M | 48 | 4 |
| `evejs-booster-production-1.1.0.json` | 21 M | 32 | 2 |
| `evejs-gas-sites-1.0.0.json` | 15 M | 74 | 2 |
| `evejs-homefront-1.0.5.json` | 13 M | 24 | 24 |
| `evejs-drone-anomalies-1.4.0.json` | 7.8 M | 22 | 1 |
| `evejs-mining-missions-1.0.0.json` | 3.9 M | 40 | **0** |
| `evejs-ghost-sites-1.1.0.json` | 3.7 M | 20 | 1 |
| `evejs-air-tutorial-1.0.{0,1,2,3}.json` | 108–128 K | 4 each | 0 |
| `evejs-gallente-depots-1.0.0.json` | 12 K | 2 | 1 |
| `evejs-fw-open-1.0.0.json` | 8 K | 2 | 2 |
| `evejs-gallente-fabricators-1.0.1.json` | 8 K | 1 | 1 |

(Counts are the manifests' own `counts.dungeonTemplates` / `counts.spawnProfiles`, and they match
`grep -c '"templateID"'` exactly in every pack. Four versions of `air-tutorial` ship side by side;
`doc/CONTENT_PACKS.md` says the newest wins and the others are ignored with a log line.)

Cheap room-count proxy (`grep -c '"roomKey"'`, counts every reference not every room):
ded-rated 40,050 · pirate-exploration 39,237 · combat-anomalies 25,794 · wormhole 12,838 ·
booster 10,248 · ore 7,039 · gas 3,382 · drone 3,162 · homefront 2,874 · ghost 1,076 ·
mining-missions 855 · air-tutorial 156 · the three authored election packs ≤ 2.

## 2.1 Top-level schema

```
{ "schema": "evejs.content-pack.v1",
  "manifest": { … },
  "payload":  { "dungeon": { "templatesByID": {…}, "spawnProfilesByFamily": {…} } } }
```

Exactly three top-level keys. Every one of the 18 packs has `payload.dungeon` as its only payload
section — `dungeon` is the only payload namespace in use.

## 2.2 Manifest schema

```
manifest.id                     "evejs.combat-anomalies"      (reverse-dotted pack id)
manifest.version                "1.4.4"                       (semver, numeric compare)
manifest.title / .description   human text
manifest.contentApi             1
manifest.engine.minVersion      "0.12.7"
manifest.engine.maxVersionExclusive "0.13.0"
manifest.capabilities           ["dungeon.templates.combat-anomaly", "dungeon.spawn-profile.<family>", …]
manifest.builtAgainst.serverSdeBuild      3396210
manifest.builtAgainst.clientDungeonBuild  3475087   (or .clientBuild / .donorGraphBuild)
manifest.counts.dungeonTemplates / .spawnProfiles
manifest.provenance { … }        (see below)
manifest.payloadSha256          canonical-JSON hash of payload
```

`engine.minVersion` observed values: 0.12.6 (ore), 0.12.7 (most), 0.12.8 (ded-rated), 0.12.9
(homefront) — all `maxVersionExclusive: "0.13.0"`.

**Capability strings observed** (the engine refuses a pack declaring an unimplemented one — the
version-negotiation mechanism):
`dungeon.templates.<family>`, `dungeon.spawn-profile.<family>`,
`dungeon.content-pack-template-selection`, `dungeon.template-security-bands`,
`dungeon.spawn-profile.system-spawn-chance`, `dungeon.spawn-profile.site-tier-bands`,
`dungeon.spawn-profile.faction-scope-anywhere`, `dungeon.spawn-profile.wormhole-classes`,
`dungeon.spawn-profile.wormhole-faction-scope`, `dungeon.spawn-profile.region-site-count-multipliers`,
`dungeon.spawn-profile.cross-pack-template-ids`, `dungeon.exact-client-encounters`,
`dungeon.exact-client-environment-props`, `dungeon.exact-client-containers`,
`dungeon.exact-client-mining-rocks`, `dungeon.exact-client-gates`,
`dungeon.gate-type-list-restrictions`, `dungeon.homefront.v1`.

`manifest.provenance` carries the audit trail — this is where the extraction is documented:
```
provenance.builder               "scripts/ContentPacks/build-combat-anomalies-pack.js"
provenance.family / .families    "combat_anomaly" | [{family, templateCount, clientOverlaySha256, clientOverlayCounts}]
provenance.clientOverlaySha256   sha256 of the client extract overlay
provenance.clientOverlayCounts   { templateCount, sourceObjectCount, roomCount, connectionCount,
                                   entityAnchorCount, initialEntityAnchorCount, deferredEntityAnchorCount,
                                   guardCommandCount, triggerCount, environmentPropCount, miningRockCount,
                                   controllerCount, weatherObjectCount, sentryGunCount, containerCount,
                                   spawnAnchorObjectCount, gateCount, encounterPlanCount, npcSpawnEntryCount,
                                   chanceWaveCount, promotedInitialWaveCount,
                                   researchedPopulationTemplateCount, fallbackReconstructedTemplateCount,
                                   unsupportedSemanticCount, authoredQuantity }
provenance.targetManifest        { schema:"evejs.client-dungeon-target-manifest.v1", family, packID,
                                   sourcePath:"ClientDungeonCompiler/manifests/<family>-3475087.json",
                                   targetCount, clientBuild, sha256 }
provenance.reconstruction        { compositionSpecVersion, sources[{title,url}], assumptions[],
                                   authority, chanceGroupMinNpcGroupingID, chanceGroupSpawnChance,
                                   gateDestinationAuthority, triggerAuthority,
                                   npcCompositionAuthority, completionAuthority }
provenance.populationCatalog     { schema:"evejs.combat-anomaly-populations.v1", catalogID,
                                   catalogVersion, clientBuild, sha256, bindingCount, appliedTemplateCount }
provenance.unsupportedSemanticsByKind  { <kind>: count }
provenance.spawnProfileVersion   1 | 2
provenance.previousPack / .previousPackSha256 / .templateSetChange{addedTemplateIDs,removedTemplateIDs}
provenance.sourceRefs            [urls]  (authored packs)
provenance.sourceFiles           { path: sha256 }  (air-tutorial)
provenance.omitted               ["captured player and ball IDs", "captured collision tails",
                                  "warp and effect presentation overrides", "runtime and player state"]
provenance.nativeClientVerified  false (air-tutorial)
provenance.targetGraphAuthority  "dungeonAuthority.clientData.tables.nodeGraphsByID" (air-tutorial)
```

**`unsupportedSemanticsByKind` is the honest gap list** — i.e. what CCP's data says that EvEJS does
*not* implement. Across packs the kinds are:
`guard_command_timing_and_ranges`, `npc_grouping_composition`, `room_entry_message`,
`object_trigger_spawn_without_anchors`, `massive_environment_effect`,
`asteroid_without_authored_quantity`, `gate_destination_unavailable`, `client_objective_execution`.
Combat anomalies: 2,993 unsupported semantics over 176 templates; DED-rated: 4,857 over 50.

## 2.3 One site/dungeon entry (`payload.dungeon.templatesByID[<templateID>]`)

Client-derived template, 38 keys (sample: combat anomalies `client-dungeon:10760`, "Angel Forsaken
Sanctum"):

```
templateID                      "client-dungeon:10760"      (authored packs: "evejs-fw:open:gallente")
source                          "client" | "authored"
sourcePriority                  100 (client) | 200 (authored)
sourceConfidence                { label, score }            e.g. "Exact Client Layout + Reviewed Population", 100
mergeExistingRegardlessOfPriority  true
siteFamily                      "combat" | "data" | "relic" | "ice" | "ghost" | "combat_hacking" | "unknown"
siteKind                        "anomaly" | "signature"
siteOrigin                      "client_dungeon" | "universe_dungeon" | "eve_university_site_family"
spawnFamilyKey                  "combat_anomaly"
sourceDungeonID                 10760                       ← CCP dungeonID
resolvedName                    "Angel Forsaken Sanctum"
description                     null
archetypeID                     24
archetype                       { _key, titleID, descriptionID, contentTags[], location }
dungeonNameID                   710419                      ← CCP localization message id
descriptionID                   null
factionID                       500011                      ← CCP factionID (Angel Cartel)
factionKey                      "angels"
siteTier                        "sanctum"
siteVariant                     "forsaken"
difficulty                      5                           ← CCP 1..5 field
entryObjectTypeID               28356                       ← "Cosmic Anomaly" typeID
entryRoomKey                    "room:29797"
rooms                           [ {…} ]
connections                     [ {…} ]
objectivesID                    20
clientObjectives                { blackboardParameters[], objectiveChainID }
objectiveMetadata               { objectivesID, objectiveRecord, objectiveChainID, objectiveChain,
                                  objectiveTypeIDs, objectiveTypesByID, objectiveTaskTypeIDs,
                                  objectiveTaskTypesByID, nodeGraphID, nodeGraph, nodeTypeIDs, nodeTypesByID }
environmentTemplates            null
useDefaultSpaceMusic            false
resourceComposition             { oreTypeIDs[], gasTypeIDs[], iceTypeIDs[], hasAnyResources }
populationHints                 { …30 keys, see 2.4 }
siteSceneProfile                { source, exactClientContent, roomProfiles[], gateProfiles[],
                                  structureProfiles[], objectiveVisualProfiles[] }
clientDungeonContent            { …24 keys, see 2.6 }
provenance                      { kind, clientBuild, geometry, environmentProps, miningRocks,
                                  entityAnchors, sentryGuns, npcComposition, waveCount, hullCounts,
                                  anchorAssignment, waveOrder, waveTrigger, spawnBands, objectiveMarkers,
                                  lootProfiles, sourceFields, compositionSpecVersion, populationAuthority,
                                  populationResolutionID, populationResolutionStatus, clientLayoutFingerprint }
securityBands                   ["nullsec"]
securityBandSourceRefs          [3 refs]
anomalyLadderClass              10
```

Family-specific extras: mining-mission templates add `missionPresentation {bookmarkDeadspace,
bookmarkHint:"UI/Agents/Bookmarks/Mining/Mining"}`, `missionBriefingID`, `resourceHints`, and
`missionResource { resourceFamily, yieldKind, objectiveTypeID, objectiveQuantity, missionLevel, … }`.
AIR-tutorial templates add `airEncounter` and have **no** `clientDungeonContent`.

### Room entry
```
roomKey "room:29797" · roomID 29797 · label "R1" · shortName "Room1"
grid [0,0,0] · position [0,0,0] · stage "entry" · initialState "active"
source "client_static_3475087" · message null
sourceFields { roomID, roomName "﻿R1", shortName, messageIcon "", x, y, z }
```
Note the client's `roomName` carries a **leading U+FEFF BOM** — preserved verbatim. `stage` values
seen: `entry`; `initialState`: `active`.

### Connection / gate entry
Template-level `connections[]` elements are the raw client pairs, tiny:
```
{ "allowedShipsList": 482, "fromObjectID": 96198 }
```
The resolved form lives in `siteSceneProfile.gateProfiles[]`:
```
gateKey "client-gate:96198" · source "client_static_3475087" · exact true
objectID / dunObjectID 96198 · roomID 4434 · roomKey "room:4434"
destinationRoomID 4435 · destinationRoomKey "room:4435"
destinationInference "entry_first_then_next_room_in_id_order"   ← NOT from the client
initialState "locked" · typeID 12273 · groupTag "default"
allowedShipsList 482
allowedShipsTypeList { typeListID 482, resolved true,
    name "Dungeon Ship Restrictions [111, 116, 149, 178, 179, 2250]",
    description "Generated from dungeon type and group restrictions for dungeons [...]",
    includedTypeIDs [], includedGroupIDs [25,31,237,324,830,831,834,893,1283,1527],
    includedCategoryIDs [], excludedTypeIDs [], excludedGroupIDs [], excludedCategoryIDs [],
    sourceFields { … same, with _key } }
disallowedFittedTypesList / disallowedFittedTypeList / allowedShipRaces  (null here)
positionOffset { x: 31457280, y: 0, z: -20000 }   ← note the 31457280 = 30 * 2^20 room-grid stride
sourcePosition [0, 0, -20000] · dunRotation [360, 0, 0] · sourceConnectionIndex 0
sourceFields { allowedShipsList, fromObjectID }
```
**Important CCP fact:** gate *destinations* are **not present in the client extraction**. DED-rated
records `gateDestinationAuthority: "unavailable_in_client_reconstructed_room_order"` and
255 × `gate_destination_unavailable` unsupported semantics; the destination is inferred by
"entry first, then next room in ID order". We must solve room topology ourselves.

## 2.4 Spawn / room / object entries — `populationHints` (30 keys)

```
source                 "client_static_3475087"
exactClientContent     true
geometryAuthority      "exact_client_extract"
populationAuthority    "researched" | "reconstructed_faction_and_difficulty"
siteFamily / siteKind / spawnFamilyKey / factionKey
safeFromNpc            false
dangerousWarpIn        true
environmentProps[]  miningRocks[]  controllers[]  weatherObjects[]
entityAnchors[]  guardCommands[]  triggers[]  sentryGuns[]
encounter{}  encounters[]  waveModel{}  populationResearch{}
roomMessages[]  containers[]  hazards[]  lootProfiles[]  objectiveMarkers[]
resources { oreTypeIDs[], gasTypeIDs[], iceTypeIDs[] }
exactContentCaps { environmentProps, miningRocks, encounterNpcs }
completion { objectiveMode: "destroy_all_encounters", completeObjectiveOnEncounterClear: true }
```

### `entityAnchors[]` — NPC spawn points (exact client data)
```
key "client-entity:1202560:149163" · source "client_static_3475087" · exactAnchor true
objectID 1202560 · roomID 29797 · roomKey "room:29797"
objectRole "spawn_anchor" · objectOrder 0 · objectLocalName null · objectTriggerSpawn false
sourcePosition [-6898.17236, -1766.95703, 19698.63477]     ← CCP dunX/Y/Z
positionOffset { x, y, z }                                  ← engine-space, entry re-centred to origin
entityKey 149163 · entityID 149163
npcGroupingID 31856                                         ← CCP key; composition table NOT in client
entityNameID null · triggerSpawn true
sourceFields { npcGroupingID, entityID, triggerSpawn: 1, objectID }
```

### `triggers[]`
```
{ kind: "entity_trigger_spawn", objectID, entityID, sourceFields{npcGroupingID, entityID, triggerSpawn, objectID} }
```

### `guardCommands[]` — respawn/aggro, preserved but **not applied**
```
objectID · roomID · objectRole
sourceFields {
  entityMinRespawnDelay 3600, entityMaxRespawnDelay 3600,
  objectMinRespawnDelay 3600, objectMaxRespawnDelay 3600,
  entitySpawnOnce 0, entityDelaySpawn 0, objectDelaySpawn 0, objectTriggerSpawn 0,
  aggressionRange 250000, orbitRange 1500 }
```
Those are **CCP's own numbers**: 3600 s respawn delays, 250,000 m aggression range, 1,500 m orbit
range (5,000 m for the weather object). Flagged `guard_command_timing_and_ranges` as unimplemented.

### `environmentProps[]` — scenery
```
key "client-object:1202556" · source · exact true · role "environment"
objectID / dunObjectID · roomID · roomKey
typeID 61925 · typeName "Non Interactable Small Rock (Do not translate)"
groupID 1975 · categoryID 2 · groupTag "Belt" · order 19
objectNameID / objectName / localName (null)
objectTriggerSpawn false
sourceRadius 22388.02734 · sourcePosition[3] · positionOffset{x,y,z} · dunRotation [yaw,pitch,roll]
suppressSlimName true · suppressSlimGraphicID true     ← client presentation hints
```

### `miningRocks[]` (mining-missions pack — exact ore)
```
key "client-object:867587" · exact true · objectID/dunObjectID 867587
roomID 19516 · roomKey · typeID 28617 · groupTag "Mission Ore" · order 0
count 1 · quantity 20000 · yieldKind "ore"
sourcePosition[3] · positionOffset{x,y,z} · dunRotation null
```
Pack-level: `miningRockCount: 106`, `environmentPropCount: 481`,
`objectiveQuantityTotal: 321555`, `quantities: "exact_dunRadius"` — **the client stores asteroid
quantity in the dungeon object's `dunRadius` field**. That is a concrete reusable fact.

### `containers[]` (ghost-sites)
```
key "client-container:1032344" · exact true · role "research_cache" · count 1
analyzer "data" · typeID 33530 · typeName "Secure Depot" · typeNameCandidates[]
label "Lesser Research Cache" · bonus false
persistsAfterResponse false · failureExplodes true · trigger null
lootProfile "ghost_research_loot" · lootTags ["datacore","decryptor","research_component"]
hackingDifficulty null  (authored packs use e.g. "7")
objectID/dunObjectID · roomID · roomKey · order 6
objectNameID 292435 · objectName "Secure Depot"
sourcePosition[3] · positionOffset{x,y,z} · dunRotation null
provenance { type "exact", position "exact", role "reconstructed_from_position", loot "engine_contract" }
```

### `weatherObjects[]`
```
objectID · roomID/roomKey · typeID 19746 · typeName "Dust Storm Environment"
groupID 1882 · groupName "MassiveEnvironments" · categoryID 2 · categoryName "Celestial"
groupTag "Weather" · role "weather" · position[3] · rotation null
guardCommand{…} · objectTriggerSpawn true · entities null · entityCount 0 · sourceFields{…}
```
Flagged `massive_environment_effect` as unimplemented.

### `encounters[]` — the reconstructed wave plan
```
key "initial" · label "Initial defenders" · supported true · exact false
spawnQuery "angels_standard" · amount 19 · maxSpawnEntries 19
spawnEntries[] { key "client-entity:1202560:149172#initial:1",
                 entityKind "npc", role "pirate_ship", factionKey "angels",
                 hullClass "battlecruiser", commander false, label,
                 positionOffset{x,y,z}, spreadMeters 1500,
                 exactAnchor { objectID, entityID, npcGroupingID, sourcePosition, positionOffset,
                               triggerSpawn, objectTriggerSpawn },
                 provenance { anchor "exact", composition "researched_single_source",
                              faction "exact_client_factionID",
                              anchorAssignment "deterministic_client_anchor",
                              hullClass "researched" } }
trigger · prerequisiteKey · waveIndex · roomKey · deadspace
chanceGroup · spawnChance · wavePosition
sourceAnchorKeys[] · sourceObjectIDs[] · npcGroupingIDs[]
researchedHullRanges · researchedAmountRange
populationResolutionID · researchSourceRefs · materializationPolicy · provenance · notes
```

### `waveModel{}`
```
rule (prose) · waveCount 19 · initialAnchorCount 0 · deferredUnitCount 6
promotedInitialWave true · chanceWaveCount 12 · singleSpawnerSite false · spawnerObjectCount 7
populationAuthority "researched" · layoutFingerprint "sha256:…"
populationResolutionID "angel-forsaken-sanctum-cross-faction"
populationResolutionSiteName · populationResolutionLayoutVariant · populationResolutionStatus "single_source"
guaranteedWaveCount 7
usedAnchorKeys[] unusedAnchorKeys[] reusedAnchorKeys[] assignedAnchorKeys[]
disabledOptionalAnchorKeys[] recordedOptionalGroupCount disabledOptionalGroupCount
```

## 2.5 `spawnProfilesByFamily` schema

```
<familyKey>: {
  version                     1 | 2
  enabled                     true
  selectionAuthority          "content_pack_templates"
  requireConcreteLocalPirateFaction  true          (combat_anomaly)
  templateIDs                 ["client-dungeon:10760", …]   (176 entries)
  bands                       ["highsec","lowsec","nullsec"] (+ "wormhole")
  siteLifetimeMinutes         1440
  siteOrigin                  "universe_dungeon"
  systemSpawnChance           0.1                   (where present)
  bandTuning                  { highsec:{difficultyRange:[1,5]}, lowsec:{…}, nullsec:{…} }
  templateFilters             { siteFamilies[], siteKinds[], nameIncludesAny[], nameExcludesAny[] }
  provenance                  { kind "authored", owner "<packID>", sourceRefs[{key,title,url}], assumptions[] }
}
```

The combat_anomaly profile's `provenance.assumptions` state CCP-side rules explicitly:
- The client's `difficulty` field is a **1–5 scale over a ten-rung site ladder** and therefore
  cannot express the real band placement. Hence the separate `securityBands` and `anomalyLadderClass`
  (observed 10) on each template, and hence profile version 2.
- Band assignment: **high-sec 1.0–0.5** hosts Hideaways + all variants, Burrows, Refuges, base Den;
  **low-sec 0.4–0.1** hosts base Den, Yards, Ports, base/Hidden/Forlorn Rally Points, base Hub,
  Den variants; **null-sec** hosts everything from Yard up, incl. all Hub variants, Havens, Sanctums,
  Forsaken Sanctum. **Wormhole, Pochven, Zarzakh and Jove never host pirate anomalies.**
- `sourceRefs` cite EVE University `Template:CMBSiteBox`, EVE University `Combat_sites`, and CCP's
  own support article on Sovereignty Hub Combat Anomalies, plus
  "Client build 3475087 `dungeons.jsonl` difficulty field (1 Hideaway/Burrow .. 5 Haven/Sanctum)".

Chance-group constants (combat + drone anomalies): `chanceGroupMinNpcGroupingID: 20000`,
`chanceGroupSpawnChance: 0.35` — i.e. **an anchor whose `npcGroupingID ≥ 20000` is an optional
("chance") wave, modelled at 35%**. Ghost sites set both to `null` and instead assume a hidden timer
of **30–180 s from site activation followed by a 30 s visible countdown**.

`sourcePriorities` from the base authority: `client 100, client_plus_repo 90, eve_survival 80,
repo 60, fallback 20`. Templates carry `sourcePriority` 100 (client) or 200 (authored, i.e. authored
content deliberately outranks client).

## 2.6 CCP datamined, or EvEJS-authored? — **Both, and the packs label which.**

**Evidence, strongest first:**

1. **`tools/DatabaseCreator/staticTables/dungeonAuthority/data.json` names CCP's own client
   resources.** `source.client.resourceEntries` lists **17 client static resources**, each with
   CCP `resourceHash`, `sizeBytes` and the ResFiles hashed cache path (`<2 hex>/<16 hex>_<md5>`):
   ```
   res:/staticdata/dungeons.fsdbinary                              714,440
   res:/staticdata/nodegraphs.fsdbinary                          7,271,384
   res:/staticdata/objectivechains.fsdbinary                       183,152
   res:/staticdata/environmenttemplates.fsdbinary                  117,112
   res:/staticdata/objectivetypes.fsdbinary                         40,480
   res:/staticdata/oretypesbydungeonid.static                       25,230
   res:/staticdata/nodetypes.fsdbinary                              22,784
   res:/staticdata/environmenttemplateanchortypeids.fsdbinary       13,320
   res:/staticdata/objectivetasktypes.fsdbinary                      8,478
   res:/staticdata/dungeonenvironmenttemplates.fsdbinary              6,912
   res:/staticdata/dungeonobjectives.fsdbinary                        6,616
   res:/staticdata/archetypes.fsdbinary                               5,563
   res:/staticdata/gastypesbydungeonid.static                         1,958
   res:/staticdata/objectives.fsdbinary                               1,187
   res:/staticdata/icetypesbydungeonid.static                         1,114
   res:/staticdata/environmenttemplateanchorgroupids.fsdbinary          168
   res:/staticdata/environmenttemplateanchorcategoryids.fsdbinary       112
   ```
   with `source.exportScript: "scripts/DataSync/Export-ClientDungeonData.ps1"` and
   `source.staticFsdDecoder: "scripts/DataSync/decode-static-fsd.py"`
   (`source.staticTables.provider: "Python 3 standalone static FSD decoder"`).
2. **`tools/DatabaseCreator/staticTables/dungeonClientContent/data.json`** records a second-stage
   JSONL export (`provider: "ElysianSDE"`, `clientBuild: 3475087`) with per-file bytes+sha256:
   `dungeons.jsonl` **55,196,379 B**, `localization_fsd_en-us.jsonl` **58,582,364 B**,
   `types.jsonl` 12,077,256 B, `groups.jsonl` 247,308, `asteroidcountbytypeidroomiddungeonid.jsonl`
   70,055, `oretypesbydungeonid.jsonl` 45,621, `dungeonobjectives.jsonl` 8,797, `categories.jsonl`
   3,316, `gastypesbydungeonid.jsonl` 2,802, `icetypesbydungeonid.jsonl` 1,016, `archetypes.jsonl`
   6,967.
3. **Every template says so.** `source: "client"`, `siteOrigin: "client_dungeon"`,
   `populationHints.source: "client_static_3475087"`, `geometryAuthority: "exact_client_extract"`,
   and a real CCP `sourceDungeonID`. `clientDungeonContent.rawDungeonRecord` is the **verbatim CCP
   dungeon record**, keyed `_key: 10760`, with `rooms.<roomID>.objects.<objectID>` carrying
   `typeID, groupTag, objectID, yaw, pitch, roll, radius, position[3], roomID, order`.
   `sourceDungeonFields` is the flat CCP row:
   `{entryObjectID, _key, archetypeID, entryTypeID, difficulty, objectivesID, factionID,
   useDefaultSpaceMusic, dungeonNameID, dungeonID}`.
4. **Template `source` by pack** (grep of `"source": "client"` / `"authored"`):
   | pack | client | authored |
   |---|---|---|
   | combat-anomalies | 176 | 0 |
   | pirate-exploration | 204 | 0 |
   | ore-anomalies | **122** | 0 (remaining 77 have `siteOrigin: "eve_university_site_family"`) |
   | gas-sites | 74 | 0 |
   | ded-rated-sites | 50 | 0 |
   | wormhole-sites | 48 | 0 |
   | mining-missions | 40 | 0 |
   | booster-production | 32 | 0 |
   | homefront | 24 (`client_static_3503375`) | 0 |
   | drone-anomalies | 22 | 0 |
   | ghost-sites | 20 | 0 |
   | air-tutorial | 0 | 4 |
   | gallente-depots | 0 | 2 |
   | fw-open | 0 | 2 |
   | gallente-fabricators | 0 | 1 |
   Across all packs: `siteOrigin` = `client_dungeon` 804, `eve_university_site_family` 77,
   `universe_dungeon` 72. Only two `client_static_*` tags exist: `3475087` and `3503375`.

**What is NOT CCP's:** the packs are explicit that **geometry is exact, population is not**.
- `npcGroupingID` is present and exact, but **the table that maps it to NPC types/counts is not in
  the client**. DED-rated: `npcCompositionAuthority: "unavailable_reconstructed_faction_and_difficulty"`,
  2,512 × `npc_grouping_composition` unsupported. Combat anomalies: 1,717 ×.
- Wave composition is reconstructed from EVE University pages (the `reconstruction.sources` lists
  per-site wiki URLs) with stated assumptions: "a wave's ship total comes from the tier table,
  **scaled by the square root of the number of anchors spawning together**, then split evenly
  across them (single-anchor waves get the whole total; six-anchor initial waves get roughly
  **2.5×** it)".
- Gate destinations: unavailable (see 2.3).
- Loot: `lootProfile`/`lootTags` are an **engine contract**, not client data
  (`provenance.loot: "engine_contract"`).
- The three election packs + air-tutorial are wholly authored, cite
  `wiki.eveuniversity.org/Gallente_Election_YC128` / `Faction_warfare` / eveonline patch notes, and
  carry `assumptions` such as "Three containers and their positions are authored, not recovered
  retail geometry" and "Existing simplified hacking board, with documented **70/90 core coherence**".
- Homefront: `assumptions: ["Server spawn tables unavailable; encounter estimates are explicitly
  authored."]`.

**Verdict:** the big packs are **CCP's datamined dungeon layouts** (rooms, object IDs, typeIDs,
positions, rotations, radii, gate type-lists, asteroid quantities, faction IDs, name IDs, guard
command numbers) wrapped in an EvEJS schema, with **EvEJS-authored NPC rosters, wave plans, loot
and gate topology** layered on top and individually labelled. For a clean-room reimplementation the
labels are the valuable part: they tell us exactly which facts are recoverable from a client we have
and which we must author ourselves.

## 2.7 Game rules stated in prose (`doc/CONTENT_PACKS.md`, `tools/ContentPacks/README.md`)

- Pirate relic/data signatures **hold no NPCs** — "how they have worked since the **Odyssey**
  expansion removed the spawns in **2013**".
- A **mining-mission site holds exactly the amount the mission requires and no more**, so every
  rock must be mined dry — "that is what retail does".
- Drone anomalies spawn in every ordinary region and **five times as densely in the eight drone
  regions**.
- Ghost sites: Lesser → high-sec, Standard → low-sec, Improved → null-sec, Superior → wormhole.
- Pirate exploration tiers: Crumbling/Local → high-sec, Decayed/Regional → low-sec,
  Ruined/Central → null-sec and wormhole classes 1–3.
- Gas: Mykoserocin → high/low/null; Cytoserocin → low/null only; fullerite → its own WH classes.
  (Manifest `templateCountByGasKind`: mykoserocin 16, cytoserocin 49, fullerite 9.)
- DED-rated: 48 of 50 may appear as signatures; **two Rogue Drone templates are escalation-only**;
  Rogue Drone signatures are "ordinary-space neutral".
- Wormhole class rule: each site stays in its class; **shattered systems, Thera and the small-ship
  shattered systems widen the band**.
- Mining missions by level: L1 12, L2 8, L3 10, L4 10; by family: ore 34, ice 3, gas 3.

## 2.8 Tooling: generator vs. validator — **split finding**

**A validator exists; the generators do not.**

- **Present:** `tools/ContentPacks/index.js` (340 lines) + `tools/ContentPacks/README.md`,
  driven by `ContentPacks.bat` or the Docker `content-tools` service.
  Commands: `status`, `verify`, `install`, `import`, `remove`, `rollback`, `doctor`, `help`.
  - `verify` checks **schema, engine version range, declared counts, and internal hashes**, and
    prints the **canonical pack SHA-256** (canonical-JSON hash, not the raw byte hash
    `Get-FileHash` gives). It **refuses a pack declaring a capability the engine does not
    implement**. There is **no publisher signature** in the format.
  - `install`/`rollback`/`doctor` build the effective dungeon authority in memory and **reject
    template, source-dungeon and spawn-profile collisions** with the base game or another active
    pack before touching the registry. A *different build* of an already-installed version is
    refused unless `--replace`.
  - It consumes a pack `.json` file (`--pack`) or a directory (`import --dir`), plus
    `--store-root` / `--content-packs-dir` / `--dry-run`.
  - Installed layout: `<storeRoot>/content-packs/installed.json` +
    `packs/<pack-id>/<version>/pack.json`. The server reads the **managed** `pack.json`, not the
    file passed to `install`. Bookkeeping: `.auto-install.json` (what the folder held last start,
    by content) and `.validated-packs.json` (raw SHA-256s that already passed full validation).
    Store roots: native `_local/gameStore/content-packs/`, Docker
    `/var/lib/evejs/gameStore/content-packs/`.
  - Runtime path (`AGENT.md:110-125`): boot stage 3 `autoInstallContentPacks` — the folder is the
    source of truth, newest version wins, skipped when the fingerprint is unchanged (otherwise
    **~55 s of a 65 s boot**); stage 4 `initializeContentPacks` + `dungeonAuthority.initializeDungeonAuthority()`
    — **pack conflicts fail startup**. No hot reload. Fingerprint is `name:size:mtimeMs`
    (`contentPacks/autoInstallContentPacks.js:50-67`, per `OPTIMIZATION_OPPORTUNITIES.md` §3).

- **Absent (confirmed by `find` over the whole worktree):**
  - `scripts/ContentPacks/build-*-pack.js` — every pack's `provenance.builder` names one
    (`build-combat-anomalies-pack.js`, `build-ded-rated-sites-pack.js`, `build-drone-anomalies-pack.js`,
    `build-ghost-sites-pack.js`, `build-wormhole-sites-pack.js`, `build-pirate-exploration-pack.js`,
    `build-gas-sites-pack.js`, `build-booster-production-pack.js`, `build-mining-missions-pack.js`,
    `build-ore-anomalies-pack.js`, `build-homefront-pack.js`, `build-air-tutorial-pack.js`,
    `build-fw-open-pack.js`, `build-gallente-depots-pack.js`, `build-gallente-fabricators-pack.js`).
    `scripts/` contains **only** `scripts/Diagnostics/capture-process-counters.ps1`.
  - `tools/ClientDungeonCompiler/` and its `manifests/<family>-3475087.json` target manifests —
    referenced by every client-derived pack's `provenance.targetManifest.sourcePath`. Not present.
  - `scripts/DataSync/` entirely: `Export-ClientDungeonData.ps1`, `decode-static-fsd.py`, and
    `spec/dungeonAuthoritySupplements.json`, `spec/dungeonSourcePriorities.json`,
    `spec/dungeonSourceConfidence.json`, `spec/dungeonSpawnProfiles.json`.
  - `tools/SignalAtlas/data/siteNameLookup.json`.
  - `spec/combatAnomalySecurityBands.js` (cited in the combat profile's assumptions).

  So the **extraction pipeline is documented but not shipped**. What *is* shipped is its output plus
  a precise, hash-verified description of its inputs — which is exactly what a clean-room
  reimplementation needs: we know which CCP resources to decode and what the decoded shape is,
  without having their decoder.

## 2.9 Base (non-pack) dungeon tables, for scale

`tools/DatabaseCreator/staticTables/` holds 42 static tables. The two dungeon ones:
- `dungeonAuthority/data.json` — **68 MB**, `generatedAt 2026-04-12`, `templatesByID` **5,009**
  entries (`source`: client 4,613, eve-university 396). `siteFamily`: combat 4,629, relic 172,
  data 113, unknown 85, ice 7, ghost 2, combat_hacking 1. `indexes.templateIDsBySourceDungeonID`
  has 4,613 keys. `coverage.spawnEnabledTemplatesByFamily`: combat 2,336, data 97, relic 151,
  ghost 2, combat_hacking 0.
  `counts`: archetypeCount **71**, clientDungeonCount **5,393**, dungeonObjectiveCount 67,
  objectiveChainCount 88, objectiveTypeCount 77, objectiveTaskTypeCount 53, objectiveCount 8,
  **nodeGraphCount 546**, nodeTypeCount 44, environmentTemplateCount 74,
  environmentTemplateDefinitionCount 169, environmentTemplateAnchorTypeCount 236,
  environmentTemplateAnchorGroupCount 2, environmentTemplateAnchorCategoryCount 1,
  oreResourceDungeonCount 1,148, gasResourceDungeonCount 83, iceResourceDungeonCount 21,
  universeSpawnFamilyCount **19**, resolvedClientTemplateNameCount 4,609.
  `clientData.tables` (17): `dungeonsByID`, `archetypesByID`, `objectivesByID`,
  `dungeonObjectivesByID`, `objectiveChainsByID`, `objectiveTypesByID`, `objectiveTaskTypesByID`,
  **`nodeGraphsByID`**, `nodeTypesByID`, `environmentTemplatesByDungeonID`,
  `environmentTemplateDefinitionsByID`, `environmentTemplateAnchorTypeIDs`,
  `environmentTemplateAnchorGroupIDs`, `environmentTemplateAnchorCategoryIDs`,
  `oreTypesByDungeonID`, `gasTypesByDungeonID`, `iceTypesByDungeonID`.
  `meta.universeSpawnFamilies` includes the Drifter set:
  `drifter_deepflow_rift_knownspace`, `drifter_deepflow_rift_pochven`, `drifter_observatory`,
  `drifter_observatory_infiltration`, `drifter_occupied_tabbetzur_deathless_research_outpost`,
  `drifter_occupied_tabbetzur_field_rescue`, `drifter_space_barbican_hive`,
  `drifter_space_conflux_hive`, `drifter_space_reckoning_labyrinth`,
  `drifter_space_reckoning_nexus`, `drifter_space_redoubt_hive`, `drifter_space_sentinel_hive`, …
- `dungeonClientContent/data.json` — 4.6 MB, only **26** templates (`client-dungeon:5496`…),
  i.e. a small bundled sample; the packs carry the rest.

Other static tables worth knowing exist (all CCP-derived or CCP-shaped):
`clientEntityStandings`, `clientMicroJumpComponents`, `clientPvpFilaments`,
`clientShipLinkComponents`, `clientTowComponents`, `npcLoadouts`, `npcLootTables`, `npcProfiles`,
`npcSpawnGroups`, `npcSpawnPools`, `npcStartupRules`, `npcBehaviorProfiles`,
`npcStandingsAuthority`, `npcHostileUtilities`, `agentAuthority`, `missionAuthority`,
`explorationAuthority`, `explorationWormholeStatic`, `asteroidTypesBySolarSystemID`,
`moonMiningPoints`, `reprocessingClientRandomizedMaterials`, `shipInsurancePrices`,
`skillTrainingAlphaCaps`, `fighterAbilities`, `expertSystems`, `stationDockingPlacements`,
`stationGraphicLocators`, `structureGraphicLocators`, `stargateVisualOverrides`,
`stationStandingsRestrictions`, `trigDrifterSpawnAuthority`, `capitalNpcAuthority`,
`evermarksCatalog`, `newEdenStore`, `researchFieldAuthority`, `shipSkin*Authority` (5 tables),
`dungeonAuthority`, `dungeonClientContent`.

---

## Method note (for reproducibility)

No large JSON was loaded whole except `dungeonAuthority/data.json` (68 MB) and the small packs.
Technique used on the big packs: read a bounded prefix with Python, locate `"manifest"` /
`"templatesByID"` / `"spawnProfilesByFamily"` / a named array by string index, then brace-match
one object and `json.loads` just that slice. Counts came from `grep -o … | wc -l` on distinctive
keys. `jq` was not needed.
