#!/usr/bin/env python3
"""Extract the client's static data into one content bundle, `out/content/`.

    tools/analysis.sh python3 -I /work/tools/content.py \\
        --client /client --resfiles /client-root/ResFiles \\
        --out /work/out/content [--sections universe,belts] \\
        [--systems 30000001,30000003] [--build N|any] [--check [--against B]]

The content driver (`docs/content/plan.md` §1.1-§1.2, packet CP1; every
header and the manifest grammar are `docs/content/schema.md`). It owns the
arguments, the atomic write (`<out>.tmp-<pid>/`, `MANIFEST.tsv` last, then a
rename over `<out>`), the manifest with its `extractor` hash, and the four
phase-1 sections `universe`, `belts`, `types` and `planets`. Those import
`celestials`, `asteroids`, `typedata`, `skills` and `pi_statics` unchanged,
and `--check` proves their files byte-identical to those tools' own output.

**Sections.** Every `tools/content_<name>.py` exporting `SECTION`, `DEPENDS`,
`section(ctx)` and `check(ctx)` (and optionally `CFSD`, the cFSD stems it
reads) is found by glob, so adding one edits no shared file. `section(ctx)`
returns `[(relpath, header_cols, rows_iter[, sources])]`; a row is a tuple
(encoded by `encode`) or a `str` written as is, and `header_cols` of `None`
writes no header line. `check(ctx)` returns failure strings and records its
evidence with `ctx.note`.

**`--sections` merges** (FX8): run over an existing bundle, it rewrites only
the named sections' files (plus their `DEPENDS`) and their `file` rows, and
keeps every other file and row; see `merge_base`. With no `--sections` the
run replaces the whole bundle.

**Builds** (R-BUILD BA-3): the install may be any build `tools/builds.tsv`
marks `pinned` or `equivalent` (`--build N` demands one build, `--build any`
stops checking; the same flag as `tools/dogma.py`). The manifest records the
install's own build as `client_build` and, since format 1.1, a `data_hash`
over the data files, so two extractions can be proved to hold the same data
whatever build they came from (`--check --against OTHER_BUNDLE`).

**The schema guard** (C1 §3): before a section runs, each of its cFSD stems
is re-derived from the client's `bin64/<stem>Loader.pyd` and compared with
`findings/pyd-schemas.tsv`; any disagreement on stride, delta or a field
offset stops the run. Stdlib only, read-only on the client. CCP's data:
never commit the output.
"""

# `tools/` goes LAST on `sys.path`: it holds a `dis.py` that shadows the
# stdlib's, and `-I` drops the script directory anyway (plan §1.1).
import sys as _sys
import os as _os

_HERE = _os.path.dirname(_os.path.abspath(__file__))
if _HERE in _sys.path:
    _sys.path.remove(_HERE)
_sys.path.append(_HERE)

import argparse  # noqa: E402
import collections  # noqa: E402
import glob  # noqa: E402
import hashlib  # noqa: E402
import math  # noqa: E402
import importlib.util  # noqa: E402
import os  # noqa: E402
import shutil  # noqa: E402
import sys  # noqa: E402
import tempfile  # noqa: E402

#: The manifest's `format` (`schema.md` §1.2). A header change bumps MINOR.
#: 1.1 (BA-3) adds the `data_hash` record; 1.0 bundles still read and merge.
#: 1.9 adds the `conquest/` files (new files only, #291); a 1.8 server ignores them.
#: 1.8 adds the last column `missile_groups` to `npc/factions.tsv` (#128).
#: 1.7 adds `mutators/ranges.tsv` and `mutators/mappings.tsv`.
#: 1.6 adds `econ/prices.tsv`'s last column, `build_min`.
FORMAT = "1.9"
MANIFEST = "MANIFEST.tsv"
MANIFEST_BANNER = "# eve-offline content bundle. CCP's data: never commit this directory."

#: The readers the `extractor` hash covers besides `tools/content*.py`
#: (plan §1.1, `schema.md` §1.2).
READERS = ("fsd", "dogma", "pyd_schema", "loctable", "celestials", "asteroids",
           "pi_statics", "typedata", "skills")

#: Phase-1 order. Discovered sections follow, ordered by `DEPENDS`.
BUILTIN_ORDER = ("universe", "belts", "types", "planets")

RES = "res:/staticdata/"
LOC_EN = "res:/localizationfsd/localization_fsd_en-us.pickle"
SSC = RES + "solarsystemcontent.static"
SYSTEMS = RES + "systems.static"
REGIONS = RES + "regions.static"
CONSTELLATIONS = RES + "constellations.static"
OWNING = RES + "factionsowningsolarsystems.static"
ROCK_COUNTS = RES + "asteroidcountbytypeidbeltid.static"


#: The readers that rotate `sys.path` at import (`sys.path.append(
#: sys.path.pop(0))`, `tools/dis.py`). That idiom assumes `tools/` is
#: `sys.path[0]`, which is false under `-I`: there it rotates a stdlib entry
#: behind `tools/`, and `tools/dis.py` then shadows `dis`. So `_imp` puts
#: `tools/` first for exactly the import, and the base readers go first so
#: that each module rotates once, from the state it expects.
_ROTATING = ("fsd", "loctable", "dogma", "celestials", "asteroids", "pi_statics",
             "typedata", "skills")


