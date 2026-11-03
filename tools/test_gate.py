"""Tests for the landing tools: `tools/gate.sh`, `tools/land.sh` and
`tools/hooks/pre-push` (review V4 G1-G3, RR1 L2; packet IP16).

**Why this file exists.** Every guard-script bug so far was found in
production, on a failure path nobody had watched: `;` for `&&`
(`docs/port-tracker.md` §48.5), a pipeline's status hidden by `tail`
(§54.6), `grep -q` + SIGPIPE + `pipefail` failing a green gate (the comment
in `gate.sh`), and a red test run that printed nothing (§64, §65). Each test
here drives one of those paths on purpose.

**Nothing real is touched.** Each test copies the three scripts into a
fresh temp git repo with a stub `tools/dev.sh` (cargo answers from a file;
anything else runs on the host) and a stub `tools/check_deps.py`, and pushes
to a bare repo in the same temp dir. The real `tools/land.sh` never runs,
and no real remote is ever named. `GIT_*` from the caller is scrubbed and
the user's git config is ignored, so the gate's own pre-push hook can run
this file.

Run::

    python3 -I tools/test_gate.py
"""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

STUB_DEV = """#!/usr/bin/env bash
# Stub of tools/dev.sh for tools/test_gate.py: no container.
case "${1:-} ${2:-}" in
    "cargo test") cat "$STUB_TEST_OUT"; exit "${STUB_TEST_EXIT:-0}" ;;
    "cargo "*) exit 0 ;;
    *) exec "$@" ;;
esac
"""

STUB_GATE_NO_PASS_LINE = """#!/usr/bin/env bash
# A gate that exits 0 without its pass line (one that stopped running a step).
echo "test result: ok. 5 passed; 0 failed"
exit 0
"""

DOCS_TESTS = ("test_docs", "test_status_page", "test_plan_owners", "test_friend_kit", "test_fci_scenarios")

PASS = "test result: ok. 3 passed; 0 failed; 0 ignored\n"
FAIL_MARK = "thread 'invented::case' panicked: FAIL-MARKER-7f3a\n"


