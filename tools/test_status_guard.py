"""Tests for tools/status-guard.py, on invented paths and a throwaway git repo.

Run::

    python3 -I tools/test_status_guard.py
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "status-guard.py")

_spec = importlib.util.spec_from_file_location("status_guard", SCRIPT)
sg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sg)


class Decide(unittest.TestCase):
    def test_docs_only_passes(self):
        self.assertTrue(sg.decide(["docs" + "/guides/x.md"], [""])[0])

    def test_server_without_progress_fails(self):
        ok, msg = sg.decide(["server/crates/a/src/b.rs"], ["Fix b"])
        self.assertFalse(ok)
        self.assertIn("server/crates/a/src/b.rs", msg)

    def test_content_and_scenarios_are_watched(self):
        self.assertFalse(sg.decide(["content/x.tsv"], [""])[0])
        self.assertFalse(sg.decide(["tools/client-ci/scenarios/fci_x.py"], [""])[0])

    def test_other_tools_are_not_watched(self):
        self.assertTrue(sg.decide(["tools/client-ci/run.sh", "tools/x.py"], [""])[0])

    def test_server_with_progress_passes(self):
        self.assertTrue(sg.decide(["server/a.rs", "docs/status/progress.tsv"], [""])[0])

    def test_waiver_in_commit_message_passes(self):
        self.assertTrue(sg.decide(["server/a.rs"], ["Refactor\n\nStatus: none (rename only)\n"])[0])

    def test_waiver_in_pr_body_passes(self):
        self.assertTrue(sg.decide(["server/a.rs"], ["Refactor", "body\r\nStatus: none (tests only)\r\n"])[0])

    def test_empty_reason_fails(self):
        self.assertFalse(sg.decide(["server/a.rs"], ["Status: none ( )"])[0])

    def test_waiver_must_start_a_line(self):
        self.assertFalse(sg.decide(["server/a.rs"], ["see Status: none (x)"])[0])


class Cli(unittest.TestCase):
    def run_git(self, d, *args):
        subprocess.run(["git", "-C", d, *args], check=True, capture_output=True)

    def write(self, d, rel, text):
        os.makedirs(os.path.dirname(os.path.join(d, rel)), exist_ok=True)
        with open(os.path.join(d, rel), "w", encoding="utf-8") as f:
            f.write(text)

    def guard(self, d, *args, body=""):
        env = dict(os.environ, PR_BODY=body)
        return subprocess.run([sys.executable, "-I", SCRIPT, *args], cwd=d, env=env,
                              capture_output=True, text=True).returncode

    def test_end_to_end(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_git(d, "init", "-q")
            self.write(d, "README.md", "x\n")
            self.run_git(d, "add", "-A")
            self.run_git(d, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base")
            self.run_git(d, "tag", "base")
            self.write(d, "server/a.rs", "fn a() {}\n")
            self.run_git(d, "add", "-A")
            self.run_git(d, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "code")
            self.assertEqual(self.guard(d, "--base", "base"), 1)
            self.assertEqual(self.guard(d, "--base", "base", body="Status: none (no feature)"), 0)
            self.write(d, "docs/status/progress.tsv", "row\n")
            self.assertEqual(self.guard(d, "--base", "base"), 1)
            self.assertEqual(self.guard(d, "--base", "base", "--worktree"), 0)
            self.assertEqual(self.guard(d, "--base", "no-such-ref"), 2)


if __name__ == "__main__":
    unittest.main()
