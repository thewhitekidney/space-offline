#!/usr/bin/env python3
"""Ramp bots into ONE system and find where the server (and eve-bots) bend.

Driven by tools/bench-ramp.sh (builds, makes the accounts, then runs this);
see docs/guides/bench-ramp.md. Std-only. The pure parts (roster, tick-line
parsing, step summary, knee and collapse) are unit-tested in
tools/test_bench_ramp.py; `run()` is the orchestrator.

    bench_ramp.py --bin DIR --out RUNDIR --world WORLD --tokens FILE ...
    bench_ramp.py --summarize RUNDIR     # redo steps.tsv / summary from logs
"""
import argparse
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.append(str(Path(__file__).resolve().parent))
import soak_summary as ss  # noqa: E402  (p95, the bots' act-line shape)

TICK_S = 0.25       # push.rs TICK
WINDOW_TICKS = 40   # push.rs REPORT_EVERY
BUDGET_MS = TICK_S * 1000
KNEE_FRACTION = 0.5  # of the tick budget
COLLAPSE_WINDOWS = 3  # consecutive windows with a tick over budget

# The tick-health line, with the lane's trailing counters optional (an older
# server prints none of them).
TICK = re.compile(
    r"\[world\] tick health: (?P<ticks>\d+) ticks, late mean (?P<late_mean>[\d.]+) ms max (?P<late_max>[\d.]+) ms, "
    r"over budget (?P<over>\d+), world lock wait mean (?P<wait_mean>[\d.]+) ms max (?P<wait_max>[\d.]+) ms, "
    r"world lock held mean (?P<held_mean>[\d.]+) ms max (?P<held_max>[\d.]+) ms, work max (?P<work_max>[\d.]+) ms, "
    r"skipped (?P<skipped>\d+)"
    r"(?:, work p95 (?P<work_p95>[\d.]+) ms, out (?P<out_frames>\d+) frames (?P<out_bytes>\d+) bytes, "
    r"destiny (?P<dest_frames>\d+) frames (?P<dest_bytes>\d+) bytes"
    r"(?: parts (?P<dest_parts>\d+) merged (?P<dest_merged>\d+))?)?"
    r"(?:, slice_max (?P<slice_max>[\d.]+) ms)?"
)
LIVE_PILOTS = re.compile(r"^\[live\].*? pilots (\d+) \(in space (\d+)\)")

COLUMNS = [
    "step", "target_bots", "pilots", "in_space", "windows",
    "work_p95_ms", "work_max_ms", "held_mean_ms", "held_p95_ms", "held_max_ms",
    "wait_max_ms", "late_max_ms", "over_budget", "skipped",
    "srv_cpu_pct", "srv_rss_mb", "srv_threads",
    "bots_cpu_pct", "bots_rss_mb", "bots_threads",
    "out_kBps", "destiny_kBps", "destiny_frames_per_tick", "destiny_bytes_per_bot_s",
    "act_failed", "act_refused", "usererrors",
    # PA-3 lock slices: the longest single write-lock slice (max and p95 over windows; 0 before PA-3)
    "slice_max_ms", "slice_p95_ms",
    # PA-0 profile (live_status::prof): ms per tick, means over the step
    "take_due_ms", "filter_ms", "audience_ms", "session_changes_ms", "save_max_ms",
    "clock_occ_pct", "payload_occ_pct", "payload_turns_per_s", "ro_guess_turn_pct",
    "writer_frames_per_s", "writer_pass_max_ms", "deflate_ms_total", "longest_write_ms",
    # PA-12a read turns (payload_occ_pct above is the EXCLUSIVE occupancy from here on)
    "ro_turns_per_s", "ro_served_turn_pct", "ro_fallbacks",
]


# ---- the scenario ---------------------------------------------------------

