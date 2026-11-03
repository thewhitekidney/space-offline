#!/usr/bin/env python3
"""End-of-run summary for tools/soak.sh (lane soak-checks, S10).

    soak_summary.py OUTDIR [--allow KEY=N ...]

Reads OUTDIR/server.log, bots.log and offline.txt (the pilots count the
server reported after the bots stopped; soak.sh writes it). Prints
`soak: ...` lines, writes OUTDIR/summary.tsv (metric, key, value) and exits
1 on a deny-list hit or a bot left online, else 0. Std-only.

Tick numbers come from the server's `[world] tick health:` line, one per
window of ticks (push.rs). p95 is the nearest-rank 95th percentile OVER
WINDOWS of each window's max, so it says "95% of windows had no tick worse
than this"; max is the worst single tick of the run.
"""
import re
import sys
from collections import Counter
from pathlib import Path

TICK = re.compile(
    r"\[world\] tick health: (\d+) ticks, late mean ([\d.]+) ms max ([\d.]+) ms, "
    r"over budget (\d+), world lock wait mean ([\d.]+) ms max ([\d.]+) ms, "
    r"world lock held mean ([\d.]+) ms max ([\d.]+) ms, work max ([\d.]+) ms, skipped (\d+)"
)
# PA-0 profile lines (`live_status::prof`), printed beside the tick-health line.
F = r"([\d.]+)"
PROF_SECTIONS = re.compile(
    r"\[world\] tick sections: secs=[\d.]+, "
    + ", ".join(
        rf"{n} n \d+ mean {F} ms max {F} ms"
        for n in ("take_due", "filter", "audience", "session_changes", "save")))
PROF_LOCK = re.compile(
    rf"\[world\] tick lock: secs=[\d.]+, clock held {F} ms occupancy {F}%, "
    rf"payload held {F} ms occupancy {F}% turns (\d+) hold max {F} ms, "
    rf"payload wait mean {F} ms max {F} ms, total occupancy {F}%")
PROF_METHODS = re.compile(
    rf"\[world\] tick methods: secs=[\d.]+, turns (\d+) methods (\d+) hold {F} ms, "
    rf"ro-guess turns {F}% hold {F}%, top_hold (\S+?)[:,]")
# PA-12a: the read-turn fields appended to the lock and methods lines (absent
# in an older log, so searched separately and optional).
PROF_READ_LOCK = re.compile(rf"read turns (\d+) held {F} ms ")
PROF_READ_METHODS = re.compile(rf"ro-served turns {F}% hold {F}%, fallbacks (\d+)")
PROF_WRITERS = re.compile(
    rf"\[world\] tick writers: secs=[\d.]+, frames (\d+) \({F}/s\) bytes (\d+) "
    rf"\({F} kB/s\), passes (\d+) pass mean {F} ms max {F} ms, "
    rf"deflate n (\d+) total {F} ms max {F} ms, writes (\d+) write total {F} ms "
    rf"longest write {F} ms")
USERERR = re.compile(r"^\[svc\] (\S+?)(?:/\d+)? as \d+ raises UserError")
BOTLINE = re.compile(r"^(\S+) \+\d+:\d+(?:\.\d+)? (\S+)\s+(.*)$")
# The deny-list: (name, regex, applies to). Each hit is a verdict failure.
DENY = [
    ("panic", re.compile(r"panicked"), ("server", "bots")),
    ("UNANSWERED", re.compile(r"UNANSWERED"), ("server", "bots")),
    ("error", re.compile(r"\bERROR\b|\berror:"), ("server", "bots")),
    ("tainted", re.compile(r"TAINTED|poisoned"), ("server",)),
]
# A bot that exits drops its chat socket, which the chat listener logs. Not a fault.
BENIGN = re.compile(r"^\[chat\] .* ended: read error: Connection reset by peer")


def parse_prof(lines):
    """The PA-0 profile lines -> {"sections": [..], "lock": [..],
    "methods": [..], "writers": [..]}, each a list of tuples of floats (the
    top-hold method name last in `methods`), one per report window."""
    out = {"sections": [], "lock": [], "methods": [], "writers": [], "read": []}
    for l in lines:
        m = PROF_READ_LOCK.search(l)
        if m:
            out["read"].append(("lock",) + tuple(float(x) for x in m.groups()))
        m = PROF_READ_METHODS.search(l)
        if m:
            out["read"].append(("methods",) + tuple(float(x) for x in m.groups()))
        for key, rx in (("sections", PROF_SECTIONS), ("lock", PROF_LOCK),
                        ("methods", PROF_METHODS), ("writers", PROF_WRITERS)):
            m = rx.search(l)
            if m:
                g = m.groups()
                if key == "methods":
                    out[key].append(tuple(float(x) for x in g[:-1]) + (g[-1],))
                else:
                    out[key].append(tuple(float(x) for x in g))
                break
    return out