def _imp(name):
    """Import a reader, leaving `sys.path` as it was (stdlib first)."""
    if name in sys.modules:
        return sys.modules[name]
    if name not in _ROTATING:
        return importlib.import_module(name)
    for base in _ROTATING[:min(_ROTATING.index(name), 3)]:
        _imp(base)
    saved = list(sys.path)
    sys.path[:] = [_HERE] + [p for p in saved if p != _HERE]
    try:
        return importlib.import_module(name)
    finally:
        sys.path[:] = saved


def fsdbin(stem):
    return RES + stem + ".fsdbinary"


#: `groupStation` (`inventorycommon/const.pyc`) [V]: the dock-locator types.
GROUP_STATION = 15
#: U0 [V-data] (r-upwell §4): the structure groups whose graphic location carries dock
#: locators too (Astrahus group 1657 'Citadel', Raitaru 1404, Athanor 1406).
GROUPS_STRUCTURE = (1657, 1404, 1406)
LOCATOR_GROUPS = (GROUP_STATION,) + GROUPS_STRUCTURE


class ContentError(Exception):
    """A refusal: bad arguments, a schema-guard disagreement, a bad section."""


# --- encoding (`schema.md` §1.1) -----------------------------------------------

def clean(s):
    """A field never holds TAB, CR or LF; each becomes one space."""
    return str(s).replace("\t", " ").replace("\r", " ").replace("\n", " ")


def encode(v):
    """One field. `None` is empty; bools `0`/`1`; floats by `repr`; a list is
    `a,b,c`; a dict is `k=v;k=v` in its own order."""
    if v is None:
        return ""
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return repr(v)
    if isinstance(v, str):
        return clean(v)
    if hasattr(v, "items") and callable(v.items):
        return ";".join("%s=%s" % (encode(k), encode(x)) for k, x in v.items())
    return ",".join(encode(x) for x in v)


def encode_row(row):
    return "\t".join(encode(v) for v in row)


def decode_list(s, conv=int):
    """`a,b,c` back to a list; empty is the empty list."""
    return [conv(x) for x in s.split(",")] if s else []


def decode_map(s, kconv=int, vconv=float):
    """`k=v;k=v` back to a dict; empty is the empty dict."""
    out = {}
    for part in s.split(";") if s else []:
        k, _, v = part.partition("=")
        out[kconv(k)] = vconv(v)
    return out


def is_data(line):
    """A data line: not empty and not a `#` header or comment."""
    return bool(line) and not line.startswith("#")


def read_rows(path):
    """The data lines of a bundle file, split on TAB."""
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if is_data(line):
                yield line.split("\t")


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def extractor_files(tools_dir):
    """`tools/content*.py` plus the listed readers, sorted by path."""
    paths = set(glob.glob(os.path.join(tools_dir, "content*.py")))
    paths.update(os.path.join(tools_dir, r + ".py") for r in READERS)
    return sorted(p for p in paths if os.path.isfile(p))


def extractor_hash(tools_dir=_HERE):
    """sha256 over each file's bytes, in sorted path order (`schema.md` §1.2)."""
    h = hashlib.sha256()
    for p in extractor_files(tools_dir):
        with open(p, "rb") as fh:
            h.update(fh.read())
    return h.hexdigest()


def sort_rocks(rows):
    """`belts/rocks.tsv` order (plan §1.1): by system, then belt, and the
    client's own row order inside a belt. `sorted` is stable, so the in-belt
    order is exactly the order the rows arrive in. A row is
    `(belt_id, ordinal, type_id, count, system, ...)`."""
    return sorted(rows, key=lambda r: (r[4], r[0]))


# --- sections ------------------------------------------------------------------

#: One section: its `DEPENDS`, its two functions and the stems it guards.
Section = collections.namedtuple("Section", "name depends section check cfsd origin",
                                 defaults=((), "content.py"))


def _load_module(path):
    name = "content_section_" + os.path.basename(path)[len("content_"):-3]
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    for attr in ("SECTION", "DEPENDS", "section", "check"):
        if not hasattr(mod, attr):
            raise ContentError("%s: a section module must export %s" % (path, attr))
    return Section(mod.SECTION, tuple(mod.DEPENDS), mod.section, mod.check,
                   getattr(mod, "CFSD", ()), origin=os.path.basename(path))


def discover(section_dir=_HERE):
    """The built-in sections, then every `content_*.py` in `section_dir`."""
    found = [Section(n, (), *BUILTINS[n]) for n in BUILTIN_ORDER]
    for path in sorted(glob.glob(os.path.join(section_dir, "content_*.py"))):
        found.append(_load_module(path))
    seen = {}
    for s in found:
        if s.name in seen:
            raise ContentError("section %r is defined by both %s and %s"
                               % (s.name, seen[s.name], s.origin))
        seen[s.name] = s.origin
    return found


def order(sections, wanted=None):
    """`wanted` (or all) plus their `DEPENDS`, each after what it depends on.
    Ties keep discovery order. A missing dependency or a cycle refuses."""
    by_name = {s.name: s for s in sections}
    names = list(by_name) if wanted is None else list(wanted)
    need, stack = set(), list(names)
    while stack:
        n = stack.pop()
        if n in need:
            continue
        if n not in by_name:
            raise ContentError("no such section: %s (known: %s)"
                               % (n, ",".join(by_name)))
        need.add(n)
        stack.extend(by_name[n].depends)
    out, done = [], set()
    while len(out) < len(need):
        ready = [s for s in sections
                 if s.name in need and s.name not in done
                 and all(d in done for d in s.depends)]
        if not ready:
            raise ContentError("DEPENDS cycle among: %s"
                               % ",".join(sorted(need - done)))
        out.append(ready[0])
        done.add(ready[0].name)
    return out


