#!/usr/bin/env python3
"""The doctrine layer's gates G1-G10, as pass/fail with the numbers (DOC-9).

    tools/analysis.sh python3 tools/doctrine-report.py out/soak/bots.log [out/soak/server.log ..]
        [--arena out/arena/arena-*.log] [--run 1=out/soak1/bots.log --run 4=out/soak4/bots.log]
        [--min-days 1.0] [--min-fights 20] [--json]

Reads the bots' `[doc]` and `[soc]` lines (and the arena's `[arena]` lines,
`tools/arena.sh`) and judges the ten gates of R-DOCTRINE section 9. A gate with
no instrument in the logs says `n/a` and why, never a pass. Exit status 1 if
any gate FAILs, 0 otherwise. Only the standard library.

Lines read (last `roll` block wins; every earlier line still counts):

  [doc] roll group=G current=K since=D fancy=F switches=N flaps=F min_gap=D
        fighters=N theatres=N committed=N open=S,..      one per leader
  [doc] roll-all leaders=N doctrines=N layer_range=N switches=N flaps=N
        min_gap=D span=D fighters=N theatres=N committed=N calls=N
  [doc] lead group=G current=K fancy=L/R/F/C decision=First { key: "K" } ...
  [doc] theatre group=G system=S from -> to conf=C foes=N actions=N
  [doc] battle ... est_theirs=N true_theirs=N          (G7; the soak's census)
  [turns] ... hold_max=Ms                              (G10; the server log)
  [arena] pair ... isk_ratio_median=R ... / summary    (G8; tools/arena.sh)
  [soc] bots N online N mix .. switches_per_bot_day X flap_max Y

`--run LEVEL=FILE` (repeat, two or more) gives G9 its difficulty ladder.
"""
import argparse
import json
import math
import re
import sys

# The spec's constants (R-DOCTRINE sections 9 and 11); the report takes them as
# options where a run can differ.
SWITCHES_PER_WEEK = (0.3, 4.0)
RESERVE = 0.30
OBS_ERROR_NORMAL = 0.35
HOLD_MS = 25.0
FANCY_MATCH = 0.70
MIN_SPAN_DAYS = 3.0

PASS, FAIL, NA = "PASS", "FAIL", "n/a"

TOKEN = re.compile(r"(\w[\w-]*)=(\S+)")
FIRST = re.compile(r'decision=First \{ key: "([^"]*)" \}')
SWITCH = re.compile(r'decision=Switch \{ from: "([^"]*)", to: "([^"]*)" \}')
LEAD_HEAD = re.compile(r"^\[doc\] lead group=(\d+)")


def kv(line):
    """The `key=value` tokens of a line (the first of a repeated key)."""
    out = {}
    for k, v in TOKEN.findall(line):
        out.setdefault(k, v)
    return out


def num(x, default=None):
    try:
        return float(x)
    except (TypeError, ValueError):
        return default


def median(v):
    v = sorted(v)
    if not v:
        return float("nan")
    m = len(v) // 2
    if len(v) % 2:
        return v[m]
    a, b = v[m - 1], v[m]
    if math.isinf(a) or math.isinf(b):
        return a if a == b else min(a, b)
    return (a + b) / 2


def layer_range(key):
    return "/".join(key.split("/")[:2])


def fancy_matches(fancy, key):
    """A doctrine key `layer/range/stance/class/family` against a fancy
    `layer/range/family/class` (doctrine_lead::Fancy::line)."""
    f, k = fancy.split("/"), key.split("/")
    if len(f) != 4 or len(k) != 5:
        return False
    return (f[0], f[1], f[2], f[3]) == (k[0], k[1], k[4], k[3])


