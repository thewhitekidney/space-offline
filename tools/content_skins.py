"""The `skins` content section: SKIN licences and the SKINs they unlock.

Lane L2 SKINS (`handback/r-nes-econ/report.md` §2.2, F9). A real `content.py`
section, found by glob like `store`, so `tools/up.sh` extracts it into whichever
bundle it selects (`out/content`, or `out/content-<build>` for an unverified
client build): the tables are the **client build's own**, and between builds
two SKIN ids swapped meaning (port review r-port-diff §4), which is why the
server keys everything it persists by **licence type**, never by SKIN id.

Written into the bundle:

    skins/licenses.tsv  one row per licence type: the item the pilot holds,
                        the SKIN it unlocks, its length and whether it is single
                        use. Carries the item type's own group, volume and name
                        too, because `typedata.tsv` has no SKIN licences.
    skins/skins.tsv     one row per SKIN: the hulls it fits (type ids, comma
                        separated), whether it shows on Tranquility, and
                        whether it is a structure SKIN.

The SKIN data is the client's own, three fsdlite sqlite caches [V
`shipskins/static.py`]: `skinlicenses.static` (`licenseTypeID, skinID,
duration, isSingleUse`; `duration` -1 is permanent), `skins.static`
(`skinMaterialID, types[], visibleTranquility`; `isStructureSkin` is an
**optional** key, absent on most rows) and `skinmaterials.static`, which the
server does not need (the client resolves materials itself) and so is not
extracted.

The derivation is pure (plain dicts in, rows out); the unit tests feed it
invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "skins"
DEPENDS = ()
CFSD = ("types", "groups")

_HERE = os.path.dirname(os.path.abspath(__file__))

LICENSES_HEADER = ("type_id", "group_id", "skin_id", "duration", "single_use", "volume", "name")
SKINS_HEADER = ("skin_id", "visible", "structure", "hulls")

#: `categoryShipSkin` [V `inventorycommon/const.py`].
CATEGORY_SHIP_SKIN = 91

LICENCES_RES = "res:/staticdata/skinlicenses.static"
SKINS_RES = "res:/staticdata/skins.static"
SOURCES = [LICENCES_RES, SKINS_RES, "res:/staticdata/types.fsdbinary",
           "res:/staticdata/groups.fsdbinary", "res:/localizationfsd/localization_fsd_en-us.pickle"]


def _clean(text):
    """One line, no tabs: a TSV field."""
    return " ".join(str(text or "").replace("\t", " ").split())


def derive(licences, skins, types, groups, text):
    """`(licence rows, skin rows, problems)`.

    `licences` is `{licenseTypeID: row}`, `skins` `{skinID: row}`, `types`
    `{typeID: row}` (`groupID`, `volume`, `typeNameID`), `groups`
    `{groupID: row}` (`categoryID`); `text(message_id)` is the en-us string.

    A licence is written only when its type is a client type in category 91
    and its SKIN exists; a licence for a SKIN the client does not have, or a
    type that is not a SKIN licence, is a problem (the extraction refuses, the
    data is not what this reads). A licence type that is not in `types` at all
    is skipped and counted by the caller: it cannot be an item. `isStructureSkin`
    is optional on a SKIN row (absent means false)."""
    lic_rows, problems, used = [], [], set()
    for lid in sorted(licences):
        row = licences[lid]
        sid = row.get("skinID")
        t = types.get(lid)
        if t is None:
            continue
        if groups.get(t.get("groupID"), {}).get("categoryID") != CATEGORY_SHIP_SKIN:
            problems.append("licence %d is not category %d" % (lid, CATEGORY_SHIP_SKIN))
            continue
        if sid not in skins:
            problems.append("licence %d names SKIN %r, which is not in skins.static" % (lid, sid))
            continue
        duration = row.get("duration")
        if not isinstance(duration, int):
            problems.append("licence %d: duration %r" % (lid, duration))
            continue
        used.add(sid)
        lic_rows.append((lid, t.get("groupID"), sid, duration,
                         1 if row.get("isSingleUse") else 0,
                         repr(float(t.get("volume") or 0.0)),
                         _clean(text(t.get("typeNameID")))))
    skin_rows = []
    for sid in sorted(used):
        s = skins[sid]
        hulls = ",".join(str(h) for h in sorted(set(s.get("types") or [])))
        skin_rows.append((sid, 1 if s.get("visibleTranquility") else 0,
                          1 if s.get("isStructureSkin") else 0, hulls))
    return lic_rows, skin_rows, problems


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


def _derive(ctx):
    lic, skins, types, groups = gather(ctx)
    return derive(lic, skins, types, groups, ctx.text)


def section(ctx):
    lic_rows, skin_rows, problems = _derive(ctx)
    if problems:
        raise ValueError("skins: " + "; ".join(problems[:20]))
    ctx.note("skins: %d licence types, %d SKINs" % (len(lic_rows), len(skin_rows)))
    return [("skins/licenses.tsv", LICENSES_HEADER, lic_rows, SOURCES),
            ("skins/skins.tsv", SKINS_HEADER, skin_rows, SOURCES)]


def check(ctx):
    content = _content()
    lic_rows, skin_rows, problems = _derive(ctx)
    bad = list(problems)
    for name, want in (("licenses", lic_rows), ("skins", skin_rows)):
        got = list(content.read_rows(ctx.path("skins/%s.tsv" % name)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("skins/%s.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (name, len(got), len(want)))
    return bad
