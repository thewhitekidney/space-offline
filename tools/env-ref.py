#!/usr/bin/env python3
r"""Keep docs/reference/launch-and-env.md honest (stdlib only).

Scans the code for every `EVE_*` name and for the CLI flags of eve-server
(its USAGE text plus the flags modules add) and eve-bots, and compares them
with the names the reference doc lists in backticks.

    python3 -I tools/env-ref.py            # print what the scan finds
    python3 -I tools/env-ref.py --check    # exit 1 on any drift

`--check` fails when a name in the code is missing from the doc, or when a
table row of the doc names one the code no longer has.

What counts as "in the code":
  * env: every `EVE_[A-Z0-9_]+` token in a non-Markdown file under tools/,
    deploy/ or server/crates/ (server/target excluded). Paths the public
    export drops (private/export-private-paths.txt) are optional: a name
    only they read may have a row and need not. Shell variables, Python
    `os.environ`, Rust `env::var`, test knobs and comments alike: a comment
    that names a variable is how a reader finds it. A few tokens are Rust
    constants, not variables; the doc lists them in its own table.
  * flags: `--name` words in eve-server's USAGE string (server/crates/
    eve-server/src/cli.rs), `name: "--x"` module flags under
    server/crates/eve-mod-*/src and in eve-server's own modules
    (server/crates/eve-server/src, e.g. `air_npe`), and `"--x"` literals in
    server/crates/eve-bot/src/bin/eve-bots.rs. Test modules (after
    `#[cfg(test)]`) are ignored.

What counts as "in the doc": a backticked name that starts a table row
(`| \`EVE_X\` |` or `| \`--flag VALUE\` |`). Other backticked mentions
(prose, examples) are not checked against the code.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DOC = os.path.join("docs", "reference", "launch-and-env.md")
ENV_DIRS = ("tools", "deploy", os.path.join("server", "crates"))
ENV_RE = re.compile(r"\bEVE_[A-Z0-9_]*[A-Z0-9]\b")
FLAG_RE = re.compile(r"(?<![\w-])--[a-z][a-z0-9-]*")
SKIP_DIRS = {"target", ".git", "__pycache__", "node_modules"}
# Files that name EVE_* tokens without reading them: this checker and its test.
SKIP_FILES = {
    os.path.join("tools", "env-ref.py"),
    os.path.join("tools", "test_env_ref.py"),
}
# Paths the public export drops (one path or prefix a line); their names are
# not part of the public reference. Absent in a public checkout.
PRIVATE_LIST = os.path.join("private", "export-private-paths.txt")


def private_paths(root):
    """The export's private paths and prefixes, or an empty tuple."""
    try:
        text = _read(os.path.join(root, PRIVATE_LIST))
    except OSError:
        return ()
    out = []
    for line in text.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            out.append(line)
    return tuple(out)
SKIP_EXT = {".md", ".png", ".jpg", ".pyc", ".gz", ".zip", ".exe", ".dll"}

SERVER_CLI = os.path.join("server", "crates", "eve-server", "src", "cli.rs")
MODULE_GLOB_DIR = os.path.join("server", "crates")
BOTS_CLI = os.path.join("server", "crates", "eve-bot", "src", "bin", "eve-bots.rs")


def _read(path):
    with open(path, "rb") as f:
        return f.read().decode("utf-8", "replace")


def _strip_tests(text):
    """Rust source up to its test module."""
    cut = text.find("#[cfg(test)]")
    return text if cut < 0 else text[:cut]


def scan_env(root=ROOT, private_only=False):
    """{name: sorted [relpath, ...]} of every EVE_* token in the code.

    Files under the export's private paths are left out, or, with
    `private_only`, are the only ones scanned."""
    found = {}
    private = private_paths(root)
    for d in ENV_DIRS:
        base = os.path.join(root, d)
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(x for x in dirnames if x not in SKIP_DIRS)
            for fn in sorted(filenames):
                if os.path.splitext(fn)[1].lower() in SKIP_EXT:
                    continue
                path = os.path.join(dirpath, fn)
                rel = os.path.relpath(path, root)
                if rel in SKIP_FILES:
                    continue
                is_private = bool(private) and rel.replace(os.sep, "/").startswith(private)
                if is_private != private_only:
                    continue
                if os.path.islink(path) or not os.path.isfile(path):
                    continue
                try:
                    text = _read(path)
                except OSError:
                    continue
                for name in set(ENV_RE.findall(text)):
                    found.setdefault(name, set()).add(rel)
    return {k: sorted(v) for k, v in found.items()}


def _usage_block(text):
    m = re.search(r'const USAGE: &str = "(.*?)";', text, re.S)
    return m.group(1) if m else ""


