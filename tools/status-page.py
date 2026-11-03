#!/usr/bin/env python3
"""Build the progress page from one table (issue #164).

`docs/status/progress.tsv` is the single source: one row per feature. This
tool writes `docs/status/README.md` and four SVGs beside it, so every number
on the page comes from that table and `docs/status/waves.tsv`. Nothing here
reads `var/`, the clock or the network when it writes, so two machines get the
same bytes and `--check` can run in the gate.

    python3 -I tools/status-page.py           # write the page
    python3 -I tools/status-page.py --check   # fail if it is stale or a row is bad
    python3 -I tools/status-page.py --audit   # compare rows with var/regress-*; clean: stamp checked-at
    python3 -I tools/status-page.py --waves   # add var/regress-wave* counts to waves.tsv

`--audit` and `--waves` read the gitignored `var/`, so only a machine with the
regression results can run them. `--audit` reports and never edits a row: a
person decides what a red means. A clean audit stamps `# checked-at <HEAD>`
unless `--no-stamp` is given (the gate passes it). It also fails on a scenario
with a verdict, or a regress.list row, that no row cites; without `var/` it skips.

Issue numbers appear on the page only when `EVE_ISSUES_URL` is set (they are
rendered as links `$EVE_ISSUES_URL/<n>`); the committed page is built without
it, so it never names a tracker.

The SVGs have an opaque background of their own. A viewer picks a page theme
that the image cannot see, so an image without one would risk dark ink on a
dark page; with one it reads wherever it lands.
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import subprocess
import sys
from html import escape

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATUS_DIR = "docs/status"
COLUMNS = ["system", "feature", "state", "evidence", "issue", "theme", "note"]
WAVE_COLUMNS = ["wave", "date", "sha", "rows", "green", "red", "no_verdict", "scope", "note"]

# What each state means, in the order the page lists them.
STATES = {
    "client-proved": "A regression scenario passed on the real client in the newest wave that ran it. "
    "It proves what the scenario asserts and no more.",
    "done": "Finished, and proved some other way than a client scenario: a tool that runs, a measured result.",
    "built-unproved": "The code landed and unit tests pass, but no client has passed it: "
    "its scenario was red last time, has never run, or does not exist.",
    "partial": "Some of it works, and the row says what is not there.",
    "stubbed": "The server gives a correctly shaped empty answer so the client does not fail. No real feature behind it.",
    "missing": "Nothing yet.",
    "unknown": "Could not be placed from the evidence. Never a guess.",
    "parked": "The owner has said not now. Left out of every percentage.",
}

# The four ways the graphs group the eight states, from most to least finished.
GROUPS = [
    ("proved", "Proved", ("client-proved", "done")),
    ("built", "Built, not proved", ("built-unproved",)),
    ("part", "Part built", ("partial", "stubbed")),
    ("missing", "Missing", ("missing",)),
    ("unknown", "Unknown", ("unknown",)),
]
GROUP_OF = {state: key for key, _, states in GROUPS for state in states}
FINISHED = ("client-proved", "done")
NO_THEME = "Not on the plan yet"

# Regression rows that test no feature, so --audit does not ask for a row.
NOT_FEATURES = {
    "fci_api_probe": "a read-only probe of the routes the scenarios use",
    "fci_gw_notice_echo": "skips itself: the gateway notice echo has no dev hook yet",
}

# Colours: an ordinal blue ramp, darkest for most finished on a light surface
# and reversed on a dark one, each checked with the dataviz skill's validator
# (--ordinal). Missing is the bar's own track; unknown takes a second hue
# because it is not a step on that scale.
STYLE = """<style>
.bg{fill:#fcfcfb}.t{fill:#0b0b0b}.s{fill:#52514e}.track{fill:#e1e0d9}
.proved{fill:#184f95}.built{fill:#3987e5}.part{fill:#86b6ef}.missing{fill:#e1e0d9}.unknown{fill:#eb6834}
.bad{fill:#eb6834}.none{fill:#c3c2b7}
.in-proved,.in-built,.in-unknown,.in-bad{fill:#fff}.in-part,.in-missing,.in-none{fill:#0b0b0b}
@media (prefers-color-scheme:dark){
.bg{fill:#1a1a19}.t{fill:#f0efec}.s{fill:#c3c2b7}.track{fill:#383835}
.proved{fill:#86b6ef}.built{fill:#3987e5}.part{fill:#184f95}.missing{fill:#383835}.unknown{fill:#d95926}
.bad{fill:#d95926}.none{fill:#52514e}
.in-proved{fill:#0b0b0b}.in-part,.in-missing,.in-none{fill:#f0efec}
}
text{font-family:system-ui,-apple-system,"Segoe UI",sans-serif;font-size:14px}
.big{font-size:40px;font-weight:600}.b{font-weight:600}.sm{font-size:13px}
</style>"""

WIDTH = 480  # small on purpose: scaled to a phone, 14 px stays readable
PAD = 16
BAR = WIDTH - 2 * PAD
GAP = 2  # the surface shows between segments


class Bad(Exception):
    pass


def read_table(path: str, columns: list[str]) -> tuple[list[dict], list[str]]:
    """The rows and the leading `#` comment lines of a tsv."""
    rows, comments = [], []
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    header_seen = False
    for n, line in enumerate(lines, 1):
        if not line.strip():
            continue
        if line.startswith("#"):
            comments.append(line)
            continue
        cells = line.split("\t")
        if not header_seen:
            if cells != columns:
                raise Bad(f"{path}:{n}: the header must be {' '.join(columns)}")
            header_seen = True
            continue
        if len(cells) != len(columns):
            raise Bad(f"{path}:{n}: {len(cells)} cells, want {len(columns)}")
        row = dict(zip(columns, cells))
        row["line"] = n
        rows.append(row)
    return rows, comments


def scenario_names(root: str) -> set[str]:
    return {
        os.path.basename(p)[:-3]
        for p in glob.glob(os.path.join(root, "tools/client-ci/scenarios/*.py"))
        if not os.path.basename(p).startswith("_")
    }


def scenarios_in(text: str, known: set[str]) -> list[str]:
    """Scenario names a row's evidence cites, with any `@label` kept."""
    found = []
    for word in re.findall(r"[A-Za-z_][A-Za-z0-9_]*(?:@[a-z0-9_]+)?", text):
        if word.split("@")[0] in known or word.startswith("fci_"):
            found.append(word)
    return found


def validate(rows: list[dict], known: set[str], themes: dict[str, str]) -> list[str]:
    """Every complaint about the table; empty means it is sound."""
    fail, seen = [], set()
    for r in rows:
        at = f"line {r['line']} ({r['feature'] or 'no feature'})"
        for col in ("system", "feature", "state", "evidence"):
            if not r[col]:
                fail.append(f"{at}: empty {col}")
        if r["state"] and r["state"] not in STATES:
            fail.append(f"{at}: state {r['state']!r} is not one of {', '.join(STATES)}")
        key = (r["system"], r["feature"])
        if key in seen:
            fail.append(f"{at}: the same feature twice in {r['system']}")
        seen.add(key)
        if r["issue"] and not re.fullmatch(r"#\d+(, #\d+)*", r["issue"]):
            fail.append(f"{at}: issue must read #N or #N, #M")
        if r["theme"] and r["theme"] not in themes:
            fail.append(f"{at}: theme {r['theme']} is not a row of {THEMES}")
        if r["state"] and r["state"] not in FINISHED and not r["theme"]:
            fail.append(f"{at}: an open row needs a theme (a row of {THEMES})")
        if r["state"] in FINISHED and r["theme"]:
            fail.append(f"{at}: a finished row has no theme")
        cited = scenarios_in(r["evidence"], known)
        for name in cited:
            if name.split("@")[0] not in known:
                fail.append(f"{at}: no scenario called {name}")
        if r["state"] == "client-proved" and not cited and "census" not in r["evidence"]:
            fail.append(f"{at}: client-proved must name a scenario or a census")
        if re.search(r"(?<![\d.])\d{8,9}(?![\d.])", "\t".join(r[c] for c in COLUMNS)):
            fail.append(f"{at}: an 8-9 digit number looks like a client id")
    return fail


THEMES = "docs/status/themes.tsv"


def plan_themes(root: str) -> dict[str, str]:
    """The themes, `3.N<TAB>Name` per line of docs/status/themes.tsv (public; it
    mirrors the master plan's `### 3.N` headings, which a test checks)."""
    out = {}
    with open(os.path.join(root, THEMES), encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line and not line.startswith("#"):
                key, _, name = line.partition("\t")
                out[key] = name
    return out


def issue_text(issue: str) -> str:
    """The issue cell as the page shows it: nothing unless EVE_ISSUES_URL is set."""
    base = os.environ.get("EVE_ISSUES_URL", "").rstrip("/")
    if not issue or not base:
        return ""
    return ", ".join(f"[{n}]({base}/{n[1:]})" for n in re.findall(r"#\d+", issue))


def counts(rows: list[dict]) -> dict[str, int]:
    out = {state: 0 for state in STATES}
    for r in rows:
        out[r["state"]] += 1
    return out


def group_counts(rows: list[dict]) -> dict[str, int]:
    out = {key: 0 for key, _, _ in GROUPS}
    for r in rows:
        if r["state"] in GROUP_OF:
            out[GROUP_OF[r["state"]]] += 1
    return out


def percent(part: int, whole: int) -> int:
    return round(100 * part / whole) if whole else 0


def headline(rows: list[dict]) -> dict[str, int]:
    """Row counts behind the two percentages. Parked rows are not counted."""
    g = group_counts(rows)
    counted = sum(g.values())
    return {
        "counted": counted,
        "proved": g["proved"],
        "built": g["proved"] + g["built"],
        "proved_pct": percent(g["proved"], counted),
        "built_pct": percent(g["proved"] + g["built"], counted),
        "parked": counts(rows)["parked"],
    }


def systems(rows: list[dict]) -> list[str]:
    """Systems in the table's own order."""
    return list(dict.fromkeys(r["system"] for r in rows))


def rect(x, y, w, h, cls: str, r: float = 0) -> str:
    corner = f' rx="{r}"' if r else ""
    return f'<rect x="{x:.1f}" y="{y}" width="{w:.1f}" height="{h}"{corner} class="{cls}"/>'


def text(x, y, s, cls: str = "t", anchor: str = "") -> str:
    a = f' text-anchor="{anchor}"' if anchor else ""
    return f'<text x="{x:.1f}" y="{y}" class="{cls}"{a}>{escape(str(s))}</text>'


def svg(height: int, label: str, body: list[str]) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {height}" role="img" '
        f'aria-label="{escape(label, quote=True)}">\n<title>{escape(label)}</title>\n{STYLE}\n'
        f'<rect width="{WIDTH}" height="{height}" rx="8" class="bg"/>\n' + "\n".join(body) + "\n</svg>\n"
    )


def stacked(x, y, width, height, parts: list[tuple[str, int]], total: int, labels: bool = True) -> list[str]:
    """A bar on a track, split by `parts`; a count sits inside a segment that fits it."""
    out = [rect(x, y, width, height, "track", 4)]
    at = x
    for cls, n in parts:
        if not n:
            continue
        w = width * n / total
        if cls != "missing":
            out.append(rect(at, y, max(w - GAP, 1), height, cls, 4 if w > 8 else 0))
        if labels and w >= 8 * len(str(n)) + 8:
            out.append(text(at + w / 2 - GAP / 2, y + height / 2 + 5, n, f"in-{cls} sm", "middle"))
        at += w
    return out


def legend(y: int, entries: list[tuple[str, str]]) -> tuple[list[str], int]:
    """Swatch and label, two to a line, so it fits a phone."""
    out, column = [], (WIDTH - 2 * PAD) / 2
    for i, (cls, label) in enumerate(entries):
        x, line = PAD + (i % 2) * column, y + (i // 2) * 22
        out.append(rect(x, line - 11, 12, 12, cls, 2))
        out.append(text(x + 18, line, label, "s sm"))
    return out, y + (len(entries) + 1) // 2 * 22


def overall_svg(rows: list[dict]) -> str:
    h, g = headline(rows), group_counts(rows)
    body = [
        text(PAD, 56, f"{h['proved_pct']}%", "t big"),
        text(PAD, 78, "proved", "s sm"),
        text(WIDTH / 2 + 8, 56, f"{h['built_pct']}%", "t big"),
        text(WIDTH / 2 + 8, 78, "built, proved or not", "s sm"),
    ]
    body += stacked(PAD, 96, BAR, 32, [(k, g[k]) for k, _, _ in GROUPS], h["counted"])
    marks, end = legend(152, [(k, f"{label}: {g[k]}") for k, label, _ in GROUPS])
    body += marks
    label = f"{h['proved_pct']} percent proved, {h['built_pct']} percent built, of {h['counted']} features"
    return svg(end + 2, label, body)


def systems_svg(rows: list[dict]) -> str:
    def rank(name):
        g = group_counts([r for r in rows if r["system"] == name])
        return (-percent(g["proved"], sum(g.values())), name)

    y, body = 26, []
    names = [n for n in sorted(systems(rows), key=rank) if group_counts([r for r in rows if r["system"] == n])]
    for name in names:
        g = group_counts([r for r in rows if r["system"] == name])
        total = sum(g.values())
        if not total:
            continue
        body.append(text(PAD, y, name, "t"))
        body.append(text(WIDTH - PAD, y, f"{g['proved']} of {total} proved", "s sm", "end"))
        body += stacked(PAD, y + 8, BAR, 20, [(k, g[k]) for k, _, _ in GROUPS], total)
        y += 54
    marks, end = legend(y - 2, [(k, label) for k, label, _ in GROUPS])
    return svg(end + 2, "Progress by system", body + marks)


def left_rows(rows: list[dict]) -> list[dict]:
    return [r for r in rows if r["state"] not in FINISHED and r["state"] != "parked"]


def theme_order(rows: list[dict], themes: dict[str, str]) -> list[tuple[str, str]]:
    """(key, title) for each theme with work left, in plan order, the unplanned last."""
    left = left_rows(rows)
    out = [(k, f"{k} {name}") for k, name in themes.items() if any(r["theme"] == k for r in left)]
    if any(not r["theme"] for r in left):
        out.append(("", NO_THEME))
    return out


def left_svg(rows: list[dict], themes: dict[str, str]) -> str:
    left = left_rows(rows)
    order = theme_order(rows, themes)
    biggest = max((sum(r["theme"] == k for r in left) for k, _ in order), default=1)
    keys = [k for k, _, _ in GROUPS if k != "proved"]
    y, body = 26, []
    for key, title in order:
        g = group_counts([r for r in left if r["theme"] == key])
        total = sum(g.values())
        body.append(text(PAD, y, title, "t"))
        body.append(text(WIDTH - PAD, y, f"{total} left", "s sm", "end"))
        at = PAD
        for k in keys:
            if g[k]:
                w = BAR * g[k] / biggest
                # Missing has no track to sit on here, so it wears the track's colour as a mark.
                body.append(rect(at, y + 8, max(w - GAP, 1), 16, k, 4 if w > 8 else 0))
                if w >= 8 * len(str(g[k])) + 8:
                    body.append(text(at + w / 2 - GAP / 2, y + 21, g[k], f"in-{k} sm", "middle"))
                at += w
        y += 50
    marks, end = legend(y - 2, [(k, label) for k, label, _ in GROUPS if k != "proved"])
    return svg(end + 2, "What is left, by theme", body + marks)


def regress_svg(waves: list[dict]) -> str:
    biggest = max((int(w["rows"]) for w in waves), default=1)
    y, body = 26, []
    for w in waves:
        n, green, red, none = int(w["rows"]), int(w["green"]), int(w["red"]), int(w["no_verdict"])
        body.append(text(PAD, y, f"Wave {w['wave']}, {w['scope']}", "t"))
        body.append(text(WIDTH - PAD, y, f"{green} of {n} passed", "s sm", "end"))
        at = PAD
        for cls, c in (("proved", green), ("bad", red), ("none", none)):
            if c:
                seg = BAR * c / biggest
                body.append(rect(at, y + 8, max(seg - GAP, 1), 16, cls, 4 if seg > 8 else 0))
                at += seg
        y += 50
    marks, end = legend(y - 2, [("proved", "Passed"), ("bad", "Failed"), ("none", "No verdict")])
    return svg(end + 2, "Client regression waves, passed of rows run", body + marks)


def row_line(r: dict) -> str:
    mark = "x" if r["state"] in FINISHED else " "
    bits = [f"- [{mark}] {r['feature']}: *{r['state']}*, {r['evidence']}"]
    if issue_text(r["issue"]):
        bits.append(f" ({issue_text(r['issue'])})")
    if r["note"]:
        bits.append(f". {r['note']}")
    return "".join(bits)


GUIDE = """## What the states mean

| state | meaning |
|---|---|
{states}

The graphs group these: **Proved** is client-proved plus done, **Built, not proved** is
built-unproved, **Part built** is partial plus stubbed.

## How to read the numbers

- **Each percentage counts rows, and rows are not equal.** One row may be a window and the next a
  whole game system. Treat the figures as a direction, not a measure of effort.
- **The table lists what the reviews and plans name**, not all of EVE, so 100% would mean this
  list is finished, not the game.
- **Proved** is rows proved divided by all rows except parked ones. **Built** adds the rows with
  landed code and no client pass. Partial, stubbed, missing and unknown rows count against both.
- **A green scenario proves what it asserts and no more.** A row is about a feature; its scenario
  may touch only the main path.
- **Red does not mean broken.** Many reds have been scenario bugs. A row with a red scenario is
  built-unproved until it passes.

## Words used here

- **Scenario** (`fci_cloak`): a scripted test that drives the real client against the server.
- **Wave** (`W8` is wave 8): one run of the scenarios. Rows name the wave that last ran them.
- **Soak**: a long run of many bots. **Gate**: the checks every change passes before it lands.
- **Lane**: one parallel piece of work.
- **`ABYSS-3`, `NW-10`**: names of planned pieces of work.

## How to update this page

Do not edit this file or the SVGs: they are generated, and the gate fails if they drift.

1. Edit `docs/status/progress.tsv`. Columns, separated by tabs: `system`, `feature`, `state`,
   `evidence` (code, a commit or a scenario), `issue` (`#N`, or `#N, #M`), `theme` (a `3.N` row of
   `docs/status/themes.tsv`; required while the row is open, empty when it is finished), `note`.
2. A lane that implements or changes a feature updates its rows or adds new ones. It uses
   `built-unproved` until its scenario has passed; only a regression result may say `client-proved`.
3. Run `python3 -I tools/status-page.py`.
4. After a wave, run `python3 -I tools/status-page.py --waves`, then `--audit`, on the machine
   that holds `var/`. The audit lists rows that disagree with the newest verdicts; fix those by hand.
   A clean audit stamps the table's `# checked-at` line.
5. Evidence names a scenario with its wave (`fci_cloak W8`), or a file. Names of features only:
   no client ids and no extracted data.
"""


def readme(rows: list[dict], waves: list[dict], themes: dict[str, str], checked_at: str) -> str:
    h = headline(rows)
    out = [
        "# Status of the reimplementation",
        "",
        "How far the EVE server has come, feature by feature. Generated by `tools/status-page.py` from",
        f"`docs/status/progress.tsv`, last checked against the code at `{checked_at}`.",
        "",
        f"**{h['proved_pct']}% proved** ({h['proved']} of {h['counted']} features passed on the real client"
        " or were proved another way).",
        f"**{h['built_pct']}% built** ({h['built']} of {h['counted']} have landed code, proved or not).",
        f"{h['parked']} more are parked by the owner and not counted. "
        "See [how to read the numbers](#how-to-read-the-numbers).",
        "",
        "![Overall progress: the share of features proved and built](overall.svg)",
        "",
        "## By system",
        "",
        "![Each system as a bar of proved, built, part built, missing and unknown](systems.svg)",
        "",
        "| system | proved | built, not proved | part built | missing | unknown |",
        "|---|---|---|---|---|---|",
    ]
    for name in systems(rows):
        g = group_counts([r for r in rows if r["system"] == name])
        if sum(g.values()):
            out.append(
                f"| {name} | {g['proved']} | {g['built']} | {g['part']} | {g['missing']} | {g['unknown']} |"
            )
    out += ["", "## What is left, by theme", ""]
    out += ["![Rows left in each theme](left.svg)", ""]
    left = left_rows(rows)
    for key, title in theme_order(rows, themes):
        out += [f"### {title}", ""]
        out += [row_line(r) for r in left if r["theme"] == key]
        out.append("")
    out += ["## Client regression waves", ""]
    if waves:
        out += ["![Passed, failed and no-verdict rows of each wave](regress.svg)", ""]
        out += ["| wave | date | commit | rows | passed | failed | no verdict | what ran |"]
        out += ["|---|---|---|---|---|---|---|---|"]
        for w in waves:
            note = f"{w['scope']}. {w['note']}" if w["note"] else w["scope"]
            out.append(
                f"| {w['wave']} | {w['date']} | `{w['sha']}` | {w['rows']} | {w['green']} "
                f"| {w['red']} | {w['no_verdict']} | {note} |"
            )
        out += [
            "",
            "A wave is one run of `tools/client-ci/regress.list` against real clients. Compare whole "
            "runs with whole runs: a rerun holds only the rows that were red.",
            "",
        ]
    out += ["## Every feature, by system", ""]
    for name in systems(rows):
        members = [r for r in rows if r["system"] == name and r["state"] != "parked"]
        if members:
            out += [f"### {name}", ""]
            out += [row_line(r) for r in members]
            out.append("")
    out += ["## Parked by the owner", ""]
    out += [f"- {r['feature']}: {r['evidence']}" + (f" ({issue_text(r['issue'])})" if issue_text(r["issue"]) else "")
            for r in rows if r["state"] == "parked"]
    out.append("")
    table = "\n".join(f"| {k} | {v} |" for k, v in STATES.items())
    return "\n".join(out) + "\n" + GUIDE.format(states=table)


def build(root: str) -> dict[str, str]:
    """Every generated file, by repository path. Raises Bad on an unsound table."""
    rows, comments = read_table(os.path.join(root, STATUS_DIR, "progress.tsv"), COLUMNS)
    waves, _ = read_table(os.path.join(root, STATUS_DIR, "waves.tsv"), WAVE_COLUMNS)
    themes = plan_themes(root)
    fail = validate(rows, scenario_names(root), themes)
    if fail:
        raise Bad("\n".join(fail))
    found = [c.split()[-1] for c in comments if c.startswith("# checked-at ")]
    checked_at = found[0] if found else "unknown"
    return {
        f"{STATUS_DIR}/README.md": readme(rows, waves, themes, checked_at),
        f"{STATUS_DIR}/overall.svg": overall_svg(rows),
        f"{STATUS_DIR}/systems.svg": systems_svg(rows),
        f"{STATUS_DIR}/left.svg": left_svg(rows, themes),
        f"{STATUS_DIR}/regress.svg": regress_svg(waves),
    }


def stale(root: str, files: dict[str, str]) -> list[str]:
    out = []
    for rel, content in files.items():
        path = os.path.join(root, rel)
        if not os.path.exists(path):
            out.append(rel)
            continue
        with open(path, encoding="utf-8") as f:
            if f.read() != content:
                out.append(rel)
    return out


def regress_dirs(var: str) -> list[tuple[str, str]]:
    """Every var/regress-*/regress.tsv, oldest first by mtime, as (run name, path)."""
    found = []
    for path in glob.glob(os.path.join(var, "regress-*/regress.tsv")):
        name = os.path.basename(os.path.dirname(path))[len("regress-"):]
        found.append((os.path.getmtime(path), name, path))
    return [(name, path) for _, name, path in sorted(found)]


def wave_dirs(var: str) -> list[tuple[str, str]]:
    found = []
    for path in glob.glob(os.path.join(var, "regress-wave*/regress.tsv")):
        m = re.search(r"regress-wave(\d+)/", path)
        if m:
            found.append((m.group(1), path))
    return sorted(found, key=lambda p: int(p[0]))


def read_verdicts(path: str) -> dict[str, str]:
    """name -> exit code of one wave. 245 is the runner's "no verdict"."""
    out = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            cells = line.rstrip("\n").split("\t")
            if len(cells) >= 2:
                out[cells[0]] = cells[1]
    return out


def newest_verdicts(paths: list[tuple[str, str]]) -> dict[str, tuple[str, bool]]:
    """name -> (run, passed) from the newest run that gave it a verdict; paths oldest first."""
    latest = {}
    for wave, path in paths:
        for name, code in read_verdicts(path).items():
            if code != "245":
                latest[name] = (wave, code == "0")
    return latest


def audit(rows: list[dict], known: set[str], latest: dict[str, tuple[str, bool]], listed: list[str]) -> list[str]:
    """Where the table and the regression results disagree."""
    out, claimed = [], set()
    for r in rows:
        cited = scenarios_in(r["evidence"], known)
        claimed.update(cited)
        seen = [latest[c] for c in cited if c in latest]
        red = [c for c in cited if c in latest and not latest[c][1]]
        if r["state"] in FINISHED and red:
            out.append(f"{r['feature']}: {r['state']}, but the newest verdict is red: "
                       + ", ".join(f"{c} ({latest[c][0]})" for c in red))
        elif r["state"] in ("built-unproved", "missing", "unknown") and seen and not red:
            out.append(f"{r['feature']}: {r['state']}, but the newest verdict is green: "
                       + ", ".join(f"{c} ({latest[c][0]})" for c in cited if c in latest))
        if r["state"] == "client-proved" and cited and not seen:
            out.append(f"{r['feature']}: no verdict for {', '.join(cited)}")
    bare = {c.split("@")[0] for c in claimed}
    for name in list(listed) + sorted(n for n in latest if n not in listed):
        if name in NOT_FEATURES or name.split("@")[0] in NOT_FEATURES:
            continue
        if name not in claimed and name.split("@")[0] not in bare:
            where = "a regression row" if name in listed else f"a scenario with a verdict ({latest[name][0]})"
            out.append(f"{name}: {where} no feature cites")
    return out


def listed_rows(root: str) -> list[str]:
    out = []
    with open(os.path.join(root, "tools/client-ci/regress.list"), encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                out.append(line.split()[0])
    return out


def update_waves(root: str, var: str) -> int:
    """Refresh the counts in waves.tsv from var/, keeping what a person wrote.

    A wave with no results here keeps its row, so a worktree without `var/`
    cannot empty the table.
    """
    path = os.path.join(root, STATUS_DIR, "waves.tsv")
    old = {}
    comments: list[str] = []
    if os.path.exists(path):
        rows, comments = read_table(path, WAVE_COLUMNS)
        old = {r["wave"]: r for r in rows}
    found = wave_dirs(var)
    if not found:
        raise Bad(f"no regress-wave*/regress.tsv under {var}")
    out = []
    for wave, tsv in found:
        codes = list(read_verdicts(tsv).values())
        none = sum(c == "245" for c in codes)
        green = sum(c == "0" for c in codes)
        row = old.get(wave, {"date": "", "sha": "", "scope": "full", "note": ""})
        out.append([wave, row["date"], row["sha"], str(len(codes)), str(green),
                    str(len(codes) - green - none), str(none), row["scope"], row["note"]])
    fresh = {r[0] for r in out}
    out += [[r[c] for c in WAVE_COLUMNS] for w, r in old.items() if w not in fresh]
    out.sort(key=lambda r: int(r[0]))
    with open(path, "w", encoding="utf-8") as f:
        for c in comments:
            f.write(c + "\n")
        f.write("\t".join(WAVE_COLUMNS) + "\n")
        for r in out:
            f.write("\t".join(r) + "\n")
    return len(out)


def stamp_checked_at(root: str) -> None:
    """Rewrite the table's `# checked-at` line to the short HEAD (a clean audit only)."""
    try:
        head = subprocess.run(["git", "-C", root, "rev-parse", "--short", "HEAD"], capture_output=True,
                              text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return
    path = os.path.join(root, STATUS_DIR, "progress.tsv")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = re.sub(r"^# checked-at \S+$", f"# checked-at {head}", text, count=1, flags=re.M)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"status-page: checked-at {head}")


def main(argv: list[str] | None = None, root: str = ROOT) -> int:
    p = argparse.ArgumentParser(description="Build the progress page.")
    p.add_argument("--check", action="store_true", help="fail if a generated file is stale")
    p.add_argument("--audit", action="store_true", help="compare rows with var/regress-*")
    p.add_argument("--waves", action="store_true", help="refresh waves.tsv from var/regress-wave*")
    p.add_argument("--var", default=os.path.join(root, "var"), help="where the wave results are (default: var/)")
    p.add_argument("--no-stamp", action="store_true", help="with --audit: report only, never rewrite a file")
    args = p.parse_args(argv)
    try:
        if args.waves:
            print(f"status-page: {update_waves(root, args.var)} waves in waves.tsv")
        files = build(root)
    except (Bad, OSError) as e:
        print(f"status-page: {e}", file=sys.stderr)
        return 1
    if args.audit:
        if not regress_dirs(args.var):
            print(f"status-page: audit skipped, no regress-*/regress.tsv under {args.var}")
            return 0
        rows, _ = read_table(os.path.join(root, STATUS_DIR, "progress.tsv"), COLUMNS)
        found = audit(rows, scenario_names(root), newest_verdicts(regress_dirs(args.var)), listed_rows(root))
        for line in found:
            print(line)
        print(f"status-page: {len(found)} to look at")
        if not found and not args.no_stamp:
            stamp_checked_at(root)
            for rel, content in build(root).items():
                with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
                    f.write(content)
        return 1 if found else 0
    if args.check:
        bad = stale(root, files)
        if bad:
            print("status-page: stale, run python3 -I tools/status-page.py: " + ", ".join(bad), file=sys.stderr)
            return 1
        print("status-page: current")
        return 0
    for rel, content in files.items():
        with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
            f.write(content)
    print(f"status-page: wrote {len(files)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
