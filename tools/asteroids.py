#!/usr/bin/env python3
"""Extract what rocks are in a solar system's asteroid belts, from the client.

    tools/analysis.sh python3 /work/tools/asteroids.py \
        --client /client --resfiles /client-root/ResFiles \
        --system 30000001 --out /work/out/belt-rocks-30000001.tsv

The server reads the result with `--belt-rocks FILE` (beside `--celestials`,
which supplies where each belt *is*) and puts one ball per rock into the
ballpark. `docs/lanes/asteroids.md` is the spec; `crate::asteroid` is the
reader and the layout.

**Why a tool and not a table in the source** is the same answer as
`tools/celestials.py`: the counts are CCP's data, and no extracted client data
goes in the repository. What ships is this reader; `out/` is gitignored.

What it reads, all [V] in build 3619879:

- `res:/staticdata/asteroidcountbytypeidbeltid.static` -- `{beltID:
  {oreTypeID: rockCount}}` for 40,928 belts. The client reads the same file
  to tell a pilot how depleted a belt is (`eveuniverse/client/asteroids.py`).
  It has counts, **not positions**; the layout is the server's, and [A].
- `res:/staticdata/solarsystemcontent.static` -- which belts are in the
  system, so `--system` can pick them.
- the dogma tables -- each ore type's radius law: `asteroidRadiusGrowthFactor`
  (1980), `asteroidRadiusUnitSize` (1981) and `asteroidMaxRadius` (2727), each
  falling back to the attribute's own `defaultValue` when the type does not
  carry it (`ballparkCommon/ballRadius.py`). Emitted per row so the server
  applies the client's law with the client's numbers, not ours.

The `ordinal` column is the belt's index in the **sorted key list of the whole
count table**, not of this system's belts: rock item ids are
`70,000,000 + ordinal * 200 + n`, and a global ordinal keeps them unique
across systems should the server ever serve more than one.

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
import dogma  # noqa: E402
import fsd  # noqa: E402

COUNTS = "res:/staticdata/asteroidcountbytypeidbeltid.static"
CONTENT = "res:/staticdata/solarsystemcontent.static"

# `ballRadius.py`'s three attributes, by id. [V]
GROWTH = 1980
UNIT = 1981
MAX_RADIUS = 2727

HEADER = (
    "belt_id",
    "ordinal",
    "type_id",
    "count",
    "system",
    "growth",
    "unit_size",
    "max_radius",
)


def belts_of(system):
    """Every asteroid-belt id in one system's content row."""
    out = []
    for _pid, planet in sorted((system.get("planets") or {}).items()):
        for bid, _belt in sorted((planet.get("asteroidBelts") or {}).items()):
            out.append(int(bid))
    return out


def radius_laws(d, type_ids):
    """`{typeID: (growth, unitSize, maxRadius)}`, defaults applied.

    One pass over `typedogma` rather than `Dogma.of_type` per type, which
    rescans the whole table each call.
    """
    attrs = d.attributes
    default = {a: float(attrs[a].get("defaultValue") or 0.0) for a in (GROWTH, UNIT, MAX_RADIUS)}
    out = {t: dict(default) for t in type_ids}
    for key, row in d.table("typedogma").rows():
        if key not in out:
            continue
        for e in row["dogmaAttributes"]:
            if e["attributeID"] in default:
                out[key][e["attributeID"]] = float(e["value"])
    return {t: (v[GROWTH], v[UNIT], v[MAX_RADIUS]) for t, v in out.items()}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--client", help="client install dir (default $EVE_CLIENT)")
    ap.add_argument("--resfiles", help="ResFiles dir (default: sibling of --client)")
    ap.add_argument("--system", type=int, default=30000001, help="solar system id")
    ap.add_argument("--all", action="store_true", help="every solar system")
    ap.add_argument("--out", help="write here instead of stdout")
    a = ap.parse_args(argv)

    client = fsd.Client(a.client, a.resfiles)
    counts = client.load(COUNTS)
    content = client.load(CONTENT)
    ordinal = {int(b): i for i, b in enumerate(sorted(int(k) for k in counts))}

    systems = sorted(int(s) for s in content) if a.all else [a.system]
    picked = []  # (system, belt)
    for sid in systems:
        system = content.get(sid)
        if system is None:
            if not a.all:
                sys.stderr.write("no such solar system: %d\n" % sid)
                return 2
            continue
        for bid in belts_of(system):
            if bid in ordinal:
                picked.append((sid, bid))

    rows = []
    for sid, bid in picked:
        per_type = counts.get(bid)
        for tid in sorted(int(t) for t in per_type):
            rows.append((bid, ordinal[bid], tid, int(per_type.get(tid)), sid))

    laws = radius_laws(dogma.Dogma(client), {r[2] for r in rows})

    lines = []
    if not a.all:
        lines.append("# solarSystemID\t%d" % a.system)
    lines.append("#" + "\t".join(HEADER))
    for bid, o, tid, n, sid in rows:
        g, u, m = laws[tid]
        lines.append("%d\t%d\t%d\t%d\t%d\t%r\t%r\t%r" % (bid, o, tid, n, sid, g, u, m))
    text = "\n".join(lines) + "\n"

    if a.out:
        d = os.path.dirname(os.path.abspath(a.out))
        if d and not os.path.isdir(d):
            os.makedirs(d)
        with open(a.out, "w", encoding="utf-8") as fh:
            fh.write(text)
        sys.stderr.write(
            "%d rocks in %d belts -> %s\n" % (sum(r[3] for r in rows), len(picked), a.out)
        )
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
