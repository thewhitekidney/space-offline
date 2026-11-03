# The content bundle

The server's static world comes from the bundle in `out/content/`: the
universe, types and dogma, skills, NPCs, agents, industry, dungeons, and
more. Our tools extract it from **your own client install** and merge in our
authored tables from `content/`. `out/` is gitignored. The bundle is CCP's
data and never enters the repository.

This page covers the inputs, the steps, the outputs, verification, and
rebuilding for a new client build. Column-level detail is in
[docs/content/schema.md](../content/schema.md), and nothing here copies it.
The short path is step 4 of [getting-started.md](getting-started.md).

## The command

```sh
tools/analysis.sh --build          # once: the analysis image (containers/analysis.Dockerfile)
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content --check
```

`tools/analysis.sh` runs the analysis container with `--network none`, as
your user. It mounts:

- the repository at `/work`;
- `EVE_CLIENT` read-only at `/client`;
- the client's parent directory read-only at `/client-root`, so the
  `ResFiles` beside the install is visible.

`EVE_CLIENT` defaults to the offline copy,
`$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/$EVE_CLIENT_DIR`,
where `EVE_CLIENT_DIR` defaults to `tq-offline`. It never falls back to the
online install.

`content.py` flags:

| Flag | Meaning |
|---|---|
| `--client DIR` | the install (default `$EVE_CLIENT`; no default inside the container) |
| `--resfiles DIR` | the ResFiles store (default: `$EVE_RESFILES`, else `ResFiles` beside the install) |
| `--out DIR` | the bundle (default `out/content`) |
| `--sections a,b` | rebuild only these sections, plus what they depend on, merged into the existing bundle |
| `--systems id,id` | a sandbox: restricts `universe` and `belts` only |
| `--build N\|any` | accept this build (default: the ones `tools/builds.tsv` marks pinned or equivalent) |
| `--check` | verify a bundle instead of writing one |
| `--against BUNDLE` | with `--check`: prove two bundles hold the same data |

**You rarely run it by hand.** `tools/up.sh` and `tools/serve.sh` re-extract
before boot when:

- there is no `MANIFEST.tsv`;
- the extractor's hash changed;
- an authored input's sha256 changed;
- a client source file's md5 no longer matches `resfileindex.txt`.

Either way, `--check` runs next, its log goes to `logs/content-check.log`,
and a failure refuses the boot. `tools/serve.sh --bundle-frozen` skips both
steps, for a host that receives a bundle built elsewhere
([hosting.md](hosting.md)).

`tools/content-lib.sh` picks the directory:

- a pinned or equivalent build uses `out/content`;
- an unverified build uses `out/content-<build>`;
- an unregistered build is refused;
- `EVE_CONTENT` overrides the directory, which must stay under `out/`.

**Writes are atomic.** `content.py` writes into `out/content.tmp-<pid>/`,
writes `MANIFEST.tsv` last, then swaps the directory in, so a crash never
leaves a half bundle. A bare `--sections` run merges into the existing
bundle and keeps the other sections.

## Inputs

| Input | Source | How to get it | Licence / provenance | Required |
|---|---|---|---|---|
| `start.ini` | the client install | from the launcher's download (installed by the human) | CCP; read only | yes: the build number |
| `resfileindex.txt` | the client install | as above | CCP; read only | yes: maps `res:` paths to ResFiles and gives each file's md5 |
| `res:/staticdata/*.static`, `*.fsdbinary` | ResFiles, through the index | as above (every resource downloaded) | CCP; read only, never committed | yes |
| `res:/localizationfsd/localization_fsd_en-us.pickle` | ResFiles | as above | CCP; read only | yes: names and text |
| `bin64/*Loader.pyd` | the client install | as above | CCP; only their schemas are re-derived and compared with our committed `findings/pyd-schemas.tsv` | yes: the schema guard |
| `out/pyc/` (unpacked client code) | the client's `code.ccp` | our RE tools | CCP; local only | no: without it `facwar/max_lp.tsv` is not written |
| `content/wormholes/combat-sites.tsv` | this repository | in the clone | ours (authored, `[A]`, public game knowledge) | yes, for the `wormholes` section: the combat anomalies' class by name |
| `out/geode-picks.local.tsv` | a local file, gitignored | written by hand | local only | no |
| `content/econ/price-overrides.tsv`, `content/econ/meta-mult.tsv` | this repository | in the clone | ours (authored) | no: absent means no rows |
| `content/econ/geode-picks.tsv` | this repository | in the clone | ours | yes |
| `content/store/offers.tsv`, `content/store/categories.tsv` | this repository | in the clone | ours | yes |
| `content/agents/lp-overrides.tsv` | this repository | in the clone | ours | no |
| `content/npc/region-pirates.tsv` | this repository | in the clone | ours | no |

