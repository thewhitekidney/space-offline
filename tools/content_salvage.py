"""The `salvage` content section: what a salvaged wreck yields.

Packet X-SALV (docs/lanes/x-salv.md). The client ships **no** table of what
a wreck holds for a salvager [V: the wreck types carry four attributes and
none of them is a yield; `findings/user-errors.tsv` has the salvaging
messages and nothing that says what comes out], so this table is ours [A],
built **by class** from the bundle, never as hand-written type ids. Every
header is `docs/content/schema.md`'s. Writes two bundle files:

    salvage/wrecks.tsv   one row per wreck type some hull leaves: its size
                         class and the race (or faction) of those hulls
    salvage/yields.tsv   one row per (size, race, salvage type): the weight
                         and quantity range of one draw

It reads earlier sections' files from the run's own bundle:
`types/market.tsv` (CP12: group, category, race, faction, `wreck_type_id`,
`basePrice`) and `econ/prices.tsv` (CP24: `P_ref`).

**Size** [A] (the groups are [V] `inventorycommon/const`; which group is
which size is the extractor's `SIZE_OF_GROUP`): 1 for frigates and
destroyers, 2 for cruisers, battlecruisers and industrials, 3 for
battleships and capitals. A wreck type that several hulls share takes the
largest size among them; a hull in no listed group counts as `DEFAULT_SIZE`.

**Race** is the hulls' `race_id`, else `faction_id`, else 0; when the hulls
of a wreck type disagree it is 0 (any).

**Yields** are the published salvage (group 754: T1 salvage and the T2 rig
salvage) in three value bands (the cheapest third, the middle, the dearest),
`P_ref` else `basePrice`. A size draws the bands by `BAND_WEIGHTS[size]` and
a band's quantity is `BAND_QTY[band]` [A]. A race favours every
`FAVOUR_MOD`-th type of the value order, starting at its own rank, by
`FAVOUR_BOOST` [A]: a faction's wrecks lean to its own kind of salvage, as
TQ's do, without the extractor knowing which. Race 0 has no favourites, and
is the fallback for a race with no rows of its own.

The derivation is pure (plain dicts in, rows out), so the unit tests feed it
invented rows; `gather(ctx)` is the only bundle reader.
"""

import os
import sys

SECTION = "salvage"
DEPENDS = ("types", "types2", "econ")

_HERE = os.path.dirname(os.path.abspath(__file__))

# The header lines, copied from `schema.md`.
WRECKS_HEADER = tuple("wreck_type_id size race_id".split())
YIELDS_HEADER = tuple("size race_id type_id weight qty_min qty_max value".split())

#: `groupSalvagedMaterials` [V `inventorycommon/const`].
GROUP_SALVAGE = 754
#: `categoryShip` [V].
CATEGORY_SHIP = 6

#: [A] a hull group's size class; the groups are [V], the sizes ours.
#: Falsified by a wreck that holds salvage of the wrong tier for its hull.
SIZE_OF_GROUP = {
    # 1: frigate and destroyer lines, shuttles, rookie ships
    25: 1, 31: 1, 237: 1, 324: 1, 420: 1, 541: 1, 830: 1, 831: 1, 834: 1,
    893: 1, 1283: 1, 1305: 1, 1527: 1, 1534: 1,
    # 2: cruiser and battlecruiser lines, industrials
    26: 2, 28: 2, 358: 2, 380: 2, 419: 2, 463: 2, 540: 2, 543: 2, 832: 2,
    833: 2, 894: 2, 906: 2, 941: 2, 963: 2, 1201: 2, 1202: 2,
    # 3: battleships and everything capital
    27: 3, 30: 3, 485: 3, 513: 3, 547: 3, 659: 3, 883: 3, 898: 3, 900: 3,
    902: 3, 1538: 3, 4594: 3,
}
#: [A] a hull in no listed group.
DEFAULT_SIZE = 2
SIZES = (1, 2, 3)

#: [A] per size, the weights of the (cheap, middle, dear) value bands.
#: Falsified by frigate wrecks that pay the dear salvage as often as a
#: battleship's, or a battleship's that never do.
BAND_WEIGHTS = {1: (70, 25, 5), 2: (25, 55, 20), 3: (5, 30, 65)}
#: [A] per band, one draw's quantity range.
BAND_QTY = ((1, 4), (1, 3), (1, 2))
#: [A] a race favours every `FAVOUR_MOD`-th salvage type (in value order,
#: from its own rank) at `FAVOUR_BOOST` times the weight.
FAVOUR_MOD = 4
FAVOUR_BOOST = 3


def value_of(t, market, prices):
    """`P_ref`, else `basePrice`, else 0."""
    if t in prices:
        return prices[t]
    return float(market.get(t, {}).get("base_price") or 0.0)


def salvage_types(market, prices):
    """Published salvage, ascending by `(value, type_id)`."""
    got = [t for t, m in market.items() if m["published"] and m["group_id"] == GROUP_SALVAGE]
    return sorted(got, key=lambda t: (value_of(t, market, prices), t))


def bands(ordered):
    """`ordered` in thirds, cheapest first; with fewer than three types the
    cheap bands come out empty."""
    n = len(ordered)
    lo, hi = n // 3, 2 * n // 3
    return [ordered[:lo], ordered[lo:hi], ordered[hi:]]


def hull_race(row):
    return row.get("race_id") or row.get("faction_id") or 0


