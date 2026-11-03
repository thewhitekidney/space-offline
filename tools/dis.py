"""Disassemble named code objects out of one `.pyc`, with the client's own
line numbers.

    tools/analysis.sh python3 /work/tools/dis.py /work/out/pyc/<path>.pyc [name ...]

With no names, every code object in the file is printed; with names, only
those (matched against `co_name`, so a method is named by its own name and not
by `Class.method`). Use `--names` to list what is in a file without printing
any bytecode, which is the fastest way to find the function you want.

**Why this exists beside `tools/call_lines.py`.** That one answers "where is
this name referenced", which is the right question when you know what you are
looking for. This one answers "what does this function actually do", which is
the question that follows it. Both read `co_lnotab` for real source lines, for
the reason `call_lines.py`'s header gives at length: uncompyle6 prints line
numbers from *its own reconstructed output*, and three lanes in one round
cited those in source comments before anyone noticed, with offsets as large as
720 lines. A citation that disagrees with every traceback the client has ever
printed is worse than no citation.

Read-only: it opens one file and writes nothing.
"""

# **The stdlib has to win over `tools/`, and this file is why.** `tools/` is
# `sys.path[0]` whenever any script in it runs, because that is where the
# script lives -- so *this* module, named `dis.py`, shadows the standard
# library's `dis` for anything that imports it *indirectly*. Two real chains
# reach it: `argparse` -> `_colorize` -> `dataclasses` -> `inspect` -> `dis`
# on Python 3.14 (the host), and `capstone` -> `distutils` -> `setuptools`
# -> `inspect` -> `dis` on 3.11 (the analysis container). It killed 19 of the
# 28 tools here, each dying inside `ArgumentParser()` with an
# `AttributeError` about `COMPILER_FLAG_NAMES` and a traceback naming no file
# of ours.
#
# Rotating our directory to the end of the path fixes every one of them and
# keeps sibling imports (`pyc27`, `fsd`, `loctable`) resolving, because the
# directory is still on the path -- just last.
#
# **A bare `sys.path.append(...)` does not work**, and was in six of these
# files for weeks claiming to: it adds a second copy of a directory that is
# already first. The name stays `dis.py` on purpose -- about 25 lane and
# `docs/re/` documents cite `tools/dis.py` as the tool a past lane ran, and
# several say "all line numbers are `tools/dis.py` numbers", so renaming it
# would falsify those records to fix something one line settles.
# `docs/port-tracker.md` **42** has the measurement.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys

# `tools/` itself is already on the path -- the rotation above put it last,
# behind the stdlib, which is the whole point. See that comment.
import pyc27


def linemap(code):
    """`[(bytecode offset, source line)]`, ascending, from `co_lnotab`."""
    rows, line, off = [], code.firstlineno, 0
    tab = code.lnotab
    for i in range(0, len(tab), 2):
        off += tab[i]
        line += tab[i + 1]
        rows.append((off, line))
    return rows


def line_at(rows, first, off):
    """The source line a bytecode offset belongs to."""
    cur = first
    for o, l in rows:
        if o <= off:
            cur = l
        else:
            break
    return cur


def name_of(code):
    return code.name.decode('utf8', 'replace')


# `cmp_op` from CPython 2.7's `opcode.py`, in order: `COMPARE_OP`'s argument
# indexes this table.
CMP_OP = ('<', '<=', '==', '!=', '>', '>=', 'in', 'not in', 'is', 'is not',
          'exception match', 'BAD')


def operand(op, arg, rep):
    """Render an instruction's argument, never blank when there is one.

    `pyc27.dis` renders an argument only when it can name it (a constant, a
    local, an attribute). Everything whose argument is a bare number -- a
    jump target, an argument count, a comparison -- came out as an empty
    column, and that blank has cost this project two rounds:

      - `COMPARE_OP` with nothing after it was read as `> 0` when the operand
        was 0, i.e. `<`. The claim went into a lane note as a [V] and accused
        a server that was behaving correctly.
      - `CALL_FUNCTION`'s count is how you see that `.Length()` is called at
        all; a dropped `.Length()` was the other half of the same incident.

    A jump target you cannot see is also how "the raise is outside the try"
    gets asserted rather than read. So: name it if `pyc27` could, otherwise
    print the number, and decode the one table that is pure magic.
    """
    if rep:
        return rep
    if arg is None:
        return ''
    if op == 'COMPARE_OP' and 0 <= arg < len(CMP_OP):
        return '%d (%s)' % (arg, CMP_OP[arg])
    return str(arg)


def main(argv):
    if not argv:
        sys.stderr.write(__doc__)
        return 2
    path = argv[0]
    rest = argv[1:]
    only_names = '--names' in rest
    wanted = {a for a in rest if not a.startswith('--')}

    top = pyc27.load_pyc(path)
    for code in pyc27.walk(top):
        name = name_of(code)
        if wanted and name not in wanted:
            continue
        if only_names:
            print('%-40s line %d' % (name, code.firstlineno))
            continue
        rows = linemap(code)
        print('=== %s @ line %d' % (name, code.firstlineno))
        for off, op, arg, rep in pyc27.dis(code):
            print('%5d  %4d  %-22s %s' % (line_at(rows, code.firstlineno, off),
                                          off, op, operand(op, arg, rep)))
        print()
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
