#!/usr/bin/env python3
"""CLI over `tools/fsd.py`: look at the client's static-data corpus.

**Inside `tools/analysis.sh`, pass both paths explicitly.** `$EVE_CLIENT` is a
host path and the container's `$HOME` is `/`, so the bare invocation dies with
`FileNotFoundError` on `/.local/share/...`. The install is mounted at
`/client`, with `ResFiles` as its sibling:

    tools/analysis.sh python3 /work/tools/fsd_dump.py \
        --client /client --resfiles /client-root/ResFiles <subcommand>

`--client` goes **after** the subcommand for any subparser that declares it
too, or the subparser's own `None` wins. Two lanes lost time to both of these
on 2026-10-22.

    tools/fsd_dump.py list                       every res:/staticdata/* resource
    tools/fsd_dump.py schema <resource>          the FSD schema, as a tree
    tools/fsd_dump.py rows <resource> [-n N]     N rows, decoded
    tools/fsd_dump.py keys <resource>            just the primary keys
    tools/fsd_dump.py header <resource>          cFSD container header
    tools/fsd_dump.py raw <resource> [-k KEY]    raw record slots of a cFSD row
    tools/fsd_dump.py profile <resource>         per-slot statistics of a cFSD table
    tools/fsd_dump.py names <resource>           attribute names from the native loader
    tools/fsd_dump.py dungeon <id>               one dungeon's rooms and object types
    tools/fsd_dump.py inventory                  the corpus-inventory.tsv rows

A resource may be given in full (``res:/staticdata/types.fsdbinary``) or as a
bare name (``types``), which is resolved against ``res:/staticdata/``.

Message-id columns print as integers. Resolve them with ``tools/loctable.py
id <N>``, which owns the localisation corpus; ``--loc`` asks this tool to do
that for you by importing it.

Stdlib only. Read-only. See the `fsd` module docstring for the formats, and
``docs/re/static-corpus.md`` for the prose.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import os
import sys

# `tools/` itself is already on the path -- the rotation above put it last,
# behind the stdlib, which is the whole point. See that comment.

import fsd  # noqa: E402


def resolve(client, name):
    if name.startswith("res:"):
        return name
    for ext in (
        "",
        ".fsdbinary",
        ".static",
        ".pickle",
        ".schema",
    ):
        cand = "res:/staticdata/%s%s" % (name, ext)
        if cand in client.index:
            return cand
    raise SystemExit("fsd_dump: no such static-data resource: %s" % name)


def _loc(enabled):
    if not enabled:
        return None
    try:
        import loctable
    except ImportError:
        return None
    lang = loctable.load_lang("en-us")
    msgs = lang[1] if isinstance(lang, tuple) else lang
    return msgs


def _text(msgs, mid, width=70):
    if not msgs:
        return mid
    e = msgs.get(mid)
    if e is None:
        return mid
    t = e[0] if isinstance(e, (tuple, list)) else e
    t = str(t).replace("\n", " ")
    return "%d %r" % (mid, t[:width])


# --- commands --------------------------------------------------------------


def cmd_list(a, c):
    names = c.staticdata()
    for n in names:
        try:
            kind = c.kind(n)
        except Exception as exc:  # noqa: BLE001
            kind = "error:%s" % exc
        print("%-10s %12d  %s" % (kind, c.index[n]["size"], n))
    print("# %d resources" % len(names))


def cmd_schema(a, c):
    res = resolve(c, a.resource)
    print("\n".join(fsd.describe(c.schema(res), maxdepth=a.depth)))


def cmd_rows(a, c):
    res = resolve(c, a.resource)
    kind = c.kind(res)
    msgs = _loc(a.loc)
    if kind == "cfsd":
        t = fsd.CfsdTable(c.path(res), res.split("/")[-1])
        rows = fsd.cfsd_rows(t, limit=a.n)
        for k in sorted(rows)[: a.n]:
            parts = []
            for name, v in rows[k].items():
                if isinstance(v, list):
                    v = "[%d items]" % len(v)
                elif name.endswith("ID") and name.endswith("NameID"):
                    v = _text(msgs, v)
                parts.append("%s=%s" % (name, v))
            print("%-10s %s" % (k, "  ".join(parts)))
        return
    obj = c.load(res)
    if isinstance(obj, dict) and "_connection" in obj:
        obj.pop("_connection").close()
        print("# fsdlite sqlite cache; JSON fields: %s"
              % ", ".join(fsd.sqlite_json_fields(c.path(res))))
        for i, (k, v) in enumerate(fsd.sqlite_rows(c.path(res), "cache", a.n)):
            if i >= a.n:
                break
            print("%-12s %r" % (k, v))
        return
    if isinstance(obj, (fsd.FsdDict, fsd.FsdMultiIndex)):
        print("# %d rows, value type %s" % (len(obj), obj.value_type))
        if obj.columns:
            print("# columns: %s" % ", ".join(obj.columns))
        for i, (k, v) in enumerate(obj.items()):
            if i >= a.n:
                break
            print("%-12s %r" % (k, fsd._unwrap(v, a.depth)))  # noqa: SLF001
        return
    if isinstance(obj, fsd.FsdList):
        for i, v in enumerate(obj):
            if i >= a.n:
                break
            print("%-6d %r" % (i, fsd._unwrap(v, a.depth)))  # noqa: SLF001
        return
    if isinstance(obj, dict):
        for i, (k, v) in enumerate(sorted(obj.items(), key=lambda kv: repr(kv[0]))):
            if i >= a.n:
                break
            print("%-12r %r" % (k, v))
        return
    print(repr(obj)[:4000])


def cmd_keys(a, c):
    res = resolve(c, a.resource)
    if c.kind(res) == "cfsd":
        ks = fsd.CfsdTable(c.path(res), res.split("/")[-1]).keys()
    else:
        obj = c.load(res)
        ks = list(obj.keys())
    print(" ".join(str(k) for k in ks[: a.n]))
    print("# %d keys" % len(ks))


def cmd_header(a, c):
    res = resolve(c, a.resource)
    t = fsd.CfsdTable(c.path(res), res.split("/")[-1])
    print("resource      %s" % res)
    print("file          %s (%d bytes)" % (t.path, len(t.raw)))
    print("tag           %s" % t.tag.hex())
    print("payloadSize   %d  (file-32 = %d, match=%s)"
          % (t.payload_size, len(t.raw) - 32, t.prologue_ok))
    print("headerSize    %d" % t.header_size)
    print("rowCount      %d" % t.row_count)
    print("bucketCount   %d" % t.bucket_count)
    print("structure ok  %s%s" % (t.ok, "" if t.ok else " (%s)" % t.why))
    print("stride        %s" % t.stride)


def cmd_raw(a, c):
    res = resolve(c, a.resource)
    t = fsd.CfsdTable(c.path(res), res.split("/")[-1])
    if t.stride is None:
        raise SystemExit("fsd_dump: record stride unknown for %s" % res)
    msgs = _loc(a.loc)
    keys = t.keys()
    want = [a.key] if a.key is not None else keys[: a.n]
    stem = res.split("/")[-1].split(".")[0]
    known = fsd.CFSD_COLUMNS.get(stem, {})
    import struct

    for k in want:
        print("=== %s key=%s (stride %d)" % (stem, k, t.stride))
        r = dict(t._locate()).get(k)  # noqa: SLF001
        if r is None:
            # _locate() yields (key, offset); dict() above inverts wrongly for
            # duplicate keys, which cannot happen, but be explicit anyway.
            for kk, rr in t._locate():  # noqa: SLF001
                if kk == k:
                    r = rr
                    break
        for off in range(0, t.stride, 4):
            v = struct.unpack_from("<I", t.pl, r + off)[0]
            note = []
            if off in known:
                note.append("%s (%s)" % known[off])
            if off % 8 == 0 and off + 8 <= t.stride:
                d = struct.unpack_from("<d", t.pl, r + off)[0]
                if d and 1e-6 < abs(d) < 1e15:
                    note.append("f64=%.6g" % d)
                s = t.maybe_string(v)
                if s:
                    note.append("str=%r" % s[:50])
            # Only guess at a message id when the value is big enough that
            # the guess is worth anything: ids used as FSD columns in this
            # corpus all run above 50,000, while small ints like a groupID
            # also happen to be valid ids and resolve to nonsense.
            if msgs and v >= 50000 and v in msgs:
                note.append("msg?=%s" % _text(msgs, v, 40))
            print("  +%-4d %-12d %s" % (off, v, "  ".join(note)))


def cmd_profile(a, c):
    res = resolve(c, a.resource)
    t = fsd.CfsdTable(c.path(res), res.split("/")[-1])
    if t.stride is None:
        raise SystemExit("fsd_dump: record stride unknown for %s" % res)
    for i, s in enumerate(t.profile(sample=a.n)):
        print(
            "slot %-3d (+%-4d) min=%-14d max=%-14d zero=%-6d strings=%-5d %s"
            % (i, 8 * i, s["min"], s["max"], s["zero"], s["strings"],
               s["examples"] or "")
        )


def cmd_names(a, c):
    res = resolve(c, a.resource)
    names = fsd.cfsd_attribute_names(c, res)
    if names is None:
        raise SystemExit("fsd_dump: no native loader found for %s" % res)
    print("# attribute names from the table's own .pyd. Unordered, untyped,")
    print("# [I] not a column list -- see docs/re/static-corpus.md.")
    for n in names:
        print("  %s" % n)


def cmd_dungeon(a, c):
    t = fsd.CfsdTable(c.path("res:/staticdata/dungeons.fsdbinary"), "dungeons")
    msgs = _loc(True)
    import struct

    did = int(a.resource)
    for k, r in t._locate():  # noqa: SLF001
        if k != did:
            continue
        print("dungeon %d" % did)
        for off, (name, kind) in sorted(fsd.CFSD_COLUMNS["dungeons"].items()):
            if kind == "msgid":
                print("  %-22s %s" % (name, _text(msgs, struct.unpack_from("<I", t.pl, r + off)[0], 100)))
            elif kind == "u32":
                print("  %-22s %d" % (name, struct.unpack_from("<I", t.pl, r + off)[0]))
        break
    types = {}
    try:
        T = fsd.CfsdTable(c.path("res:/staticdata/types.fsdbinary"), "types")
        for tk, tr in T._locate():  # noqa: SLF001
            types[tk] = struct.unpack_from("<I", T.pl, tr + 132)[0]
    except Exception:  # noqa: BLE001
        pass
    for rid, objs in fsd.dungeon_rooms(t, did):
        import collections

        cnt = collections.Counter(objs)

        def label(type_id):
            """``<typeID> '<name>'``.

            **The typeID, not the name id.** This used to print
            `_text(msgs, types[o])`, which renders as `<messageID> '<text>'` --
            so the number beside each object name was its `typeNameID`. That
            made Veldspar read as `1031140 'Veldspar'` where the independent
            `asteroidcountbytypeidroomiddungeonid.static` says typeID `1230`,
            and the apparent disagreement was reported as a real one. There
            was never a second type id; there was a mislabelled column.
            """
            name = _text(msgs, types[type_id], 30) if type_id in types else None
            if isinstance(name, str) and "'" in name:
                name = name[name.index("'"):]
            return "%d %s" % (type_id, name) if name else str(type_id)

        pretty = {label(o): n for o, n in cnt.most_common(12)}
        print("  room %-8d objects=%-4d %s" % (rid, len(objs), pretty))


def cmd_inventory(a, c):
    for line in inventory_rows(c):
        print("\t".join(line))


def inventory_rows(c):
    """Rows of ``findings/corpus-inventory.tsv``. Shape only, never content."""
    header = [
        "resource",
        "bytes",
        "format",
        "readable",
        "rows",
        "key",
        "columns_or_slots",
        "notes",
    ]
    yield header
    for res in c.staticdata():
        size = c.index[res]["size"]
        try:
            kind = c.kind(res)
        except Exception as exc:  # noqa: BLE001
            yield [res, str(size), "unknown", "no", "unknown", "unknown",
                   "unknown", "kind failed: %s" % exc]
            continue
        try:
            yield _inventory_row(c, res, size, kind)
        except Exception as exc:  # noqa: BLE001
            yield [res, str(size), kind, "no", "unknown", "unknown", "unknown",
                   "failed: %s" % type(exc).__name__]


def _inventory_row(c, res, size, kind):
    import struct

    if kind == "cfsd":
        t = fsd.CfsdTable(c.path(res), res.split("/")[-1])
        stem = res.split("/")[-1].split(".")[0]
        known = fsd.CFSD_COLUMNS.get(stem)
        if not t.ok:
            return [res, str(size), "cfsd", "header-only", "unknown",
                    "unknown", "unknown", t.why]
        st = t.stride
        cols = "unknown"
        note = "container decoded; record layout native (bin64/%sLoader.pyd)" % stem
        readable = "keys"
        if st is None:
            readable = "rowcount"
            note = "stride undetermined (too few multi-row buckets); " + note
        if known:
            cols = ";".join(
                "%s@+%d:%s" % (n, o, k) for o, (n, k) in sorted(known.items())
            )
            readable = "columns"
            note = "confirmed columns only; rest of record unknown"
        return [res, str(size), "cfsd", readable, str(t.row_count),
                "uint32 (hash %% %d buckets)" % t.bucket_count,
                cols if cols != "unknown" else ("stride=%s" % st), note]

    if kind == "sqlite":
        meta = c.load(res)
        con = meta.pop("_connection")
        tables = sorted(meta)
        rows = meta.get("cache", {}).get("rows", 0)
        con.close()
        fields = fsd.sqlite_json_fields(c.path(res))
        cols = ";".join(fields[:24])
        if len(fields) > 24:
            cols += ";...(%d fields; keys look data-dependent)" % len(fields)
        return [res, str(size), "sqlite", "yes", str(rows),
                "string (cache.key)", cols or "unknown",
                "fsdlite sqlite cache: cache(key,value JSON,time) + "
                "indexes(key,value); tables=%s" % ",".join(tables)]

    if kind in ("classic", "classic-sidecar"):
        sch = c.schema(res)
        obj = c.load(res)
        top = sch.get("type")
        if top == "dict":
            n = len(obj)
            key = sch["keyTypes"].get("type")
            ct = fsd.column_types(sch)
            if ct:
                cols = ";".join("%s:%s" % (a, b) for a, b in ct)
            else:
                cols = "value type %s" % sch["valueTypes"].get("type")
            flags = [f for f in ("buildIndex", "multiIndex") if sch.get(f)]
        elif top == "list":
            n = len(obj)
            key = "index"
            ct = None
            it = sch["itemTypes"]
            if it.get("type") == "object":
                cols = ";".join(
                    "%s:%s" % (k, it["attributes"][k].get("type"))
                    for k in sorted(it["attributes"])
                )
            else:
                cols = "item type %s" % it.get("type")
            flags = []
        else:
            n = 1
            key = "-"
            cols = top
            flags = []
        note = "embedded schema" if kind == "classic" else "schema in sidecar .schema"
        if flags:
            note += "; " + ",".join(flags)
        return [res, str(size), "classic", "yes", str(n), str(key), cols, note]

    if kind == "pickle":
        obj = c.load(res)
        if isinstance(obj, dict):
            k0 = next(iter(obj)) if obj else None
            return [res, str(size), "pickle", "yes", str(len(obj)),
                    type(k0).__name__ if k0 is not None else "-",
                    "value type %s" % (type(obj[k0]).__name__ if obj else "-"),
                    "plain Python pickle"]
        return [res, str(size), "pickle", "yes", "1", "-",
                type(obj).__name__, "plain Python pickle"]

    if kind == "schema":
        sch = c.load(res)
        return [res, str(size), "schema", "yes", "-", "-",
                "optimized FSD schema, type=%s" % sch.get("type"),
                "YAML sidecar for the matching .static"]

    return [res, str(size), kind, "no", "unknown", "unknown", "unknown", ""]


def main(argv=None):
    def add_common(parser, sub):
        """The flags every subcommand takes, on either side of the subcommand.

        These used to come from one ``parents=[common]`` parser given to both
        the top-level parser and every subparser. A subparser copies its whole
        namespace over the outer one, defaults included, so ``--client X -n 30
        rows T`` parsed the flags and then reset them to ``None`` and ``10``
        (bpo-9351; `tools/dogma.py` had the same bug, `docs/port-tracker.md`
        §62). `argparse.SUPPRESS` on the subparser side makes a key exist only
        for a flag actually given, so either placement works.
        """
        d = (lambda v: argparse.SUPPRESS) if sub else (lambda v: v)
        parser.add_argument("--client", default=d(None),
                            help="client install dir (default $EVE_CLIENT)")
        parser.add_argument("--resfiles", default=d(None),
                            help="ResFiles dir (default: sibling of --client)")
        parser.add_argument("-n", type=int, default=d(10),
                            help="how many rows/keys to show")
        parser.add_argument("--depth", type=int, default=d(3),
                            help="nesting depth to render")
        parser.add_argument("--loc", action="store_true", default=d(False),
                            help="resolve message ids via tools/loctable.py")

    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    add_common(p, sub=False)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name, fn, needs_res in (
        ("list", cmd_list, False),
        ("schema", cmd_schema, True),
        ("rows", cmd_rows, True),
        ("keys", cmd_keys, True),
        ("header", cmd_header, True),
        ("raw", cmd_raw, True),
        ("profile", cmd_profile, True),
        ("names", cmd_names, True),
        ("dungeon", cmd_dungeon, True),
        ("inventory", cmd_inventory, False),
    ):
        s = sub.add_parser(name)
        add_common(s, sub=True)
        if needs_res:
            s.add_argument("resource")
        if name == "raw":
            s.add_argument("-k", "--key", type=int)
        s.set_defaults(func=fn)
    a = p.parse_args(argv)
    c = fsd.Client(a.client, a.resfiles)
    a.func(a, c)


if __name__ == "__main__":
    main()
