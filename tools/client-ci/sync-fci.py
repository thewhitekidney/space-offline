#!/usr/bin/env python3
"""Copy the canonical f-ci block into every f-ci scenario.

`fci-block.txt` is the copy to edit.  Each f-ci scenario (a file in
`scenarios/` named `fci_*.py`) carries it between two marker lines, so the
file stays one self-contained unit the client can `execfile`, like the prelude
(`sync-prelude.py`).  Same idea, same two modes:

    tools/client-ci/sync-fci.py            rewrite the stale ones
    tools/client-ci/sync-fci.py --check    write nothing; exit 1 if any is stale

`tools/test_fci_scenarios.py` runs the check, and the gate runs that test.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCEN = os.path.join(HERE, 'scenarios')
CANON = os.path.join(HERE, 'fci-block.txt')
START = re.compile(r'^    # -- fci block: ', re.M)
END = re.compile(r'^    # -- end of the fci block[^\n]*\n', re.M)


def split(text, path):
    """(before, block, after); raises SystemExit if the markers are wrong."""
    s, e = START.search(text), END.search(text)
    if not (s and e and s.start() < e.start()):
        raise SystemExit('%s: fci block markers missing or out of order' % path)
    return text[:s.start()], text[s.start():e.end()], text[e.end():]


def scenarios():
    return sorted(n for n in os.listdir(SCEN)
                  if n.startswith('fci_') and n.endswith('.py'))


def main():
    check = '--check' in sys.argv[1:]
    with open(CANON) as fh:
        canon = fh.read()
    if not START.match(canon) or not END.search(canon):
        raise SystemExit('fci-block.txt: markers missing')
    stale = []
    for name in scenarios():
        path = os.path.join(SCEN, name)
        with open(path) as fh:
            text = fh.read()
        before, had, after = split(text, path)
        if had == canon:
            continue
        stale.append(name)
        if not check:
            with open(path, 'w') as fh:
                fh.write(before + canon + after)
    if check:
        if stale:
            print('out of date: %s' % ', '.join(stale))
            print('run tools/client-ci/sync-fci.py to fix')
            return 1
        print('every f-ci scenario carries the canonical block')
        return 0
    print('synced: %s' % (', '.join(stale) if stale else 'nothing to do'))
    return 0


if __name__ == '__main__':
    sys.exit(main())
