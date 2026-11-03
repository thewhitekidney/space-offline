"""The `types2` content section: types phase 2 (market columns, the market
tree, type lists, reprocessing materials and compression).

Packet CP12 (`docs/content/plan.md` §6.2, W12). Spec: C1 §2.3, C6 R7, C5
§2.2; every header is `docs/content/schema.md`'s. Writes six bundle files:

    types/market.tsv            one row per `types` key: the columns
                                `typedata.tsv` lacks (basePrice,
                                marketGroupID, metaLevel, factionID, ...)
    types/marketgroups.tsv      one row per `marketgroups` key
    types/typelists.tsv         one row per `typelist` key
    types/materials.tsv         `typematerials[].materials`, one row each
    types/materials-random.tsv  `typematerials[].randomizedMaterials`
    types/compress.tsv          `compressibletypes`: source -> compressed

Phase 1's `types` section (`content.py`, CP1) still writes `typedata.tsv`;
CP12's other half is `tools/typedata.py`'s categories (17 and 25 added).
The section is named `types2` because `types` is CP1's built-in name.

The derivation is pure (plain dicts in, rows out), so the unit tests feed it
invented rows; `gather(ctx)` is the only client reader.
"""

import sys

SECTION = "types2"
DEPENDS = ()
#: The cFSD stems the schema guard covers. `compressibletypes` is a scalar
#: table with no committed field layout (`fsd.CFSD_BARE_SCHEMAS`), so the
#: guard cannot cover it; `check` closes it into `types` instead.
CFSD = ("types", "groups", "marketgroups", "typelist", "typematerials")

# The header lines, copied from `schema.md` §2.
MARKET_HEADER = tuple(
    "type_id group_id category_id market_group_id base_price meta_group_id "
    "meta_level tech_level variation_parent_type_id faction_id race_id "
    "wreck_type_id portion_size published".split())
MARKETGROUPS_HEADER = ("market_group_id", "parent_id", "has_types", "icon_id", "name")
TYPELISTS_HEADER = tuple(
    "list_id inc_types inc_groups inc_cats exc_types exc_groups exc_cats".split())
MATERIALS_HEADER = ("type_id", "material_type_id", "quantity")
RANDOM_HEADER = ("type_id", "material_type_id", "qmin", "qmax")
COMPRESS_HEADER = ("type_id", "compressed_type_id")

#: `typelist` fields in `TYPELISTS_HEADER` order [V: `fsd.rows(c, 'typelist')`].
LIST_FIELDS = ("includedTypeIDs", "includedGroupIDs", "includedCategoryIDs",
               "excludedTypeIDs", "excludedGroupIDs", "excludedCategoryIDs")


def _opt(v):
    """An optional int column: None stays None (written empty)."""
    return None if v is None else int(v)


def _ints(v):
    return [int(x) for x in (v or ())]


def market_rows(types, group_cat):
    """`types/market.tsv` rows. `types` is `{type_id: types row}`,
    `group_cat` is `{group_id: category_id}`."""
    out = []
    for t in sorted(types):
        r = types[t]
        out.append((
            int(t), int(r["groupID"]), _opt(group_cat.get(r["groupID"])),
            _opt(r.get("marketGroupID")),
            float(r.get("basePrice") or 0.0),
            _opt(r.get("metaGroupID")), _opt(r.get("metaLevel")),
            _opt(r.get("techLevel")), _opt(r.get("variationParentTypeID")),
            _opt(r.get("factionID")), _opt(r.get("raceID")),
            _opt(r.get("wreckTypeID")),
            int(r.get("portionSize") or 1),
            bool(r.get("published")),
        ))
    return out


def marketgroup_rows(groups, text):
    """`types/marketgroups.tsv` rows; `text(message_id)` names a group."""
    return [(int(g), _opt(r.get("parentGroupID")), bool(r.get("hasTypes")),
             _opt(r.get("iconID")), text(r.get("nameID")))
            for g, r in sorted(groups.items())]


def typelist_rows(lists):
    """`types/typelists.tsv` rows, each list in the client's own order."""
    return [(int(k),) + tuple(_ints(r.get(f)) for f in LIST_FIELDS)
            for k, r in sorted(lists.items())]


def material_rows(table):
    """`(materials, random)` rows from `typematerials`, in the client's
    element order within a type."""
    plain, rnd = [], []
    for t, r in sorted(table.items()):
        for m in r.get("materials") or ():
            plain.append((int(t), int(m["materialTypeID"]), int(m["quantity"])))
        for m in r.get("randomizedMaterials") or ():
            rnd.append((int(t), int(m["materialTypeID"]), int(m["quantityMin"]),
                        int(m["quantityMax"])))
    return plain, rnd


