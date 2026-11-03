# Lane: `evejs` — a comprehensive review of what lives in the EvEJS fork

Ran 2026-10-24 against `$EVEJS` at the owner's
request, after a warp-arrival defect turned out to be something that repo had
already solved: *"this is server-side data, but a LOT of this already
datamined stuff is shipped with EVEJS, which is why i think you haven't poked
the release close enough."* That was correct.

Four read-only lanes, ~4,300 lines of findings, all four files beside this
one:

| file | what it covers |
|---|---|
| [`docs/evejs/review/static-data.md`](static-data.md) | the datamined/static data: what ships, in what format, and the schemas |
| [`docs/evejs/review/destiny.md`](destiny.md) | their space/Destiny simulation: constants, formulas, visibility, protocol |
| [`docs/evejs/review/features.md`](features.md) | our four blocked features — stargates, a second station, asteroid fields, the sun |
| [`docs/evejs/review/docs-packs.md`](docs-packs.md) | their documentation, and the 339 MB of content packs |

## 0. The two things to read before using any of it

**It is AGPL-3.0** (`LICENSE`, 619 lines, the full Affero text). That, and
not any rule of our own, is why these notes are facts-only: AGPL reaches the
source of anything that incorporates it, and we are not making that choice
for this server by accident. Re-using other people's work is fine here — the
house constraint is **supply chain** (avoid unneeded dependencies, hence
`check_deps.py`'s empty allowlist), not licence purity; see
`docs/port-tracker.md` §45.1. An MIT source such as `carbonengine/destiny`
may simply be used, with its notice.

So everything in these four files is **facts** — constants,
field names, formulas as mathematics, schemas, counts, and `path:line`
citations. No source was copied; the four reports contain **zero** fenced
JavaScript blocks, which was checked rather than assumed. Use them to know
*what is true*, then write our own code, and prefer verifying against our own
client wherever the fact is client-visible.

**It targets a different client build, and that governs everything else.**
`AGENT.md:130-131` gives their compatibility tuple as `clientBuild 3396210,
machoVersion 496, projectVersion "V24.01@ccp", birthday 170472`, and states
that changing one alone breaks handshakes and cache CRCs. **Our build is
3619879.** The `projectVersion` matches; the build does not. Their own data
is drawn from three *further* builds — 3475087 (the dungeon extract), 3503375
(homefront), 3487903 (AIR node graphs) — and two tables still cite 3294658.
So a protocol or layout claim from that repo is **`[V2]`: a second opinion,
never an oracle.** The house rule already said this; now there is a number
behind it.

**One lead worth chasing at the source.** Three of their READMEs
(`server/src/space/destiny/{simulation,batching,visibility}/README.md`) carry
a full **MIT License** notice, *"Copyright (c) 2026 CCP Games"*, for
`carbonengine/destiny` **v3.1.1**, and assert that the integrator, the
warp-factor behaviour, Park packaging and the visibility law are adapted from
it. Verified that the notices exist and say that. **Not verified: that CCP
actually published it under MIT** — that is a third-party assertion in a
third-party repo. If it is true, an MIT CCP Destiny implementation is a
primary source we may read *and adapt directly*, which would change the shape
of all remaining movement work. Worth an hour of someone's time to find at
CCP rather than inferring it from here.

## 1. The biggest surprise: the universe is not in there

**There is no committed celestial, station or stargate table anywhere.**
`.gitignore:40-41` ignores `/server/src/gameStore/data/*/data.json`. The
universe is **downloaded from CCP at install time** and converted in-tree:

- `tools/DatabaseCreator/CreateDatabase.bat:6-7` pins `SDE_BUILD=3396210` and
  `https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip`
- `tools/DatabaseCreator/EnsureSde.ps1:24-60` requires 35 `*.jsonl` files —
  `mapSolarSystems`, `mapStars`, `mapPlanets`, `mapMoons`, `mapAsteroidBelts`,
  `mapStargates`, `npcStations`, `planetResources`, `types`, `typeDogma`, …
- `tools/DatabaseCreator/database-creator.js` (3,367 lines) converts them into
  36 tables

The format is CCP's **newer static-data JSONL** — one file per table, one JSON
object per line, primary key in `_key`. Not the sqlite SDE, not the classic
YAML SDE. `mapDenormalize` does not exist in that repo at all.

**What this means for us is good news, not bad.** The authority is CCP's own
published static data, which anyone may download legally — exactly the shape
the owner's rule requires (*"if anyone needs anything from the original client
they still have to obtain that in a legal way"*). We do not need their repo
for the universe; we need the same CCP endpoint and our own converter.

## 2. What *is* committed, and which parts matter

| bucket | size | note |
|---|---|---|
| `tools/DatabaseCreator/staticTables/` | 166 MB, 42 dirs | the hand-curated authorities the SDE lacks |
| `content-packs/` | 339 MB, 18 files | `evejs.content-pack.v1` dungeon/site data |
| `server/assets/` | 112 KB | **one** file, a client overview preset. No universe data. |

The static tables are committed *outputs* whose generators are mostly **absent**
(`scripts/DataSync/*`, `scripts/ContentPacks/*`, `tools/ClientDungeonCompiler/*`).
For clean-room work that absence is close to ideal: `staticTables/dungeonAuthority/data.json`
**names the 17 CCP client resources** it was built from, with `resourceHash`,
`sizeBytes` and the ResFiles cache layout `<2hex>/<16hex>_<md5>` —
`res:/staticdata/dungeons.fsdbinary` (714,440 B), `nodegraphs.fsdbinary`
(7,271,384 B), `objectives`, `archetypes`, `environmenttemplates`,
`ore/gas/icetypesbydungeonid.static` and the rest. **We learn which resources
to decode and the exact decoded shape, without their decoder.**

Largest tables: `dungeonAuthority` 68 MB (5,009 templates, 546 node graphs),
`moonMiningPoints` 27 MB (157,905 points, `beaconID = 1e9 + moonID`),
`missionAuthority` 15 MB (2,878 missions), `agentAuthority` 11 MB (10,941
agents), `explorationWormholeStatic` 6.1 MB (8,490 systems, 99 wormhole codes,
3,015 static slots).

## 3. The payoff that started this: the warp-in point

Their `server/src/space/destiny/simulation/warpInPointParity.js` is a faithful
transcription of three client functions. The *filename* was the find — "parity"
meant it mirrors the client, so **our own client is the oracle** and this stops
being `[V2]`. Everything is now `[V]` from this build's bytecode, and is
implemented in `server/crates/eve-server/src/warp_in.rs`. See
`docs/port-tracker.md` §44. Two corrections to their transcription fell out of
reading the client directly: the planet seed is the client's `ballID` (not
`itemID`), and their extra asteroid-belt exclusion is **not** CCP's rule —
`HasWarpInPoint` has exactly three conditions and no belt case.