# --- the context ---------------------------------------------------------------

class Ctx:
    """What a section sees: the client, the bundle directory, the options."""

    def __init__(self, args, out_dir):
        self.args = args
        self.out_dir = out_dir
        self.systems = args.systems_list
        self.inputs = {}
        self._client = None
        self._cache = {}
        self._cdn = {}

    @property
    def client(self):
        if self._client is None:
            dogma = _imp("dogma")
            self._client = dogma.resolve_client(self.args.client, self.args.resfiles,
                                                want_build=getattr(self.args, "build", None))
        return self._client

    def load(self, res):
        """A `.static`/`.pickle` resource, loaded once per run."""
        if res not in self._cache:
            self._cache[res] = self.client.load(res)
        return self._cache[res]

    def path(self, relpath):
        return os.path.join(self.out_dir, relpath)

    def cdn(self, key, path):
        """Register a file the client downloaded from CCP's CDN (not part of the
        install; e.g. the achievement definitions in the user cache) as source
        `cdn:<key>`, so the manifest records its digest and a new download
        re-extracts."""
        self._cdn["cdn:" + key] = path
        return "cdn:" + key

    def md5(self, res):
        if res.startswith("cdn:"):
            with open(self._cdn[res], "rb") as fh:
                return hashlib.md5(fh.read()).hexdigest()
        if res.startswith("pyc:"):  # unpacked client code, not a client file
            root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "out", "pyc")
            with open(os.path.join(root, res[len("pyc:"):]), "rb") as fh:
                return hashlib.md5(fh.read()).hexdigest()
        return self.client.index[res]["md5"]

    def note(self, msg):
        print("content: " + msg, file=sys.stderr)

    def input(self, path):
        """Record an authored `content/` file a section read (`input` rows)."""
        self.inputs[os.path.relpath(path, os.path.dirname(_HERE))] = sha256_file(path)
        return path

    def messages(self):
        """The en-us message table: `{messageID: (text, ...)}`."""
        if "loc" not in self._cache:
            dogma = _imp("dogma")
            self._cache["loc"] = dogma.Dogma(self.client).messages or {}
        return self._cache["loc"]

    def text(self, message_id):
        e = self.messages().get(message_id) if message_id is not None else None
        if e is None:
            return ""
        return clean(e[0] if isinstance(e, (tuple, list)) else e)

    def resolve_args(self):
        """The client and ResFiles paths, for running an old tool's `main`."""
        return ["--client", self.client.root, "--resfiles", self.client.resfiles]


# --- the schema guard (C1 §3) --------------------------------------------------

def layout_disagreements(stem, live, committed):
    """Where the client's loader (`live`, a `pyd_schema.TableSchema`) and the
    committed layout (`committed`, an `fsd.PydSchema`) differ: stride, delta,
    or a field's offset."""
    out = []
    for attr in ("stride", "delta"):
        a, b = getattr(live, attr), getattr(committed, attr)
        if a != b:
            out.append("%s: %s is %s in the loader, %s in pyd-schemas.tsv" % (stem, attr, a, b))
    # The loader's offsets are struct-relative; the TSV's already include
    # `delta` (`pyd_schema.py` writes `offset + delta`).
    d = live.delta or 0
    lf = {f.name: f.offset + d for f in live.fields}
    cf = {f.name: f.offset for f in committed.fields}
    for name in sorted(set(lf) | set(cf)):
        if lf.get(name) != cf.get(name):
            out.append("%s.%s: offset %s in the loader, %s in pyd-schemas.tsv"
                       % (stem, name, lf.get(name), cf.get(name)))
    return out


def schema_guard(ctx, stems):
    if not stems:
        return
    fsd = _imp("fsd")
    # Raises for a build with no layout entry (fsd.CFSD_BUILD_LAYOUTS): its
    # loader may differ where the guard cannot see, in the presence bits.
    build = fsd.client_layout_build(ctx.client)
    fsd.layout_for_build(build)
    pyd_schema = _imp("pyd_schema")
    bad = []
    for stem in stems:
        committed = fsd.load_pyd_schemas(build=build).get(stem)
        if committed is None or committed.status != "ok" or not committed.fields:
            bad.append("%s: no committed layout in pyd-schemas.tsv" % stem)
            continue
        path = fsd._find_loader_pyd(ctx.client.bin64, stem)  # noqa: SLF001
        if path is None:
            bad.append("%s: no %sLoader.pyd in %s" % (stem, stem, ctx.client.bin64))
            continue
        bad.extend(layout_disagreements(stem, pyd_schema.schema_for(path, stem), committed))
    if bad:
        raise ContentError("schema guard refused:\n  " + "\n  ".join(bad))


# --- section: universe ---------------------------------------------------------

# The header lines, copied from `schema.md` §2.
SYSTEMS_HEADER = tuple(
    "system_id region_id constellation_id faction_id owner_faction security "
    "security_class hub border fringe corridor regional international "
    "wormhole_class_id disallow_scanning dscan_range sun_type_id name".split())
REGIONS_HEADER = tuple("region_id nebula_id faction_id wormhole_class_id neighbours name".split())
CONSTELLATIONS_HEADER = ("constellation_id",) + tuple(
    "region_id faction_id wormhole_class_id neighbours name".split())
CENTERS_HEADER = ("kind", "id", "x", "y", "z")
SYSTEM_EFFECTS_HEADER = tuple(
    "system_id beacon_type_id sun_type_id sun_item_id x y z".split())