class Log:
    """Everything the report reads from one run's files."""

    def __init__(self):
        self.roll = []  # blocks: {"leaders": [kv..], "all": kv}
        self.leads = []  # (group, kv, line)
        self.theatres = []
        self.battles = []
        self.holds = []
        self.soc = []
        self.arena_pairs = []
        self.arena_skirmishes = []
        self.arena_summary = None

    def feed(self, lines):
        block = None
        for line in lines:
            line = line.rstrip("\n")
            i = line.find("[")
            # A bots.log line may carry a prefix (a bot name, a timestamp).
            if i > 0 and any(line[i:].startswith(t) for t in ("[doc]", "[soc]", "[arena]", "[turns]")):
                line = line[i:]
            if line.startswith("[doc] roll-all"):
                a = kv(line)
                if block is None:
                    block = []
                self.roll.append({"leaders": block, "all": a})
                block = None
            elif line.startswith("[doc] roll "):
                if block is None:
                    block = []
                block.append(kv(line))
            elif line.startswith("[doc] lead "):
                m = LEAD_HEAD.match(line)
                d = kv(line)
                first = FIRST.search(line)
                sw = SWITCH.search(line)
                d["_first"] = first.group(1) if first else None
                d["_switch"] = (sw.group(1), sw.group(2)) if sw else None
                self.leads.append((m.group(1) if m else d.get("group"), d, line))
            elif line.startswith("[doc] theatre "):
                self.theatres.append(kv(line))
            elif line.startswith("[doc] battle"):
                self.battles.append(kv(line))
            elif line.startswith("[turns]"):
                h = num(kv(line).get("hold_max", "").rstrip("ms"))
                if h is not None:
                    self.holds.append(h)
            elif line.startswith("[soc]"):
                self.soc.append(kv(line))
            elif line.startswith("[arena] skirmish"):
                self.arena_skirmishes.append(kv(line))
            elif line.startswith("[arena] pair"):
                self.arena_pairs.append(kv(line))
            elif line.startswith("[arena] summary"):
                self.arena_summary = kv(line)

    def last_roll(self):
        return self.roll[-1] if self.roll else None


def read(paths):
    log = Log()
    for p in paths:
        with open(p, errors="replace") as f:
            log.feed(f)
    return log


# ---- the gates --------------------------------------------------------------


def gate_g1(log):
    r = log.last_roll()
    if not r or not r["leaders"]:
        return NA, "no `[doc] roll` lines: the doctrine layer was off (needs --society --society-pvp and --content)"
    keys = {l.get("current") for l in r["leaders"] if l.get("current") not in (None, "-")}
    heads = {layer_range(k) for k in keys}
    n = len(r["leaders"])
    span = num(r["all"].get("span"), 0.0)
    detail = f"{len(keys)} distinct doctrines over {n} leaders, {len(heads)} layer/range pairs, span {span:.1f} game days"
    if span < MIN_SPAN_DAYS:
        return NA, detail + f" (the run is under {MIN_SPAN_DAYS:.0f} game days)"
    ok = (len(keys) >= 2 or n < 3) and len(heads) >= min(3, max(1, n))
    return (PASS if ok else FAIL), detail + " (need >= 2 doctrines with >= 3 leaders, >= 3 layer/range pairs)"


def gate_g2(log):
    firsts = [(g, d) for g, d, _ in log.leads if d.get("_first") and d.get("fancy", "-") not in ("-", "")]
    if not firsts:
        return NA, "no `[doc] lead ... decision=First` lines with a fancy"
    seen = {}
    for g, d in firsts:
        seen.setdefault(g, d)
    hit = sum(1 for d in seen.values() if fancy_matches(d["fancy"], d["_first"]))
    share = hit / len(seen)
    detail = f"{hit} of {len(seen)} leaders' first doctrine matches their fancy ({share:.0%}, need >= {FANCY_MATCH:.0%}); losses-yield half: no instrument"
    return (PASS if share >= FANCY_MATCH else FAIL), detail