## 4. Where their facts disagree with our own reverse engineering

**These are the most valuable lines in the whole review, and none of them is
settled.** Do not "fix" our code to match; resolve each against our own
client, remembering their build is 3396210.

1. **The 52-byte WARP tail's last two fields.** They report the order as
   `target(3×f64) → effectStamp(i32) → totalDistance(f64) → minimumRange(f64)
   → ownerID(i64)`. We have `+28` as a `-1.0` marked **`[I]`** and `+44` as
   the warp factor (AU/s ×1000, 4000 for the capsule). `destiny::WARP_TAIL_UNKNOWN`'s
   own doc comment says it is "falsified by a dump in which it is anything
   else" — **this is that challenge arriving.** But our `+44 = 4000` is a poor
   fit for an `ownerID`, so at most one of us is right about each field, and
   possibly the layout moved between builds. Dump a warp again and read those
   16 bytes.
2. **The WARP ball may not be massive.** They report flag `0x09` for a proper
   WARP record and `0x0d` for the aligning (negative-effect-stamp) one, with a
   stated reason: Carbon aliases the WARP record's total distance through
   `mLastCollision`, so a *massive* WARP record has that input overwritten
   before the solver reads it. Check our `AddBalls` flag byte.
3. **Warp completion is speed-based, not time-based**, in their model:
   complete when `v < dropoutSpeed = min(maxVelocity/2, 100)`.
