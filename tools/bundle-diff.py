#!/usr/bin/env python3
"""Say how two content bundles differ (PORT-BUNDLE, T3 of r-port-1109).

  python3 -I tools/bundle-diff.py OLD NEW [--out FILE] [--uses] [--root DIR]

OLD and NEW are bundle directories (each holds a `MANIFEST.tsv`). The manifest's
sha256 per file is compared first, so an equal file costs nothing; a differing
file is diffed row by row, keyed by its leading columns (`KEYS`: `typedata.tsv`
by (row kind, id), `npc/components.tsv` by (type, component), the rest by their
first column unless listed). Per file it prints rows added, removed and changed,
with ids and the changed column names.

Flags, each a thing a reader must look at before trusting the new bundle:

  REMOVED     an id present in OLD and gone from NEW
  CLASS       a type's group, category or published bit changed (typedata `type`)
  DOGMA       an attribute, effect, attrmeta or modifier row changed or went
  COMPONENTS  a component's row count in `npc/components.tsv` changed
              (a wrong presence bit shows here: `refuse_universal_components`)

`--uses` greps the changed ids (four digits or more, or noise swamps it) against
`server/crates`, `content/` and `out/geode-picks.local.tsv` under `--root`
(default: the repository). Read-only. Exit 0: equal files; 1: differences, none
flagged; 3: at least one flag. Compare bundles extracted by the same extractor
commit (the manifest prints both `extractor` hashes), or the difference is the
extractor's, not the client's.

Std-only. No client byte is kept here: it reads bundles the caller points at.
"""

from __future__ import annotations

import argparse
import collections
import os
import re
import sys

MANIFEST = "MANIFEST.tsv"

#: Leading columns that make a row's key. Everything else keys on column 0.
KEYS = {
    "types/typedata.tsv": 2,
    "npc/components.tsv": 2,
    "dungeons/objects.tsv": 3,
}

#: `typedata.tsv` has no header line; these are the columns of a `type` row that
#: count as the type's class (`tools/typedata.py`: kind id group category published).
TYPE_CLASS = {2: "groupID", 3: "categoryID", 4: "published"}
TYPE_COLS = {5: "portionSize", 6: "volume", 7: "capacity", 8: "mass", 9: "radius",
             10: "defaultEffectID", 11: "effectIDs", 12: "attrs", 13: "name"}
TYPE_DOGMA = ("effectIDs", "attrs", "defaultEffectID")
DOGMA_KINDS = ("attribute", "effect", "attrmeta", "modifier")
TYPEDATA = "types/typedata.tsv"
COMPONENTS = "npc/components.tsv"
MIN_USES_ID = 1000  # a shorter id matches half the source tree