def compress_rows(table):
    """`types/compress.tsv`: the key is the source, the scalar value the
    compressed type (`reprocess/tables.rs`'s direction) [V]."""
    return [(int(k), int(v)) for k, v in sorted(table.items())]


def expand(row, type_group, group_cat):
    """The type ids a `typelists.tsv` row (`TYPELISTS_HEADER` order, lists
    decoded) stands for: included types, groups and categories, minus the
    excluded ones. `type_group` is `{type_id: group_id}`, `group_cat`
    `{group_id: category_id}`.

    **[A]** an exclusion at any level beats an inclusion at any level (a
    type excluded by id stays out though its group is included, and a type
    included by id stays out when its category is excluded). Falsifier: a
    client `typelist` consumer (`evetypes` list membership) that lets an
    explicit `includedTypeIDs` entry override an excluded group or category.
    """
    _lid, inc_t, inc_g, inc_c, exc_t, exc_g, exc_c = row

    def members(ids, groups, cats):
        groups, cats = set(groups), set(cats)
        out = set(ids)
        for t, g in type_group.items():
            if g in groups or group_cat.get(g) in cats:
                out.add(t)
        return out

    return members(inc_t, inc_g, inc_c) - members(exc_t, exc_g, exc_c)


def unclosed(rows, cols, known):
    """`(row, col, id)` for every id in `cols` of `rows` not in `known`."""
    return [(r, c, r[c]) for r in rows for c in cols
            if r[c] is not None and r[c] not in known]


def derive(data):
    """`{relpath: rows}` from `gather`'s plain dicts."""
    group_cat = {g: r["categoryID"] for g, r in data["groups"].items()}
    plain, rnd = material_rows(data["typematerials"])
    return {
        "types/market.tsv": market_rows(data["types"], group_cat),
        "types/marketgroups.tsv": marketgroup_rows(data["marketgroups"], data["text"]),
        "types/typelists.tsv": typelist_rows(data["typelist"]),
        "types/materials.tsv": plain,
        "types/materials-random.tsv": rnd,
        "types/compress.tsv": compress_rows(data["compressibletypes"]),
    }


# --- the client reader ---------------------------------------------------------

def _content():
    return sys.modules.get("content") or __import__("content")


def gather(ctx):
    """The plain dicts `derive` reads, from the client."""
    if "types2" in ctx._cache:  # noqa: SLF001
        return ctx._cache["types2"]  # noqa: SLF001
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    data = {stem: {k: (dict(v) if hasattr(v, "keys") else v)
                   for k, v in fsd.rows(c, stem).items()}
            for stem in ("types", "groups", "marketgroups", "typelist", "typematerials",
                         "compressibletypes")}
    data["text"] = ctx.text
    ctx._cache["types2"] = data  # noqa: SLF001
    return data


def _sources(*stems, loc=False):
    content = _content()
    return [content.fsdbin(s) for s in stems] + ([content.LOC_EN] if loc else [])


def section(ctx):
    rows = derive(gather(ctx))
    return [
        ("types/market.tsv", MARKET_HEADER, rows["types/market.tsv"],
         _sources("types", "groups")),
        ("types/marketgroups.tsv", MARKETGROUPS_HEADER, rows["types/marketgroups.tsv"],
         _sources("marketgroups", loc=True)),
        ("types/typelists.tsv", TYPELISTS_HEADER, rows["types/typelists.tsv"],
         _sources("typelist")),
        ("types/materials.tsv", MATERIALS_HEADER, rows["types/materials.tsv"],
         _sources("typematerials")),
        ("types/materials-random.tsv", RANDOM_HEADER, rows["types/materials-random.tsv"],
         _sources("typematerials")),
        ("types/compress.tsv", COMPRESS_HEADER, rows["types/compress.tsv"],
         _sources("compressibletypes")),
    ]


# --- the client-backed check (counts live only here, plan §1.6) ---------------

