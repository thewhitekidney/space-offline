"""The `store` content section: the NES catalogue, resolved against the client's data.

Lane L1 NES-BACKEND (`docs/lanes/l1-nes-backend.md`, `handback/r-nes-econ/
report.md` §2.1). A real `content.py` section, so `tools/up.sh` extracts it into
whichever bundle it selects (`out/content`, or `out/content-<build>` for an
unverified client build) and re-extracts when an authored input below changes.
Authored, in `content/store/` (our words and our prices, no type ids, no names):

    offers.tsv      key category selector qty price_plx base_plx single why
    categories.tsv  path name why

The hull classes a SKIN prices by are [`HULL_CLASSES`]: ship group ids, plan
§2.1 class A vocabulary, so they live in code, not in a data file.

Written into the bundle:

    store/catalogue.tsv  one row per product of every offer, selectors resolved
    store/types.tsv      the item types the store delivers (`typedata.tsv`
                         carries neither PLEX, the extractor nor SKIN licences)

Selectors:

    const:<name>                   a type the server names in code (`CONSTS`)
    service:<name>                 a product that is not an item (`SERVICES`)
    skin:hullclass=<c>:permanent[:cap=N]
                                   one offer per permanent SKIN licence whose
                                   hulls are all of class `<c>`, lowest SKIN id
                                   first, at most N of them

The SKIN data is the client's own: `skinlicenses.static` (`licenseTypeID,
skinID, duration, isSingleUse`; `duration` -1 is permanent), `skins.static`
(`types[]`, `visibleTranquility`, `isStructureSkin`) [V `shipskins/static.py`].

The derivation is pure (plain dicts in, rows out); the unit tests feed it
invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "store"
DEPENDS = ()
CFSD = ("types", "groups")

_HERE = os.path.dirname(os.path.abspath(__file__))

CATALOGUE_HEADER = ("key", "offer_id", "category", "name", "description", "price_plx",
                    "base_plx", "single", "type_id", "qty", "service")
TYPES_HEADER = ("type_id", "group_id", "category_id", "singleton", "volume", "name")

#: Types the server names in code (protocol vocabulary, `inventorycommon/
#: const.py`): `typeSkillExtractor`, and `typePlex` for the vault.
CONSTS = {"skill_extractor": 40519}
TYPE_PLEX = 44992
#: Services: `(name, description)`, our words (the client has no type to name them).
SERVICES = {
    "bonus_remap": ("Bonus Neural Remap",
                    "One extra neural remap, applied to the pilot at purchase."),
}
#: `categoryShipSkin` [V `inventorycommon/const.py`].
CATEGORY_SHIP_SKIN = 91
#: Ship group ids by SKIN price class (class A vocabulary, `inventorycommon/
#: const.py`). A SKIN whose hulls straddle two classes, or sit in a group not
#: listed, is not sold. [A] the grouping; falsifier: the owner's price list.
HULL_CLASSES = {
    "frigate": (25, 324, 831, 830, 834, 893, 1527, 1283, 31, 237),
    "destroyer": (420, 1534, 1305),
    "cruiser": (26, 358, 832, 833, 541, 894, 963, 906, 1202, 380, 543, 463, 28, 1972),
    "battlecruiser": (419, 1201, 540, 941, 4902),
    "battleship": (27, 900, 898),
    "capital": (485, 659, 30, 547, 513, 902, 1538, 883, 4594, 5120),
}


def hullclass():
    """`{group_id: class}` from [`HULL_CLASSES`]; a group in two classes is an error."""
    out = {}
    for cls, groups in HULL_CLASSES.items():
        for g in groups:
            if g in out:
                raise ValueError("HULL_CLASSES: group %d twice" % g)
            out[g] = cls
    return out


def fnv31(text):
    """A stable 31-bit id for a key (FNV-1a), never 0. The server's
    `nes::category_id` is the same function."""
    h = 0x811C9DC5
    for b in text.encode("utf-8"):
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return max(h & 0x7FFFFFFF, 1)


def _clean(text):
    """One line, no tabs: a TSV field."""
    return " ".join(str(text or "").replace("\t", " ").split())


def _rows(lines, header, name):
    """The data rows of an authored file's lines: `header` must be line 1."""
    lines = [l.rstrip("\r") for l in lines]
    lines = [l for l in lines if l.strip() and not l.startswith("#")]
    if not lines or lines[0] != header:
        raise ValueError("%s: header is %r, expected %r" % (name, lines[:1], header))
    out = []
    for n, l in enumerate(lines[1:], 2):
        f = l.split("\t")
        if len(f) != header.count("\t") + 1:
            raise ValueError("%s: line %d: %d fields" % (name, n, len(f)))
        out.append(f)
    return out


