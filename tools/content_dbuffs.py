"""The `dbuffs` content section: the client's warfare-buff collections.

Packet K-BOOST (`docs/lanes/k-boost.md`). Writes two bundle files, every header
`docs/content/schema.md`'s:

    dbuffs/collections.tsv   one row per collection (`dbuffcollections.static`):
                             the operator, how overlapping buffs aggregate,
                             whether the buff bar shows the value as is,
                             inverted or not at all, its name and note
    dbuffs/modifiers.tsv     one row per modifier of a collection: which
                             attribute it moves and on what (the ship itself,
                             everything fitted, a group of it, what needs a
                             skill)

The table is the client's own fsdlite sqlite cache, read with
`fsd.sqlite_rows`. What each field means is the client's own code, read from
`out/pyc/dbuff/` [V]:

- `storage.py` (`DbuffCollectionStorage`): `operationName` is one of nine
  dogma operations (`OPERATION_IDS_BY_NAME`); `itemModifiers`,
  `locationModifiers`, `locationGroupModifiers` and
  `locationRequiredSkillModifiers` are lists of `dogmaAttributeID` plus the
  `groupID` or `skillID` the last two name;
- `common/registry.py` (`ModifierApplier`): an item modifier lands on the ship
  itself, the three location modifiers on what is fitted in the ship, each
  with the buff's output value as the modifier's value;
- `common/aggregators.py` (`CreateAggregator`): `aggregateMode` is `Minimum`
  or `Maximum`;
- `client/uiFormatters.py` (`GetDisplayNameAndValue`): `showOutputValueInUI`
  is `ShowNormal`, `ShowInverted` or `Hide`.

The derivation is pure (plain dicts in, rows out): the unit tests feed it
invented rows; `gather(ctx)` is the only bundle reader.
"""

import os
import sys

SECTION = "dbuffs"
DEPENDS = ("types",)

_HERE = os.path.dirname(os.path.abspath(__file__))

COLLECTIONS_RES = "res:/staticdata/dbuffcollections.static"

COLLECTIONS_HEADER = tuple(
    "collection_id operation aggregate show name description".split())
MODIFIERS_HEADER = tuple(
    "collection_id kind attribute_id group_id skill_id".split())

#: `dbuff/storage.py` `OPERATION_IDS_BY_NAME` [V]: the nine operations.
OPERATIONS = ("PreAssignment", "PreMul", "PreDiv", "ModAdd", "ModSub",
              "PostMul", "PostDiv", "PostPercent", "PostAssignment")
#: `dbuff/common/aggregators.py` `AGGREGATOR_CLASS_BY_MODE` [V].
AGGREGATES = ("Minimum", "Maximum")
#: `dbuff/client/uiFormatters.py` `GetDisplayNameAndValue` [V].
SHOWS = ("ShowNormal", "ShowInverted", "Hide")
#: The four lists of a collection, in `dbuff/storage.py`'s order [V], and the
#: kind each one is in `modifiers.tsv`.
KINDS = (("itemModifiers", "item"), ("locationModifiers", "location"),
         ("locationGroupModifiers", "group"),
         ("locationRequiredSkillModifiers", "skill"))

#: `dogma/const.py` `attributeDbuff1ID..4ID` and their value and multiplier
#: attributes [V]: the four buff slots of a command burst module (value) and
#: of its charge (multiplier), and `attributeBuffDuration`.
BUFF_SLOTS = ((2468, 2469, 2596), (2470, 2471, 2597), (2472, 2473, 2598),
              (2536, 2537, 2599))
BUFF_DURATION = 2535
#: The effect that carries a charge's id and multiplier onto its module:
#: `chargeBonusWarfareCharge` [V-data] (`modifier` rows of 6737).
CHARGE_EFFECT = 6737


def _opt(v):
    return "" if v is None else v


def collections_rows(collections, text):
    """`dbuffs/collections.tsv` rows, ascending by id. `collections` is
    `{id: row}` from `dbuffcollections.static`; `text` maps a message id to
    its string (empty for none). A missing field is an empty column, which
    `problems` refuses for the three enums."""
    rows = []
    for c, r in sorted(collections.items()):
        name = r.get("displayNameID")
        rows.append((c, r.get("operationName", ""), r.get("aggregateMode", ""),
                     r.get("showOutputValueInUI", ""),
                     text(name) if name else "",
                     r.get("developerDescription", "")))
    return rows


def modifiers_rows(collections):
    """`dbuffs/modifiers.tsv` rows, ascending by `(collection, kind order,
    attribute, group, skill)`. `group_id` and `skill_id` are empty where the
    kind has none."""
    rows = []
    for c, r in sorted(collections.items()):
        for key, kind in KINDS:
            part = []
            for m in r.get(key) or []:
                part.append((c, kind, m["dogmaAttributeID"],
                             _opt(m.get("groupID")), _opt(m.get("skillID"))))
            part.sort(key=lambda x: (x[2], x[3] if x[3] != "" else -1,
                                     x[4] if x[4] != "" else -1))
            rows.extend(part)
    return rows