def scan_flags(root=ROOT):
    """{'eve-server': set, 'eve-bots': set} of the flags in the code."""
    server = set()
    cli = os.path.join(root, SERVER_CLI)
    if os.path.isfile(cli):
        server |= set(FLAG_RE.findall(_usage_block(_strip_tests(_read(cli)))))
    crates = os.path.join(root, MODULE_GLOB_DIR)
    if os.path.isdir(crates):
        for crate in sorted(os.listdir(crates)):
            # eve-server's own modules (`crate::air_npe`) add flags the same way.
            if not (crate.startswith("eve-mod-") or crate == "eve-server"):
                continue
            src = os.path.join(crates, crate, "src")
            for dirpath, dirnames, filenames in os.walk(src):
                dirnames.sort()
                for fn in sorted(filenames):
                    if fn.endswith(".rs"):
                        text = _strip_tests(_read(os.path.join(dirpath, fn)))
                        server |= set(re.findall(r'name:\s*"(--[a-z0-9-]+)"', text))
    bots = set()
    b = os.path.join(root, BOTS_CLI)
    if os.path.isfile(b):
        bots |= set(re.findall(r'"(--[a-z][a-z0-9-]*)', _strip_tests(_read(b))))
    return {"eve-server": server, "eve-bots": bots}


ROW_RE = re.compile(r"^\|\s*`([^`]+)`")


def doc_names(doc_text):
    """(env names, flags) that start a table row of the doc."""
    env, flags = set(), set()
    for line in doc_text.splitlines():
        m = ROW_RE.match(line)
        if not m:
            continue
        cell = m.group(1).strip()
        first = cell.split()[0] if cell.split() else ""
        if ENV_RE.fullmatch(first):
            env.add(first)
        elif first.startswith("--"):
            # `--prefix P --count N`: every flag in the cell is listed.
            flags |= set(FLAG_RE.findall(cell))
    return env, flags


def rows_citing_absent(doc_text, root):
    """Env names whose row cites source files (backticked paths in its second cell)
    none of which exist under root: a clone of the public repo lacks the files the
    export drops (deploy/), so such a row can't be judged stale there."""
    out = set()
    for line in doc_text.splitlines():
        m = ROW_RE.match(line)
        if not m:
            continue
        cells = line.strip().strip("|").split("|")
        first = cells[0].strip().strip("`").split()
        if not first or not ENV_RE.fullmatch(first[0]) or len(cells) < 2:
            continue
        paths = [t for t in re.findall(r"`([^`]+)`", cells[1]) if "/" in t]
        if paths and not any(os.path.exists(os.path.join(root, t)) for t in paths):
            out.add(first[0])
    return out


def all_mentions(doc_text):
    """Every EVE_* token and flag the doc mentions in backticks."""
    ticks = re.findall(r"`([^`]+)`", doc_text)
    env, flags = set(), set()
    for t in ticks:
        env |= set(ENV_RE.findall(t))
        flags |= set(FLAG_RE.findall(t))
    return env, flags


def check(root=ROOT, doc_path=None):
    """List of problems; empty when the doc and the code agree."""
    doc_path = doc_path or os.path.join(root, DOC)
    if not os.path.isfile(doc_path):
        return ["%s: missing" % os.path.relpath(doc_path, root)]
    text = _read(doc_path)
    row_env, row_flags = doc_names(text)
    code_env = scan_env(root)
    # A name read only by a private file (the export drops it) may have a
    # row, and need not. Without the private list (a public clone), a row whose
    # cited source file is absent from the tree is the same case.
    private_env = set(scan_env(root, private_only=True)) | rows_citing_absent(text, root)
    code_flags = scan_flags(root)
    all_code_flags = code_flags["eve-server"] | code_flags["eve-bots"]
    problems = []
    for name in sorted(code_env):
        if name not in row_env:
            problems.append(
                "undocumented: %s (in %s)" % (name, ", ".join(code_env[name][:3]))
            )
    for tool in ("eve-server", "eve-bots"):
        for flag in sorted(code_flags[tool]):
            if flag not in row_flags:
                problems.append("undocumented: %s %s" % (tool, flag))
    for name in sorted(row_env - set(code_env) - set(private_env)):
        problems.append("stale: %s is in the doc but not in the code" % name)
    for flag in sorted(row_flags - all_code_flags):
        problems.append("stale: %s is in the doc but not in the code" % flag)
    return problems


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="exit 1 on any drift")
    ap.add_argument("--root", default=ROOT, help="repository root")
    args = ap.parse_args(argv)
    if args.check:
        problems = check(args.root)
        for p in problems:
            print("env-ref: " + p)
        if problems:
            print("env-ref: %d problem(s); fix %s" % (len(problems), DOC))
            return 1
        print("env-ref: ok")
        return 0
    for name, files in sorted(scan_env(args.root).items()):
        print("%s\t%s" % (name, ",".join(files)))
    for tool, flags in sorted(scan_flags(args.root).items()):
        for f in sorted(flags):
            print("%s\t%s" % (tool, f))
    return 0


if __name__ == "__main__":
    sys.exit(main())
