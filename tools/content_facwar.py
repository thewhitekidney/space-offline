"""The `facwar` content section: the client's warzone territory list (FW-3, #99).

`handback/r-fw/report.md` §4. Writes two bundle files:

    facwar/systems.tsv   `system_id warzone_id`, one row per system the client
                         draws as warzone territory
    facwar/max_lp.tsv    `group_id max_lp`, one row per hull group of
                         `appConst.facwarMaxLPPayout` (#133): the most one
                         kill of that group pays. Read from the bytecode of
                         `out/pyc`; with no `out/pyc` the file is not written
                         and the server falls back to `facwarMaxLPUnknownGroup`.

The source is `warzonespositiondata.fsdbinary` [V, R-FW §2]:
`{warzoneID: {systems: {ssid: {solarSystemId, isVisible, territorySprite,
...}}}}`. A system is territory when its row is visible **and** carries a
`territorySprite` (`mapData.GetWarzoneSystems`, 163 systems on build
3662168). The 8 hidden rows with a sprite stay out (R-FW §7 unknown). The
other rows are navigation systems and are not written.

The derivation is pure (plain dicts in, rows out); the unit tests feed it
invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "facwar"
DEPENDS = ()
CFSD = ("warzonespositiondata",)

_HERE = os.path.dirname(os.path.abspath(__file__))

HEADER = ("system_id", "warzone_id")
SOURCES = ["res:/staticdata/warzonespositiondata.fsdbinary"]

MAX_LP_HEADER = ("group_id", "max_lp")
MAX_LP_SOURCES = ["pyc:eve/common/lib/appConst.pyc", "pyc:inventorycommon/const.pyc"]
APPCONST = os.path.join("eve", "common", "lib", "appConst.pyc")
GROUP_CONST = os.path.join("inventorycommon", "const.pyc")
MAX_LP_NAME = "facwarMaxLPPayout"


def max_lp_rows(ops, groups):
    """`(rows, problems)` from the disassembly of `appConst` and `{name: id}`
    of `inventorycommon/const`.

    `ops` is `[(op, arg, repr)]`. The dict is built as `BUILD_MAP` then, per
    entry, `LOAD_CONST value`, `LOAD_NAME key`, `STORE_MAP`, then
    `STORE_NAME facwarMaxLPPayout`. A key that is no known group name, or a
    value that is not a plain number, is a problem."""
    rows, problems = [], []
    end = next((i for i, o in enumerate(ops)
                if o[0] == "STORE_NAME" and o[2] == MAX_LP_NAME), None)
    if end is None:
        return rows, ["%s: not found" % MAX_LP_NAME]
    i = end - 1
    entries = []
    while i >= 2 and ops[i][0] == "STORE_MAP":
        entries.append((ops[i - 1], ops[i - 2]))
        i -= 3
    if i < 0 or ops[i][0] != "BUILD_MAP":
        return rows, ["%s: not a literal dict" % MAX_LP_NAME]
    for key, val in reversed(entries):
        if key[0] != "LOAD_NAME" or val[0] != "LOAD_CONST":
            problems.append("%s: entry %r %r is not name: constant" % (MAX_LP_NAME, key, val))
        elif key[2] not in groups:
            problems.append("%s: %s is no group constant" % (MAX_LP_NAME, key[2]))
        else:
            try:
                rows.append((groups[key[2]], int(val[2])))
            except ValueError:
                problems.append("%s: %s has value %r" % (MAX_LP_NAME, key[2], val[2]))
    rows.sort()
    ids = [r[0] for r in rows]
    if len(set(ids)) != len(ids):
        problems.append("%s: a group appears twice" % MAX_LP_NAME)
    return rows, problems


def _pyc_root():
    return os.path.join(os.path.dirname(_HERE), "out", "pyc")


def _max_lp(root=None):
    """`(rows, problems)`, or `None` when `out/pyc` has not been unpacked."""
    root = root or _pyc_root()
    a, g = os.path.join(root, APPCONST), os.path.join(root, GROUP_CONST)
    if not (os.path.isfile(a) and os.path.isfile(g)):
        return None
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import pyc27
    co = pyc27.load_pyc(g)
    groups, pend = {}, None
    for _o, op, arg, rep in pyc27.dis(co):
        if op == "LOAD_CONST":
            pend = co.consts[arg]
        elif op == "STORE_NAME" and isinstance(pend, int) and not isinstance(pend, bool):
            groups[rep] = pend
            pend = None
        else:
            pend = None
    ops = [(op, arg, rep) for _o, op, arg, rep in pyc27.dis(pyc27.load_pyc(a))]
    return max_lp_rows(ops, groups)


def derive(zones):
    """`(rows, problems)` from `{warzoneID: row}`.

    A system listed under two warzones, or a row whose `solarSystemId` does
    not match its key, is a problem (the data is not what this reads)."""
    rows, problems, seen = [], [], {}
    for zone in sorted(zones):
        systems = zones[zone].get("systems") or {}
        for key in sorted(systems, key=lambda k: int(k)):
            r = systems[key]
            if not r.get("isVisible") or not r.get("territorySprite"):
                continue
            sid = r.get("solarSystemId", int(key))
            if int(key) != sid:
                problems.append("warzone %s: key %s names system %s" % (zone, key, sid))
                continue
            if sid in seen:
                problems.append("system %d is territory of warzones %d and %d"
                                % (sid, seen[sid], int(zone)))
                continue
            seen[sid] = int(zone)
            rows.append((sid, int(zone)))
    rows.sort()
    return rows, problems


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _derive(ctx):
    fsd = _content()._imp("fsd")  # noqa: SLF001
    return derive(fsd.rows(ctx.client, "warzonespositiondata"))


def section(ctx):
    rows, problems = _derive(ctx)
    if problems:
        raise ValueError("facwar: " + "; ".join(problems[:20]))
    per = {}
    for _s, z in rows:
        per[z] = per.get(z, 0) + 1
    ctx.note("facwar/systems.tsv: %d systems (%s)"
             % (len(rows), ", ".join("warzone %d: %d" % kv for kv in sorted(per.items()))))
    out = [("facwar/systems.tsv", HEADER, rows, SOURCES)]
    got = _max_lp()
    if got is None:
        ctx.note("facwar/max_lp.tsv: out/pyc is not unpacked, not written")
        return out
    lp, lp_problems = got
    if lp_problems:
        raise ValueError("facwar: " + "; ".join(lp_problems[:20]))
    ctx.note("facwar/max_lp.tsv: %d hull groups" % len(lp))
    out.append(("facwar/max_lp.tsv", MAX_LP_HEADER, lp, MAX_LP_SOURCES))
    return out


def check(ctx):
    content = _content()
    rows, problems = _derive(ctx)
    bad = list(problems)
    got = list(content.read_rows(ctx.path("facwar/systems.tsv")))
    if got != [content.encode_row(r).split("\t") for r in rows]:
        bad.append("facwar/systems.tsv: differs from a fresh derive (%d vs %d rows)"
                   % (len(got), len(rows)))
    got_lp = _max_lp()
    if got_lp is not None:
        lp, lp_problems = got_lp
        bad.extend(lp_problems)
        have = list(content.read_rows(ctx.path("facwar/max_lp.tsv")))
        if have != [content.encode_row(r).split("\t") for r in lp]:
            bad.append("facwar/max_lp.tsv: differs from a fresh derive (%d vs %d rows)"
                       % (len(have), len(lp)))
    return bad