4. **Their `k_d` floor diverges from Carbon by their own admission** —
   `min(max(wf/3000, 0.001), 2)`, with a comment that Carbon applies no
   low-factor floor to deceleration. Ours matches Carbon, so this one is
   probably *theirs* to fix, not ours.
5. **Accelerated distance closed form**: they use `v_base/k_a` with
   `v_base = min(v_max, k_a·e^{k_a s})` — an `e^{k_a t}`, where our curve has
   `e^{k_a t} − 1`. Our curve closes on `d−1` with error exactly 0, so this
   needs care rather than adoption.

## 5. The four blocked features, in one paragraph each

**Stargates are comprehensively answered** — see `docs/evejs/review/features.md` §1. One call,
`beyonce.CmdStargateJump([fromGateID, toGateID, shipID?])`, returning **no
payload**; the client drives everything off the session change and the Destiny
stream. Jump range **2500 m** hull-to-hull. Completion is **not inline**: it is
scheduled at `max(4000 ms, JumpOut stamp boundary + 3000 ms)` in simulation
time. Arrival is **stationary**, in a random direction within a **45° cone** off
the gate's forward axis, at a **fixed** distance `gateRadius + shipRadius +
12000 m` — the direction is scattered and the distance deliberately is not, to
preserve approach times and gate-camp coverage. Gate cloak **60 s** (distinct
from undock invulnerability, **30 s**). **No capacitor or fuel cost** — a
confirmed absence. Linkage is one-directional per row (`destinationID` → the
paired gate's `itemID`). A gate reads **CLOSED when its destination system's
scene is not loaded**. And **routing is not a server feature at all**: the
client owns the jump map from its own SDE and fires one call per hop. Our
separate finding that the client also asks the gRPC gateway for
`GetRestrictedSystemsRequest` resolves cheaply — they answer **HTTP 200 with a
zero-length response**, i.e. "nothing is locked", and their own comments
explain why a 404 would be wrong there.

**A second station needs no new mechanism.** N stations per system is already
supported — stations are keyed by `stationID`, held as per-system arrays, and
dock resolution is purely by id. The only single-station assumptions are
cosmetic (`[[0]]`-style spawn-anchor picks). **Confirmed absence: no
station-ID allocator and no "add a station" path**, so adding one means
writing a `stations` row with an id we allocate plus a locator profile for its
type. Note two radii coexist: docking range is **2500 m** hull-to-hull against
an `interactionRadius` defaulting to **1000**, while a separate physical radius
defaults to **15000** and is used only for sentry placement. Undock geometry is
**authored, not computed** — from `stationGraphicLocators` (44 profiles,
categories `undockPoint{,Small,Large,Capital,Supercapital}`) or a stored
`dockPosition`; `extraUndockDistance` is 0 in practice, so the undock point
equals the dock point, with **no station-radius multiplier**.

**Asteroid fields split into a datamined half and an invented half**, and the
distinction matters. The *ore per system* is real datamined data, from the
client pickle `res:/staticdata/asteroidtypesbysolarsystemid.pickle` — 3,974
systems, 36 distinct types, 24,071 assignments. The *field geometry* is
**server-authored, not CCP's**: a closed-form `seededInt(seed, salt, min, max)
= min + sha256("seed:salt")[0..3] % (max−min+1)` seeded on the belt's
`itemID`, with four style profiles chosen by security band. So we can take
their ore tables as fact and must treat their roid layout as one invention
among many. **Confirmed absence: no belt respawn rule anywhere** (ice has one,
360 minutes; belts do not). Separately, the **ore anomaly content packs are
exact client extracts** with per-rock `typeID` and `quantity`, and the
provenance records that **CCP stores asteroid quantity in the dungeon object's
`dunRadius`** — a fact we can use directly against our own FSD reader.

**The sun is a real ball, not scenery.** A `celestials` row with
`kind:"sun"` that becomes a Destiny static ball in `SetState`: `groupID 6`,
`categoryID 2`, `itemID` the **raw SDE `mapStars._key`** (40xxxxxx, no offset
convention), `position` hardcoded `{0,0,0}`, radius from the SDE with a 1000 m
fallback. Ball `mode = RIGID (11)`, `flags = IS_GLOBAL (0x02)`, tail a single
`0xFF` byte. **Untargetable** (group set `{6,7,8,9,10,885,995}`) but warp-to
works, and **it decloaks ships** where belts do not. `sunTypeID` is never sent
— the star's visual comes from the client's own SDE. Their sun warp-in formula
independently matches what we read out of `GetSunWarpInPoint`, which is a
pleasing cross-check of both.

## 6. Oracles: thinner than hoped, and two real ones

**There is no golden/parity test-vector corpus for the simulation.** The
capture statistics quoted throughout their comments (46 recordings / 6,203
updates, Jita gate group `50001250` at 11.2 Mm, salvage golden tick 62674) are
**comment assertions with no checked-in evidence** — hypotheses, not oracles.
`doc/PARITY_WARP_IN_MATERIALIZATION.md` is cited by name in their code and is
**not in the checkout**; 21 referenced docs are missing in total, and they are
exactly the per-feature rule specs we would want most.

Two are real and worth using:

1. `server/src/abyssal/catalog/fixtures/t0TranquilRetailCapturedSceneryFixture.json`
   (310 KB) — an explicit **captured-from-Tranquility** golden file citing
   source file *and line*, with row tuples
   `[itemID, typeID, ownerID, ?, null, null, pos[3], rot[3], groupID, categoryID]`.
   The clearest "what the real server actually sent" artefact in the repo.
2. `staticTables/stationDockingPlacements/data.json` — 5,210 rows of
   `[stationID, stationTypeID, dockEntry, dockPosition, dockOrientation,
   undockDirection, undockPosition]`. Its own provenance says it was extracted
   from a generated stations table, i.e. it *is* a snapshot.

Also: `moonMiningPoints` ships a **universe-parity fingerprint** —
`celestialTableSha256 = 9c6f352f…` and `solarSystemTableSha256 = f31209d3…`.
Match those and our universe is byte-identical to theirs. The hash input format
is not documented in-tree, so this is a target, not yet a check.

## 7. Two of their bugs, recorded so we do not copy them

- `asteroidBelts` is a filter of the same array `celestials` is written from
  unfiltered, and the row builder does not dedupe — so belts appear **twice**
  in `GetSolarsystemItems`.
- `connector` is **never true** in that rowset; on Tranquility stargates are
  `connector = True`.

## 8. What to do with all this

1. **The universe converter.** Their path — CCP's published static-data JSONL,
   pinned by build — is the right one and is legal. Ours should read the same
   endpoint for *our* build and write our own extract. This is the single
   biggest unlock: it ends the one-system sandbox.
2. **Chase the MIT `carbonengine/destiny` v3.1.1 claim at CCP.** If it holds it
   dwarfs everything else here.
3. **Resolve §4's five disagreements against our own client**, starting with
   the WARP tail's last 16 bytes, because our `[I]` is explicitly waiting to be
   falsified and now has a challenger.
4. Stargates can be specced from `docs/evejs/review/features.md` §1 today; the gRPC
   `GetRestrictedSystems` answer is a two-line stub.
5. Treat every protocol claim as `[V2]` until our own bytecode agrees. The
   build numbers differ, and the warp-in point is the worked example of doing
   it properly: their file told us *where to look*, our client told us *what is
   true*, and the two disagreed in two places.
