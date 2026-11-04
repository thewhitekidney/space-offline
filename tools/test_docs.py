"""The doc lint (review V5 P5 (a)-(g) merged with V4 G4; packet IP31).

The gate runs every `tools/test_*.py`, so this runs on every landing. It is
stdlib-only and reads the working tree (git's tracked plus untracked,
non-ignored files when `.git` exists; a walk that skips the ignored
directories otherwise, as in a lane's `git archive` copy).

  (a) every `docs/...md` path cited in `server/`, `tools/` and `docs/`
      exists. Docs pin paths (V5 §2.2), so a rename breaks them; this says
      where;
  (b) every doc under `docs/` has a row in the index, `docs/README.md`
      (flat lane notes, `docs/lanes/<name>.md`, are listed in the generated
      `docs/lanes/INDEX.md` instead and need no row);
  (c) every index row whose role is HISTORY has a `> **Demoted` banner in
      its doc's first 5 lines;
  (d) `docs/packets.tsv` is well formed and its ids are unique (lanes
      rule 12);
  (e) no file outside `docs/lanes/` holds a literal session scratchpad path
      (lanes rule 15);
  (f) every `existing:` test citation in the plans and lane notes names a
      `fn` or `mod` that exists (lanes rule 14; V4 G4, scoped as G4 asks to
      `docs/review/`, `docs/content/plan.md` and `docs/lanes/`);
  (h) every relative markdown link in `docs/`, `AGENTS.md`, `CLAUDE.md` and `README.md`
      resolves to a file or directory;
  (g) the human-check card holds at most 6 items and 60 minutes, and its
      total row is the sum.

Scoping, where today's tree needs it (each list fails when it goes stale,
so it can only shrink):

  - (a) skips placeholders (`docs/lanes/X.md`, `docs/...md`), the review
    lanes' frozen reports (`docs/review/V*.md`, which describe the tree they
    were written against, as old lane notes do), and the fixtures of the
    two plan tests. PLANNED docs are cited before they exist; an entry
    fails once its doc exists. KNOWN_BROKEN citations wait on a fix; an
    entry fails once the citation is gone.
  - (b) and (c) need the index, which IP23 writes; until it exists they are
    skipped, and they assert the moment it lands.
  - (e) also skips the review lanes' frozen reports, for the reason above;
    KNOWN_SESSION files wait on a fix, and an entry fails once its file is
    clean.

Run::

    python3 -I tools/test_docs.py
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# The literal prefix of a session scratchpad, built so this file holds none.
SESSION = "/tmp/" + "claude-"

SKIP_DIRS = {".git", "target", "out", "logs", "var", "tmp", ".claude", "__pycache__", "captures"}

RECORD = re.compile(r"^docs/review/V\d+-[\w-]+\.md$")

# (a): files whose test fixtures cite invented doc paths.
FIXTURE_FILES = {"tools/test_docs.py", "tools/test_plan_owners.py", "tools/test_public_export.py"}

# (a): docs a landed doc cites before their packet writes them.
PLANNED = {
    "docs/STATUS.md": "IP23, never written: the tracker's last sections and docs/README.md do its job",
}

# (a): (citing file, cited path) -> the fix. The coordinator diff from IP31
# repoints both; drop the entry with it.
# tools/eve-offline-jail.in is the source of the root-installed jail helper,
# and the passwordless launch checks the two are byte-identical; its comments
# change only together with a `sudo tools/install-jail-helper.sh` by the
# owner, so its doc paths predate DOCS-CLEAN's moves.
_JAIL = "tools/eve-offline-jail.in"
KNOWN_BROKEN: dict = {
    (_JAIL, "docs/" + "unattended-root.md"): "now docs/reference/unattended-root.md",
    (_JAIL, "docs/" + "census.md"): "now docs/guides/census.md",
}

# (e): files that still hold one, each waiting on the IP31 coordinator diff;
# an entry fails once its file is clean.
KNOWN_SESSION: dict = {}

# A flat lane note: docs/lanes/<name>.md, but not the lanes README or the generated index.
LANE_NOTE = re.compile(r"^docs/lanes/(?!README\.md$|INDEX\.md$)[^/]+\.md$")

LINK_EXT = (".md", ".tsv", ".txt", ".rs", ".py", ".sh", ".json", ".toml", "/")
MD_LINK = re.compile(r"\]\(([^)\s#]+)(?:#[^)\s]*)?(?:\s+\"[^\"]*\")?\)")

DOC_PATH = re.compile(r"docs/[A-Za-z0-9_./-]+\.md")

TSV_HEADER = ["# id", "series", "plan", "lane_note", "landed", "state"]
SERIES = {"IP", "CP", "R"}
STATES = {"landed", "open"}

CARD_ITEMS = 6
CARD_MINUTES = 60


# ---------------------------------------------------------------- private docs


def private_prefixes(root: str) -> list[str]:
    """The `docs/` paths the public export leaves out: `private/export-private-paths.txt`,
    else (in an exported tree, which has no private/) `tools/doc-private-prefixes.txt`."""
    for rel in ("private/export-private-paths.txt", "tools/doc-private-prefixes.txt"):
        p = os.path.join(root, rel)
        if os.path.exists(p):
            with open(p, encoding="utf-8") as f:
                lines = [x.strip() for x in f if x.strip() and not x.lstrip().startswith("#")]
            return [x for x in lines if x.startswith("docs/")]
    return []


def absent_private(root: str, rel: str, prefixes: list[str] | None = None) -> bool:
    """True when `rel` is a private doc that this tree does not hold (an exported tree).
    A dead link to a public doc, or to a private doc in the private tree, still fails."""
    prefixes = private_prefixes(root) if prefixes is None else prefixes
    rel = os.path.normpath(rel).replace(os.sep, "/")
    return not os.path.exists(os.path.join(root, rel)) and any(
        rel == p or rel.startswith(p.rstrip("/") + "/") for p in prefixes
    )


# ---------------------------------------------------------------- the tree


def tree_files(root: str) -> list[str]:
    """Relative paths of the working tree's files, ignored ones excluded."""
    if os.path.isdir(os.path.join(root, ".git")):
        r = subprocess.run(
            ["git", "-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            capture_output=True,
        )
        if r.returncode == 0:
            names = sorted({n.decode("utf-8", "replace") for n in r.stdout.split(b"\0") if n})
            return [n for n in names if os.path.isfile(os.path.join(root, n))]
    out = []
    for d, dirs, files in os.walk(root):
        dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS)
        for f in files:
            out.append(os.path.relpath(os.path.join(d, f), root).replace(os.sep, "/"))
    return sorted(out)