The exact `res:` files behind each output are in that file's `sources`
column in `MANIFEST.tsv`.

The other authored tables in `content/` (bots, population, wormhole
statics, and so on) are not bundle inputs. The server reads them directly
through `--authored content`, or uses the copy compiled into the binary
([content/README.md](../../content/README.md) says what may go there).

**Not inputs.** No EVE Static Data Export, no `bulkdata`, and no download of
any kind: the container has no network.

### EvEJS

EvEJS is a separate AGPL project. No step of the pipeline reads an EvEJS
file, and none of its code or data is vendored here. `tools/evejs-up.sh`
runs EvEJS in a container for protocol observation only. It is not part of
the bundle.

Where an EvEJS fact helped (for example, a hint about loyalty-point offers,
marked `[I]` in `tools/content_lp.py`), it was a lead to verify against the
client's own data, which is what the extractor then reads. We use it for
facts, never for code or prose, so the AGPL does not reach this pipeline.

## Steps

`content.py` runs four built-in sections first, then every
`tools/content_*.py` that exports `SECTION`, `DEPENDS`, `section(ctx)` and
`check(ctx)`, in dependency order. Before each section, a schema guard
checks the client's loader schemas against `findings/pyd-schemas.tsv`, and
any disagreement stops the run. Later sections read earlier outputs from the
bundle being built.

| Section | Writer | Writes |
|---|---|---|
| `universe` | `content.py`, `tools/celestials.py` | `universe/*` |
| `belts` | `tools/asteroids.py` | `belts/rocks.tsv` |
| `types` | `tools/typedata.py`, `tools/skills.py` | `types/typedata.tsv`, `types/skills.tsv` |
| `planets` | `tools/pi_statics.py` | `planets/pi-statics.tsv` |
| `agents` | `tools/content_agents.py` | `agents/*` |
| `dbuffs` | `tools/content_dbuffs.py` | `dbuffs/*` |
| `deploy` | `tools/content_deploy.py` | `deploy/*` |
| `dungeons` | `tools/content_dungeons.py` | `dungeons/*` |
| `facwar` | `tools/content_facwar.py` | `facwar/*` |
| `fighters` | `tools/content_fighters.py` | `fighters/*` |
| `geodes` | `tools/content_geodes.py` | `mining/*` |
| `npc` | `tools/content_npc.py` | `npc/{factions,corporations,types,components,reactive-spawn,bounty}.tsv` |
| `npc_regions` | `tools/content_npc_regions.py` | `npc/{regions,home-pirate,trades,loot}.tsv` |
| `police` | `tools/content_police.py` | `npc/{police,concord}.tsv` |
| `skins` | `tools/content_skins.py` | `skins/*` |
| `store` | `tools/content_store.py` | `store/*` |
| `types2` | `tools/content_types.py` | `types/{market,marketgroups,typelists,materials,materials-random,compress}.tsv` |
| `econ` | `tools/content_econ.py` | `industry/blueprints.tsv`, `stations/*`, `econ/prices.tsv` |
| `explore` | `tools/content_explore.py` | `explore/*` |
| `lp` | `tools/content_lp.py` | `agents/lp-offers.tsv` |
| `salvage` | `tools/content_salvage.py` | `salvage/*` |
| `wormholes` | `tools/content_wormholes.py` | `wormholes/{sites,sleepers}.tsv` |

## Outputs