LOCATORS_HEADER = tuple("station_type_id graphic_id graphic_location_id category "
                        "name px py pz dx dy dz hull".split())


def _first(*vals):
    for v in vals:
        if v is not None:
            return v
    return None


def selected_systems(ctx, content):
    """Every system, or the `--systems` sandbox (refusing an unknown id)."""
    if ctx.systems is None:
        return sorted(int(s) for s in content)
    missing = [s for s in ctx.systems if content.get(s) is None]
    if missing:
        raise ContentError("no such solar system: %s" % ",".join(map(str, missing)))
    return list(ctx.systems)


def system_rows(ctx):
    content, systems = ctx.load(SSC), ctx.load(SYSTEMS)
    # [A] `factionsowningsolarsystems.static` is a bare list of faction ids
    # (22 [V]), not a system map, so a system's owner is its own `factionID`
    # when that faction is on the list. Falsifier: a system whose sovereign
    # (as the client's map shows it) differs from `systems.static.factionID`.
    owning = set(int(f) for f in ctx.load(OWNING))
    for sid in selected_systems(ctx, content):
        c, s = content.get(sid), systems.get(sid)
        s_get = s.get if s is not None else (lambda k: None)
        faction, dscan = s_get("factionID"), c.get("dscanRange")
        yield (
            sid, s_get("regionID"), s_get("constellationID"), faction,
            faction if faction in owning else None,
            float(c.get("security")),
            _first(c.get("securityClass"), s_get("securityClass")),
            bool(c.get("hub")), bool(c.get("border")), bool(c.get("fringe")),
            bool(c.get("corridor")), bool(c.get("regional")),
            bool(c.get("international")),
            _first(c.get("wormholeClassID"), s_get("wormholeClassID")),
            bool(c.get("disallowScanning")), None if dscan is None else float(dscan),
            c.get("sunTypeID"), ctx.text(c.get("solarSystemNameID")),
        )


def effect_rows(content, sids):
    """`system-effects.tsv`: one row per system whose `secondarySun` carries an
    effect beacon (r-wormhole §3). Only J-space systems have one **[V-data]**,
    but the rows are read, never computed: the beacon's class need not match
    the system's. A `secondarySun` without a beacon type is skipped."""
    for sid in sids:
        sun = content.get(sid).get("secondarySun")
        if not sun or not sun.get("effectBeaconTypeID"):
            continue
        p = sun.get("position")
        pos = (0.0, 0.0, 0.0) if not p else (float(p["x"]), float(p["y"]), float(p["z"]))
        yield ((sid, int(sun.get("effectBeaconTypeID")), sun.get("typeID"),
                sun.get("itemID")) + pos)


def system_effect_rows(ctx):
    content = ctx.load(SSC)
    return effect_rows(content, selected_systems(ctx, content))


def area_rows(ctx, res, cols):
    """`regions.tsv` and `constellations.tsv`: the key, `cols`, then the
    faction, wormhole class, neighbours and name."""
    table = ctx.load(res)
    for key in sorted(int(k) for k in table):
        r = table.get(key)
        yield ((key,) + tuple(r.get(c) for c in cols)
               + (r.get("factionID"), r.get("wormholeClassID"),
                  [int(x) for x in (r.get("neighbours") or ())], ctx.text(r.get("nameID"))))


def _vec3(c):
    """A `center` as `(x, y, z)` floats: a record read by key (`x`, `y`, `z`)
    or by index, whichever the table's reader gives (`eveCfg.LoadEveLocations`
    reads regions by index and the others by attribute)."""
    if c is None:
        return None
    if hasattr(c, "get"):
        try:
            v = tuple(c.get(k) for k in "xyz")
            if all(a is not None for a in v):
                return tuple(float(a) for a in v)
        except (TypeError, KeyError):
            pass
    return tuple(float(c[i]) for i in range(3))


def center_rows(ctx):
    """`centers.tsv`: kind `s`, `c`, `r`, then the id, then the record's
    `center` in galaxy-frame metres. A `--systems` sandbox keeps the selected
    systems and the constellations and regions that hold them."""
    systems = ctx.load(SYSTEMS)
    consts, regions = ctx.load(CONSTELLATIONS), ctx.load(REGIONS)
    if ctx.systems is None:
        sids = sorted(int(k) for k in systems)
        cids = sorted(int(k) for k in consts)
        rids = sorted(int(k) for k in regions)
    else:
        sids = sorted(int(s) for s in ctx.systems)
        cids = sorted({int(systems.get(s).get("constellationID")) for s in sids})
        rids = sorted({int(systems.get(s).get("regionID")) for s in sids})
    for kind, table, ids in (("s", systems, sids), ("c", consts, cids), ("r", regions, rids)):
        for key in ids:
            v = _vec3(table.get(key).get("center"))
            if v is None:
                raise ContentError("%s %d has no center" % (kind, key))
            yield (kind, key) + v


def celestial_lines(ctx):
    """`celestials.render`, the `--all` form, or the `--systems` sandbox."""
    celestials = _imp("celestials")
    content = ctx.load(SSC)
    sids = selected_systems(ctx, content)
    radii = celestials.type_radii(ctx.client)
    text, _n = celestials.render(content, ctx.load(SYSTEMS), sids, radii,
                                 celestials.gate_systems(content))
    return text.splitlines()