def read(root: str, rel: str) -> str:
    with open(os.path.join(root, rel), "rb") as f:
        return f.read().decode("utf-8", "replace")


def is_placeholder(path: str) -> bool:
    stem = os.path.basename(path)[: -len(".md")]
    return ".." in path or stem in ("", "X", "N") or stem.startswith(".")


# ---------------------------------------------------------------- checks


def check_cited_paths(root, files, planned=None, known=None) -> list[str]:
    """(a)"""
    planned = PLANNED if planned is None else planned
    known = KNOWN_BROKEN if known is None else known
    fail, seen_known = [], set()
    prefixes = private_prefixes(root)
    for rel in files:
        if not rel.startswith(("server/", "tools/", "docs/")):
            continue
        if rel in FIXTURE_FILES or RECORD.match(rel):
            continue
        text = read(root, rel)
        for m in DOC_PATH.finditer(text):
            p = m.group(0)
            if is_placeholder(p) or os.path.exists(os.path.join(root, p)):
                continue
            if absent_private(root, p, prefixes):
                continue  # a private doc, absent from the exported tree on purpose
            if p in planned:
                continue
            if (rel, p) in known:
                seen_known.add((rel, p))
                continue
            line = text.count("\n", 0, m.start()) + 1
            fail.append(f"{rel}:{line}: cites {p}, which does not exist")
    for p, who in sorted(planned.items()):
        if os.path.exists(os.path.join(root, p)):
            fail.append(f"PLANNED lists {p} ({who}), which now exists: drop the entry")
    for key in sorted(set(known) - seen_known):
        if not os.path.exists(os.path.join(root, key[0])):
            continue  # the citing file is itself absent (an exported tree)
        fail.append(f"KNOWN_BROKEN lists {key[0]} -> {key[1]}, which is fixed: drop the entry")
    return sorted(set(fail))


def index_rows(index: str) -> list[list[str]]:
    rows = []
    for line in index.split("\n"):
        if line.startswith("|") and not re.match(r"\|\s*-", line):
            rows.append([c.strip() for c in re.split(r"(?<!\\)\|", line.strip())[1:-1]])
    return rows


def row_path(cells: list[str]) -> str | None:
    for c in cells:
        m = re.search(r"`?((?:docs/)?[\w./-]+\.md)`?", c)
        if m:
            p = m.group(1)
            return p if p.startswith("docs/") else "docs/" + p
    return None