def gate_g3(log, min_days):
    r = log.last_roll()
    if not r:
        return NA, "no `[doc] roll` lines"
    a = r["all"]
    span = num(a.get("span"), 0.0)
    leaders = num(a.get("leaders"), 0)
    switches = num(a.get("switches"), 0)
    flaps = num(a.get("flaps"), 0)
    gap = num(a.get("min_gap"))
    if span < MIN_SPAN_DAYS or not leaders:
        return NA, f"span {span:.1f} game days, {int(switches)} switches (too short to rate)"
    per_week = switches / leaders / (span / 7.0)
    bad = []
    if not (SWITCHES_PER_WEEK[0] <= per_week <= SWITCHES_PER_WEEK[1]):
        bad.append(f"switches/leader/week {per_week:.2f} outside {SWITCHES_PER_WEEK}")
    if gap is not None and gap < min_days:
        bad.append(f"min gap {gap:.1f} d < {min_days}")
    if flaps:
        bad.append(f"{int(flaps)} flaps")
    detail = f"{int(switches)} switches, {per_week:.2f}/leader/week, min gap {'-' if gap is None else f'{gap:.1f} d'}, {int(flaps)} flaps"
    return (FAIL if bad else PASS), detail + ("; " + ", ".join(bad) if bad else "")


def gate_g4(log):
    return NA, "no instrument: the hall does not log fighters' hulls against the doctrine (DOC-5 reads them per bot)"


def gate_g5(log):
    rolls = [b for b in log.roll if num(b["all"].get("fighters"), 0) > 0]
    if not rolls:
        return NA, "no `[doc] roll-all` with fighters"
    worst = max(num(b["all"].get("committed"), 0) / num(b["all"].get("fighters")) for b in rolls)
    limit = 1.0 - RESERVE * 0.8
    detail = f"committed/fighters peaked at {worst:.2f} (reserve rule: <= {limit:.2f}); first-contact and time-to-commit: no timestamps in the lines"
    return (PASS if worst <= limit else FAIL), detail


def gate_g6(log):
    return NA, "read `tools/econ-report.py` (class doctrine) and the `[econ]` bids; not in the doctrine lines"


def gate_g7(log):
    errs = []
    for b in log.battles:
        est, true = num(b.get("est_theirs")), num(b.get("true_theirs"))
        if est is not None and true:
            errs.append(abs(est - true) / true)
    if not errs:
        return NA, "no `[doc] battle ... est_theirs= true_theirs=` lines (the soak's out-of-band census does not print them yet)"
    m = median(errs)
    return (PASS if m <= OBS_ERROR_NORMAL else FAIL), f"median |est-true|/true = {m:.2f} over {len(errs)} battles (need <= {OBS_ERROR_NORMAL} at normal)"


def gate_g8(arena, min_fights):
    pairs = arena.arena_pairs
    if not pairs:
        return NA, "no `[arena] pair` lines: run tools/arena.sh"
    works = 0
    ratios = []
    fights = 0
    parts = []
    for p in pairs:
        n = int(num(p.get("n"), 0))
        fights += n
        r = num(p.get("isk_ratio_median"), float("nan"))
        ratios.append(r)
        if r > 1.0:
            works += 1
        parts.append(f"{r:.2f}({p.get('wins_predicted')}-{p.get('wins_other')}-{p.get('draws')})")
    med = median(ratios)
    detail = (
        f"{works} of {len(pairs)} counters work in the engine; median of per-pair median ISK ratios {med:.2f} "
        f"over {fights} fights [ratio(win-loss-draw): {' '.join(parts)}]"
    )
    if fights < min_fights:
        detail += f" -- small sample (< {min_fights} fights)"
    return (PASS if med > 1.0 else FAIL), detail


def level_metrics(log):
    """What G9 compares across difficulty levels, from one run."""
    m = {}
    errs = []
    for b in log.battles:
        est, true = num(b.get("est_theirs")), num(b.get("true_theirs"))
        if est is not None and true:
            errs.append(abs(est - true) / true)
    if errs:
        m["estimate error"] = (median(errs), "down")
    r = log.last_roll()
    if r:
        a = r["all"]
        span, leaders = num(a.get("span"), 0.0), num(a.get("leaders"), 0)
        if span >= MIN_SPAN_DAYS and leaders:
            m["switches per leader-week"] = (num(a.get("switches"), 0) / leaders / (span / 7.0), "up")
    return m