def locator_rows(ctx):
    """`docking.rs`'s regeneration chain for every group-15 type and every structure group (1657, 1404, 1406):
    `types.graphicID -> graphicids.graphicLocationID -> graphiclocations`,
    one row per directional locator."""
    fsd = _imp("fsd")
    c = ctx.client
    types = fsd.cfsd_rows(fsd.CfsdTable(c.path(fsdbin("types")), "types"), "types")
    gids = fsd.rows(c, "graphicids")
    locs = fsd.graphic_locations(
        fsd.CfsdTable(c.path(fsdbin("graphiclocations")), "graphiclocations"))
    for t in sorted(types):
        r = types[t]
        if r.get("groupID") not in LOCATOR_GROUPS or not r.get("graphicID"):
            continue
        g = int(r["graphicID"])
        lid = (gids.get(g) or {}).get("graphicLocationID")
        loc = locs.get(lid) if lid is not None else None
        if loc is None:
            continue
        for d in loc.get("directionalLocators") or ():
            p, v = d["position"], d["direction"]
            yield (t, g, int(lid), d.get("category") or "", d.get("name") or "",
                   float(p[0]), float(p[1]), float(p[2]),
                   float(v[0]), float(v[1]), float(v[2]), loc.get("hull") or "")


def section_universe(ctx):
    return [
        ("universe/systems.tsv", SYSTEMS_HEADER, system_rows(ctx),
         [SYSTEMS, SSC, OWNING, LOC_EN]),
        ("universe/system-effects.tsv", SYSTEM_EFFECTS_HEADER, system_effect_rows(ctx),
         [SSC]),
        ("universe/regions.tsv", REGIONS_HEADER, area_rows(ctx, REGIONS, ("nebulaID",)), [REGIONS, LOC_EN]),
        ("universe/constellations.tsv", CONSTELLATIONS_HEADER, area_rows(ctx, CONSTELLATIONS, ("regionID",)),
         [CONSTELLATIONS, LOC_EN]),
        ("universe/centers.tsv", CENTERS_HEADER, center_rows(ctx),
         [SYSTEMS, CONSTELLATIONS, REGIONS]),
        ("universe/celestials.tsv", None, celestial_lines(ctx),
         [SSC, SYSTEMS, fsdbin("types")]),
        ("universe/dock-locators.tsv", LOCATORS_HEADER, locator_rows(ctx),
         [fsdbin("types"), fsdbin("graphicids"), fsdbin("graphiclocations")]),
    ]


# --- section: belts ------------------------------------------------------------

def rock_rows(ctx):
    """`asteroids.py`'s rows for every belt of the selected systems, sorted
    (`sort_rocks`), with each ore's radius law."""
    asteroids = _imp("asteroids")
    dogma = _imp("dogma")
    counts, content = ctx.load(ROCK_COUNTS), ctx.load(SSC)
    ordinal = {int(b): i for i, b in enumerate(sorted(int(k) for k in counts))}
    rows = []
    for sid in selected_systems(ctx, content):
        for bid in asteroids.belts_of(content.get(sid)):
            if bid not in ordinal:
                continue
            per_type = counts.get(bid)
            for tid in sorted(int(t) for t in per_type):
                rows.append((bid, ordinal[bid], tid, int(per_type.get(tid)), sid))
    laws = asteroids.radius_laws(dogma.Dogma(ctx.client), {r[2] for r in rows})
    return [r + laws[r[2]] for r in sort_rocks(rows)]


def section_belts(ctx):
    asteroids = _imp("asteroids")
    return [("belts/rocks.tsv", asteroids.HEADER, rock_rows(ctx),
             [ROCK_COUNTS, SSC, fsdbin("typedogma"), fsdbin("dogmaattributes")])]


# --- sections: types and planets (the kind-tagged files) -----------------------

def section_types(ctx):
    skills = _imp("skills")
    typedata = _imp("typedata")
    tdata, _counts, _effects = typedata.extract(ctx.client, None)
    sk, _n = skills.extract(ctx.client, None)
    dogma_src = [fsdbin(s) for s in ("types", "groups", "typedogma",
                                    "dogmaattributes", "dogmaeffects")]
    return [
        ("types/typedata.tsv", None, tdata.splitlines(), dogma_src + [LOC_EN]),
        ("types/skills.tsv", None, sk.splitlines(),
         [fsdbin(s) for s in ("types", "groups", "typedogma", "dogmaattributes",
                              "requiredskillsfortypes")] + [LOC_EN]),
    ]


def section_planets(ctx):
    pi_statics = _imp("pi_statics")
    text, _counts = pi_statics.extract(ctx.client, None)
    return [("planets/pi-statics.tsv", None, text.splitlines(),
             [fsdbin(s) for s in ("types", "groups", "typedogma", "dogmaattributes",
                                  "schematics", "globaltypes")] + [LOC_EN])]


# --- checks (client-backed; counts live only here, plan §1.6) ------------------

def _same_bytes(ctx, relpath, tool, argv, keep=None):
    """Run an old tool's `main` into a temp file and compare it with the
    bundle's file byte for byte. `keep(line)` filters the tool's data lines
    for a `--systems` bundle."""
    with tempfile.TemporaryDirectory() as tmp:
        ref = os.path.join(tmp, "ref.tsv")
        rc = tool.main(argv + ctx.resolve_args() + ["--out", ref])
        if rc:
            return ["%s: %s exited %s" % (relpath, tool.__name__, rc)]
        with open(ref, "rb") as fh:
            want = fh.read()
    if keep is not None:
        want = b"".join(ln for ln in want.splitlines(keepends=True)
                        if ln.startswith(b"#") or keep(ln))
    try:
        with open(ctx.path(relpath), "rb") as fh:
            got = fh.read()
    except OSError as e:
        return ["%s: %s" % (relpath, e)]
    if got != want:
        return ["%s: differs from %s's output (%d vs %d bytes)"
                % (relpath, tool.__name__, len(got), len(want))]
    ctx.note("%s: byte-identical to %s (%d bytes)" % (relpath, tool.__name__, len(got)))
    return []