def check_index(root, files) -> list[str]:
    """(b) and (c); the caller skips them when there is no index."""
    rows = index_rows(read(root, "docs/README.md"))
    listed = {row_path(r) for r in rows} - {None}
    fail = []
    for rel in files:
        if LANE_NOTE.match(rel):
            continue  # a flat lane note is listed in docs/lanes/INDEX.md, which the coordinator regenerates
        if rel.startswith("docs/") and rel.endswith(".md") and rel not in listed:
            fail.append(f"{rel}: no row in docs/README.md")
    for r in rows:
        if any(c.strip("*` ").upper() == "HISTORY" for c in r):
            p = row_path(r)
            if p and absent_private(root, p):
                continue
            if not p or not os.path.exists(os.path.join(root, p)):
                fail.append(f"docs/README.md: HISTORY row names no existing doc: {r}")
                continue
            head = read(root, p).split("\n")[:5]
            if not any(h.startswith("> **Demoted") for h in head):
                fail.append(f"{p}: role HISTORY but no `> **Demoted` banner in its first 5 lines")
    return fail


def check_links(root, files) -> list[str]:
    """(h) every relative markdown link in a doc names a file or directory that exists.

    Scoped to docs/, AGENTS.md, CLAUDE.md and the root README; the review lanes' frozen
    reports are skipped, as in (a).
    """
    fail = []
    prefixes = private_prefixes(root)
    for rel in files:
        if not rel.endswith(".md") or RECORD.match(rel):
            continue
        if not (rel.startswith("docs/") or rel in ("AGENTS.md", "CLAUDE.md", "README.md")):
            continue
        base = os.path.dirname(rel)
        text = read(root, rel)
        for m in MD_LINK.finditer(text):
            t = m.group(1)
            if not re.fullmatch(r"[\w./-]+", t) or not t.endswith(LINK_EXT):
                continue  # not a path (code such as `x](row)` inside a snippet)
            p = os.path.normpath(os.path.join(root, base, t))
            if not os.path.exists(p):
                if absent_private(root, os.path.relpath(p, root), prefixes):
                    continue
                line = text.count("\n", 0, m.start()) + 1
                fail.append(f"{rel}:{line}: link to {t}, which does not exist")
    return fail


def check_packets(root) -> list[str]:
    """(d)"""
    path = os.path.join(root, "docs/packets.tsv")
    if not os.path.exists(path):
        if absent_private(root, "docs/packets.tsv"):
            return []
        return ["docs/packets.tsv is missing"]
    lines = read(root, "docs/packets.tsv").rstrip("\n").split("\n")
    fail = []
    if lines[0].split("\t") != TSV_HEADER:
        fail.append(f"docs/packets.tsv:1: header is not {TSV_HEADER}")
    seen: dict[str, int] = {}
    for n, line in enumerate(lines[1:], 2):
        f = line.split("\t")
        if len(f) != len(TSV_HEADER):
            fail.append(f"docs/packets.tsv:{n}: {len(f)} fields, want {len(TSV_HEADER)}")
            continue
        pid, series, plan, note, sha, state = f
        if pid in seen:
            fail.append(f"docs/packets.tsv:{n}: {pid} repeats line {seen[pid]}")
        seen.setdefault(pid, n)
        if series not in SERIES or not pid.startswith(series):
            fail.append(f"docs/packets.tsv:{n}: {pid} is not written with a registered series ({series})")
        if state not in STATES:
            fail.append(f"docs/packets.tsv:{n}: state {state!r} is not one of {sorted(STATES)}")
        if (state == "landed") != bool(re.fullmatch(r"[0-9a-f]{7,40}", sha)):
            fail.append(f"docs/packets.tsv:{n}: {pid} is {state} with landed {sha!r}")
        if note != "—" and not os.path.exists(os.path.join(root, note)):
            fail.append(f"docs/packets.tsv:{n}: lane note {note} does not exist")
        if not plan:
            fail.append(f"docs/packets.tsv:{n}: {pid} names no plan")
    return fail


def check_session_paths(root, files, known=None) -> list[str]:
    """(e)"""
    known = KNOWN_SESSION if known is None else known
    fail = []
    needle = SESSION.encode()
    for rel in files:
        if rel.startswith("docs/lanes/") or RECORD.match(rel):
            continue
        with open(os.path.join(root, rel), "rb") as f:
            data = f.read()
        i = data.find(needle)
        if rel in known:
            if i < 0:
                fail.append(f"KNOWN_SESSION lists {rel}, which is clean: drop the entry")
            continue
        if i >= 0:
            line = data.count(b"\n", 0, i) + 1
            fail.append(f"{rel}:{line}: a literal session path (write $SCRATCH/<lane>)")
    for rel in sorted(set(known) - set(files)):
        fail.append(f"KNOWN_SESSION lists {rel}, which is gone: drop the entry")
    return fail


CITE_END = re.compile(r"\||\bnew:|\bcheck:|\bhuman:|\n\s*\n|\n\s*[-*] |;|\.(?:\s|$)")
CITE_TOKEN = re.compile(r"(?:[\w./-]+::)*[A-Za-z_]\w*")


