#!/usr/bin/env python3
"""Extract solar systems' celestials from the client's own static data.

    tools/analysis.sh python3 /work/tools/celestials.py \
        --client /client --resfiles /client-root/ResFiles \
        --systems 30000001,30000003 --out /work/out/celestials-sandbox.tsv

**Why this is a tool and not a table in the source.** The positions are CCP's
data, and this project's standing rule is that no extracted client data goes
in the repository -- so the one option not available is the easy one, baking
them into a `.rs` file beside `destiny::hull::HULLS`. What ships is this
reader; the data stays on the disk of whoever legally obtained the client.
`out/` is gitignored, which is why the output goes there.

The server reads the result with `--celestials FILE` and puts what it can
serve into the ballpark. That is the point of the exercise: a celestial the
client cannot see as a ball is a celestial nobody can right-click, which is
why "warp to a planet" did nothing while "warp to the station" worked -- the
station was the only celestial we had ever put in the park.

**Faithful, not filtered.** Every celestial in the system is emitted,
including the ones the server currently declines to serve (the star, which
would sit inside our station at the system origin). Deciding what to serve is
the server's business and belongs in one place; this file's job is to be a
complete extract.

## The format (P3.3, `docs/lanes/stargates.md` §3.2-3.3)

Ten tab-separated columns, `kind id type_id radius x y z system dest
dest_system`, and
one structured comment per system, `#system <id> <constellationID>
<regionID>`. Both additions are **backward compatible by construction**: the
two new columns come *after* the seven every reader already takes, and the
new lines start with `#`, which every reader already skips.
`celestial::Catalogue`'s tests pin a 7-, an 8- and a 9-column file.

- `dest` is a gate's partner gate (`solarsystemcontent.static`'s
  `stargates[gid]['destination']`) and `0` for every other kind, and
  `dest_system` the partner's own solar system. The server needs both for
  the gate's slim `jumps = [(toCelestialID, locationID)]` row -- for *every*
  gate, including the ones whose partner is not in the file -- and for the
  jump itself.
- A gate's **radius comes from its type** (`types.fsdbinary`), because the
  content table carries none -- every gate came out `0.0` before -- and the
  universe has 29 gate types, so a table of three in Rust would be wrong on
  the first gate outside Derelik.

Read-only on the client: it opens the static data and writes one TSV.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import os
import sys

sys.path.append(os.path.dirname(os.path.abspath(__file__)))
import fsd  # noqa: E402

CONTENT = "res:/staticdata/solarsystemcontent.static"
SYSTEMS = "res:/staticdata/systems.static"
TYPES = "res:/staticdata/types.fsdbinary"

# The columns, in order. `kind` is first because it is what a reader scans
# for, and the server's policy is expressed in terms of it.
HEADER = (
    "kind",
    "id",
    "type_id",
    "radius",
    "x",
    "y",
    "z",
    "system",
    "dest",
    "dest_system",
)


def position(obj):
    """A celestial's position, or the system centre for something that has
    none.

    The **star** has no `position` field: it is the system's origin, which is
    what every other position in the file is measured from.
    """
    p = obj.get("position")
    if not p:
        return (0.0, 0.0, 0.0)
    return (float(p["x"]), float(p["y"]), float(p["z"]))


def rows_for(system, system_id, type_radius, gate_system):
    """Every celestial in one system, as `HEADER` tuples.

    Every NPC station is emitted, wherever the content table lists it: under
    the star, under a planet or under a moon. A station row follows its
    parent's row (the star's, the moon's, or, after the planet's moons and
    belts, the planet's), so the order is still one pass over the tree.

    `type_radius(typeID)` answers a type's own radius and
    `gate_system(gateID)` a gate's solar system, both for the gates.
    """
    out = []

    def emit(kind, cid, obj, radius=None, dest=0, dest_system=0):
        if radius is None:
            radius = obj.get("radius") or 0
        x, y, z = position(obj)
        out.append(
            (
                kind,
                int(cid),
                int(obj.get("typeID")),
                float(radius),
                x,
                y,
                z,
                int(system_id),
                int(dest),
                int(dest_system),
            )
        )

    def stations(parent):
        # A station's position is its own, in system coordinates, whatever
        # it orbits; the parent only decides where in the file it lands.
        for sid, station in sorted((parent.get("npcStations") or {}).items()):
            emit("station", sid, station, radius=station.get("radius") or 0)

    star = system.get("star")
    if star:
        out.append(
            (
                "star",
                int(star.get("id")),
                int(star.get("typeID")),
                float(star.get("radius") or 0),
                0.0,
                0.0,
                0.0,
                int(system_id),
                0,
                0,
            )
        )
        # **Stations under the star** (ruling 6). The content table can list
        # a station there rather than under a planet or a moon; it is
        # emitted right after the star, its parent.
        stations(star)
    for pid, planet in sorted((system.get("planets") or {}).items()):
        emit("planet", pid, planet)
        for mid, moon in sorted((planet.get("moons") or {}).items()):
            emit("moon", mid, moon)
            # **Stations under a moon** (C4 §1, §11.5). This extractor used
            # to keep only the planet-orbit stations and dropped these. Each follows its moon, so a system with no
            # moon station is emitted exactly as before.
            stations(moon)
        # Asteroid belts carry no radius of their own -- they are a field of
        # rocks rather than a body -- so they get zero and the server's own
        # standoff does the work.
        for bid, belt in sorted((planet.get("asteroidBelts") or {}).items()):
            emit("belt", bid, belt, radius=0)
        stations(planet)
    # **A gate's radius is its type's**: the content table has none, and a
    # gate ball of radius 0 is a bracket the warp standoff and the client's
    # 2,500 m jump range both measure from the wrong surface.
    for gid, gate in sorted((system.get("stargates") or {}).items()):
        emit(
            "gate",
            gid,
            gate,
            radius=gate.get("radius") or type_radius(gate.get("typeID")) or 0,
            dest=gate.get("destination") or 0,
            dest_system=gate_system(gate.get("destination")) or 0,
        )
    return out


def gate_systems(content):
    """`{gateID: solarSystemID}` for every gate in the universe.

    A gate's `destination` names the partner *gate*; its system is wherever
    that gate is listed. One pass over all 8,490 systems, which is what
    `--all` already pays.
    """
    out = {}
    for sid in content:
        system = content.get(sid)
        if system is None:
            continue
        for gid in system.get("stargates") or {}:
            out[int(gid)] = int(sid)
    return out


def type_radii(client):
    """`{typeID: radius}` out of `types.fsdbinary`, the recipe
    `tools/fsd_dump.py` uses for `rows`."""
    t = fsd.CfsdTable(client.path(TYPES), TYPES.split("/")[-1])
    rows = fsd.cfsd_rows(t)
    return {tid: (r.get("radius") or 0.0) for tid, r in rows.items()}


def render(content, systems, sids, radii, where):
    """The whole file for these systems, as text, and the row count."""
    lines = ["#" + "\t".join(HEADER)]
    rows = []
    for sid in sids:
        system = content.get(sid)
        if system is None:
            continue
        info = systems.get(sid) if systems is not None else None
        constellation = (info.get("constellationID") if info else None) or 0
        region = (info.get("regionID") if info else None) or 0
        # **One structured comment per system.** It starts with `#`, so every
        # reader that predates it skips it; `celestial::Catalogue` reads it
        # for the jump's session change (`constellationid`/`regionid`).
        lines.append("#system\t%d\t%d\t%d" % (int(sid), int(constellation), int(region)))
        rows.extend(rows_for(system, sid, radii.get, where.get))
    for r in rows:
        lines.append("%s\t%d\t%d\t%.1f\t%.1f\t%.1f\t%.1f\t%d\t%d\t%d" % r)
    return "\n".join(lines) + "\n", len(rows)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--client", help="client install dir (default $EVE_CLIENT)")
    ap.add_argument("--resfiles", help="ResFiles dir (default: sibling of --client)")
    ap.add_argument(
        "--system", type=int, default=30000001, help="one solar system id"
    )
    # **Several systems, in one file**, for the jump (P3.3): a gate's partner
    # is only "loaded" when its row is in the same catalogue.
    ap.add_argument(
        "--systems",
        help="comma-separated solar system ids (overrides --system)",
    )
    # **Every system, in one file.** The server was a one-system sandbox for
    # as long as this tool could only answer one `--system`, and the data was
    # never the reason: `solarsystemcontent.static` holds all 8,490 of them,
    # for *this* build. The owner asked "don't we have this data already?" --
    # we did, and only the extractor was in the way.
    ap.add_argument(
        "--all",
        action="store_true",
        help="every solar system",
    )
    ap.add_argument("--out", help="write here instead of stdout")
    a = ap.parse_args(argv)

    client = fsd.Client(a.client, a.resfiles)
    content = client.load(CONTENT)
    systems = client.load(SYSTEMS)
    radii = type_radii(client)

    if a.all:
        sids = sorted(content)
    elif a.systems:
        sids = [int(x) for x in a.systems.split(",") if x.strip()]
    else:
        sids = [a.system]
    missing = [sid for sid in sids if content.get(sid) is None]
    if missing and not a.all:
        sys.stderr.write("no such solar system: %s\n" % ",".join(map(str, missing)))
        return 2
    text, n = render(content, systems, sids, radii, gate_systems(content))

    if a.out:
        d = os.path.dirname(os.path.abspath(a.out))
        if d and not os.path.isdir(d):
            os.makedirs(d)
        with open(a.out, "w", encoding="utf-8") as fh:
            fh.write(text)
        sys.stderr.write("%d celestials in %d systems -> %s\n" % (n, len(sids), a.out))
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