def parse_offers(lines):
    rows = _rows(lines, "key\tcategory\tselector\tqty\tprice_plx\tbase_plx\tsingle\twhy",
                 "offers.tsv")
    return [{"key": r[0], "category": r[1], "selector": r[2], "qty": int(r[3]),
             "price": int(r[4]), "base": int(r[5]), "single": int(r[6])} for r in rows]


def parse_categories(lines):
    return [r[0] for r in _rows(lines, "path\tname\twhy", "categories.tsv")]


def parse_selector(sel):
    """`('const', name)`, `('service', name)` or `('skin', {class, permanent, cap})`."""
    kind, _, rest = sel.partition(":")
    if kind in ("const", "service"):
        return kind, rest
    if kind == "skin":
        opts = {"class": None, "permanent": False, "cap": None}
        for part in rest.split(":"):
            if part == "permanent":
                opts["permanent"] = True
            elif part.startswith("hullclass="):
                opts["class"] = part[len("hullclass="):]
            elif part.startswith("cap="):
                opts["cap"] = int(part[4:])
            else:
                raise ValueError("selector %r: unknown part %r" % (sel, part))
        if not opts["class"]:
            raise ValueError("selector %r: no hullclass" % sel)
        return "skin", opts
    raise ValueError("selector %r: unknown kind" % sel)


def skin_licences(licences, skins, types, groups, hullclass, cls, permanent, cap):
    """The licence type ids that sell as class `cls`, in SKIN id order.

    `licences` is `{licenseTypeID: row}`, `skins` `{skinID: row}`, `types`
    `{typeID: row}` (`groupID`), `hullclass` `{groupID: class}`. A SKIN is
    skipped unless it is visible on Tranquility, not a structure SKIN, fits at
    least one hull, and every hull it fits is of class `cls`. One licence per
    SKIN (the lowest type id); `permanent` keeps `duration == -1` and not
    single-use. `groups` is unused here beyond the caller's own checks."""
    best = {}
    for lid, row in sorted(licences.items()):
        if permanent and (row.get("duration") != -1 or row.get("isSingleUse")):
            continue
        best.setdefault(row["skinID"], lid)
    out = []
    for sid in sorted(best):
        s = skins.get(sid)
        if not s or not s.get("visibleTranquility") or s.get("isStructureSkin"):
            continue
        hulls = s.get("types") or []
        classes = {hullclass.get(types.get(t, {}).get("groupID")) for t in hulls}
        if hulls and classes == {cls}:
            out.append(best[sid])
    return out[:cap] if cap else out