def derive_wrecks(market):
    """The rows of `salvage/wrecks.tsv`, ascending by wreck type."""
    by = {}
    for t, m in market.items():
        w = m.get("wreck_type_id")
        if m["category_id"] != CATEGORY_SHIP or not w:
            continue
        by.setdefault(w, []).append(m)
    rows = []
    for w, hulls in sorted(by.items()):
        size = max(SIZE_OF_GROUP.get(h["group_id"], DEFAULT_SIZE) for h in hulls)
        races = {hull_race(h) for h in hulls}
        rows.append((w, size, races.pop() if len(races) == 1 else 0))
    return rows


def derive_yields(market, prices, wrecks):
    """The rows of `salvage/yields.tsv`: race 0 for every size, and each
    race some wreck row names, ascending by `(size, race, type_id)`."""
    ordered = salvage_types(market, prices)
    split = bands(ordered)
    races = sorted({r for _, _, r in wrecks if r})
    rows = []
    for size in SIZES:
        for race in [0] + races:
            rank = races.index(race) if race else None
            for b, members in enumerate(split):
                lo, hi = BAND_QTY[b]
                for t in members:
                    weight = BAND_WEIGHTS[size][b]
                    pos = ordered.index(t)
                    if rank is not None and pos % FAVOUR_MOD == rank % FAVOUR_MOD:
                        weight *= FAVOUR_BOOST
                    rows.append((size, race, t, weight, lo, hi, float(value_of(t, market, prices))))
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    return rows


# --- the bundle readers ---------------------------------------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _opt(v):
    return int(v) if v else None


def read_market(path):
    return {int(f[0]): {"group_id": int(f[1]), "category_id": _opt(f[2]),
                        "base_price": float(f[4]) if f[4] else 0.0,
                        "faction_id": _opt(f[9]), "race_id": _opt(f[10]),
                        "wreck_type_id": _opt(f[11]), "published": f[13] == "1"}
            for f in _content().read_rows(path)}


def read_prices(path):
    return {int(f[0]): float(f[1]) for f in _content().read_rows(path)}


def gather(ctx):
    if "salvage" in ctx._cache:  # noqa: SLF001
        return ctx._cache["salvage"]  # noqa: SLF001
    got = (read_market(ctx.path("types/market.tsv")), read_prices(ctx.path("econ/prices.tsv")))
    ctx._cache["salvage"] = got  # noqa: SLF001
    return got


def derive(market, prices):
    wrecks = derive_wrecks(market)
    return wrecks, derive_yields(market, prices, wrecks)


def section(ctx):
    wrecks, yields = derive(*gather(ctx))
    ctx.note("salvage: %d wreck types, %d yield rows over %d salvage types"
             % (len(wrecks), len(yields), len({r[2] for r in yields})))
    c = _content()
    return [("salvage/wrecks.tsv", WRECKS_HEADER, wrecks, [c.fsdbin("types")]),
            ("salvage/yields.tsv", YIELDS_HEADER, yields, [c.fsdbin("types")])]


# --- the bundle-backed check (counts live only here, plan §1.6) -------------------

def check(ctx):
    content = _content()
    market, prices = gather(ctx)
    wrecks, yields = derive(market, prices)
    bad = []
    for name, rows in (("wrecks", wrecks), ("yields", yields)):
        got = list(content.read_rows(ctx.path("salvage/%s.tsv" % name)))
        if got != [content.encode_row(r).split("\t") for r in rows]:
            bad.append("salvage/%s.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (name, len(got), len(rows)))
    if not wrecks:
        bad.append("salvage/wrecks.tsv: empty")
    for size in SIZES:
        if not any(r[0] == size and r[1] == 0 for r in yields):
            bad.append("salvage/yields.tsv: no race-0 rows for size %d" % size)
    # Every resource feeds manufacturing: each published rig blueprint takes
    # salvage, and each salvage type a rig recipe takes can be salvaged.
    used = set()
    rigs = missing = 0
    bp = ctx.path("industry/blueprints.tsv")
    if os.path.exists(bp):
        mats = {}
        products = {}
        for f in content.read_rows(bp):
            if f[1] != "1":
                continue
            if f[2] == "material":
                mats.setdefault(int(f[0]), []).append(int(f[3]))
            elif f[2] == "product":
                products.setdefault(int(f[0]), int(f[3]))
        for b, p in sorted(products.items()):
            m = market.get(p)
            if not m or not m["published"] or m["category_id"] != 7 or not _is_rig(ctx, p):
                continue
            rigs += 1
            salv = [x for x in mats.get(b, []) if market.get(x, {}).get("group_id") == GROUP_SALVAGE]
            if not salv:
                missing += 1
            used.update(salv)
    yielded = {r[2] for r in yields}
    lost = sorted(used - yielded)
    if lost:
        bad.append("salvage/yields.tsv: %d salvage types that rigs take are never yielded "
                   "(first %d)" % (len(lost), lost[0]))
    if missing:
        bad.append("industry/blueprints.tsv: %d of %d rig blueprints take no salvage"
                   % (missing, rigs))
    ctx.note("salvage: %d wreck types, %d yield rows; %d rig blueprints, %d salvage types "
             "feed them" % (len(wrecks), len(yields), rigs, len(used)))
    return bad


def _is_rig(ctx, type_id):
    """A rig is a module with `rigSize` (1547); read once from typedata."""
    cache = ctx._cache  # noqa: SLF001
    if "salvage_rigs" not in cache:
        c = _content()
        rigs = set()
        for f in c.read_rows(ctx.path("types/typedata.tsv")):
            if f[0] == "type" and len(f) >= 14 and 1547 in c.decode_map(f[12]):
                rigs.add(int(f[1]))
        cache["salvage_rigs"] = rigs
    return type_id in cache["salvage_rigs"]
