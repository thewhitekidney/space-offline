"""Refresh findings/session-keys.tsv against a client build (lane findings-gen,
issue #18; the table's meaning is `docs/re/session-and-binding.md`).

    tools/analysis.sh python3 -I tools/scan_session_keys.py \
        [--pyc out/pyc] [--seed findings/session-keys.tsv] [--out findings/session-keys.tsv]

**What can and cannot be generated.** The table has nine columns. `key`,
`readers` (the counts) and the *existence* of each `citation` come from the
bytecode. `type`, `origin`, `set_by`, `absent`, `wrong` and `confidence` are
judgement written by a research lane after reading `basesession.py`,
`machoNetTransport.py` and `gameui.py`: they are not derivable from code, and a
generator that invented them would be worse than none. So this tool is seeded:
the previous table supplies the judgement columns and the key list, and the tool
re-measures everything measurable.

- `readers`: the leading `N read(s) in M module(s)` is replaced by a fresh
  count of `LOAD_ATTR <key>` whose object is the global/name `session`
  (`LOAD_GLOBAL`/`LOAD_NAME`, not a local that happens to be called `session`),
  over every code object in the tree. The hand-written remainder after the
  first `;` is kept verbatim. A `0 direct reads:` row stays zero-read prose
  unless the count is no longer zero, which is reported.
- `citation`: every `path.py[:line]` is checked against the tree. A path with no
  module, or a line that no code object in that module attributes bytecode to,
  is reported as drifted, because after a client update that is exactly what a
  moved citation looks like.
- Session attributes read in the tree that are not in the seed are listed
  (most-read first), so a key new in a build stands out.

With `--seed` equal to `--out` (the default) the file is rewritten in place;
running it twice changes nothing the second time. The drift report goes to
stderr and the exit status stays 0: a drift is a to-do, not a failure.
"""
import argparse
import collections
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import pyc27
import findings_lib as L

COLUMNS = ['key', 'type', 'origin', 'set_by', 'readers', 'absent', 'wrong', 'confidence', 'citation']
COUNT_RE = re.compile(r'^(\d+) reads? in (\d+) modules?(?=;|$)|^0 direct reads(?=:|;|$)')
CITE_RE = re.compile(r'^([A-Za-z0-9_./-]+\.py)(?::(\d+))?$')
SESSION_OBJECTS = ('session',)


def count_reads(root, keys):
    """({key: (reads, set of modules)}, {other attribute: reads}) over the tree.
    A module is the .pyc file, i.e. one source file."""
    got = {k: [0, set()] for k in keys}
    other = collections.Counter()
    lines_of = {}       # embedded source path -> set of lines with bytecode
    for rel, top in L.load_tree(root):
        for c in pyc27.walk(top):
            src = L.rel_filename(c)
            lines_of.setdefault(src, set()).update(ln for _a, ln in L.line_table(c))
            ins = list(pyc27.dis(c))
            for i in range(1, len(ins)):
                off, op, arg, rep = ins[i]
                if op != 'LOAD_ATTR':
                    continue
                prev = ins[i - 1]
                if prev[1] not in ('LOAD_GLOBAL', 'LOAD_NAME') or prev[3] not in SESSION_OBJECTS:
                    continue
                if rep in got:
                    got[rep][0] += 1
                    got[rep][1].add(rel)
                else:
                    other[rep] += 1
    return got, other, lines_of


def phrase(n, m):
    return '%d read%s in %d module%s' % (n, '' if n == 1 else 's', m, '' if m == 1 else 's')


def refresh(rows, got, lines_of):
    """Return (new rows, drift list). `rows` are lists of the nine columns."""
    out, drift = [], []
    suffix_index = {}
    for src in lines_of:
        suffix_index.setdefault(src, src)
    for r in rows:
        key = r[0]
        n, mods = got.get(key, (0, set()))
        old = r[4]
        mm = COUNT_RE.match(old)
        if mm:
            rest = old[mm.end():]
            if mm.group(0).startswith('0 direct'):
                if n:
                    drift.append('%s: seed says 0 direct reads, tree has %s' % (key, phrase(n, len(mods))))
                    r[4] = phrase(n, len(mods)) + rest
            else:
                r[4] = phrase(n, len(mods)) + rest
                if (int(mm.group(1)), int(mm.group(2))) != (n, len(mods)):
                    drift.append('%s: readers %s -> %s' % (key, mm.group(0), phrase(n, len(mods))))
        for cite in [c.strip() for c in r[8].split(';') if c.strip()]:
            m = CITE_RE.match(cite)
            if not m:
                continue
            path, line = m.group(1), m.group(2)
            if path not in lines_of:
                drift.append('%s: citation %s: no such module in the tree' % (key, cite))
            elif line and int(line) not in lines_of[path]:
                drift.append('%s: citation %s: line has no bytecode' % (key, cite))
        out.append(r)
    return out, drift


def read_seed(path):
    rows = []
    with open(path, encoding='utf8') as fh:
        lines = fh.read().split('\n')
    if lines and lines[-1] == '':
        lines.pop()
    head = lines[0].split('\t')
    if head != COLUMNS:
        sys.exit('scan_session_keys: %s: unexpected header %r' % (path, head))
    for ln in lines[1:]:
        cols = ln.split('\t')
        if len(cols) != len(COLUMNS):
            sys.exit('scan_session_keys: %s: row with %d columns: %r' % (path, len(cols), ln[:60]))
        rows.append(cols)
    return rows


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--pyc', default=L.DEFAULT_PYC)
    ap.add_argument('--seed', default='findings/session-keys.tsv')
    ap.add_argument('--out', default='findings/session-keys.tsv', help="'-' for stdout")
    ap.add_argument('--unlisted', type=int, default=15, help='how many unlisted session attributes to print')
    a = ap.parse_args(argv)
    if not os.path.isdir(a.pyc):
        sys.exit('scan_session_keys: no such tree: %s' % a.pyc)
    rows = read_seed(a.seed)
    got, other, lines_of = count_reads(a.pyc, [r[0] for r in rows])
    rows, drift = refresh(rows, got, lines_of)
    L.write_tsv(a.out, [L.tsv_line(COLUMNS)] + [L.tsv_line(r) for r in rows])
    for d in drift:
        print('drift: ' + d, file=sys.stderr)
    top = other.most_common(a.unlisted)
    print('session-keys: %d keys, %d drifted; most-read session attributes not in the table: %s'
          % (len(rows), len(drift), ', '.join('%s(%d)' % kv for kv in top)), file=sys.stderr)


if __name__ == '__main__':
    main()