#: [V] build 3619879, `schema.md` §2 and C1 §2.3.
EXPECT = {
    "types/market.tsv": 53288,
    "types/marketgroups.tsv": 2114,
    "types/typelists.tsv": 469,
    "types/compress.tsv": 212,
}
#: Counts that differ in another build, ``{build: {rel: rows}}``, over `EXPECT`.
#: 3662168 **[V]** against `r-port-diff` section 3: market +11 (Herald and
#: Penitence, four emblems, a crate, two backgrounds, two SKIN licences) and
#: typelists +3 (913-915). A build not listed is held to `EXPECT`.
EXPECT_BY_BUILD = {
    3662168: {"types/market.tsv": 53299, "types/typelists.tsv": 472},
}
#: Builds whose counts are verified: `EXPECT` is 3619879's, 3628513 shares it,
#: and `EXPECT_BY_BUILD` holds the rest. Any other build (the next client, before
#: someone has read its counts) is not failed on a count it cannot have a
#: reference for: `expected_counts` says so, `check` notes it, and the closures
#: still run. After the first extraction add the build's row to `EXPECT_BY_BUILD`.
VERIFIED_BASE_BUILDS = (3619879, 3628513)
#: [V] `typematerials` keys with `randomizedMaterials` set (`schema.md` §2).
EXPECT_RANDOM_TYPES = 10


def expected_counts(build):
    """``(counts, verified)`` for a client build (int or None, the reference).

    ``verified`` is False for a build with no reference counts; ``counts`` is
    then `EXPECT` (a hint, not a requirement).
    """
    if build is None:
        build = 3662168
    counts = dict(EXPECT)
    counts.update(EXPECT_BY_BUILD.get(build, {}))
    return counts, build in EXPECT_BY_BUILD or build in VERIFIED_BASE_BUILDS


def _allowed_ship_lists(ctx):
    """Every `dungeons.connections[].allowedShipsList` id (C1 §2.3)."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    out = set()
    for _d, row in fsd.rows(ctx.client, "dungeons").items():
        for conn in row.get("connections") or ():
            v = conn.get("allowedShipsList")
            if v:
                out.add(int(v))
    return out


def check(ctx):
    content = _content()
    data = gather(ctx)
    rows = derive(data)
    bad = []
    build = content._imp("fsd").client_layout_build(ctx.client)  # noqa: SLF001
    expect, verified = expected_counts(build)
    for rel, want in rows.items():
        got = list(content.read_rows(ctx.path(rel)))
        if got != [content.encode_row(w).split("\t") for w in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
        ctx.note("%s: %d rows" % (rel, len(got)))
        if rel in expect and len(got) != expect[rel]:
            if verified:
                bad.append("%s: %d rows, expected %d [V]" % (rel, len(got), expect[rel]))
            else:
                ctx.note("%s: %d rows, UNVERIFIED build %s (older base says %d): add a row "
                         "to content_types.EXPECT_BY_BUILD" % (rel, len(got), build, expect[rel]))
    types = set(data["types"])
    groups = set(data["groups"])
    mgroups = set(data["marketgroups"])
    lists = set(data["typelist"])
    rnd = {r[0] for r in rows["types/materials-random.tsv"]}
    if len(rnd) != EXPECT_RANDOM_TYPES:
        bad.append("types/materials-random.tsv: %d types, expected %d [V]"
                   % (len(rnd), EXPECT_RANDOM_TYPES))
    market = rows["types/market.tsv"]
    closures = (
        ("market.tsv group_id -> groups", unclosed(market, (1,), groups)),
        ("market.tsv market_group_id -> marketgroups", unclosed(market, (3,), mgroups)),
        ("market.tsv variation_parent_type_id -> types", unclosed(market, (8,), types)),
        ("market.tsv wreck_type_id -> types", unclosed(market, (11,), types)),
        ("marketgroups.tsv parent_id -> marketgroups",
         unclosed(rows["types/marketgroups.tsv"], (1,), mgroups)),
        ("materials.tsv -> types", unclosed(rows["types/materials.tsv"], (0, 1), types)),
        ("materials-random.tsv -> types",
         unclosed(rows["types/materials-random.tsv"], (0, 1), types)),
        ("compress.tsv -> types", unclosed(rows["types/compress.tsv"], (0, 1), types)),
    )
    for what, miss in closures:
        ctx.note("types: %s: %d unclosed" % (what, len(miss)))
        if miss:
            bad.append("types: %s: %d unclosed (first %r)" % (what, len(miss), miss[0][2]))
    factions = sum(1 for r in market if r[9] is not None)
    ctx.note("types/market.tsv: %d with market_group_id, %d with faction_id"
             % (sum(1 for r in market if r[3] is not None), factions))
    allowed = _allowed_ship_lists(ctx)
    open_ = sorted(allowed - lists)
    ctx.note("types/typelists.tsv: %d allowed_ships_list ids, %d unclosed"
             % (len(allowed), len(open_)))
    if open_:
        bad.append("types/typelists.tsv: allowed_ships_list not closed: %s" % open_[:10])
    return bad