def defined_names(root, files) -> set[str]:
    names = set()
    for rel in files:
        if rel.startswith("server/crates/") and rel.endswith(".rs"):
            names.update(re.findall(r"\b(?:fn|mod)\s+(\w+)", read(root, rel)))
        elif rel.startswith("tools/") and rel.endswith(".py"):
            names.update(re.findall(r"\bdef\s+(\w+)", read(root, rel)))
    return names


def strip_parens(text: str) -> str:
    prev = None
    while prev != text:
        prev = text
        text = re.sub(r"\([^()]*\)", "", text)
    return text


def check_existing_citations(root, files) -> list[str]:
    """(f)"""
    names = defined_names(root, files)
    fail = []
    for rel in files:
        if not (rel.startswith(("docs/review/", "docs/lanes/")) or rel == "docs/content/plan.md"):
            continue
        if not rel.endswith(".md"):
            continue
        text = read(root, rel)
        for m in re.finditer(r"\bexisting:", text):
            rest = text[m.end():]
            e = CITE_END.search(rest)
            seg = strip_parens(rest[: e.start() if e else len(rest)])
            for tok in re.findall(r"`([^`]+)`", seg):
                if not CITE_TOKEN.fullmatch(tok) or ("::" not in tok and "/" in tok):
                    continue
                last = tok.split("::")[-1]
                if last not in names:
                    line = text.count("\n", 0, m.start()) + 1
                    fail.append(f"{rel}:{line}: existing: `{tok}` names no fn or mod in the tree")
    return fail


def check_card(text: str) -> list[str]:
    """(g)"""
    m = re.search(r"^## 1\. The card.*?$(.*?)^## ", text, re.S | re.M)
    if not m:
        return ["docs/guides/human-checks.md: no `## 1. The card` section"]
    items, total = [], None
    for line in m.group(1).split("\n"):
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().split("|")[1:-1]]
        if len(cells) < 4:
            continue
        if re.fullmatch(r"C\d+", cells[0]) and re.fullmatch(r"\d+", cells[-1]):
            items.append(int(cells[-1]))
        elif "total" in " ".join(cells).lower():
            t = re.search(r"\d+", cells[-1])
            total = int(t.group(0)) if t else None
    fail = []
    if not items:
        fail.append("docs/guides/human-checks.md: the card has no items")
    if len(items) > CARD_ITEMS:
        fail.append(f"docs/guides/human-checks.md: the card has {len(items)} items, over {CARD_ITEMS}")
    if sum(items) > CARD_MINUTES:
        fail.append(f"docs/guides/human-checks.md: the card's minutes sum to {sum(items)}, over {CARD_MINUTES}")
    if total is not None and total != sum(items):
        fail.append(f"docs/guides/human-checks.md: the card's total row says {total}, the items sum to {sum(items)}")
    return fail


# ---------------------------------------------------------------- tests


class Tree(unittest.TestCase):
    """Every check on today's tree."""

    @classmethod
    def setUpClass(cls):
        cls.files = tree_files(ROOT)

    def test_a_cited_doc_paths_exist(self):
        self.assertEqual(check_cited_paths(ROOT, self.files), [])

    def test_b_c_index_and_history_banners(self):
        if not os.path.exists(os.path.join(ROOT, "docs/README.md")):
            self.skipTest("no docs/README.md yet (IP23)")
        self.assertEqual(check_index(ROOT, self.files), [])

    def test_h_relative_links_resolve(self):
        self.assertEqual(check_links(ROOT, self.files), [])

    def test_d_packets_tsv(self):
        self.assertEqual(check_packets(ROOT), [])

    def test_e_no_session_paths(self):
        self.assertEqual(check_session_paths(ROOT, self.files), [])

    def test_f_existing_citations_resolve(self):
        self.assertEqual(check_existing_citations(ROOT, self.files), [])

    def test_g_the_card_fits_one_sitting(self):
        if absent_private(ROOT, "docs/guides/human-checks.md"):
            self.skipTest("docs/guides/human-checks.md is private and absent from this tree")
        self.assertEqual(check_card(read(ROOT, "docs/guides/human-checks.md")), [])


D = "docs" + "/"  # fixture paths are built, so (a) never sees them literally