One directory per area, in TSV. What each file holds, its header, its writer
and its server readers are in
[schema.md section 2](../content/schema.md). The bundle's own record is
`MANIFEST.tsv`, which works like this:

- Line 1 is a banner: `# eve-offline content bundle. CCP's data: never
  commit this directory.`
- `format`: the bundle format. The server refuses any major version but 1.
- `client_build`: the build the data came from.
- `extractor`: a sha256 over `tools/content*.py` and the reader tools.
- `systems`: `all`, or the sandbox list.
- `data_hash`: one hash over every file's path and sha256.
- One `file` row per file: path, data rows, sha256, and the `res:` sources.
- One `input` row per authored input: path, sha256.
- One `source` row per client file read: `res:` path, md5.

The grammar is in [schema.md section 1.2](../content/schema.md).

## Verify

- **`content.py --check`** checks that every listed file exists with its
  row count and sha256, that the format matches and the `data_hash` adds
  up, and then runs each section's own check against the client. It exits 1
  on failure. A build with no expected counts prints `UNVERIFIED build`
  notes instead of failing.
- **`--check --against OTHER`** proves that two bundles hold the same data
  (equal `data_hash`).
- **`python3 -I tools/bundle-diff.py OLD NEW [--uses]`** compares two
  bundles file by file, then row by row. It flags removed rows and class,
  dogma or component changes. It exits 0 when they are equal, 1 when they
  differ, and 3 on a flag.
- **The server, at `--content`**, refuses:
  - a wrong format major;
  - a `client_build` it does not accept, unless that `data_hash` is
    registered for an accepted build;
  - a missing file;
  - a wrong row count;
  - a header that differs from schema.md.

  Without `--content`, it runs on invented fixture rows and says so loudly.

A quick look:

```sh
head -n 6 out/content/MANIFEST.tsv     # banner, format, client_build, extractor, systems, data_hash
grep -c '^file' out/content/MANIFEST.tsv
```

## A new client build

Full detail is in [build-compat.md](build-compat.md). In order:

1. `tools/import-build.sh tq-<name>` copies the online install into the
   offline prefix, hard-linking the blobs.
2. `tools/new-build.sh DIR [--report]` walks the checks:
   - the install, its pin, and `build-delta.py` against the pinned build;
   - the cFSD layout and schema overlays, and the expected counts;
   - registering the build in `tools/builds.tsv` and
     `eve_marshal::build::BUILDS`. This comes before any data, because an
     unregistered build is refused.
3. Extract the new build's bundle, then compare it with the old one:

   ```sh
   EVE_CLIENT_DIR=tq-<name> tools/analysis.sh python3 -I /work/tools/content.py \
       --client /client --resfiles /client-root/ResFiles --out /work/out/content-<build> --build <build>
   EVE_CLIENT_DIR=tq-<name> tools/analysis.sh python3 -I /work/tools/content.py \
       --client /client --resfiles /client-root/ResFiles --out /work/out/content-<build> --check --against /work/out/content
   python3 -I tools/bundle-diff.py out/content out/content-<build>
   ```

4. If the data is equal, register the `data_hash` in the build's
   `tools/builds.tsv` row, so the old bundle serves the new build.
5. Only when the new build becomes the pinned one, run
   `tools/re-pin.sh NEW_DIR`. It moves `out/content` to
   `out/content-<old>` and does not extract. The next `tools/up.sh`, or a
   manual run of the command above, rebuilds `out/content`.

## Never commit

- `out/` is gitignored as a whole. That covers the bundle, `out/pyc/` and
  the `*.local.tsv` files.
- `content/` holds only our own authored rows. It holds no client rows,
  counts, names or real ids ([content/README.md](../../content/README.md)).
- Tests use invented ids and rows. A test that needs the real bundle reads
  it at run time through `EVE_TEST_BUNDLE`, and skips without it.
- No scanner checks commits for client data. The guard is the
  `.gitignore`, the banner, and review. Before you commit, check that
  `git status` shows nothing under `out/`.
- A container image you push must not contain the bundle. Mount it at run
  time ([launch-and-env.md](../reference/launch-and-env.md#launch-examples)).