def read_manifest(d):
    """`(singletons, {relpath: sha256})` of the bundle in `d`."""
    single, files = {}, {}
    with open(os.path.join(d, MANIFEST), "r", encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if line.startswith("#") or len(f) < 2:
                continue
            if f[0] == "file" and len(f) >= 4:
                files[f[1]] = f[3]
            elif f[0] in ("format", "client_build", "extractor", "systems", "data_hash"):
                single[f[0]] = f[1]
    return single, files


def read_rows(path):
    """`(header or None, rows)`; the header is the first `#name<TAB>name` line."""
    header, rows = None, []
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if line.startswith("#"):
                if header is None and "\t" in line and not line.startswith("# "):
                    header = line[1:].split("\t")
                continue
            if line:
                rows.append(line.split("\t"))
    return header, rows


def group(rows, nkey):
    g = collections.defaultdict(list)
    for r in rows:
        g[tuple(r[:nkey])].append(r)
    return g


class FileDiff:
    def __init__(self, rel, header, nold, nnew):
        self.rel, self.header, self.nold, self.nnew = rel, header, nold, nnew
        self.added, self.removed, self.changed = [], [], {}  # keys; keys; key -> column names
        self.rows = {}  # key -> (old rows, new rows) for the flag checks
        self.header_differs = False


def diff_file(rel, old_path, new_path):
    ha, ra = read_rows(old_path)
    hb, rb = read_rows(new_path)
    d = FileDiff(rel, hb or ha, len(ra), len(rb))
    if ha != hb:
        d.header_differs = True
        return d
    nkey = KEYS.get(rel, 1)
    ga, gb = group(ra, nkey), group(rb, nkey)
    d.added = sorted(set(gb) - set(ga), key=_ord)
    d.removed = sorted(set(ga) - set(gb), key=_ord)
    for k in sorted(set(ga) & set(gb), key=_ord):
        if sorted(ga[k]) == sorted(gb[k]):
            continue
        d.rows[k] = (ga[k], gb[k])
        cols = set()
        if len(ga[k]) == 1 and len(gb[k]) == 1:
            for i, (x, y) in enumerate(zip(ga[k][0], gb[k][0])):
                if x != y:
                    cols.add(_col(rel, d.header, ga[k][0], i))
            if len(ga[k][0]) != len(gb[k][0]):
                cols.add("(width)")
        else:
            cols.add("(%d -> %d rows)" % (len(ga[k]), len(gb[k])))
        d.changed[k] = sorted(cols)
    d.rows_removed = {k: ga[k] for k in d.removed}
    return d


def _ord(k):
    return tuple((0, int(p), "") if re.fullmatch(r"-?\d+", p) else (1, 0, p) for p in k)


def _col(rel, header, row, i):
    if rel == TYPEDATA and row and row[0] == "type" and (i in TYPE_CLASS or i in TYPE_COLS):
        return TYPE_CLASS.get(i) or TYPE_COLS[i]
    if header and i < len(header):
        return header[i]
    return "col%d" % i


def flags_for(d, old_dir, new_dir):
    """`[(flag, text)]` for one differing file."""
    out = []
    if d.removed:
        out.append(("REMOVED", "%s: %s" % (d.rel, _keys(d.removed))))
    if d.rel == TYPEDATA:
        cls = [k for k, (a, b) in d.rows.items()
               if k[0] == "type" and len(a) == 1 and len(b) == 1
               and any(a[0][i] != b[0][i] for i in TYPE_CLASS if i < len(a[0]) and i < len(b[0]))]
        if cls:
            out.append(("CLASS", "types whose group, category or published changed: %s" % _keys(cls)))
        dg = [k for k in list(d.changed) + d.removed if k[0] in DOGMA_KINDS]
        if dg:
            out.append(("DOGMA", "changed or removed dogma rows: %s" % _keys(dg)))
        td = [k for k, cs in d.changed.items() if k[0] == "type" and any(c in TYPE_DOGMA for c in cs)]
        if td:
            out.append(("DOGMA", "types whose effects or attributes changed: %s" % _keys(td)))
    if d.rel == COMPONENTS:
        ca = _component_counts(os.path.join(old_dir, d.rel))
        cb = _component_counts(os.path.join(new_dir, d.rel))
        moved = {c: (ca.get(c, 0), cb.get(c, 0)) for c in set(ca) | set(cb) if ca.get(c, 0) != cb.get(c, 0)}
        if moved:
            out.append(("COMPONENTS", "per-component row counts: " + ", ".join(
                "%s %d -> %d" % (c, a, b) for c, (a, b) in sorted(moved.items()))))
    return out


def _component_counts(path):
    _h, rows = read_rows(path)
    return collections.Counter(r[1] for r in rows if len(r) > 1)


def _keys(keys, limit=40):
    s = ["/".join(k) for k in keys[:limit]]
    return ", ".join(s) + (" ... (%d in all)" % len(keys) if len(keys) > limit else "")


def changed_ids(d):
    """Numeric ids named by a difference in `d`."""
    ids = set()
    for k in d.added + d.removed + list(d.changed):
        ids.update(p for p in k if p.isdigit())
    return ids


def find_uses(ids, root):
    """`{id: {path: count}}` for ids of MIN_USES_ID or more, as whole numbers."""
    wanted = {i for i in ids if int(i) >= MIN_USES_ID}
    if not wanted:
        return {}
    pat = re.compile(r"(?<![\w.])(%s)(?![\w.])" % "|".join(sorted(wanted, key=len, reverse=True)))
    found = collections.defaultdict(lambda: collections.Counter())
    targets = [os.path.join(root, "server", "crates"), os.path.join(root, "content")]
    files = [os.path.join(root, "out", "geode-picks.local.tsv")]
    for t in targets:
        for dp, dn, fn in os.walk(t):
            dn[:] = [x for x in dn if x != "target"]
            files.extend(os.path.join(dp, f) for f in fn)
    for p in files:
        if not os.path.isfile(p) or os.path.getsize(p) > 8 << 20:
            continue
        try:
            with open(p, "r", encoding="utf-8") as fh:
                for line in fh:
                    for m in pat.finditer(line):
                        found[m.group(1)][os.path.relpath(p, root)] += 1
        except (OSError, UnicodeDecodeError):
            continue
    return found


def report(old_dir, new_dir, uses=False, root=None):
    """`(text, exit code)`."""
    so, fo = read_manifest(old_dir)
    sn, fn = read_manifest(new_dir)
    lines = []
    w = lines.append
    w("bundle-diff OLD %s (build %s, extractor %s)" % (old_dir, so.get("client_build", "?"), so.get("extractor", "?")[:12]))
    w("            NEW %s (build %s, extractor %s)" % (new_dir, sn.get("client_build", "?"), sn.get("extractor", "?")[:12]))
    if so.get("extractor") != sn.get("extractor"):
        w("NOTE: the extractors differ; some differences below may be the extractor's, not the client's.")
    only_old, only_new = sorted(set(fo) - set(fn)), sorted(set(fn) - set(fo))
    common = sorted(set(fo) & set(fn))
    differ = [f for f in common if fo[f] != fn[f]]
    w("files: %d in OLD, %d in NEW, %d differ, %d equal" % (len(fo), len(fn), len(differ), len(common) - len(differ)))
    if only_old:
        w("only in OLD: " + ", ".join(only_old))
    if only_new:
        w("only in NEW: " + ", ".join(only_new))
    flags, ids = [], set()
    for rel in differ:
        try:
            d = diff_file(rel, os.path.join(old_dir, rel), os.path.join(new_dir, rel))
        except OSError as e:
            w("== %s: unreadable (%s)" % (rel, e))
            continue
        w("== %s rows %d -> %d" % (rel, d.nold, d.nnew))
        if d.header_differs:
            w("   HEADER DIFFERS; not row-diffed")
            flags.append(("HEADER", rel))
            continue
        w("   added %d, removed %d, changed %d" % (len(d.added), len(d.removed), len(d.changed)))
        if d.added:
            w("   added: " + _keys(d.added))
        if d.removed:
            w("   removed: " + _keys(d.removed))
        cols = collections.Counter(c for cs in d.changed.values() for c in cs)
        if cols:
            w("   changed columns: " + ", ".join("%s x%d" % (c, n) for c, n in cols.most_common(12)))
            w("   changed: " + _keys(list(d.changed)))
        ids |= changed_ids(d)
        flags.extend((f, t) for f, t in flags_for(d, old_dir, new_dir))
    if only_old:
        flags.append(("REMOVED", "files gone from NEW: " + ", ".join(only_old)))
    w("")
    if flags:
        w("FLAGS (%d)" % len(flags))
        for f, t in flags:
            w("  %-10s %s" % (f, t))
    else:
        w("FLAGS: none")
    if uses:
        u = find_uses(ids, root or os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        w("")
        w("USES of changed ids (>= %d) in server/crates, content/, out/geode-picks.local.tsv:" % MIN_USES_ID)
        if not u:
            w("  none")
        for i in sorted(u, key=int):
            w("  %s: %s" % (i, ", ".join("%s x%d" % (p, n) for p, n in sorted(u[i].items()))))
    code = 3 if flags else (1 if (differ or only_new) else 0)
    return "\n".join(lines) + "\n", code


def main(argv=None):
    ap = argparse.ArgumentParser(description="Row-level diff of two content bundles.")
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--out", metavar="FILE", help="write the report here as well as stdout")
    ap.add_argument("--uses", action="store_true", help="grep changed ids against the sources")
    ap.add_argument("--root", metavar="DIR", help="repository root for --uses")
    a = ap.parse_args(argv)
    for d in (a.old, a.new):
        if not os.path.isfile(os.path.join(d, MANIFEST)):
            ap.error("%s: no %s" % (d, MANIFEST))
    text, code = report(a.old, a.new, a.uses, a.root)
    sys.stdout.write(text)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as fh:
            fh.write(text)
    return code


if __name__ == "__main__":
    sys.exit(main())