class Negatives(unittest.TestCase):
    """One synthetic tree per assertion; each must fail as described."""

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_docs-")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, text):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)

    def files(self):
        return tree_files(self.root)

    def test_a_a_missing_cited_doc_fails_and_a_present_one_passes(self):
        self.put(D + "here.md", "x")
        self.put("tools/x.py", f"# see {D}here.md and {D}gone.md and {D}lanes/X.md\n")
        fail = check_cited_paths(self.root, self.files(), planned={}, known={})
        self.assertEqual(fail, [f"tools/x.py:1: cites {D}gone.md, which does not exist"])

    def test_a_stale_planned_and_known_entries_fail(self):
        self.put(D + "new.md", "x")
        self.put("tools/x.py", "# nothing\n")
        fail = check_cited_paths(self.root, self.files(), planned={D + "new.md": "P1"},
                                 known={("tools/x.py", D + "gone.md"): "fix"})
        self.assertEqual(len(fail), 2, fail)

    def test_b_an_unindexed_doc_fails(self):
        self.put(D + "README.md", f"| path | role |\n|---|---|\n| `{D}README.md` | INDEX |\n")
        self.put(D + "orphan.md", "x")
        self.assertEqual(check_index(self.root, self.files()), [f"{D}orphan.md: no row in docs/README.md"])

    def test_c_a_history_doc_without_a_banner_fails(self):
        self.put(D + "README.md", f"| path | role |\n|---|---|\n| `{D}README.md` | INDEX |\n"
                 f"| `{D}old.md` | HISTORY |\n| `{D}older.md` | HISTORY |\n")
        self.put(D + "old.md", "# Old\n\nno banner\n")
        self.put(D + "older.md", "# Older\n\n> **Demoted** 2026-10-25\n")
        fail = check_index(self.root, self.files())
        self.assertEqual(len(fail), 1, fail)
        self.assertIn(D + "old.md", fail[0])

    def test_absent_private_docs_are_skipped_but_public_ones_are_not(self):
        self.put("tools/doc-private-prefixes.txt", "# written by the export\n" + D + "secret/\n" + D + "hidden.md\n")
        self.put(D + "README.md", f"| path | role |\n|---|---|\n| `{D}README.md` | INDEX |\n| `{D}hidden.md` | HISTORY |\n")
        self.put(D + "a.md", f"see {D}secret/x.md and {D}hidden.md and {D}public-gone.md and [l]({'../' + D}hidden.md) and [m](gone.md)\n")
        self.put("tools/x.py", f"# {D}secret/y.md\n")
        self.assertEqual(check_cited_paths(self.root, self.files(), planned={}, known={}),
                         [f"{D}a.md:1: cites {D}public-gone.md, which does not exist"])
        self.assertEqual(check_index(self.root, self.files()), [f"{D}a.md: no row in docs/README.md"])
        self.assertEqual(check_links(self.root, self.files()), [f"{D}a.md:1: link to gone.md, which does not exist"])
        self.assertEqual(check_packets(self.root), ["docs/packets.tsv is missing"])
        self.put("tools/doc-private-prefixes.txt", D + "packets.tsv\n")
        self.assertEqual(check_packets(self.root), [])

    def test_d_a_duplicate_id_fails(self):
        hdr = "\t".join(TSV_HEADER) + "\n"
        self.put(D + "packets.tsv", hdr + "IP1\tIP\tp\t—\t—\topen\nIP1\tIP\tp\t—\tabc1234\tlanded\n")
        self.assertEqual(check_packets(self.root), [f"docs/packets.tsv:3: IP1 repeats line 2"])

    def test_d_a_bare_id_and_a_landed_row_without_a_sha_fail(self):
        hdr = "\t".join(TSV_HEADER) + "\n"
        self.put(D + "packets.tsv", hdr + "L1\tIP\tp\t—\t—\topen\nCP2\tCP\tp\t—\t—\tlanded\n")
        self.assertEqual(len(check_packets(self.root)), 2)

    def test_e_a_session_path_outside_lanes_fails(self):
        self.put("tools/brief.txt", f"write to {SESSION}1000/x\n")
        self.put(D + "lanes/old.md", f"history: {SESSION}1000/x\n")
        self.put(D + "review/V9-old.md", f"record: {SESSION}1000/x\n")
        self.put("tools/clean.txt", "write to $SCRATCH/<lane>\n")
        self.assertEqual(sorted(check_session_paths(self.root, self.files(), known={"tools/clean.txt": "fixed"})),
                         ["KNOWN_SESSION lists tools/clean.txt, which is clean: drop the entry",
                          "tools/brief.txt:1: a literal session path (write $SCRATCH/<lane>)"])

    def test_f_an_existing_citation_of_a_missing_fn_fails(self):
        self.put("server/crates/k/src/lib.rs", "mod tests { fn kept() {} }\n")
        self.put(D + "lanes/a.md", "- **Tests:** existing: `k::tests::kept`, `k::tests::gone` (and `whatever`).\n"
                 "  new: `also_gone`.\n")
        self.assertEqual(check_existing_citations(self.root, self.files()),
                         [f"{D}lanes/a.md:1: existing: `k::tests::gone` names no fn or mod in the tree"])

    def test_g_an_overfull_card_fails(self):
        rows = "".join(f"| C{n} | #{n} | x | 11 |\n" for n in range(1, 7))
        text = f"## 1. The card\n\n| C | ids | what | min |\n|---|---|---|---|\n{rows}| | | **total** | **60** |\n\n## 2. B\n"
        fail = check_card(text)
        self.assertEqual(len(fail), 2, fail)  # 66 minutes, and the total row disagrees

    def test_g_a_seventh_item_fails(self):
        rows = "".join(f"| C{n} | #{n} | x | 5 |\n" for n in range(1, 8))
        text = f"## 1. The card\n\n| C | ids | what | min |\n|---|---|---|---|\n{rows}\n## 2. B\n"
        self.assertEqual(check_card(text), ["docs/guides/human-checks.md: the card has 7 items, over 6"])



