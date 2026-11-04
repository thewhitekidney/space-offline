#!/usr/bin/env python3 -I
"""List numeric client constants still written by hand in non-test Rust (#105).

A "hand-kept value" is a `const`/`static` integer whose name says it is a client
id (TYPE, GROUP, ATTR, EFFECT, CATEGORY, SKILL, FACTION, ..._ID) and that sits
outside test code. Such values should come from the content bundle instead.

Output is a TSV: file, line, name, mark, subsystem[, value]. The value column
holds real client values, so the TSV is only ever written under `out/`
(gitignored); the console shows counts per subsystem, never values.
Invented test ids are skipped by the project's invented ranges (schema.md 1.1):
99xxxx, systems 39000001+, stations 69000001+, factions 599001+,
corporations 1999001+.

Usage: python3 -I tools/hand-data.py [--root server/crates] [--out out/hand-data.tsv]
Stdlib only.
"""
from __future__ import annotations

import os
import re
import sys

NAME_HINT = re.compile(r"(TYPE|GROUP|ATTR|EFFECT|CATEGORY|SKILL|FACTION|(^|_)ID($|_))")
CONST = re.compile(
    r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:\s*"
    r"(?:i|u)(?:8|16|32|64|size)\s*=\s*(-?[0-9][0-9_]*)\s*;(.*)$"
)
MARK = re.compile(r"\[(V|A|I)(?:-[a-z]+)?\]")
MIN_VALUE = 10  # tiny numbers are counts and flags, not ids


def invented(v: int) -> bool:
    return (
        990000 <= v <= 999999
        or v >= 39000001 and 39000001 <= v < 40000000
        or 69000001 <= v < 70000000
        or 599001 <= v <= 599999
        or 1999001 <= v <= 1999999
    )


def is_test_file(path: str) -> bool:
    parts = path.replace("\\", "/").split("/")
    return "tests" in parts[:-1] or parts[-1] in ("tests.rs", "test.rs") or parts[-1].endswith("_tests.rs")


def scan(text: str, path: str):
    """Yield (line, name, mark, value) for hand-kept constants outside tests."""
    if is_test_file(path):
        return
    depth = 0
    skip_until = None  # brace depth at which a cfg(test) item ends
    pending_cfg = False
    doc = []  # preceding comment lines
    for n, raw in enumerate(text.split("\n"), 1):
        line = raw.strip()
        if skip_until is None:
            if re.match(r"#\[(cfg\(test\)|test)\]", line):
                pending_cfg = True
            elif pending_cfg and line and not line.startswith(("#[", "//")):
                pending_cfg = False
                if "{" in line:
                    skip_until = depth  # leave when depth returns here
                # a one-line cfg(test) item (e.g. a const) is simply skipped
                else:
                    doc = []
                    depth += raw.count("{") - raw.count("}")
                    continue
        if skip_until is None:
            m = CONST.match(raw)
            if m and NAME_HINT.search(m.group(1)):
                value = int(m.group(2).replace("_", ""))
                if value >= MIN_VALUE and not invented(value):
                    mk = MARK.search(m.group(3)) or next(
                        (x for x in map(MARK.search, reversed(doc)) if x), None
                    )
                    yield n, m.group(1), mk.group(1) if mk else "-", value
            if line.startswith("//"):
                doc.append(line)
            elif line.startswith("#["):
                pass
            else:
                doc = []
        depth += raw.count("{") - raw.count("}")
        if skip_until is not None and depth <= skip_until:
            skip_until = None


def walk(root: str):
    for d, dirs, files in os.walk(root):
        dirs[:] = sorted(x for x in dirs if x not in ("target", ".git"))
        for f in sorted(files):
            if f.endswith(".rs"):
                yield os.path.join(d, f)


def subsystem(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


def collect(root: str):
    rows = []
    for p in walk(root):
        with open(p, encoding="utf-8") as fh:
            text = fh.read()
        for n, name, mark, value in scan(text, p):
            rows.append((os.path.relpath(p, root), n, name, mark, subsystem(p), value))
    return rows


def to_tsv(rows, values=True) -> str:
    head = ["file", "line", "name", "mark", "subsystem"] + (["value"] if values else [])
    out = ["\t".join(head)]
    for f, n, name, mark, sub, v in rows:
        cols = [f, str(n), name, mark, sub] + ([str(v)] if values else [])
        out.append("\t".join(cols))
    return "\n".join(out) + "\n"


def main(argv):
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    root = os.path.join(repo, "server", "crates")
    out = os.path.join(repo, "out", "hand-data.tsv")
    i = 0
    while i < len(argv):
        if argv[i] == "--root" and i + 1 < len(argv):
            root = argv[i + 1]; i += 2
        elif argv[i] == "--out" and i + 1 < len(argv):
            out = os.path.abspath(argv[i + 1]); i += 2
        else:
            print(__doc__); return 2
    outdir = os.path.join(repo, "out") + os.sep
    if not os.path.abspath(out).startswith(outdir):
        print("refusing: the TSV holds real values and must go under out/", file=sys.stderr)
        return 2
    rows = collect(root)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(to_tsv(rows))
    counts = {}
    for r in rows:
        counts[r[4]] = counts.get(r[4], 0) + 1
    for k in sorted(counts, key=lambda k: (-counts[k], k)):
        print(f"{counts[k]}\t{k}")
    print(f"total\t{len(rows)}\twritten to {os.path.relpath(out, repo)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