def parse_mix(text):
    """`miners=60,ratters=25,fighters=15` -> {kind: weight}; kinds checked."""
    mix = {}
    for part in text.split(","):
        k, _, v = part.partition("=")
        k = k.strip()
        if k not in KINDS or not v.strip().isdigit() or int(v) < 0:
            raise ValueError(f"--mix {text!r}: want {'|'.join(KINDS)}=N pairs")
        mix[k] = int(v)
    if sum(mix.values()) == 0:
        raise ValueError("--mix: all weights are zero")
    return {k: v for k, v in mix.items() if v}


# kind -> (group prefix, role, skills, ship, side)
KINDS = {
    "miners": ("bm", "mine", "role", "Hulk", "civ"),
    "ratters": ("br", "sites", "role", "Vexor", "civ"),
    "fighters": ("bf", "hunter", "role", "Rifter", "red"),
}
SPREAD_GROUP = 5     # miners per fleet in --grid spread: one boss (a belt) per five
FLEET_CAP = 250      # Fleets::MAX_MEMBERS_IN_FLEET is 256


def split_counts(total, mix):
    """Largest-remainder split of `total` bots over the mix."""
    weight = sum(mix.values())
    base = {k: total * v // weight for k, v in mix.items()}
    rest = total - sum(base.values())
    order = sorted(mix, key=lambda k: (-(total * mix[k] % weight), k))
    for k in order[:rest]:
        base[k] += 1
    return base


def roster(total, mix, grid):
    """The roster TSV text for `total` bots, and the bot names (in order).

    --grid same: one group per kind (one fleet, so the miners follow one boss
    to one belt; a fight or a site is one place too), split at the fleet cap.
    --grid spread: miners in fleets of five, each boss picking a belt of its
    own; the other kinds as in `same` (they roam by themselves)."""
    counts = split_counts(total, mix)
    rows, names = [], []
    for kind, n in counts.items():
        if n == 0:
            continue
        prefix, role, skills, ship, side = KINDS[kind]
        size = SPREAD_GROUP if (grid == "spread" and kind == "miners") else FLEET_CAP
        gi = 0
        while n > 0:
            c = min(n, size)
            gi += 1
            group = f"{prefix}{gi}x"
            rows.append(f"{group}\t{side}\t{role}\t{c}\t{skills}\t{ship}")
            names += [f"{group}{i}" for i in range(1, c + 1)]
            n -= c
    return "# bench-ramp roster (generated)\n" + "\n".join(rows) + "\n", names


def steps_of(n0, add, nmax):
    """The bot counts of each step: n0, n0+add, ... capped at nmax."""
    out, n = [], n0
    while n < nmax:
        out.append(n)
        n += add
    out.append(nmax)
    return out


# ---- reading the logs -----------------------------------------------------

def parse_ticks(lines):
    """One dict of floats per `tick health` line."""
    out = []
    for l in lines:
        m = TICK.search(l)
        if m:
            out.append({k: (float(v) if v is not None else None) for k, v in m.groupdict().items()})
    return out


def last_pilots(lines):
    got = None
    for l in lines:
        m = LIVE_PILOTS.match(l)
        if m:
            got = (int(m.group(1)), int(m.group(2)))
    return got


def bot_acts(lines):
    failed = refused = 0
    for l in lines:
        m = ss.BOTLINE.match(l)
        if not m or " -> " not in l:
            continue
        outcome = l.rsplit(" -> ", 1)[-1]
        failed += outcome.startswith("FAILED")
        refused += outcome.startswith("refused")
    return failed, refused


def summarize_step(idx, target, server_lines, bot_lines, srv, bots, bot_count):
    """One step's row (dict of COLUMNS). `srv`/`bots` are dicts with cpu_pct,
    rss_mb, threads (the step's means / peaks, from /proc); `bot_count` the
    bots started so far (for per-bot figures)."""
    w = parse_ticks(server_lines)
    n = len(w)
    secs = max(1.0, n * WINDOW_TICKS * TICK_S)
    held_max = [x["held_max"] for x in w]
    work_p95 = [x["work_p95"] if x["work_p95"] is not None else x["work_max"] for x in w]
    pil = last_pilots(server_lines) or (0, 0)
    have_out = [x for x in w if x["out_bytes"] is not None]
    out_b = sum(x["out_bytes"] for x in have_out)
    dest_b = sum(x["dest_bytes"] for x in have_out)
    dest_f = sum(x["dest_frames"] for x in have_out)
    ticks = sum(x["ticks"] for x in w) or 1
    failed, refused = bot_acts(bot_lines)
    usererr = sum(1 for l in server_lines if ss.USERERR.match(l))
    row = {
        "step": idx, "target_bots": target, "pilots": pil[0], "in_space": pil[1], "windows": n,
        "work_p95_ms": ss.p95(work_p95), "work_max_ms": max([x["work_max"] for x in w] or [0]),
        "held_mean_ms": (sum(x["held_mean"] * x["ticks"] for x in w) / ticks) if w else 0,
        "held_p95_ms": ss.p95(held_max), "held_max_ms": max(held_max or [0]),
        "wait_max_ms": max([x["wait_max"] for x in w] or [0]),
        "late_max_ms": max([x["late_max"] for x in w] or [0]),
        "over_budget": int(sum(x["over"] for x in w)), "skipped": int(sum(x["skipped"] for x in w)),
        "srv_cpu_pct": srv.get("cpu_pct", 0), "srv_rss_mb": srv.get("rss_mb", 0),
        "srv_threads": srv.get("threads", 0),
        "bots_cpu_pct": bots.get("cpu_pct", 0), "bots_rss_mb": bots.get("rss_mb", 0),
        "bots_threads": bots.get("threads", 0),
        "out_kBps": out_b / secs / 1024, "destiny_kBps": dest_b / secs / 1024,
        "destiny_frames_per_tick": dest_f / ticks,
        "destiny_bytes_per_bot_s": (dest_b / secs / bot_count) if bot_count else 0,
        "act_failed": failed, "act_refused": refused, "usererrors": usererr,
    }
    slices = [x["slice_max"] for x in w if x["slice_max"] is not None]
    row["slice_max_ms"] = max(slices or [0.0])
    row["slice_p95_ms"] = ss.p95(slices) if slices else 0.0
    row.update(prof_columns(ss.parse_prof(server_lines), secs))
    row["_over_runs"] = longest_over_run(w)
    return row


def prof_columns(prof, secs):
    """The PA-0 profile columns of a step (zeros for an older server)."""
    def mean(vals):
        return sum(vals) / len(vals) if vals else 0.0

    def mx(vals):
        return max(vals) if vals else 0.0
    sec, lk, me, wr = prof["sections"], prof["lock"], prof["methods"], prof["writers"]
    turns = sum(w[4] for w in lk)
    mturns = sum(w[0] for w in me)
    return {
        "take_due_ms": mean([w[0] for w in sec]), "filter_ms": mean([w[2] for w in sec]),
        "audience_ms": mean([w[4] for w in sec]), "session_changes_ms": mean([w[6] for w in sec]),
        "save_max_ms": mx([w[9] for w in sec]),
        "clock_occ_pct": mean([w[1] for w in lk]), "payload_occ_pct": mean([w[3] for w in lk]),
        "payload_turns_per_s": turns / secs,
        "ro_guess_turn_pct": (sum(w[3] * w[0] for w in me) / mturns) if mturns else 0.0,
        "writer_frames_per_s": mean([w[1] for w in wr]), "writer_pass_max_ms": mx([w[6] for w in wr]),
        "deflate_ms_total": sum(w[8] for w in wr), "longest_write_ms": mx([w[12] for w in wr]),
        "ro_turns_per_s": sum(w[1] for w in prof.get("read", []) if w[0] == "lock") / secs,
        "ro_served_turn_pct": mean([w[1] for w in prof.get("read", []) if w[0] == "methods"]),
        "ro_fallbacks": sum(w[3] for w in prof.get("read", []) if w[0] == "methods"),
    }


def longest_over_run(windows):
    best = run = 0
    for x in windows:
        run = run + 1 if x["over"] > 0 else 0
        best = max(best, run)
    return best


def knee(rows, fraction=KNEE_FRACTION):
    """(row, reason) of the first step past the knee, else (None, '')."""
    limit = BUDGET_MS * fraction
    for r in rows:
        why = []
        if r["held_p95_ms"] > limit:
            why.append(f"lock held p95 {r['held_p95_ms']:.1f} ms > {limit:.0f} ms")
        if r["work_p95_ms"] > limit:
            why.append(f"tick work p95 {r['work_p95_ms']:.1f} ms > {limit:.0f} ms")
        if r["over_budget"] > 0:
            why.append(f"{r['over_budget']} ticks over budget")
        if r["skipped"] > 0:
            why.append(f"{r['skipped']} ticks skipped")
        if why:
            return r, "; ".join(why)
    return None, ""


def collapse(rows):
    """A reason to stop after the last of `rows`, else ''."""
    if not rows:
        return ""
    last = rows[-1]
    if last.get("_over_runs", 0) >= COLLAPSE_WINDOWS:
        return f"over budget in {last['_over_runs']} consecutive windows"
    if last["skipped"] >= WINDOW_TICKS // 2:
        return f"{last['skipped']} ticks skipped in one step"
    if len(rows) >= 2:
        prev = rows[-2]
        if prev["pilots"] > 0 and last["pilots"] < 0.9 * prev["pilots"]:
            return f"pilots fell {prev['pilots']} -> {last['pilots']}: bots disconnecting"
    return ""


def fmt(v):
    return f"{v:.2f}" if isinstance(v, float) else str(v)


def write_tsv(path, rows):
    with open(path, "w") as f:
        f.write("\t".join(COLUMNS) + "\n")
        for r in rows:
            f.write("\t".join(fmt(r[c]) for c in COLUMNS) + "\n")


def final_summary(rows, stopped, args_line=""):
    lines = [f"bench-ramp: {len(rows)} step(s){(' ' + args_line) if args_line else ''}"]
    if rows:
        lines.append("bench-ramp: step  bots  pilots  work_p95  held_p95  held_max  srv_cpu%  srv_MB  bots_cpu%  bots_MB  out_kB/s")
        for r in rows:
            lines.append(
                f"bench-ramp: {r['step']:4d} {r['target_bots']:5d} {r['pilots']:7d} {r['work_p95_ms']:9.1f} "
                f"{r['held_p95_ms']:9.1f} {r['held_max_ms']:9.1f} {r['srv_cpu_pct']:9.1f} {r['srv_rss_mb']:7.0f} "
                f"{r['bots_cpu_pct']:10.1f} {r['bots_rss_mb']:8.0f} {r['out_kBps']:9.1f}")
    k, why = knee(rows)
    if k:
        lines.append(f"bench-ramp: KNEE at {k['target_bots']} bots (step {k['step']}): {why}")
    else:
        top = rows[-1]["target_bots"] if rows else 0
        lines.append(f"bench-ramp: no knee up to {top} bots (limit {BUDGET_MS * KNEE_FRACTION:.0f} ms, "
                     f"no late/skipped ticks)")
    lines.append(f"bench-ramp: {'STOPPED EARLY: ' + stopped if stopped else 'ramp finished'}")
    return lines


# ---- /proc ----------------------------------------------------------------

HZ = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100


def proc_stat(pid):
    """(cpu seconds, rss MB, threads) or None."""
    try:
        st = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        cpu = (int(st[11]) + int(st[12])) / HZ
        rss = thr = 0
        for l in Path(f"/proc/{pid}/status").read_text().splitlines():
            if l.startswith("VmRSS:"):
                rss = int(l.split()[1]) / 1024
            elif l.startswith("Threads:"):
                thr = int(l.split()[1])
        return cpu, rss, thr
    except (OSError, IndexError, ValueError):
        return None


class Sampler:
    """CPU% (of one core) over an interval, RSS and threads peaks, for a pid."""

    def __init__(self, pid):
        self.pid, self.last, self.peak_rss, self.peak_thr = pid, None, 0.0, 0
        self.begin()

    def begin(self):
        s = proc_stat(self.pid)
        self.t0 = time.monotonic()
        self.c0 = s[0] if s else 0.0
        self.peak_rss, self.peak_thr = (s[1], s[2]) if s else (0.0, 0)

    def poll(self):
        s = proc_stat(self.pid)
        if s:
            self.peak_rss, self.peak_thr = max(self.peak_rss, s[1]), max(self.peak_thr, s[2])
            self.last = s

    def end(self):
        self.poll()
        dt = max(1e-6, time.monotonic() - self.t0)
        cpu = ((self.last[0] - self.c0) / dt * 100) if self.last else 0.0
        return {"cpu_pct": cpu, "rss_mb": self.peak_rss, "threads": self.peak_thr}


# ---- the run --------------------------------------------------------------

def lines_between(path, a, b):
    try:
        with open(path, "rb") as f:
            f.seek(a)
            return f.read(max(0, b - a)).decode(errors="replace").splitlines()
    except OSError:
        return []


def size(path):
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def ready(lines, want):
    """--prep: every bot is in the game and most are out of the station
    (their kits are bought and fitted, which is what the prep is for)."""
    got = last_pilots(lines)
    return bool(got) and got[0] >= want and got[1] >= 0.85 * want


def run(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    text, names = roster(a.max, parse_mix(a.mix), a.grid)
    (out / "roster.tsv").write_text(text)
    steps = steps_of(a.start, a.add, a.max)
    slog, blog = out / "server.log", out / "bots.log"
    world = Path(a.world)
    stop_file = Path(str(world) + ".stop")
    for stale in (stop_file,):
        stale.unlink(missing_ok=True)
    env = dict(os.environ, EVE_GIT_SHA="bench")
    srv_args = [f"{a.bin}/{os.environ.get('EVE_SERVER_BIN', 'eve-offline-server')}", "--dev", "--world", str(world), "--world-accounts",
                "--status", str(out / "status.tsv"),
                "--bind", f"127.0.0.1:{a.port},signup=on,capture=off,cap={max(2048, 2 * a.max)},"
                f"chat=127.0.0.1:{a.port + 1}"]
    srv_args.append("--turn-sched")  # TT-A: cpu / runq in the [turn-slow] lines
    if a.content:
        srv_args += ["--content", a.content]
    if a.authored:
        srv_args += ["--authored", a.authored]
    # Extra eve-server flags for a run (e.g. "--tick-threads 1").
    srv_args += os.environ.get("EVE_BENCH_SERVER_ARGS", "").split()
    total_secs = a.hold * len(steps) + 60
    bot_args = [f"{a.bin}/eve-bots", "--connect", f"127.0.0.1:{a.port}", "--tokens", a.tokens,
                "--roster", str(out / "roster.tsv"), "--roster-home", a.system, "--society-pvp",
                "--chat", "on", "--ramp", f"{a.start},{a.add},{a.hold},{a.stagger_ms}",
                "--run", str(total_secs)]
    if a.content:
        bot_args += ["--content", a.content]
    # Extra eve-bots flags for a profile run (e.g. "--stats 10 --quiet").
    bot_args += os.environ.get("EVE_BENCH_BOT_ARGS", "").split()
    print(f"bench-ramp: server {a.port}, world {world}, {len(steps)} steps {steps}, hold {a.hold}s", flush=True)
    sp = subprocess.Popen(srv_args, stdout=open(slog, "w"), stderr=subprocess.STDOUT, env=env)
    bp = None
    rows, stopped = [], ""
    try:
        for _ in range(180):
            if "listening" in slog.read_text(errors="replace"):
                break
            if sp.poll() is not None:
                raise SystemExit(f"bench-ramp: the server died on startup; see {slog}")
            time.sleep(1)
        bp = subprocess.Popen(bot_args, stdout=open(blog, "w"), stderr=subprocess.STDOUT)
        t_bots = time.monotonic()
        ssamp, bsamp = Sampler(sp.pid), Sampler(bp.pid)
        seen = [None, time.monotonic()]
        for i, target in enumerate(steps):
            # Step i is bots' batch i (it logs in at i*hold); measure to (i+1)*hold.
            end = t_bots + (i + 1) * a.hold
            s0, b0 = size(slog), size(blog)
            ssamp.begin()
            bsamp.begin()
            while time.monotonic() < end:
                time.sleep(min(2.0, max(0.0, end - time.monotonic())))
                ssamp.poll()
                bsamp.poll()
                if sp.poll() is not None or bp.poll() is not None:
                    stopped = "server exited" if sp.poll() is not None else "bots exited"
                    break
                if a.prep:
                    got = last_pilots(lines_between(slog, s0, size(slog)))
                    if got != seen[0]:
                        seen[:] = [got, time.monotonic()]
                    if ready(lines_between(slog, s0, size(slog)), target) or (
                            got and got[0] >= target and time.monotonic() - seen[1] > 90):
                        break  # all in, and nobody new has left the station for 90 s
            s1, b1 = size(slog), size(blog)
            row = summarize_step(i, target, lines_between(slog, s0, s1), lines_between(blog, b0, b1),
                                 ssamp.end(), bsamp.end(), target)
            rows.append(row)
            write_tsv(out / "steps.tsv", rows)
            print(f"bench-ramp: step {i} bots {target}: pilots {row['pilots']}, work p95 {row['work_p95_ms']:.1f} ms, "
                  f"held p95 {row['held_p95_ms']:.1f} max {row['held_max_ms']:.1f} ms, srv {row['srv_cpu_pct']:.0f}% "
                  f"{row['srv_rss_mb']:.0f} MB, bots {row['bots_cpu_pct']:.0f}% {row['bots_rss_mb']:.0f} MB, "
                  f"out {row['out_kBps']:.0f} kB/s", flush=True)
            stopped = stopped or collapse(rows)
            if stopped:
                break
    finally:
        if bp and bp.poll() is None:
            bp.send_signal(signal.SIGTERM)
            try:
                bp.wait(20)
            except subprocess.TimeoutExpired:
                bp.kill()
        if sp.poll() is None:
            stop_file.touch()
            try:
                sp.wait(60)
            except subprocess.TimeoutExpired:
                sp.send_signal(signal.SIGTERM)
                sp.wait(20)
    extra = " ".join(os.environ.get(v, "") for v in ("EVE_BENCH_SERVER_ARGS", "EVE_BENCH_BOT_ARGS")).strip()
    lines = final_summary(rows, stopped, f"({a.grid}, mix {a.mix}, {a.system}{', ' + extra if extra else ''})")
    (out / "summary.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--bin")
    p.add_argument("--out")
    p.add_argument("--world")
    p.add_argument("--tokens")
    p.add_argument("--content")
    p.add_argument("--authored")
    p.add_argument("--port", type=int, default=26500)
    p.add_argument("--start", type=int, default=50)
    p.add_argument("--add", type=int, default=50)
    p.add_argument("--max", type=int, default=300)
    p.add_argument("--hold", type=int, default=75)
    p.add_argument("--stagger-ms", type=int, default=50)
    p.add_argument("--grid", choices=["same", "spread"], default="same")
    p.add_argument("--mix", default="miners=100")
    p.add_argument("--system", default="Amamake")
    p.add_argument("--prep", action="store_true",
                   help="warm the world in --world (kits bought) and stop when the bots are out")
    a = p.parse_args(argv)
    parse_mix(a.mix)
    if not (a.bin and a.out and a.world and a.tokens):
        p.error("--bin, --out, --world and --tokens are required")
    if a.start < 1 or a.add < 1 or a.max < a.start or a.hold < 20:
        p.error("want 1 <= --start <= --max, --add >= 1, --hold >= 20")
    return run(a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