# --- Plan sync and entry docs (roadmap-sync §2.5, lint L1-L10) ----------------------------
# One owner per question: feature state lives in progress.tsv, order and claims in the
# tracker, the themes in master-plan §3. These checks keep the plan free of state and the
# entry docs free of counts, private paths and unknown labels.

PROGRESS = "docs/status/progress.tsv"
PLAN = "docs/plans/master-plan.md"
CONTRIBUTING = "docs/guides/contributing.md"
FINISHED = {"client-proved", "done"}
STATE_WORDS = re.compile(
    r"\b(client-proved|built-unproved|landed|running|handed back|not started|is green|"
    r"are in|is in|done|not yet run|wave \d+|W\d+)\b", re.I)
COUNTS = re.compile(r"\b\d+ of \d+\b|\b\d+%|\b\d[\d,]* tests\b|\b3\d{6}\b")
LABEL_TOKEN = re.compile(r"`((?:status|prio|area|kind)/[\w-]+|needs-[\w-]+)`")
ENTRY_DOCS = ["AGENTS.md", "docs/README.md", PLAN, CONTRIBUTING]
PRIVATE_OK_IN = {"docs/README.md"}
# L9 runs on these: the master plan is itself private, so it is not one of them.
L9_DOCS = ["AGENTS.md", "CLAUDE.md", "docs/README.md", CONTRIBUTING]
# The owner's private set (2026-11-03), on top of private/export-private-paths.txt.
OWNER_PRIVATE = [
    "private/", "docs/lanes/", "docs/port-tracker.md", "docs/plans/",
    "docs/review/state-of-the-emulator.md", "docs/review/plan.md", "docs/packets.tsv",
    "docs/guides/human-checks.md", "docs/guides/history-purge.md",
]
# Exported file -> why it may still cite a private path. May only shrink.
KNOWN_PRIVATE_CITES: dict[str, str] = {}
# Docs whose `state` column is not feature state, or that predate the rule. May only shrink.
KNOWN_STATE_TABLES = {
    "docs/guides/human-checks.md": "the human-check results: what a person saw, not feature state",
    "docs/content/README.md": "the closed content plan's area table, kept for its owners",
    "docs/content/C5-economy.md": "the closed content plan's area table, kept for its owners",
    "docs/plans/plan-public-release.md": "deviation tracking for the parked release plan (#47)",
}


def lines_of(root: str, rel: str) -> list[str]:
    path = os.path.join(root, rel)
    if not os.path.exists(path):
        return []
    return [ln.strip() for ln in read(root, rel).split("\n") if ln.strip() and not ln.lstrip().startswith("#")]


def private_paths(root: str) -> list[str]:
    """The private list, or in an exported tree its public copy of the docs/ lines."""
    return OWNER_PRIVATE + (lines_of(root, "private/export-private-paths.txt")
                            or lines_of(root, "tools/doc-private-prefixes.txt"))


def cites_private(line: str, private: list[str]) -> bool:
    return any(re.search(r"(?<![\w./-])" + re.escape(p), line) for p in private)


def section(text: str, heading: str) -> str:
    """From the line starting with `heading` to the next heading of the same level."""
    level = len(heading) - len(heading.lstrip("#"))
    out, inside = [], False
    for line in text.split("\n"):
        if line.startswith(heading):
            inside = True
        elif inside and re.match(r"#{1,%d} " % level, line):
            break
        if inside:
            out.append(line)
    return "\n".join(out)


def progress_rows(root):
    rows = []
    for n, line in enumerate(read(root, PROGRESS).split("\n"), 1):
        if not line or line.startswith("#") or line.startswith("system\t"):
            continue
        c = (line.split("\t") + [""] * 7)[:7]
        rows.append((n, dict(zip(["system", "feature", "state", "evidence", "issue", "theme", "note"], c))))
    return rows