def check_centers(ctx):
    """`centers.tsv` holds one row per system, constellation and region of the
    tables (the sandbox's selection when `--systems`), each with a finite
    position."""
    want = {"s": 0, "c": 0, "r": 0}
    for kind, _id, *_xyz in center_rows(ctx):
        want[kind] += 1
    got = {"s": 0, "c": 0, "r": 0}
    bad = []
    for r in read_rows(ctx.path("universe/centers.tsv")):
        if r[0] not in got:
            bad.append("universe/centers.tsv: unknown kind %r" % r[0])
            continue
        got[r[0]] += 1
        if not all(math.isfinite(float(v)) for v in r[2:5]):
            bad.append("universe/centers.tsv: %s %s has a non-finite position" % (r[0], r[1]))
    if got != want:
        bad.append("universe/centers.tsv: rows %s, the source tables have %s" % (got, want))
    ctx.note("universe/centers.tsv: %d systems, %d constellations, %d regions"
             % (got["s"], got["c"], got["r"]))
    return bad


def check_universe(ctx):
    celestials = _imp("celestials")
    bad = []
    sids = ctx.systems
    argv = ["--all"] if sids is None else ["--systems", ",".join(map(str, sids))]
    bad += _same_bytes(ctx, "universe/celestials.tsv", celestials, argv)
    content, systems = ctx.load(SSC), ctx.load(SYSTEMS)
    rows = list(read_rows(ctx.path("universe/systems.tsv")))
    want = len(systems) if sids is None else len(sids)
    if len(rows) != want:
        bad.append("universe/systems.tsv: %d rows, systems.static has %d" % (len(rows), want))
    off = [r[0] for r in rows
           if float(r[5]) != float(content.get(int(r[0])).get("security"))]
    if off:
        bad.append("universe/systems.tsv: security differs from solarsystemcontent "
                   "for %d systems (first %s)" % (len(off), off[0]))
    ctx.note("universe/systems.tsv: %d rows (systems.static %d), security equal on %d"
             % (len(rows), len(systems), len(rows) - len(off)))
    bad += check_centers(ctx)
    for rel in ("regions.tsv", "constellations.tsv", "dock-locators.tsv",
                "system-effects.tsv"):
        ctx.note("universe/%s: %d rows" % (rel, sum(1 for _ in read_rows(
            ctx.path("universe/" + rel)))))
    return bad


def check_belts(ctx):
    asteroids = _imp("asteroids")
    keep = None
    if ctx.systems is not None:
        want = {str(s).encode() for s in ctx.systems}
        keep = lambda ln: ln.split(b"\t")[4] in want  # noqa: E731
    return _same_bytes(ctx, "belts/rocks.tsv", asteroids, ["--all"], keep)


def check_types(ctx):
    return (_same_bytes(ctx, "types/typedata.tsv", _imp("typedata"), [])
            + _same_bytes(ctx, "types/skills.tsv", _imp("skills"), []))


def check_planets(ctx):
    return _same_bytes(ctx, "planets/pi-statics.tsv", _imp("pi_statics"), [])


#: name -> (section, check, cFSD stems the schema guard covers).
BUILTINS = {
    "universe": (section_universe, check_universe,
                 ("types", "graphicids", "graphiclocations")),
    "belts": (section_belts, check_belts, ("typedogma", "dogmaattributes")),
    "types": (section_types, check_types,
              ("types", "groups", "typedogma", "dogmaattributes", "dogmaeffects")),
    "planets": (section_planets, check_planets,
                ("types", "groups", "typedogma", "dogmaattributes", "schematics",
                 "globaltypes")),
}


# --- the write ------------------------------------------------------------------

def write_file(ctx, relpath, header, rows):
    """Write one bundle file; return `(data rows, sha256)`."""
    path = ctx.path(relpath)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    n = 0
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        if header is not None:
            fh.write("#" + "\t".join(header) + "\n")
        for row in rows:
            line = row if isinstance(row, str) else encode_row(row)
            if is_data(line):
                n += 1
            fh.write(line + "\n")
    return n, sha256_file(path)


def client_build(args):
    dogma = _imp("dogma")
    root = args.client or os.environ.get("EVE_CLIENT")
    build = dogma.install_build(root) if root else None
    if not build:
        raise ContentError("no client build: %s has no readable start.ini"
                           % (root or "(no --client)"))
    return build


def data_hash(files):
    """The bundle's `data_hash` (`schema.md` §1.2): sha256 over one line
    ``relpath<TAB>sha256<LF>`` per `file` record, sorted by relpath.

    Sorted, not manifest order, so a `--sections` merge (which appends new
    files) and a full run that wrote the same files agree. The file digests
    already cover every byte, so this is a hash of the data and nothing
    else: no build, extractor or source md5 goes in."""
    h = hashlib.sha256()
    for rel, digest in sorted((f[0], f[2]) for f in files):
        h.update(("%s\t%s\n" % (rel, digest)).encode("utf-8"))
    return h.hexdigest()