# --- the bundle readers ---------------------------------------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    if "dbuffs" in ctx._cache:  # noqa: SLF001
        return ctx._cache["dbuffs"]  # noqa: SLF001
    fsd = _content()._imp("fsd")  # noqa: SLF001
    got = {int(k): v for k, v in fsd.sqlite_rows(ctx.client.path(COLLECTIONS_RES))}
    ctx._cache["dbuffs"] = got  # noqa: SLF001
    return got


def derive(collections, text):
    return collections_rows(collections, text), modifiers_rows(collections)


def section(ctx):
    collections = gather(ctx)
    co, mo = derive(collections, ctx.text)
    ctx.note("dbuffs: %d collections, %d modifiers" % (len(co), len(mo)))
    return [("dbuffs/collections.tsv", COLLECTIONS_HEADER, co, [COLLECTIONS_RES]),
            ("dbuffs/modifiers.tsv", MODIFIERS_HEADER, mo, [COLLECTIONS_RES])]


# --- the checks -------------------------------------------------------------------

def problems(co, mo, attributes, skills, buff_ids):
    """What is wrong with the two row lists, as strings. `attributes` is the
    set of attribute ids typedata extracted, `skills` the type ids it holds,
    `buff_ids` the collection ids the burst charges name (`BUFF_SLOTS`).
    Pure, so the tests run it on invented rows."""
    bad = []
    ids = {r[0] for r in co}
    for r in co:
        if r[1] not in OPERATIONS:
            bad.append("collections.tsv: collection %d: operation %r" % (r[0], r[1]))
        if r[2] not in AGGREGATES:
            bad.append("collections.tsv: collection %d: aggregate %r" % (r[0], r[2]))
        if r[3] not in SHOWS:
            bad.append("collections.tsv: collection %d: show %r" % (r[0], r[3]))
    have = {}
    seen = set()
    for r in mo:
        have[r[0]] = have.get(r[0], 0) + 1
        if r[0] not in ids:
            bad.append("modifiers.tsv: collection %d is not in collections.tsv" % r[0])
        if r in seen:
            bad.append("modifiers.tsv: %r twice" % (r,))
        seen.add(r)
        if r[2] not in attributes:
            bad.append("modifiers.tsv: collection %d: attribute %d is not in typedata"
                       % (r[0], r[2]))
        if r[1] == "group" and (r[3] == "" or r[4] != ""):
            bad.append("modifiers.tsv: collection %d: a group row wants a group only" % r[0])
        if r[1] == "skill":
            if r[4] == "" or r[3] != "":
                bad.append("modifiers.tsv: collection %d: a skill row wants a skill only" % r[0])
            elif r[4] not in skills:
                bad.append("modifiers.tsv: collection %d: skill %d is not in typedata"
                           % (r[0], r[4]))
        if r[1] in ("item", "location") and (r[3] != "" or r[4] != ""):
            bad.append("modifiers.tsv: collection %d: a %s row has no group or skill"
                       % (r[0], r[1]))
    for b in sorted(buff_ids - ids):
        bad.append("a burst charge names collection %d, which collections.tsv lacks" % b)
    for b in sorted(buff_ids & ids):
        if not have.get(b):
            bad.append("collections.tsv: burst collection %d has no modifier" % b)
    return bad


def _typedata_facts(path):
    """From `types/typedata.tsv`: the attribute ids, the type ids, and the
    collection ids the buff slots of every charge that carries
    `CHARGE_EFFECT` (a warfare charge) name."""
    attributes, types, buff_ids = set(), set(), set()
    id_attrs = {slot[0] for slot in BUFF_SLOTS}
    with open(path, encoding="utf-8", newline="") as fh:
        lines = fh.read().split("\n")
    for line in lines:
        f = line.split("\t")
        if f[0] == "attribute":
            attributes.add(int(f[1]))
        elif f[0] == "type" and len(f) >= 14:
            types.add(int(f[1]))
            effects = f[11].split(",") if f[11] else []
            if str(CHARGE_EFFECT) in effects:
                for kv in f[12].split(";"):
                    k, _, v = kv.partition("=")
                    if k and int(k) in id_attrs and float(v) > 0:
                        buff_ids.add(int(float(v)))
    return attributes, types, buff_ids


def check(ctx):
    content = _content()
    collections = gather(ctx)
    co, mo = derive(collections, ctx.text)
    bad = []
    for name, rows in (("collections", co), ("modifiers", mo)):
        got = list(content.read_rows(ctx.path("dbuffs/%s.tsv" % name)))
        if got != [content.encode_row(r).split("\t") for r in rows]:
            bad.append("dbuffs/%s.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (name, len(got), len(rows)))
    attributes, types, buff_ids = _typedata_facts(ctx.path("types/typedata.tsv"))
    bad += problems(co, mo, attributes, types, buff_ids)
    ctx.note("dbuffs: %d collections, %d modifiers, %d burst collections named by charges"
             % (len(co), len(mo), len(buff_ids)))
    return bad
