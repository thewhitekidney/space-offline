#!/usr/bin/env python3
"""Extract planetary interaction's static layer from the client into a TSV.

The server's PI simulation (``server/crates/eve-server/src/planet.rs``) runs
against a ``Statics`` table: every pin type with its dogma attributes, the 68
production schematics, the resource distribution per planet type, and the
item rows for the commodities and command centres the item store has to hold.
All of it is CCP's data, so -- like ``tools/celestials.py`` -- this is a
*reader* and the output goes to ``out/`` (gitignored), never into the
repository. The server loads it with ``--planets FILE``.

Usage (inside the analysis container)::

    tools/analysis.sh python3 /work/tools/pi_statics.py \\
        --client /client --resfiles /client-root/ResFiles \\
        --out /work/out/pi-statics.tsv

Every table here is read through ``tools/dogma.py``, whose layouts were
derived from the client's own loader ``.pyd`` files and checked against data
(``docs/re/dogma.md``):

======================  ============================================  =====
record                  source                                        rows
======================  ============================================  =====
``pintype``             ``types`` + ``groups`` + ``typedogma``        139
``schematic``           ``schematics.fsdbinary``                       68
``resource``            ``globaltypes.fsdbinary``                       8
``itemtype``            ``types`` (categories 42, 43; group 1027)     ~131
``taxmult``             ``typedogma`` 1640 for every ``itemtype``     ~131
``customsoffice``       ``types`` + ``typedogma`` 1780, type 4318         1
======================  ============================================  =====

Format, one record per line, tab-separated; ``#`` starts a comment::

    pintype     typeID groupID capacity basePrice metaLevel attrs
    schematic   schematicID cycleTimeSeconds pinTypeIDs inputs outputs
    resource    planetTypeID distribution
    itemtype    typeID groupID categoryID volume capacity name
    taxmult     typeID importTaxMultiplier
    customsoffice  typeID capacity radius taxRate

``attrs`` is ``attributeID=value`` joined with ``;``. **Every attribute PI
reads is written, with the attribute's own ``defaultValue`` where the type
carries no row** -- that is what the client's ``GetTypeAttribute`` returns,
and it is the only reason an ECU's decay factor is not zero (no ECU type
carries a row for any of the five ECU tuning attributes). Values are written
with ``repr``, so a float32-widened default such as ``0.012000000104308128``
reaches the server bit for bit. ``pinTypeIDs`` is a comma list; ``inputs``,
``outputs`` and ``distribution`` are ``typeID=number`` joined with ``;``.

Stdlib only, read-only on the client.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse  # noqa: E402
import os  # noqa: E402
import sys  # noqa: E402

# `tools/` itself is already on the path -- the rotation above put it last.
import dogma  # noqa: E402
import fsd  # noqa: E402

#: The PI pin groups: extractor (legacy), command, process, storage,
#: spaceport, link, ECU. ``inventorycommon/const.pyc`` **[V]**.
PIN_GROUPS = (1026, 1027, 1028, 1029, 1030, 1036, 1063)
#: The groups and categories whose types the item store must hold: the
#: command centres (deployed from a ship's hold) and every PI commodity
#: (``categoryPlanetaryResources`` 42, ``categoryPlanetaryCommodities`` 43).
ITEM_GROUPS = (1027,)
ITEM_CATEGORIES = (42, 43)
#: Every attribute ``planet.rs`` reads (`planet::attr`). Written for every
#: pin type, defaulted, for the reason in the module docstring.
PI_ATTRIBUTES = (
    11, 15, 48, 49, 709, 1631, 1632, 1633, 1634, 1635, 1636, 1638, 1639,
    1640, 1641, 1642, 1643, 1644, 1645, 1683, 1684, 1685, 1687, 1689, 1690,
    1691,
)
#: ``importTaxMultiplier``: the per-commodity factor of both customs taxes
#: (``spaceportPin.GetImportTax``/``GetExportTax``) **[V]**.
ATTR_IMPORT_TAX_MULTIPLIER = 1640
#: ``npcCustomsOfficeTaxRate``, carried by the InterBus office type **[V]**.
ATTR_NPC_CUSTOMS_OFFICE_TAX_RATE = 1780
#: The InterBus Customs Office, group 1025 (``groupPlanetaryCustomsOffices``)
#: **[V]** -- the office the server anchors at every planet.
TYPE_INTERBUS_CUSTOMS_OFFICE = 4318


def _num(v):
    """``repr`` of a float, plain digits for an integral one."""
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, float):
        return repr(v)
    return str(int(v))


def extract(client, resfiles):
    d = dogma.Dogma(client, resfiles)
    c = d.client
    types = fsd.cfsd_rows(
        fsd.CfsdTable(c.path("res:/staticdata/types.fsdbinary"), "types"), "types")
    groups = fsd.cfsd_rows(
        fsd.CfsdTable(c.path("res:/staticdata/groups.fsdbinary"), "groups"), "groups")
    names = d.type_names()
    defaults = {k: v.get("defaultValue") or 0.0 for k, v in d.attributes.items()}

    pin_types = sorted(t for t, r in types.items() if r["groupID"] in PIN_GROUPS)
    wanted = set(pin_types)
    item_types = sorted(
        t for t, r in types.items()
        if r["groupID"] in ITEM_GROUPS
        or groups.get(r["groupID"], {}).get("categoryID") in ITEM_CATEGORIES)
    wanted.update(item_types)
    wanted.add(TYPE_INTERBUS_CUSTOMS_OFFICE)
    attrs = {}
    for key, row in d.table("typedogma").rows():
        if key in wanted:
            attrs[key] = {e["attributeID"]: e["value"] for e in row["dogmaAttributes"]}

    out = []
    out.append("# eve-offline PI statics, written by tools/pi_statics.py from the")
    out.append("# player's own client install. CCP's data: never commit this file.")
    for t in pin_types:
        r = types[t]
        a = attrs.get(t, {})
        merged = {i: a.get(i, defaults.get(i, 0.0)) for i in PI_ATTRIBUTES}
        out.append("\t".join([
            "pintype", str(t), str(r["groupID"]), _num(r.get("capacity") or 0.0),
            _num(r.get("basePrice") or 0.0), str(r.get("metaLevel") or 0),
            ";".join("%d=%s" % (i, _num(float(v))) for i, v in sorted(merged.items())),
        ]))

    n_schematics = 0
    for key, row in sorted(d.table("schematics").rows()):
        ins = sorted((t, v["quantity"]) for t, v in row["types"].items() if v["isInput"])
        outs = sorted((t, v["quantity"]) for t, v in row["types"].items() if not v["isInput"])
        out.append("\t".join([
            "schematic", str(key), str(row["cycleTime"]),
            ",".join(str(p["typeID"]) for p in row["pins"]),
            ";".join("%d=%d" % p for p in ins),
            ";".join("%d=%d" % p for p in outs),
        ]))
        n_schematics += 1

    n_resources = 0
    for key, row in sorted(d.table("globaltypes").rows()):
        dist = sorted(row["resourceDistribution"].items())
        out.append("\t".join([
            "resource", str(key),
            ";".join("%d=%s" % (t, _num(float(q))) for t, q in dist),
        ]))
        n_resources += 1

    n_items = 0
    for t in sorted(types):
        r = types[t]
        cat = groups.get(r["groupID"], {}).get("categoryID")
        if r["groupID"] not in ITEM_GROUPS and cat not in ITEM_CATEGORIES:
            continue
        name = names.get(t)
        name = name if isinstance(name, str) else "type %d" % t
        out.append("\t".join([
            "itemtype", str(t), str(r["groupID"]), str(cat),
            _num(r.get("volume") or 0.0), _num(r.get("capacity") or 0.0),
            name.replace("\t", " "),
        ]))
        n_items += 1

    # The customs taxes' per-commodity factor, defaulted like every
    # attribute above so the server never has to know the default.
    for t in item_types:
        v = attrs.get(t, {}).get(ATTR_IMPORT_TAX_MULTIPLIER,
                                 defaults.get(ATTR_IMPORT_TAX_MULTIPLIER, 1.0))
        out.append("\t".join(["taxmult", str(t), _num(float(v))]))

    office = types.get(TYPE_INTERBUS_CUSTOMS_OFFICE)
    if office is not None:
        rate = attrs.get(TYPE_INTERBUS_CUSTOMS_OFFICE, {}).get(
            ATTR_NPC_CUSTOMS_OFFICE_TAX_RATE,
            defaults.get(ATTR_NPC_CUSTOMS_OFFICE_TAX_RATE, 0.0))
        out.append("\t".join([
            "customsoffice", str(TYPE_INTERBUS_CUSTOMS_OFFICE),
            _num(office.get("capacity") or 0.0), _num(office.get("radius") or 0.0),
            _num(float(rate)),
        ]))

    counts = (len(pin_types), n_schematics, n_resources, n_items)
    return "\n".join(out) + "\n", counts


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--client", default=os.environ.get("EVE_CLIENT"))
    p.add_argument("--resfiles", default=None)
    p.add_argument("--out", required=True)
    a = p.parse_args(argv)
    text, (pins, schematics, resources, items) = extract(a.client, a.resfiles)
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(text)
    print("pi_statics: %d pin types, %d schematics, %d planet types, %d item types -> %s"
          % (pins, schematics, resources, items, a.out), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