def plan_sections(text):
    """(themes {3.N: name}, [(line no, line)] of plan §3 without its headings)."""
    themes, body, in3 = {}, [], False
    for n, line in enumerate(text.split("\n"), 1):
        if line.startswith("## "):
            in3 = line.startswith("## 3.")
            continue
        if in3:
            m = re.match(r"### (3\.\d+) (.+)", line)
            if m:
                themes[m.group(1)] = m.group(2)
                continue
            body.append((n, line))
    return themes, body


def check_plan_sync(root):
    fail = []
    rows = progress_rows(root)
    plan = read(root, PLAN)
    themes, body = plan_sections(plan)
    open_rows = [(n, r) for n, r in rows if r["state"] not in FINISHED]
    for n, r in open_rows:                                                    # L1
        if r["theme"] not in themes:
            fail.append(f"{PROGRESS}:{n}: open row {r['feature']!r} has theme {r['theme']!r}, not a 3.N heading")
    by_issue = {}
    for _, r in rows:
        for i in re.findall(r"#\d+", r["issue"]):
            by_issue.setdefault(i, []).append(r["state"])
    for n, line in body:                                                      # L2
        for i in re.findall(r"#\d+", line):
            st = by_issue.get(i)
            if st and all(s in FINISHED for s in st):
                fail.append(f"{PLAN}:{n}: names {i}, whose rows are all finished: drop it")
    for n, line in body:                                                      # L3 (a backticked state name is a reference, not a claim)
        m = STATE_WORDS.search(re.sub(r"`[^`]*`", "", line))
        if m:
            fail.append(f"{PLAN}:{n}: writes a state ({m.group(0)!r}); state lives in {PROGRESS}")
    for n, line in enumerate(plan.split("\n"), 1):
        if line.startswith("**Waiting on the owner"):
            fail.append(f"{PLAN}:{n}: owner calls are tracker issues labelled needs-owner")
    for key, title in themes.items():                                         # L4
        if not any(r["theme"] == key for _, r in open_rows):
            fail.append(f"{PLAN}: theme {key} {title} has no open row in {PROGRESS}")
    proved_only = {}
    for n, r in rows:                                                         # L5
        scen = re.findall(r"\bfci_\w+", r["evidence"])
        if r["state"] in FINISHED and len(scen) == 1:
            proved_only[scen[0]] = n
    for n, line in body:
        for s in re.findall(r"\bfci_\w+", line):
            if s in proved_only:
                fail.append(f"{PLAN}:{n}: names {s}, proved by {PROGRESS}:{proved_only[s]}")
    for n, r in rows:                                                         # L10
        if re.search(r"master plan", r["evidence"], re.I):
            fail.append(f"{PROGRESS}:{n}: evidence cites the plan; cite code, a commit or a scenario")
    return fail


def check_entry_docs(root, files):
    fail = []
    labels = set(re.findall(r"^\| `([^`]+)`(?:, `([^`]+)`)*", section(read(root, CONTRIBUTING), "## 8."), re.M))
    labels = {x for pair in labels for x in (pair if isinstance(pair, tuple) else (pair,)) if x}
    labels |= set(re.findall(r"`([^`]+)`", "\n".join(
        ln for ln in section(read(root, CONTRIBUTING), "## 8.").split("\n") if ln.startswith("| `"))))
    private = private_paths(root)
    for rel in dict.fromkeys(ENTRY_DOCS + L9_DOCS):
        if not os.path.exists(os.path.join(root, rel)):
            continue
        for n, line in enumerate(read(root, rel).split("\n"), 1):
            if rel in ENTRY_DOCS and COUNTS.search(line):                     # L7
                fail.append(f"{rel}:{n}: a count or build number; link the owner instead")
            if rel in L9_DOCS and ("tracker §" in line or (cites_private(line, private)  # L9
                                       and not (rel in PRIVATE_OK_IN and "private" in line.lower()))):
                fail.append(f"{rel}:{n}: cites a path the public export drops")
    for rel in files:                                                         # L6, L8
        if not rel.endswith(".md") or rel.startswith(("docs/archive/", "docs/lanes/", "docs/status/", "private/")) \
                or RECORD.match(rel) or cites_private(rel, private):
            continue
        text = read(root, rel)
        if rel.startswith("docs/") and rel not in KNOWN_STATE_TABLES and re.search(r"^\|(?:[^|\n]*\|)*\s*state\s*\|", text, re.M | re.I):
            fail.append(f"{rel}: a state table; feature state lives in {PROGRESS}")
        for tok in LABEL_TOKEN.findall(text):
            if tok not in labels:
                fail.append(f"{rel}: label `{tok}` is not in {CONTRIBUTING} §8")
    for rel in KNOWN_STATE_TABLES:                                            # shrink-only
        if os.path.exists(os.path.join(root, rel)) and not re.search(
                r"^\|(?:[^|\n]*\|)*\s*state\s*\|", read(root, rel), re.M | re.I):
            fail.append(f"KNOWN_STATE_TABLES lists {rel}, which has no state table: drop the entry")
    for rel in KNOWN_PRIVATE_CITES:                                           # shrink-only
        if not cites_private(read(root, rel), private):
            fail.append(f"KNOWN_PRIVATE_CITES lists {rel}, which is clean: drop the entry")
    return fail