def prof_summary(prof):
    """Printed lines for the profile windows (empty for an older server)."""
    lines = []
    sec = prof["sections"]
    if sec:
        names = ("take_due", "filter", "audience", "session_changes", "save")
        parts = []
        for i, n in enumerate(names):
            means = [w[2 * i] for w in sec]
            maxes = [w[2 * i + 1] for w in sec]
            parts.append(f"{n} mean {sum(means) / len(means):.2f} max {max(maxes):.2f}")
        lines.append("soak: clock sections (ms/tick): " + ", ".join(parts))
    lk = prof["lock"]
    if lk:
        n = len(lk)
        lines.append(
            f"soak: lock occupancy: clock {sum(w[1] for w in lk) / n:.1f}%, "
            f"payload {sum(w[3] for w in lk) / n:.1f}%, total p95 {p95([w[8] for w in lk]):.1f}%, "
            f"payload hold max {max(w[5] for w in lk):.1f} ms")
    me = prof["methods"]
    if me:
        n = len(me)
        lines.append(
            f"soak: payload turns: ro-guess {sum(float(w[3]) for w in me) / n:.1f}% of turns, "
            f"{sum(float(w[4]) for w in me) / n:.1f}% of hold; last top method {me[-1][5]}")
    rd = prof.get("read", [])
    if rd:
        lock = [w for w in rd if w[0] == "lock"]
        meth = [w for w in rd if w[0] == "methods"]
        parts = []
        if lock:
            parts.append(f"{sum(w[1] for w in lock):.0f} read turns, "
                         f"{sum(w[2] for w in lock):.0f} ms held (shared, not in occupancy)")
        if meth:
            n = len(meth)
            parts.append(f"ro-served {sum(w[1] for w in meth) / n:.1f}% of turns, "
                         f"{sum(w[2] for w in meth) / n:.1f}% of hold, "
                         f"{sum(w[3] for w in meth):.0f} fallbacks")
        lines.append("soak: read turns: " + "; ".join(parts))
    wr = prof["writers"]
    if wr:
        n = len(wr)
        lines.append(
            f"soak: writers: {sum(w[1] for w in wr) / n:.0f} frames/s, {sum(w[3] for w in wr) / n:.1f} kB/s, "
            f"pass max {max(w[6] for w in wr):.1f} ms, deflate {sum(w[8] for w in wr):.0f} ms total, "
            f"longest write {max(w[12] for w in wr):.1f} ms")
    return lines


