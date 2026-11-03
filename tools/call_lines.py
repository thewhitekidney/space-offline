"""The client's own line number for every reference to a name, in one .pyc.

**The decompiler cannot answer this question.** uncompyle6 emits its own
reconstructed file, so `grep -n` over its output gives an *output* line -- and
three lanes in one round cited those in source comments before noticing, with
offsets as large as 720 lines. A citation that disagrees with every traceback
the client has ever sent us is worse than no citation at all.

This reads `co_lnotab` instead, which is where the client itself gets the
numbers it prints in a traceback. Read-only.

    tools/analysis.sh python3 tools/call_lines.py out/pyc/<path>.pyc GetOrders

Prints one row per bytecode reference to each name, with the enclosing code
object and the opcode, so a `LOAD_ATTR GetOrders` (the call) is
distinguishable from a `LOAD_CONST 'GetOrders'` (a string mentioning it).

See also `findings/client-calls.tsv`, whose `call_site` column is built the
same way for every call site at once; reach for this when you want one name in
one file, or a name the census has no row for.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys

import pyc27


def linemap(code):
    """`[(bytecode offset, source line)]`, ascending, from `co_lnotab`.

    `lnotab` is a run-length encoding of *increments*: pairs of (bytes of
    bytecode, lines of source) since the previous entry, starting from
    `co_firstlineno`.
    """
    out = [(0, code.firstlineno)]
    line = code.firstlineno
    off = 0
    t = code.lnotab
    for i in range(0, len(t), 2):
        off += t[i]
        line += t[i + 1]
        out.append((off, line))
    return out


def line_of(lm, off):
    """The source line covering `off` -- the last entry at or before it."""
    ln = lm[0][1]
    for o, l in lm:
        if o <= off:
            ln = l
        else:
            break
    return ln


def main(argv):
    if len(argv) < 3:
        sys.stderr.write(
            'usage: call_lines.py <file.pyc> <name> [name...]\n'
            '  run it in the analysis container, which mounts the repo at /work\n'
        )
        return 2
    path, needles = argv[1], argv[2:]
    top = pyc27.load_pyc(path)
    hits = 0
    for c in pyc27.walk(top):
        lm = linemap(c)
        name = c.name.decode() if isinstance(c.name, bytes) else c.name
        for off, op, _arg, txt in pyc27.dis(c):
            for n in needles:
                if n in str(txt):
                    hits += 1
                    print('%-40s %-28s line %-6d %s' % (name, n, line_of(lm, off), op))
    if not hits:
        sys.stderr.write('no reference to %s in %s\n' % (', '.join(needles), path))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
