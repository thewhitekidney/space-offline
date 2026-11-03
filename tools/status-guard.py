#!/usr/bin/env python3
"""Fail a change that moves a feature but leaves the progress table alone.

A change that touches `server/`, `content/` or `tools/client-ci/scenarios/`
must also change `docs/status/progress.tsv`, unless the head commit message or
the PR body carries a line

    Status: none (<why no feature state changed>)

Run::

    python3 -I tools/status-guard.py                     # base: origin/master
    python3 -I tools/status-guard.py --base REF --worktree
    PR_BODY="..." python3 -I tools/status-guard.py --base origin/master
    python3 -I tools/status-guard.py --body-file pr.md

Exit 0: fine. Exit 1: the progress table is owed. Exit 2: git failed.
Stdlib only; it reads git and never writes.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

PROGRESS = "docs/status/progress.tsv"
WATCHED = ("server/", "content/", "tools/client-ci/scenarios/")
WAIVER = re.compile(r"^Status: none \((.+)\)\s*$", re.M)


def decide(paths: list[str], texts: list[str]) -> tuple[bool, str]:
    """(ok, message) for these changed paths and these commit/PR texts."""
    moved = [p for p in paths if p.startswith(WATCHED)]
    if not moved:
        return True, "status-guard: no server, content or scenario path changed"
    if PROGRESS in paths:
        return True, f"status-guard: {PROGRESS} changed with the code"
    for text in texts:
        for m in WAIVER.finditer(text or ""):
            if m.group(1).strip():
                return True, f"status-guard: waived: Status: none ({m.group(1).strip()})"
    shown = ", ".join(moved[:5]) + (f" and {len(moved) - 5} more" if len(moved) > 5 else "")
    return False, (
        f"status-guard: {shown} changed, but {PROGRESS} did not.\n"
        f"  Update the feature's row (state, evidence, issue, theme), run "
        f"python3 -I tools/status-page.py, and commit both;\n"
        f"  or, if no feature state changed, put a line 'Status: none (<reason>)' "
        f"in the head commit message or the PR body.")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def changed_paths(base: str, worktree: bool) -> list[str]:
    try:
        out = git("diff", "--name-only", "--no-renames", f"{base}...HEAD")
    except subprocess.CalledProcessError:
        out = git("diff", "--name-only", "--no-renames", git("merge-base", base, "HEAD").strip(), "HEAD")
    paths = set(out.split())
    if worktree:
        paths.update(git("diff", "--name-only", "--no-renames", "HEAD").split())
        paths.update(git("ls-files", "--others", "--exclude-standard").split())
    return sorted(paths)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="Require a progress.tsv change with feature code.")
    p.add_argument("--base", default="origin/master", help="the ref the change is measured from")
    p.add_argument("--worktree", action="store_true", help="also count uncommitted and untracked files")
    p.add_argument("--body-file", help="a file holding the PR body")
    args = p.parse_args(argv)
    try:
        paths = changed_paths(args.base, args.worktree)
        texts = [git("log", "-1", "--format=%B")]
    except (OSError, subprocess.CalledProcessError) as e:
        err = getattr(e, "stderr", "") or str(e)
        print(f"status-guard: git failed: {err.strip()}", file=sys.stderr)
        return 2
    texts.append(os.environ.get("PR_BODY", ""))
    if args.body_file:
        with open(args.body_file, encoding="utf-8") as f:
            texts.append(f.read())
    ok, msg = decide(paths, texts)
    print(msg, file=sys.stdout if ok else sys.stderr)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