class PlanSync(unittest.TestCase):
    def test_i_plan_and_progress_agree(self):
        if not os.path.exists(os.path.join(ROOT, PLAN)):
            self.skipTest(f"{PLAN} is private and absent from this tree")
        self.assertEqual(check_plan_sync(ROOT), [])

    def test_j_entry_docs_link_owners(self):
        self.assertEqual(check_entry_docs(ROOT, tree_files(ROOT)), [])


class PlanSyncNegatives(unittest.TestCase):
    """An invented tree: Feature A and Feature B, issues #1 and #2, theme 3.1 Alpha."""

    HEAD = "system\tfeature\tstate\tevidence\tissue\ttheme\tnote\n"
    PLAN_OK = "# Plan\n\n## 3. Themes\n\n### 3.1 Alpha\n\nWork on B (#2).\n"
    LABELS = "# C\n\n## 8. Labels\n\n| label | meaning |\n|---|---|\n| `status/ready` | open |\n\n## 9. Next\n"

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_docs-sync-")
        self.put(PROGRESS, self.HEAD + "S\tFeature A\tclient-proved\tfci_x 11-03\t#1\t\t\n"
                 "S\tFeature B\tbuilt-unproved\tcommit abc1234\t#2\t3.1\t\n")
        self.put(PLAN, self.PLAN_OK)
        self.put(CONTRIBUTING, self.LABELS)
        self.put("AGENTS.md", "# Start\n\nRun `status/ready` tasks.\n")
        self.put("private/export-private-paths.txt", "# private\n" + D + "lanes/\n")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def sync(self):
        return check_plan_sync(self.root)

    def entry(self):
        return check_entry_docs(self.root, tree_files(self.root))

    def test_the_invented_tree_is_clean(self):
        self.assertEqual(self.sync(), [])
        self.assertEqual(self.entry(), [])

    def test_l1_an_open_row_without_a_theme_fails(self):
        self.put(PROGRESS, self.HEAD + "S\tFeature B\tmissing\tnothing\t#2\t\t\n")
        self.assertTrue(any("not a 3.N heading" in f for f in self.sync()))

    def test_l2_naming_a_finished_issue_fails(self):
        self.put(PLAN, self.PLAN_OK + "Also #1.\n")
        self.assertTrue(any("names #1" in f for f in self.sync()))

    def test_l3_a_state_word_fails(self):
        self.put(PLAN, self.PLAN_OK + "B has landed.\n")
        self.assertTrue(any("writes a state" in f for f in self.sync()))

    def test_l4_a_theme_with_no_rows_fails(self):
        self.put(PLAN, self.PLAN_OK + "\n### 3.2 Beta\n\nLater.\n")
        self.assertTrue(any("theme 3.2 Beta has no open row" in f for f in self.sync()))

    def test_l5_naming_a_proving_scenario_fails(self):
        self.put(PLAN, self.PLAN_OK + "Rerun fci_x.\n")
        self.assertTrue(any("names fci_x" in f for f in self.sync()))

    def test_l6_a_state_table_fails(self):
        self.put(D + "x.md", "| state | what |\n|---|---|\n")
        self.assertTrue(any("a state table" in f for f in self.entry()))

    def test_l7_a_count_fails(self):
        self.put("AGENTS.md", "# Start\n\n12 of 13 pass.\n")
        self.assertTrue(any("a count" in f for f in self.entry()))

    def test_l8_an_unknown_label_fails(self):
        self.put("AGENTS.md", "# Start\n\nSet `status/running`.\n")
        self.assertTrue(any("status/running" in f for f in self.entry()))

    def test_l9_a_private_path_fails(self):
        self.put("AGENTS.md", "# Start\n\nRead `" + D + "lanes/README.md`.\n")
        self.assertTrue(any("drops" in f for f in self.entry()))

    def test_l9_tracker_section_fails(self):
        self.put("AGENTS.md", "# Start\n\nSee tracker §9.\n")
        self.assertTrue(any("drops" in f for f in self.entry()))

    def test_l10_evidence_citing_the_plan_fails(self):
        self.put(PROGRESS, self.HEAD + "S\tFeature B\tmissing\tmaster plan §3\t#2\t3.1\t\n")
        self.assertTrue(any("cites the plan" in f for f in self.sync()))


if __name__ == "__main__":
    unittest.main()