def derive(offers, categories, hullclass, licences, skins, types, groups, text):
    """`(catalogue rows, type rows, problems)`.

    `text(message_id)` is the en-us string. Rows are tuples in the header
    order. `problems` is a list of strings; the caller refuses to write when
    it is not empty."""
    rows, used, problems = [], {}, []
    ids = {}

    def type_name(tid):
        return _clean(text(types[tid].get("typeNameID")))

    def type_desc(tid):
        return _clean(text(types[tid].get("descriptionID")))

    def add(key, category, name, desc, price, base, single, products):
        oid = fnv31(key)
        if oid in ids:
            problems.append("offer id %d: %r and %r collide" % (oid, ids[oid], key))
        ids[oid] = key
        if category not in categories:
            problems.append("offer %r: category %r is not in categories.tsv" % (key, category))
        for tid, qty, service in products:
            rows.append((key, oid, category, _clean(name), _clean(desc), price, base,
                         single, tid, qty, service))
            if tid:
                used[tid] = True

    for o in offers:
        kind, arg = parse_selector(o["selector"])
        if o["price"] < 1:
            problems.append("offer %r: price %d" % (o["key"], o["price"]))
        if kind == "const":
            tid = CONSTS.get(arg)
            if tid is None or tid not in types:
                problems.append("offer %r: const %r is unknown or not in the client" % (o["key"], arg))
                continue
            add(o["key"], o["category"], type_name(tid), type_desc(tid), o["price"], o["base"],
                o["single"], [(tid, o["qty"], "")])
        elif kind == "service":
            if arg not in SERVICES:
                problems.append("offer %r: service %r is unknown" % (o["key"], arg))
                continue
            name, desc = SERVICES[arg]
            add(o["key"], o["category"], name, desc, o["price"], o["base"], o["single"],
                [(0, o["qty"], arg)])
        else:
            if arg["class"] not in set(hullclass.values()):
                problems.append("offer %r: hull class %r has no group" % (o["key"], arg["class"]))
                continue
            for lid in skin_licences(licences, skins, types, groups, hullclass, arg["class"],
                                     arg["permanent"], arg["cap"]):
                if lid not in types:
                    problems.append("offer %r: licence %d is not a client type" % (o["key"], lid))
                    continue
                if groups.get(types[lid].get("groupID"), {}).get("categoryID") != CATEGORY_SHIP_SKIN:
                    problems.append("offer %r: licence %d is not category %d"
                                    % (o["key"], lid, CATEGORY_SHIP_SKIN))
                    continue
                add("%s:%d" % (o["key"], licences[lid]["skinID"]), o["category"],
                    type_name(lid), "A permanent SKIN licence for this hull class.",
                    o["price"], o["base"], o["single"], [(lid, o["qty"], "")])
    if TYPE_PLEX not in types:
        problems.append("PLEX (%d) is not a client type" % TYPE_PLEX)
    used[TYPE_PLEX] = True
    type_rows = []
    for tid in sorted(used):
        t = types.get(tid)
        if not t:
            continue
        cat = groups.get(t.get("groupID"), {}).get("categoryID", 0)
        type_rows.append((tid, t.get("groupID"), cat, 0, repr(float(t.get("volume") or 0.0)),
                          type_name(tid)))
    return rows, type_rows, problems


def _lines(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read().splitlines()


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's tables: `(licences, skins, types, groups)`."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    c = ctx.client
    licences = {int(k): v for k, v in fsd.sqlite_rows(c.path(LICENCES_RES))}
    skins = {int(k): v for k, v in fsd.sqlite_rows(c.path(SKINS_RES))}
    return licences, skins, fsd.rows(c, "types"), fsd.rows(c, "groups")


LICENCES_RES = "res:/staticdata/skinlicenses.static"
SKINS_RES = "res:/staticdata/skins.static"
SOURCES = [LICENCES_RES, SKINS_RES, "res:/staticdata/types.fsdbinary",
           "res:/staticdata/groups.fsdbinary", "res:/localizationfsd/localization_fsd_en-us.pickle"]


def _authored(ctx):
    d = os.path.join(os.path.dirname(_HERE), "content", "store")
    offers = parse_offers(_lines(ctx.input(os.path.join(d, "offers.tsv"))))
    cats = set(parse_categories(_lines(ctx.input(os.path.join(d, "categories.tsv")))))
    return offers, cats


def _derive(ctx):
    offers, cats = _authored(ctx)
    lic, skins, types, groups = gather(ctx)
    return derive(offers, cats, hullclass(), lic, skins, types, groups, ctx.text)


def section(ctx):
    rows, type_rows, problems = _derive(ctx)
    if problems:
        raise ValueError("store: " + "; ".join(problems))
    ctx.note("store: %d product rows, %d offers, %d types"
             % (len(rows), len({r[0] for r in rows}), len(type_rows)))
    return [("store/catalogue.tsv", CATALOGUE_HEADER, rows, SOURCES),
            ("store/types.tsv", TYPES_HEADER, type_rows, SOURCES)]


def check(ctx):
    content = _content()
    rows, type_rows, problems = _derive(ctx)
    bad = list(problems)
    for name, want in (("catalogue", rows), ("types", type_rows)):
        got = list(content.read_rows(ctx.path("store/%s.tsv" % name)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("store/%s.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (name, len(got), len(want)))
    return bad