def p95(values):
    """Nearest-rank 95th percentile; 0.0 for no values."""
    if not values:
        return 0.0
    s = sorted(values)
    return s[max(0, -(-95 * len(s) // 100) - 1)]


def userkey(name):
    """`skillMgr123::skillHandler.X` -> `skillMgrN::skillHandler.X`."""
    head, sep, tail = name.partition("::")
    return re.sub(r"\d+", "N", head) + sep + tail


def read(path):
    try:
        return Path(path).read_text(errors="replace").splitlines()
    except OSError:
        return None


def summarize(out, allow=None):
    """Returns (printed lines, rows, failures)."""
    allow = allow or {}
    lines, rows, fails = [], [], []
    logs = {"server": read(out / "server.log"), "bots": read(out / "bots.log")}

    def row(metric, key, value):
        rows.append((metric, key, value))

    # Tick time and lateness.
    wins = [TICK.search(l) for l in logs["server"] or []]
    wins = [tuple(float(x) for x in m.groups()) for m in wins if m]
    if wins:
        n = sum(w[0] for w in wins)
        late_mean = sum(w[0] * w[1] for w in wins) / n if n else 0.0
        late_max = max(w[2] for w in wins)
        over = int(sum(w[3] for w in wins))
        skipped = int(sum(w[9] for w in wins))
        work = [w[8] for w in wins]
        held = [w[7] for w in wins]
        vals = [("tick_work_p95_ms", p95(work)), ("tick_work_max_ms", max(work)),
                ("tick_held_p95_ms", p95(held)), ("tick_held_max_ms", max(held)),
                ("late_mean_ms", late_mean), ("late_max_ms", late_max),
                ("late_over_budget", over), ("ticks_skipped", skipped),
                ("tick_windows", len(wins))]
        for k, v in vals:
            row("tick", k, f"{v:.2f}" if isinstance(v, float) else str(v))
        lines.append(
            f"soak: tick work p95 {p95(work):.1f} ms max {max(work):.1f} ms (over {len(wins)} windows), "
            f"lock held p95 {p95(held):.1f} ms max {max(held):.1f} ms; "
            f"late mean {late_mean:.2f} ms max {late_max:.1f} ms, {over} over budget, {skipped} skipped")
    else:
        lines.append("soak: note: no '[world] tick health' line in server.log")
        row("tick", "windows", "0")

    # PA-0 profile (absent from an older server's log).
    prof = parse_prof(logs["server"] or [])
    lines += prof_summary(prof)
    if prof["lock"]:
        row("prof", "lock_total_occupancy_pct_max", f"{max(w[8] for w in prof['lock']):.1f}")

    # Refused calls: the server's UserErrors by key, the bots' refusals by kind.
    users = Counter()
    for l in logs["server"] or []:
        m = USERERR.match(l)
        if m:
            users[userkey(m.group(1))] += 1
    for k, c in users.items():
        row("usererror", k, str(c))
    lines.append(f"soak: {sum(users.values())} UserErrors" + (", top:" if users else ""))
    lines += [f"soak:   {c:5d} {k}" for k, c in users.most_common(8)]

    # Bots: homing Recovery docks, acts that failed or were refused, by kind.
    homing, failed, refused = 0, Counter(), Counter()
    for l in logs["bots"] or []:
        m = BOTLINE.match(l)
        if not m:
            continue
        kind = m.group(2)
        if "homing Recovery" in l and kind == "dock":
            homing += 1
        outcome = l.rsplit(" -> ", 1)[-1] if " -> " in l else ""
        if outcome.startswith("FAILED"):
            failed[kind] += 1
        elif outcome.startswith("refused"):
            refused[kind] += 1
    row("bots", "homing_recovery_docks", str(homing))
    for k, c in failed.items():
        row("act_failed", k, str(c))
    for k, c in refused.items():
        row("act_refused", k, str(c))
    lines.append(f"soak: bots: {homing} 'homing Recovery' docks, {sum(failed.values())} acts FAILED, "
                 f"{sum(refused.values())} refused")
    for tag, ctr in (("FAILED", failed), ("refused", refused)):
        lines += [f"soak:   {c:5d} {tag} {k}" for k, c in ctr.most_common(6)]

    # The deny-list.
    for name, rx, where in DENY:
        for log in where:
            hits = sum(1 for l in logs[log] or [] if rx.search(l) and not BENIGN.search(l))
            row("deny", f"{name}@{log}", str(hits))
            if hits:
                limit = allow.get(name, 0)
                tag = "" if hits <= limit else " FAIL"
                lines.append(f"soak: deny-list {name} in {log}.log: {hits} (allowed {limit}){tag}")
                if hits > limit:
                    fails.append(f"deny: {hits} {name} in {log}.log")

    # Cleanup: every bot offline after the bots stopped.
    off = read(out / "offline.txt")
    pilots = None
    if off:
        m = re.search(r"pilots (\d+)", off[-1])
        pilots = int(m.group(1)) if m else None
    if pilots is None:
        lines.append("soak: note: no pilots count after the bots stopped; offline not judged")
        row("cleanup", "pilots_after_stop", "unknown")
    else:
        row("cleanup", "pilots_after_stop", str(pilots))
        if pilots:
            lines.append(f"soak: {pilots} pilot(s) still online after every bot stopped FAIL")
            fails.append(f"cleanup: {pilots} online after stop")
        else:
            lines.append("soak: every bot offline after stop")
    return lines, rows, fails


def main(argv):
    args, allow, out = argv[1:], {}, None
    while args:
        a = args.pop(0)
        if a == "--allow" and args:
            k, _, v = args.pop(0).partition("=")
            allow[k] = int(v)
        else:
            out = Path(a)
    if out is None:
        print("usage: soak_summary.py OUTDIR [--allow KEY=N]", file=sys.stderr)
        return 2
    lines, rows, fails = summarize(out, allow)
    print("\n".join(lines))
    with open(out / "summary.tsv", "w") as f:
        f.write("metric\tkey\tvalue\n")
        for r in rows:
            f.write("\t".join(r) + "\n")
    for x in fails:
        print(f"soak: FAIL {x}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
