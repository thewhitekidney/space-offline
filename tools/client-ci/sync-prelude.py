#!/usr/bin/env python3
"""Copy the canonical prelude into every scenario beside it.

The prelude is duplicated into each scenario on purpose -- a scenario is one
self-contained file that the client `execfile`s, so it cannot import anything
of ours (`docs/lanes/ci-scenarios.md` section 6).  `_prelude.py` is the copy to
edit; this propagates it, which was a hand job before and is why a drift check
exists at all.

Both shared halves are replaced:

    # PRELUDE -- byte-identical ...      part 1   up to "# -- the scenario"
    ... the scenario body ...                     left alone
    # -- end of the scenario body ...    part 2   to end of file

Usage:  tools/client-ci/sync-prelude.py [--check]
`--check` writes nothing and exits 1 if any file is out of date, which is what
CI wants; without it the files are rewritten in place.
"""
import os
import re
import sys

HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'scenarios')
P1_START = re.compile(r'^# PRELUDE -- byte-identical', re.M)
P1_END = re.compile(r'^    # -- the scenario', re.M)
P2_START = re.compile(r'^    # -- end of the scenario body', re.M)


def split(text, path):
    """(head, part1, body, part2) -- raises if the markers are not all there."""
    m1, m2, m3 = P1_START.search(text), P1_END.search(text), P2_START.search(text)
    if not (m1 and m2 and m3):
        missing = [n for n, m in (('part 1 start', m1), ('part 1 end', m2),
                                  ('part 2 start', m3)) if not m]
        raise SystemExit('%s: cannot find %s' % (path, ', '.join(missing)))
    if not (m1.start() < m2.start() < m3.start()):
        raise SystemExit('%s: prelude markers are out of order' % path)
    return (text[:m1.start()], text[m1.start():m2.start()],
            text[m2.start():m3.start()], text[m3.start():])


def main():
    check = '--check' in sys.argv[1:]
    canonical = os.path.join(HERE, '_prelude.py')
    with open(canonical) as fh:
        _, part1, _, part2 = split(fh.read(), canonical)

    stale = []
    for name in sorted(os.listdir(HERE)):
        if not name.endswith('.py') or name == '_prelude.py':
            continue
        path = os.path.join(HERE, name)
        with open(path) as fh:
            text = fh.read()
        head, had1, body, had2 = split(text, path)
        if had1 == part1 and had2 == part2:
            continue
        stale.append(name)
        if not check:
            with open(path, 'w') as fh:
                fh.write(head + part1 + body + part2)

    if check:
        if stale:
            print('out of date: %s' % ', '.join(stale))
            print('run tools/client-ci/sync-prelude.py to fix')
            return 1
        print('every scenario carries the canonical prelude')
        return 0
    print('synced: %s' % (', '.join(stale) if stale else 'nothing to do'))
    return 0


if __name__ == '__main__':
    sys.exit(main())
