"""The friend kit's allowlist (IP42; ruling 7; RR1 H5).

tools/friend-kit.sh ships `git archive HEAD` of an allowlist to friends, who
never get the repo. This checks, with stdlib only:

  - no allowlisted path is client-derived or private: nothing matches
    `tables.rs`, `out/`, `captures/`, `findings/`, `server/`, `var/`, a
    config dir, a key, a login file or a client file;
  - every allowlisted path exists;
  - the kit is closed: every tools/ file an allowlisted script sources or
    runs (`source`, `$REPO/tools/...`, `python3 -I tools/...`, a
    `tools/...` assignment, a local Python import) is in the kit, or is in
    OWNER_ONLY with the reason a friend never reaches it. A kit whose
    jail-rootless.sh sources a lib the kit lacks is the bug this prevents;
  - every tools/ command in the code blocks of the doc's "A friend" section
    is in the kit;
  - the script's own --vet refuses what this test forbids;
  - with a git checkout, a built kit holds exactly the allowlist.

Run::

    python3 -I tools/test_friend_kit.py
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


def pinned_build():
    """The `pinned` row of tools/builds.tsv (BA-4: no build literal here)."""
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "builds.tsv")
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if not line.startswith("#") and len(f) == 5 and f[2] == "pinned":
                return f[0]
    raise AssertionError("tools/builds.tsv has no pinned build")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KIT = os.path.join(ROOT, "tools", "friend-kit.sh")
DOC = os.path.join(ROOT, "docs", "guides", "playing-with-friends.md")

FORBIDDEN_PREFIXES = ("out/", "captures/", "findings/", "server/", "var/", ".git/")
FORBIDDEN_PARTS = ("tables.rs", ".config", "secrets", "authorized_keys", "known_hosts",
                   "id_ed25519", "id_rsa", ".pem", "bots.tsv", "ResFiles", "code.ccp",
                   "tq-offline")
FORBIDDEN_SUFFIXES = (".exe", ".dll", ".stuff", "/login", ".env", ".key")

# Referenced by a kit script on a path a friend never takes. Each entry
# fails once no kit script references it any more.
OWNER_ONLY = {
    "tools/dev.sh": "account.sh list/grant/bots build the server; a friend runs only `login`",
    "tools/bot-accounts.sh": "account.sh bots: the owner's bots",
    # The phone-home census (docs/guides/census.md) is the owner's verification
    # run; a friend never sets EVE_CENSUS, and the shim refuses census mode
    # unless census0 exists, so these paths are never reached from the kit.
    "tools/census": "census mode only (EVE_CENSUS): the owner's verification run",
    "tools/census_canary.py": "census mode only (EVE_CENSUS): the owner's verification run",
    "tools/census_sink.py": "census mode only (EVE_CENSUS): the owner's verification run",
    "tools/census_wire.py": "census mode only (EVE_CENSUS): the owner's verification run",
    "tools/census_proxy.py": "census mode only (EVE_CENSUS): the owner's verification run",
}

REF_PATTERNS = (
    re.compile(r"(?:\$REPO|\$\{REPO\}|\$HERE|\$\{HERE\})/(tools/[A-Za-z0-9_.-]+)"),
    re.compile(r"(?:^|[\s;&|(])(?:source|\.)\s+\"?(tools/[A-Za-z0-9_.-]+)"),
    re.compile(r"python3\s+(?:-\w+\s+)*\"?(tools/[A-Za-z0-9_.-]+)"),
    re.compile(r"^\s*(?:local\s+)?[A-Za-z_][A-Za-z0-9_]*=\"?(tools/[A-Za-z0-9_.-]+)"),
)
PY_IMPORT = re.compile(r"^\s*(?:from\s+([A-Za-z_][A-Za-z0-9_]*)|import\s+([A-Za-z_][A-Za-z0-9_]*))")


def kit_paths():
    out = subprocess.run(["bash", KIT, "--list"], capture_output=True, text=True, check=True)
    return [p for p in out.stdout.splitlines() if p]


def covered(path, allow):
    for p in allow:
        if p.endswith("/"):
            if path == p.rstrip("/") or path.startswith(p):
                return True
        elif path == p:
            return True
    return False


def forbidden(path):
    return (path.startswith(FORBIDDEN_PREFIXES)
            or any(part in path for part in FORBIDDEN_PARTS)
            or path.endswith(FORBIDDEN_SUFFIXES)
            or path == "login")


def kit_files(allow):
    """Every file the allowlist ships, as repo-relative paths."""
    files = []
    for p in allow:
        full = os.path.join(ROOT, p)
        if p.endswith("/"):
            for d, _, names in os.walk(full):
                files += [os.path.relpath(os.path.join(d, n), ROOT) for n in names
                          if "__pycache__" not in d]
        else:
            files.append(p)
    return files


def references(rel):
    """The tools/ files a kit file sources, runs or imports."""
    with open(os.path.join(ROOT, rel), encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    refs = set()
    for line in lines:
        if line.lstrip().startswith("#"):
            continue
        for pat in REF_PATTERNS:
            refs.update(m.group(1) for m in pat.finditer(line))
        if rel.endswith(".py"):
            m = PY_IMPORT.match(line)
            name = m and (m.group(1) or m.group(2))
            if name and os.path.exists(os.path.join(ROOT, "tools", name + ".py")):
                refs.add("tools/" + name + ".py")
    return refs


def friend_commands():
    with open(DOC, encoding="utf-8") as f:
        text = f.read()
    m = re.search(r"^## A friend\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    assert m, "docs/guides/playing-with-friends.md has no '## A friend' section"
    blocks = re.findall(r"```[a-z]*\n(.*?)```", m.group(1), re.S)
    return {c for b in blocks for c in re.findall(r"(?<![\w/])(tools/[A-Za-z0-9_.-]+)", b)}


class FriendKit(unittest.TestCase):
    def setUp(self):
        self.allow = kit_paths()

    def test_the_allowlist_is_not_empty_and_has_no_duplicates(self):
        self.assertGreater(len(self.allow), 0)
        self.assertEqual(len(self.allow), len(set(self.allow)))

    def test_no_allowlisted_path_is_forbidden(self):
        for p in self.allow:
            with self.subTest(path=p):
                self.assertFalse(forbidden(p), p)
        for p in kit_files(self.allow):
            with self.subTest(file=p):
                self.assertFalse(forbidden(p), p)

    def test_every_allowlisted_path_exists(self):
        for p in self.allow:
            with self.subTest(path=p):
                full = os.path.join(ROOT, p)
                if p.endswith("/"):
                    self.assertTrue(os.path.isdir(full), p)
                    self.assertTrue(kit_files([p]), p + " is empty")
                else:
                    self.assertTrue(os.path.isfile(full), p)

    def test_every_tools_file_a_kit_script_sources_or_runs_is_in_the_kit(self):
        seen = set()
        for f in kit_files(self.allow):
            if not f.startswith("tools/") or f.endswith((".json", ".sha256")):
                continue
            for ref in references(f):
                seen.add(ref)
                if covered(ref, self.allow):
                    continue
                with self.subTest(script=f, ref=ref):
                    self.assertIn(ref, OWNER_ONLY,
                                  f"{f} uses {ref}, which the kit lacks: add it to KIT_PATHS "
                                  "or to OWNER_ONLY with the reason")
        for ref in OWNER_ONLY:
            with self.subTest(owner_only=ref):
                self.assertIn(ref, seen, f"OWNER_ONLY {ref} is stale: no kit script uses it")

    def test_the_closure_scan_sees_the_libs(self):
        # Falsifier for the scan itself: these are the references the kit
        # exists to carry, so a regex that misses them fails here.
        self.assertIn("tools/login-lib.sh", references("tools/jail-rootless.sh"))
        self.assertIn("tools/prefix-lib.sh", references("tools/jail-rootless.sh"))
        self.assertIn("tools/jail-relay.py", references("tools/jail-rootless.sh"))
        self.assertIn("tools/eve_patch.py", references("tools/jail-client.sh"))
        self.assertIn("tools/client-pin.sha256", references("tools/pin-client.sh"))

    def test_every_friend_command_in_the_doc_is_in_the_kit(self):
        if not os.path.exists(DOC):
            self.skipTest("the friends guide is absent from this tree")
        cmds = friend_commands()
        self.assertIn("tools/jail-rootless.sh", cmds)
        for c in cmds:
            with self.subTest(command=c):
                self.assertTrue(covered(c, self.allow), c)

    def test_the_setup_script_is_in_the_kit_and_the_doc_names_it(self):
        self.assertIn("tools/friend-setup.sh", self.allow)
        if not os.path.exists(DOC):
            self.skipTest("the friends guide is absent from this tree")
        self.assertIn("tools/friend-setup.sh", friend_commands())

    def test_vet_refuses_what_this_test_forbids(self):
        def vet(paths):
            return subprocess.run(["bash", KIT, "--vet"], input="\n".join(paths) + "\n",
                                  capture_output=True, text=True).returncode
        self.assertEqual(vet(self.allow), 0)
        for bad in ("server/src/tables.rs", "out/friend-kit.tar", "captures/a.bin",
                    "findings/x.md", "var/world.dat", "var/tunnel/keys/authorized_keys",
                    ".config/eve-offline/login", "tools/x/tables.rs",
                    "drive_c/CCP/EVE/tq-offline/bin64/exefile.exe", "secrets/token"):
            with self.subTest(path=bad):
                self.assertTrue(forbidden(bad), bad)
                self.assertNotEqual(vet([bad]), 0, bad)

    @unittest.skipUnless(shutil.which("git") and subprocess.run(
        ["git", "-C", ROOT, "rev-parse", "--git-dir"], capture_output=True).returncode == 0
        and subprocess.run(["git", "-C", ROOT, "cat-file", "-e", "HEAD:tools/friend-kit.sh"],
                           capture_output=True).returncode == 0,
        "needs a git checkout whose HEAD has the kit script")
    def test_a_built_kit_holds_exactly_the_allowlist(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "kit.tar")
            r = subprocess.run(["bash", KIT, "--out", out], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertIn(pinned_build(), r.stdout)
            self.assertIn("jail-rootless.sh --relay client", r.stdout)
            names = subprocess.run(["tar", "-tf", out], capture_output=True, text=True,
                                   check=True).stdout.split()
            files = sorted(n[len("eve-offline-kit/"):] for n in names if not n.endswith("/"))
            self.assertTrue(all(n.startswith("eve-offline-kit/") for n in names))
            # The one generated entry: which commit and build this kit is.
            self.assertIn("KIT_VERSION", files)
            files.remove("KIT_VERSION")
            version = subprocess.run(["tar", "-xOf", out, "eve-offline-kit/KIT_VERSION"],
                                     capture_output=True, text=True, check=True).stdout
            head = subprocess.run(["git", "-C", ROOT, "rev-parse", "HEAD"],
                                  capture_output=True, text=True, check=True).stdout.strip()
            self.assertIn("commit " + head, version)
            self.assertIn("build  " + pinned_build(), version)
            want = subprocess.run(["git", "-C", ROOT, "ls-tree", "-r", "--name-only", "HEAD", "--"]
                                  + self.allow, capture_output=True, text=True,
                                  check=True).stdout.split()
            self.assertEqual(files, sorted(want))
            for f in files:
                self.assertFalse(forbidden(f), f)


class BuiltFromACommit(unittest.TestCase):
    """F-KIT-FIX 11: the kit is built from a throwaway repo holding the working
    tree's kit files, so this runs whatever HEAD of the real checkout holds."""

    def make_repo(self, d, builds=None):
        listing = subprocess.run(["bash", KIT, "--list"], capture_output=True, text=True,
                                 check=True).stdout.split()
        repo = os.path.join(d, "repo")
        files = []
        for p in listing:
            src = os.path.join(ROOT, p)
            if p.endswith("/"):
                for dp, _, fs in os.walk(src):
                    files += [os.path.relpath(os.path.join(dp, f), ROOT) for f in fs]
            else:
                files.append(p)
        files.append("tools/friend-kit.sh")
        for f in sorted(set(files)):
            dst = os.path.join(repo, f)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy2(os.path.join(ROOT, f), dst)
        # A pin manifest that names the owner's home, as the real one does.
        pin = os.path.join(repo, "tools", "client-pin.sha256")
        with open(pin, "w") as fh:
            fh.write("# eve-offline client pin\n# install: /home/alice/.local/share/x/tq-offline\n"
                     "# build:   100\n")
        if builds is not None:
            with open(os.path.join(repo, "tools", "builds.tsv"), "w") as fh:
                fh.write(builds)
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": d,
               "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
               "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
               "GIT_AUTHOR_DATE": "2026-01-02T03:04:05Z", "GIT_COMMITTER_DATE": "2026-01-02T03:04:05Z"}
        for cmd in (["init", "-q"], ["add", "-A"], ["commit", "-q", "-m", "kit"]):
            subprocess.run(["git", "-C", repo, *cmd], check=True, env=env, capture_output=True)
        return repo

    @unittest.skipUnless(shutil.which("git"), "needs git")
    def test_no_owner_path_no_owner_ids_and_the_same_bytes_twice(self):
        with tempfile.TemporaryDirectory() as d:
            repo = self.make_repo(d)
            outs = []
            for n in ("a.tar", "b.tar"):
                out = os.path.join(d, n)
                r = subprocess.run(["bash", os.path.join(repo, "tools", "friend-kit.sh"), "--out", out],
                                   capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stderr)
                outs.append(out)
            with open(outs[0], "rb") as a, open(outs[1], "rb") as b:
                self.assertEqual(a.read(), b.read())
            body = subprocess.run(["tar", "-xOf", outs[0], "eve-offline-kit/tools/client-pin.sha256"],
                                  capture_output=True, text=True, check=True).stdout
            self.assertNotIn("/home/", body)
            self.assertNotIn("install:", body)
            listing = subprocess.run(["tar", "-tvf", outs[0], "--numeric-owner"],
                                     capture_output=True, text=True, check=True).stdout
            self.assertTrue(listing.strip())
            for line in listing.splitlines():
                self.assertIn(" 0/0 ", line, line)
                self.assertIn("2026-01-02", line, line)

    @unittest.skipUnless(shutil.which("git"), "needs git")
    def test_an_empty_pinned_build_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            repo = self.make_repo(d, builds="# no pinned row\n100\tv\tunverified\t-\tx\n")
            r = subprocess.run(["bash", os.path.join(repo, "tools", "friend-kit.sh"),
                                "--out", os.path.join(d, "k.tar")], capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("no pinned build", r.stderr)
            self.assertFalse(os.path.exists(os.path.join(d, "k.tar")))


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    unittest.main()
