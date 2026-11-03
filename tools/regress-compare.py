#!/usr/bin/env python3
"""Compare two client-CI regression runs row by row (PORT-REPIN, T6; issue #145).

    python3 -I tools/regress-compare.py BASE NEW [--quiet]

BASE and NEW are the `regress.tsv` files of two `tools/client-ci-regress.sh`
runs, or the directories holding them (var/regress*). After a port a bare red
count means nothing; what counts is which rows changed against the last run on
the old build. Prints, in this order:

  - the trigger warning, first, when every row of NEW never ran;
  - new reds       green (0) in BASE, red in NEW (or red in NEW and not in BASE)
  - never ran      a row that ran in BASE and did not in NEW (4, 143, `missing`,
                   or absent); rows that never ran in BASE too are "still"
  - new greens     red or never-ran in BASE, green in NEW
  - still red      red in both
  - rows only in one table

Status codes (tools/client-ci-regress.sh): 0 pass, 1 the client disagreed,
3 refused, 5 malformed, 6 the scenario raised are RED; 4 never ran, 143 stopped
or never started, `missing` (no scenario file) are NEVER RAN. A trailing
`finished` line marks a complete run; without it the table says so.

Exit status: 0 nothing new is red or lost, 3 a new red or a lost row, 4 every
row of NEW never ran (the CI trigger, `charMgr.GetSettingsInfo` read at
carbonui/services/settingsSvc.py UpdateSettingsStatistics, check it before
believing any red), 2 a table is missing or malformed.
"""
import os
import sys

GREEN = {"0"}
NEVER = {"4", "143", "missing"}
RED = {"1", "3", "5", "6"}


def kind(code):
    if code in GREEN:
        return "green"
    if code in NEVER:
        return "never"
    if code in RED:
        return "red"
    return None


def read_table(arg):
    """-> (rows dict name -> code, finished bool). Raises ValueError."""
    path = os.path.join(arg, "regress.tsv") if os.path.isdir(arg) else arg
    try:
        with open(path) as f:
            lines = f.read().splitlines()
    except OSError as err:
        raise ValueError("%s: %s" % (path, err.strerror or err))
    rows, finished = {}, False
    for n, line in enumerate(lines, 1):
        if not line.strip() or line.startswith("#"):
            continue
        if line.strip() == "finished":
            finished = True
            continue
        parts = line.split("\t")
        if len(parts) != 2 or not parts[0] or kind(parts[1].strip()) is None:
            raise ValueError("%s:%d: not `name<TAB>status` with a known status: %r" % (path, n, line))
        rows[parts[0]] = parts[1].strip()  # a re-run row: the last one stands
    return rows, finished


def compare(base, new):
    """The classified rows, each a sorted list of (name, base code, new code)."""
    out = {k: [] for k in ("new_red", "lost", "new_green", "still_red", "still_never", "only_base", "only_new")}
    for name in sorted(set(base) | set(new)):
        b, n = base.get(name), new.get(name)
        bk, nk = kind(b) if b else None, kind(n) if n else None
        row = (name, b or "-", n or "-")
        if b is None:
            out["only_new"].append(row)
            if nk == "red":
                out["new_red"].append(row)
        elif n is None:
            out["only_base"].append(row)
            if bk == "green":
                out["lost"].append(row)
        elif nk == "red":
            out["new_red" if bk == "green" or bk == "never" else "still_red"].append(row)
        elif nk == "never":
            out["lost" if bk != "never" else "still_never"].append(row)
        elif nk == "green" and bk != "green":
            out["new_green"].append(row)
    return out


def main(argv):
    args = [a for a in argv if a != "--quiet"]
    if len(args) != 2 or any(a in ("-h", "--help") for a in argv):
        sys.stderr.write(__doc__)
        return 2
    try:
        (base, bfin), (new, nfin) = read_table(args[0]), read_table(args[1])
    except ValueError as err:
        sys.stderr.write("regress-compare: %s\n" % err)
        return 2
    if not new:
        sys.stderr.write("regress-compare: %s has no rows\n" % args[1])
        return 2
    res = compare(base, new)
    trigger = all(c == "4" for c in new.values())
    if trigger:
        print("TRIGGER: every row of NEW never ran (status 4). Do not read the reds: check the "
              "CI trigger first, charMgr.GetSettingsInfo read in settingsSvc.UpdateSettingsStatistics "
              "(Crypto.Verify, marshal.loads into SettingsInfo.__code__), served by service/character.rs.")
    if not bfin:
        print("note: BASE has no `finished` line; it was an interrupted run")
    if not nfin:
        print("note: NEW has no `finished` line; it was an interrupted run")

    def show(title, rows, quiet_ok=False):
        print("%s: %d" % (title, len(rows)))
        for name, b, n in rows:
            print("  %s\t%s -> %s" % (name, b, n))

    show("new reds", res["new_red"])
    show("never ran (ran in BASE)", res["lost"])
    show("new greens", res["new_green"])
    show("still red", res["still_red"])
    if "--quiet" not in argv:
        show("still never ran", res["still_never"])
        show("only in BASE", res["only_base"])
        show("only in NEW", res["only_new"])
    print("rows: base %d, new %d; green in new %d" % (len(base), len(new), sum(1 for c in new.values() if c == "0")))
    if trigger:
        return 4
    return 3 if res["new_red"] or res["lost"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