def gate_g9(runs):
    """`runs`: {level: Log}. Monotone in the right direction on each metric
    two or more levels measure."""
    if len(runs) < 2:
        return NA, "needs two or more `--run LEVEL=FILE` (soaks at --difficulty 1 and 4)"
    levels = sorted(runs)
    table = {lv: level_metrics(runs[lv]) for lv in levels}
    names = sorted({k for m in table.values() for k in m})
    bad, ok, notes = [], [], []
    for name in names:
        pts = [(lv, table[lv][name][0]) for lv in levels if name in table[lv]]
        way = next(table[lv][name][1] for lv in levels if name in table[lv])
        if len(pts) < 2:
            continue
        vals = [v for _, v in pts]
        mono = all(a >= b - 1e-9 for a, b in zip(vals, vals[1:])) if way == "down" else all(a <= b + 1e-9 for a, b in zip(vals, vals[1:]))
        text = f"{name} {way}: " + " ".join(f"d{lv}={v:.2f}" for lv, v in pts)
        (ok if mono else bad).append(text)
    if not (ok or bad):
        return NA, "no metric measured at two levels (reaction time and counter accuracy have no instrument)"
    detail = "; ".join(bad + ok) + "; reaction time, counter accuracy: no instrument"
    return (FAIL if bad else PASS), detail


def gate_g10(log):
    if not log.holds:
        return NA, "no `[turns] ... hold_max=` lines (pass the server log)"
    h = max(log.holds)
    return (PASS if h <= HOLD_MS else FAIL), f"max world hold {h:.1f} ms over {len(log.holds)} reports (need <= {HOLD_MS:.0f} ms); the hall's daily scoring time: no instrument"


GATES = [
    ("G1", "doctrines are diverse"),
    ("G2", "taste shows, then learns"),
    ("G3", "switching is deliberate"),
    ("G4", "compliance"),
    ("G5", "staged commitment"),
    ("G6", "the economy feels it"),
    ("G7", "observation is honest"),
    ("G8", "counter works (arena)"),
    ("G9", "difficulty scales"),
    ("G10", "performance"),
]


def judge(log, arena, runs, min_days=1.0, min_fights=20):
    out = {
        "G1": gate_g1(log),
        "G2": gate_g2(log),
        "G3": gate_g3(log, min_days),
        "G4": gate_g4(log),
        "G5": gate_g5(log),
        "G6": gate_g6(log),
        "G7": gate_g7(log),
        "G8": gate_g8(arena, min_fights),
        "G9": gate_g9(runs),
        "G10": gate_g10(log),
    }
    return out


def render(results):
    lines = []
    for gid, name in GATES:
        status, detail = results[gid]
        lines.append(f"{gid:<3} {status:<4}  {name}: {detail}")
    counts = {k: sum(1 for s, _ in results.values() if s == k) for k in (PASS, FAIL, NA)}
    lines.append(f"gates: {counts[PASS]} pass, {counts[FAIL]} fail, {counts[NA]} n/a")
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("logs", nargs="*", help="bots.log / server.log files")
    ap.add_argument("--arena", action="append", default=[], help="tools/arena.sh log (repeat)")
    ap.add_argument("--run", action="append", default=[], metavar="LEVEL=FILE", help="a soak at a difficulty level (G9)")
    ap.add_argument("--min-days", type=float, default=1.0, help="DOCTRINE_MIN_DAYS of the run's difficulty (brutal: 1.0)")
    ap.add_argument("--min-fights", type=int, default=20)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args(argv)
    log = read(a.logs)
    arena = read(a.arena)
    runs = {}
    for spec in a.run:
        lv, _, path = spec.partition("=")
        runs[int(lv)] = read([path])
    res = judge(log, arena, runs, a.min_days, a.min_fights)
    if a.json:
        print(json.dumps({g: {"status": s, "detail": d} for g, (s, d) in res.items()}, indent=1))
    else:
        print(render(res))
    return 1 if any(s == FAIL for s, _ in res.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
