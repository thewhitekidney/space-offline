"""Policy check for an imported GitHub PR (tools/import-pr.sh, .github/workflows/label.yml).

Stdlib only. It READS git objects and text; it never checks out, builds or runs
anything from the PR.

  python3 -I tools/pr_guard.py diff REPO BASE HEAD   # full check; exit 3 on any refusal
  python3 -I tools/pr_guard.py names < names.txt     # path-only check of a name list; exit 3 if guarded

A PR is refused (label `needs-human`) when it touches a guarded path, a binary
file, a symlink or submodule, a file over 1 MB, or a Cargo.toml dependency
section. Refusals are printed one per line as `refuse: <path>: <reason>`.
"""

from __future__ import annotations

import fnmatch
import re
import subprocess
import sys

MAX_BYTES = 1024 * 1024
GUARDED_EXACT = {
    "CLAUDE.md", "AGENTS.md", "LICENSE", "LICENSE.md", "CONTRIBUTING-terms.md", "NOTICE", "CODEOWNERS",
    "docs/guides/public-export.md", "docs/guides/jail.md",
    "docs/lanes/README.md",
    "tools/land.sh", "tools/gate.sh", "tools/dev.sh",
    "tools/public-export.py", "tools/mirror-github.sh", "tools/import-pr.sh",
    "tools/pr_guard.py", "tools/test_mirror.py",
    "server/deps.allow", "server/Cargo.lock", "Cargo.lock",
    ".gitattributes", ".gitmodules",
}
GUARDED_PREFIX = (
    "deploy/", ".github/", ".for" + "gejo/", "private/", "containers/",
    ".cargo/", "server/.cargo/",
)
GUARDED_GLOB = ("*/build.rs", "build.rs", "private/export-*.txt")
DEP_SECTION = re.compile(r"^\s*\[(?:[^\]]*\.)?(?:dev-|build-)?dependencies(?:\.[^\]]*)?\]", re.I)


def guarded_reason(path: str) -> str | None:
    parts = path.split("/")
    if path.startswith("/") or ".." in parts or ".git" in parts:
        return "unsafe path"
    if path in GUARDED_EXACT:
        return "guarded file"
    if path.startswith(GUARDED_PREFIX):
        return "guarded directory"
    if any(fnmatch.fnmatchcase(path, g) for g in GUARDED_GLOB):
        return "guarded pattern (build script or export list)"
    if parts[-1] in ("rust-toolchain", "rust-toolchain.toml"):
        return "toolchain pin"
    return None


def dep_text(text: str) -> str:
    """Only the dependency sections of a Cargo.toml, for comparison."""
    keep, on = [], False
    for line in text.split("\n"):
        s = line.strip()
        if s.startswith("["):
            on = bool(DEP_SECTION.match(line))
        if on and s and not s.startswith("#"):
            keep.append(s)
    return "\n".join(keep)


def git(repo: str, *args: str) -> bytes:
    return subprocess.run(
        ["git", "-c", "core.quotepath=off", "-C", repo, *args],
        check=True, capture_output=True,
    ).stdout


def blob(repo: str, rev: str, path: str) -> str:
    try:
        return git(repo, "show", f"{rev}:{path}").decode("utf-8", "replace")
    except subprocess.CalledProcessError:
        return ""


def check_diff(repo: str, base: str, head: str) -> list[str]:
    out = []
    raw = git(repo, "diff", "--raw", "-z", "--no-renames", "--no-ext-diff", base, head)
    toks = raw.split(b"\0")
    entries = []
    i = 0
    while i + 1 < len(toks) and toks[i]:
        meta = toks[i].decode()
        entries.append((meta, toks[i + 1].decode("utf-8", "replace")))
        i += 2
    num = git(repo, "diff", "--numstat", "-z", "--no-renames", "--no-ext-diff", base, head)
    binary = set()
    for rec in num.split(b"\0"):
        if rec.startswith(b"-\t-\t"):
            binary.add(rec[4:].decode("utf-8", "replace"))
    for meta, path in entries:
        fields = meta.lstrip(":").split()
        old_mode, new_mode, new_sha, status = fields[0], fields[1], fields[3], fields[4]
        why = guarded_reason(path)
        if why:
            out.append(f"refuse: {path}: {why}")
        if "120000" in (old_mode, new_mode):
            out.append(f"refuse: {path}: symlink")
        if "160000" in (old_mode, new_mode):
            out.append(f"refuse: {path}: submodule")
        if path in binary:
            out.append(f"refuse: {path}: binary file")
        if not status.startswith("D") and new_mode != "160000":
            size = int(git(repo, "cat-file", "-s", new_sha).decode())
            if size > MAX_BYTES:
                out.append(f"refuse: {path}: {size} bytes, over 1 MB")
        if path.endswith("Cargo.toml") and not why:
            if dep_text(blob(repo, base, path)) != dep_text(blob(repo, head, path)):
                out.append(f"refuse: {path}: dependency section changed")
    return out


def main(argv: list[str]) -> int:
    if len(argv) == 4 and argv[0] == "diff":
        res = check_diff(argv[1], argv[2], argv[3])
    elif argv == ["names"]:
        res = []
        for line in sys.stdin.read().split("\n"):
            p = line.strip()
            if not p:
                continue
            why = guarded_reason(p) or ("Cargo.toml: dependencies are checked on import" if p.endswith("Cargo.toml") else None)
            if why:
                res.append(f"refuse: {p}: {why}")
    else:
        print(__doc__, file=sys.stderr)
        return 2
    for r in res:
        print(r)
    return 3 if res else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