class LandingTools(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="eve-test-gate-")
        self.tmp = self._tmp.name
        self.repo = os.path.join(self.tmp, "repo")
        self.remote = os.path.join(self.tmp, "remote.git")
        self.test_out = os.path.join(self.tmp, "cargo-test.out")
        tmpdir = os.path.join(self.tmp, "t")
        os.mkdir(tmpdir)
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_NAME="gate test",
            GIT_AUTHOR_EMAIL="gate@test.invalid",
            GIT_COMMITTER_NAME="gate test",
            GIT_COMMITTER_EMAIL="gate@test.invalid",
            TMPDIR=tmpdir,
            STUB_TEST_OUT=self.test_out,
            STUB_TEST_EXIT="0",
        )
        self.env = env
        self.cargo_test(PASS)

        self.git("init", "-q", "--bare", "-b", "master", self.remote, cwd=self.tmp)
        os.makedirs(os.path.join(self.repo, "tools", "hooks"))
        os.makedirs(os.path.join(self.repo, "server", "crates", "invented"))
        for rel in ("gate.sh", "land.sh", os.path.join("hooks", "pre-push")):
            shutil.copy2(os.path.join(HERE, rel), os.path.join(self.repo, "tools", rel))
        self.write("tools/dev.sh", STUB_DEV, exe=True)
        self.write("tools/check_deps.py", "pass\n")
        self.write("server/crates/invented/Cargo.toml", "")
        self.write(".gitignore", "out/\n")
        self.git("init", "-q", "-b", "master")
        self.git("remote", "add", "origin", self.remote)
        self.commit("base")
        self.git("push", "-q", "origin", "master")
        self.base = self.rev("HEAD")
        # Installed from here on, as tools/install-hooks.sh does.
        self.git("config", "core.hooksPath", "tools/hooks")
        self.commit("change", ("change.txt", "x\n"))
        self.head = self.rev("HEAD")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    # --- helpers ---------------------------------------------------------------

    def write(self, rel: str, text: str, exe: bool = False) -> None:
        path = os.path.join(self.repo, rel)
        with open(path, "w") as f:
            f.write(text)
        if exe:
            os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    def cargo_test(self, text: str, exit_code: int = 0) -> None:
        with open(self.test_out, "w") as f:
            f.write(text)
        if hasattr(self, "env"):
            self.env["STUB_TEST_EXIT"] = str(exit_code)

    def git(self, *args: str, cwd: str | None = None) -> str:
        return subprocess.run(
            ["git", *args], cwd=cwd or self.repo, env=self.env,
            check=True, capture_output=True, text=True, timeout=60,
        ).stdout

    def rev(self, ref: str) -> str:
        return self.git("rev-parse", ref).strip()

    def remote_master(self) -> str:
        return self.git("rev-parse", "master", cwd=self.remote).strip()

    def commit(self, msg: str, *files: tuple[str, str]) -> None:
        for rel, text in files:
            self.write(rel, text)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)

    def run_tool(self, script: str, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [os.path.join(self.repo, "tools", script), *args], cwd=self.repo,
            env=self.env, capture_output=True, text=True, timeout=120,
        )

    # --- V4 G2's five cases ----------------------------------------------------

    def test_a_red_test_run_prints_its_tail_and_land_pushes_nothing(self) -> None:
        self.cargo_test("compiling\n" + FAIL_MARK + "test result: FAILED. 2 passed; 1 failed\n", 101)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.assertIn("land: GATE FAILED (exit 101) -- nothing pushed", r.stderr)
        self.assertIn("FAIL-MARKER-7f3a", r.stderr)
        self.assertNotIn("land: pushed", r.stdout)
        self.assertEqual(self.remote_master(), self.base)
        self.assertFalse(os.path.exists(os.path.join(self.repo, "out", "landed", self.head)))

    def test_zero_tests_passed_is_refused(self) -> None:
        self.cargo_test("test result: ok. 0 passed; 0 failed\n")
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        self.assertIn("gate: no tests actually ran", g.stderr)
        self.assertNotIn("== gate passed", g.stdout)
        r = self.run_tool("land.sh")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.remote_master(), self.base)

    def docs_commit(self, rel: str) -> None:
        self.git("reset", "-q", "--hard", self.base)
        for name in DOCS_TESTS:
            self.write(f"tools/{name}.py", "print('docs ok')\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "docs test")
        self.git("push", "-q", "--no-verify", "--force", "origin", "master")
        self.commit("docs", (rel, "x\n"))

    def test_a_docs_only_range_skips_the_build_and_still_pushes(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.docs_commit("notes.md")
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.remote_master(), self.rev("HEAD"))

    def test_the_docs_path_runs_all_five_docs_tests_and_a_failure_in_any_stops_it(self) -> None:
        self.docs_commit("notes.md")
        r = self.run_tool("gate.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        for name in DOCS_TESTS:
            self.assertIn(f"tools/{name}.py", r.stdout)
        self.write("tools/test_status_page.py", "raise SystemExit(1)\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "page drifted")
        r = self.run_tool("gate.sh")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("tools/test_status_page.py failed", r.stderr)
        self.assertNotIn("== gate passed", r.stdout)

    def test_every_gate_stage_prints_its_time(self) -> None:
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stderr)
        self.assertGreaterEqual(g.stdout.count("   took "), 8, g.stdout)
        self.assertIn("\n== gate passed\n", "\n" + g.stdout)

    def test_every_failed_python_test_is_named(self) -> None:
        self.commit("py tests", ("tools/test_a_ok.py", "print('fine')\n"),
                    ("tools/test_b_red.py", "print('red b')\nraise SystemExit(1)\n"),
                    ("tools/test_c_red.py", "import sys\nprint('red c')\nsys.exit(3)\n"))
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        # Both reds run to the end and both are named; the green one is not.
        self.assertIn("gate: tools/test_b_red.py failed", g.stderr)
        self.assertIn("gate: tools/test_c_red.py failed", g.stderr)
        self.assertNotIn("test_a_ok.py failed", g.stderr)
        self.assertIn("2 python test(s) failed", g.stderr)
        self.assertIn("red b", g.stdout)
        self.assertIn("red c", g.stdout)
        self.assertNotIn("== gate passed", g.stdout)
        self.assertNotIn("== test", g.stdout)

    def test_markdown_under_tools_or_any_other_file_runs_the_full_gate(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        for rel in ("tools/notes.md", "notes.txt"):
            self.docs_commit(rel)
            self.assertEqual(self.run_tool("gate.sh").returncode, 101, rel)

    def test_a_pass_with_output_over_the_pipe_buffer_is_still_a_pass(self) -> None:
        # The first pass line comes early and 64 KiB+ follows it, as with a
        # workspace of crates: a `grep -q` reader would exit there and SIGPIPE
        # the writer.
        filler = "test invented::case_%06d ... ok\n"
        body = "".join(filler % i for i in range(4000))
        self.assertGreater(len(body), 64 * 1024)
        self.cargo_test(PASS + body + "test result: ok. 4 passed; 0 failed\n")
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("land: gate passed, 7 tests\n", r.stdout)
        self.assertTrue(r.stdout.endswith("land: pushed\n"), r.stdout)
        self.assertEqual(self.remote_master(), self.head)

    def test_scoped_gates_never_print_the_full_pass_line(self) -> None:
        for args in (("--dirty",), ("--only", "invented"), ("--without", "invented")):
            with self.subTest(args=args):
                g = self.run_tool("gate.sh", *args)
                self.assertEqual(g.returncode, 0, g.stderr)
                self.assertIn("== partial gate passed", g.stdout)
                self.assertNotIn("\n== gate passed\n", "\n" + g.stdout)

    def test_land_refuses_an_exit_0_gate_without_its_pass_line(self) -> None:
        self.commit("stub gate", ("tools/gate.sh", STUB_GATE_NO_PASS_LINE))
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("never printed its pass line -- nothing pushed", r.stderr)
        self.assertEqual(self.remote_master(), self.base)

    # --- G1, L2: the stamp and the hook ---------------------------------------

    def stamp(self, sha: str, tree: str, log: str) -> None:
        d = os.path.join(self.repo, "out", "landed")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, sha + ".log"), "w") as f:
            f.write(log)
        digest = hashlib.sha256(log.encode()).hexdigest()
        with open(os.path.join(d, sha), "w") as f:
            f.write(f"tree {tree}\nlog-sha256 {digest}\n")

    def test_land_head_pushes_from_a_dirty_tree_through_the_hook(self) -> None:
        self.write("lane-half-written.rs", "fn\n")
        r = self.run_tool("land.sh", "--head")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("land: gate passed, 3 tests\n", r.stdout)
        self.assertIn("land: pushed\n", r.stdout)
        self.assertIn(f"pre-push: {self.head} was gated by land.sh", r.stderr)
        self.assertEqual(self.remote_master(), self.head)
        with open(os.path.join(self.repo, "out", "landed", self.head)) as f:
            self.assertIn(f"tree {self.rev('HEAD^{tree}')}\n", f.read())

    def test_a_stamp_for_another_tree_is_refused_and_the_gate_runs(self) -> None:
        self.cargo_test(FAIL_MARK, 101)  # so a gate run by the hook refuses
        self.stamp(self.head, self.rev(self.base + "^{tree}"), "== gate passed\n")
        p = subprocess.run(["git", "push", "origin", "master"], cwd=self.repo,
                           env=self.env, capture_output=True, text=True, timeout=120)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("has no valid land.sh stamp; running the gate", p.stderr)
        self.assertEqual(self.remote_master(), self.base)

    def test_a_stamp_whose_log_was_edited_is_refused(self) -> None:
        self.cargo_test(FAIL_MARK, 101)
        self.stamp(self.head, self.rev("HEAD^{tree}"), "== gate passed\n")
        with open(os.path.join(self.repo, "out", "landed", self.head + ".log"), "a") as f:
            f.write("edited\n")
        p = subprocess.run(["git", "push", "origin", "master"], cwd=self.repo,
                           env=self.env, capture_output=True, text=True, timeout=120)
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.remote_master(), self.base)

    # --- G3 --------------------------------------------------------------------

    def test_a_shell_script_with_a_syntax_error_fails_the_gate(self) -> None:
        self.commit("typo", ("tools/broken.sh", "if then fi (\n"))
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        self.assertIn("gate: tools/broken.sh has a syntax error", g.stderr)
        self.assertNotIn("== gate passed", g.stdout)


if __name__ == "__main__":
    unittest.main()