def manifest_text(build, extractor, systems, files, inputs, sources):
    lines = [MANIFEST_BANNER, "format\t" + FORMAT, "client_build\t" + build,
             "extractor\t" + extractor,
             "systems\t" + ("all" if systems is None else ",".join(map(str, systems))),
             "data_hash\t" + data_hash(files)]
    for rel, n, digest, srcs in files:
        lines.append("file\t%s\t%d\t%s\t%s" % (rel, n, digest, ",".join(srcs)))
    for rel in sorted(inputs):
        lines.append("input\t%s\t%s" % (rel, inputs[rel]))
    for res in sorted(sources):
        lines.append("source\t%s\t%s" % (res, sources[res]))
    return "\n".join(lines) + "\n"


def _sweep_stale(out):
    """Remove `<out>.tmp-<pid>` left by a run that was killed outright."""
    for d in glob.glob(glob.escape(out) + ".tmp-*"):
        pid = d.rsplit("-", 1)[-1]
        if not pid.isdigit():
            continue
        try:
            if int(pid) != os.getpid():
                os.kill(int(pid), 0)
                continue  # a live run's directory
        except ProcessLookupError:
            pass
        except PermissionError:
            continue
        shutil.rmtree(d, ignore_errors=True)


def read_records(out):
    """`(inputs, sources)` of a bundle's manifest, each `{key: digest}`."""
    inputs, sources = {}, {}
    with open(os.path.join(out, MANIFEST), "r", encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if is_data(line.rstrip("\n")) and len(f) == 3:
                if f[0] == "input":
                    inputs[f[1]] = f[2]
                elif f[0] == "source":
                    sources[f[1]] = f[2]
    return inputs, sources


def merge_base(out, build, systems):
    """The bundle a `--sections` run merges into, or `None` to write afresh.

    `None` when `out` has no manifest. Refuses (`ContentError`) a bundle that
    fails `check_manifest`, or whose `format` MAJOR, `client_build` or
    `systems` differ from this run's: its files and the new ones would not
    describe one client. A MINOR difference merges, and the result is
    written in this run's format (a 1.0 bundle gains its `data_hash`).
    Returns `(single, files, inputs, sources)` otherwise.

    Coherence with `up.sh`'s staleness check: the merged manifest keeps the
    OLD `extractor` hash, and on an `input` or `source` both runs recorded
    the old digest wins. So a merged bundle is never marked fresher than its
    oldest file: if the extractor (or an input) changed since the kept files
    were written, `up.sh` still sees the bundle as stale and re-extracts it
    all, and when nothing changed the old values equal the new ones.

    A file the old bundle listed and this run did not write is kept, even
    if it belonged to a re-run section that no longer writes it (the
    manifest records no section per file); a full run drops it."""
    if not os.path.isfile(os.path.join(out, MANIFEST)):
        return None
    bad = check_manifest(out)
    if bad:
        raise ContentError("cannot merge --sections into %s, it fails --check "
                           "(%s); re-extract with no --sections" % (out, bad[0]))
    single, files = read_manifest(out)
    want = {"format": FORMAT.split(".")[0], "client_build": build,
            "systems": "all" if systems is None else ",".join(map(str, systems))}
    have = dict(single, format=single.get("format", "").split(".")[0])
    for k, v in want.items():
        if have.get(k) != v:
            raise ContentError("cannot merge --sections into %s: its %s is %r, this "
                               "run's is %r; re-extract with no --sections"
                               % (out, k, single.get(k), v))
    inputs, sources = read_records(out)
    return single, files, inputs, sources


def merge_manifest(base, files, inputs, sources):
    """`(extractor, files, inputs, sources)` of `base` updated by this run's
    records (`merge_base` says which side wins where)."""
    single, old_files, old_inputs, old_sources = base
    new = {f[0]: f for f in files}
    merged = [new.pop(f[0], f) for f in old_files]
    merged.extend(f for f in files if f[0] in new)
    return (single["extractor"], merged, {**inputs, **old_inputs},
            {**sources, **old_sources})


def extract(args, sections, merge=False):
    """Write the bundle. `merge` (a `--sections` run) keeps an existing
    bundle's other files: see `merge_base`."""
    out = os.path.abspath(args.out)
    build = client_build(args)
    parent = os.path.dirname(out)
    os.makedirs(parent, exist_ok=True)
    _sweep_stale(out)
    base = merge_base(out, build, args.systems_list) if merge else None
    tmp = "%s.tmp-%d" % (out, os.getpid())
    os.makedirs(tmp)
    try:
        if base is not None:
            # A copy, not hard links: `write_file` truncates in place, and a
            # failed run must leave the old bundle as it was.
            shutil.copytree(out, tmp, dirs_exist_ok=True,
                            ignore=lambda d, names: [MANIFEST] if d == out else [])
        ctx = Ctx(args, tmp)
        files, sources = [], {}
        for s in sections:
            schema_guard(ctx, s.cfsd)
            for spec in s.section(ctx):
                rel, header, rows = spec[:3]
                srcs = list(spec[3]) if len(spec) > 3 else []
                n, digest = write_file(ctx, rel, header, rows)
                files.append((rel, n, digest, srcs))
                for res in srcs:
                    sources[res] = ctx.md5(res)
                ctx.note("%s: %d rows" % (rel, n))
        extractor, listed, inputs = extractor_hash(), files, ctx.inputs
        if base is not None:
            extractor, listed, inputs, sources = merge_manifest(base, files, inputs,
                                                                sources)
            ctx.note("merged %d files into the existing bundle (%d listed)"
                     % (len(files), len(listed)))
        text = manifest_text(build, extractor, args.systems_list, listed,
                             inputs, sources)
        with open(os.path.join(tmp, MANIFEST), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        if os.path.exists(out):
            old = "%s.old-%d" % (out, os.getpid())
            os.rename(out, old)
            os.rename(tmp, out)
            shutil.rmtree(old, ignore_errors=True)
        else:
            os.rename(tmp, out)
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    return files


def read_manifest(out):
    """`(singletons, files)` of a bundle's manifest; `files` is
    `[(relpath, rows, sha256, sources)]`."""
    single, files = {}, []
    with open(os.path.join(out, MANIFEST), "r", encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if not is_data(line.rstrip("\n")):
                continue
            if f[0] == "file":
                files.append((f[1], int(f[2]), f[3], decode_list(f[4], str)))
            elif f[0] in ("format", "client_build", "extractor", "systems", "data_hash"):
                single[f[0]] = f[1]
    return single, files


def check_manifest(out):
    """Every listed file exists with its row count and sha256."""
    bad = []
    try:
        single, files = read_manifest(out)
    except OSError as e:
        return ["%s: %s" % (MANIFEST, e)]
    if single.get("format", "").split(".")[0] != FORMAT.split(".")[0]:
        bad.append("%s: format %r" % (MANIFEST, single.get("format")))
    for rel, n, digest, _srcs in files:
        path = os.path.join(out, rel)
        if not os.path.isfile(path):
            bad.append("%s: listed, missing" % rel)
            continue
        with open(path, "r", encoding="utf-8") as fh:
            got = sum(1 for ln in fh if is_data(ln.rstrip("\n")))
        if got != n:
            bad.append("%s: %d data rows, manifest says %d" % (rel, got, n))
        if sha256_file(path) != digest:
            bad.append("%s: sha256 differs from the manifest" % rel)
    # Format 1.1: the data_hash must agree with the file records it covers.
    # A 1.0 bundle has none, and that is not a failure.
    if "data_hash" in single and single["data_hash"] != data_hash(files):
        bad.append("%s: data_hash does not match its file records" % MANIFEST)
    return bad


def bundle_data_hash(out):
    """A bundle's data_hash: its manifest's, or computed from its file
    records for a 1.0 bundle that has none."""
    single, files = read_manifest(out)
    return single.get("data_hash") or data_hash(files)


def run_check(args, sections):
    out = os.path.abspath(args.out)
    bad = check_manifest(out)
    if not bad and getattr(args, "against", None):
        # Two extractions (e.g. from two builds) hold the same data exactly
        # when their data_hashes agree; both must pass check_manifest first,
        # or the hashes describe files that are not there.
        other = os.path.abspath(args.against)
        obad = check_manifest(other)
        bad.extend("--against %s" % b for b in obad)
        if not obad:
            mine, theirs = bundle_data_hash(out), bundle_data_hash(other)
            if mine != theirs:
                bad.append("data_hash %s differs from %s's %s (what differs: "
                           "python3 -I tools/bundle-diff.py %s %s)" % (mine, other, theirs, other, out))
            else:
                print("content: data_hash %s equals %s's" % (mine, other), file=sys.stderr)
    if not bad:
        single, _files = read_manifest(out)
        sysf = single.get("systems", "all")
        args.systems_list = None if sysf == "all" else decode_list(sysf)
        ctx = Ctx(args, out)
        for s in sections:
            bad.extend(s.check(ctx))
    for b in bad:
        print("content: CHECK FAILED: " + b, file=sys.stderr)
    print("content: check %s (%d failures)" % ("ok" if not bad else "FAILED", len(bad)),
          file=sys.stderr)
    return 1 if bad else 0


def parse_args(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--client", default=os.environ.get("EVE_CLIENT"),
                    help="client install dir (default $EVE_CLIENT)")
    ap.add_argument("--resfiles", help="ResFiles dir (default: sibling of --client)")
    ap.add_argument("--out", default=os.path.join(_HERE, os.pardir, "out", "content"),
                    help="the bundle directory (default out/content)")
    ap.add_argument("--sections", help="comma list (default: every section); "
                    "each section's DEPENDS are added. Merged into an existing "
                    "bundle at --out: other sections' files are kept")
    ap.add_argument("--systems", help="comma list of solar systems: restricts "
                    "universe and belts to a sandbox")
    ap.add_argument("--check", action="store_true",
                    help="check the bundle at --out instead of extracting")
    ap.add_argument("--against", metavar="BUNDLE",
                    help="with --check: also prove BUNDLE's data_hash equals --out's")
    ap.add_argument("--build", metavar="N",
                    help="the build the install must be, or 'any' (default: any "
                         "build tools/builds.tsv marks pinned or equivalent)")
    a = ap.parse_args(argv)
    if a.against and not a.check:
        ap.error("--against needs --check")
    a.systems_list = (sorted({int(x) for x in a.systems.split(",") if x.strip()})
                      if a.systems else None)
    return a


def main(argv=None, section_dir=_HERE):
    args = parse_args(argv)
    try:
        sections = discover(section_dir)
        wanted = [x for x in args.sections.split(",") if x] if args.sections else None
        if args.check:
            names = set(wanted) if wanted else None
            picked = [s for s in order(sections, wanted)
                      if names is None or s.name in names]
            return run_check(args, picked)
        files = extract(args, order(sections, wanted), merge=wanted is not None)
    except ContentError as e:
        print("content: %s" % e, file=sys.stderr)
        return 2
    print("content: %d files -> %s" % (len(files), os.path.abspath(args.out)),
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
