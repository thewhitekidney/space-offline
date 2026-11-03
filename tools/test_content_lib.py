"""Tests for `tools/content-lib.sh`, the build -> content bundle choice.

Synthetic only: a temp repo holding an invented registry and fake installs (a
start.ini each); the builds are invented numbers. No client is read.

    python3 -I tools/test_content_lib.py
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LIB = os.path.join(HERE, "content-lib.sh")
REG = ("#hdr\n"
       "100\tL1\tpinned\t-\tx\n"
       "200\tL1\tequivalent\t-\ty\n"
       "300\tL1\tunverified\t-\tz\n")


class Select(unittest.TestCase):
    def setUp(self):
        td = tempfile.TemporaryDirectory()
        self.addCleanup(td.cleanup)
        self.repo = os.path.join(td.name, "repo")
        os.makedirs(os.path.join(self.repo, "out"))
        self.reg = os.path.join(td.name, "builds.tsv")
        with open(self.reg, "w") as fh:
            fh.write(REG)
        self.td = td.name

    def client(self, name, build, crlf=False):
        d = os.path.join(self.td, name)
        os.makedirs(d)
        if build is not None:
            nl = "\r\n" if crlf else "\n"
            with open(os.path.join(d, "start.ini"), "w", newline="") as fh:
                fh.write("[main]%scodename = L1%sbuild = %s%s" % (nl, nl, build, nl))
        return d

    def select(self, client, **env):
        script = ('source "$1"; eve_content_select "$2" "$3" || exit 3; '
                  'echo "$EVE_CBUILD|$EVE_CSTATUS|$CONTENT_REL|$CONTENT|'
                  '${EVE_SERVER_BUILD_ARGS[*]-}|${EVE_CONTENT_TOOL_ARGS[*]-}"')
        e = dict(os.environ, EVE_BUILDS_TSV=self.reg)
        e.pop("EVE_CONTENT", None)
        e.update(env)
        p = subprocess.run(["bash", "-euc", script, "x", LIB, client, self.repo],
                           capture_output=True, text=True, env=e)
        if p.returncode == 0:
            return p.stdout.strip().split("|")
        return p.returncode, p.stderr

    def test_the_pinned_build_keeps_out_content_and_no_extra_flags(self):
        got = self.select(self.client("a", 100))
        self.assertEqual(got, ["100", "pinned", "out/content", self.repo + "/out/content", "", ""])

    def test_an_equivalent_build_shares_the_pinned_bundle(self):
        got = self.select(self.client("a", 200, crlf=True))
        self.assertEqual(got[:3], ["200", "equivalent", "out/content"])
        self.assertEqual(got[4:], ["", ""])

    def test_an_unverified_build_gets_its_own_directory_and_both_opt_ins(self):
        got = self.select(self.client("a", 300))
        self.assertEqual(got, ["300", "unverified", "out/content-300",
                               self.repo + "/out/content-300",
                               "--client-build 300", "--build 300"])

    def test_an_unregistered_build_is_refused(self):
        rc, err = self.select(self.client("a", 999))
        self.assertEqual(rc, 3)
        self.assertIn("not in", err)

    def test_an_unverified_build_cannot_be_pointed_at_out_content(self):
        for target in ("out/content", "out/content/", self.repo + "/out/content"):
            rc, err = self.select(self.client("c" + str(len(target)), 300), EVE_CONTENT=target)
            self.assertEqual(rc, 3, target)
            self.assertIn("must not use out/content", err)

    def test_eve_content_picks_a_directory_under_out_only(self):
        c = self.client("a", 300)
        got = self.select(c, EVE_CONTENT="out/mine")
        self.assertEqual(got[2], "out/mine")
        for bad in ("/elsewhere/x", "out/../x", "mine", "out"):
            rc, _err = self.select(c, EVE_CONTENT=bad)
            self.assertEqual(rc, 3, bad)

    def test_no_start_ini_is_the_legacy_path_and_reads_nothing_else(self):
        got = self.select(self.client("a", None))
        self.assertEqual(got[1:3], ["legacy", "out/content"])
        self.assertEqual(got[4:], ["", ""])

    def test_a_start_ini_with_no_build_is_refused(self):
        d = self.client("a", None)
        with open(os.path.join(d, "start.ini"), "w") as fh:
            fh.write("[main]\ncodename = L1\n")
        rc, _err = self.select(d)
        self.assertEqual(rc, 3)

    def test_the_real_registry_is_understood(self):
        real = os.path.join(HERE, "builds.tsv")
        got = self.select(self.client("a", 3662168), EVE_BUILDS_TSV=real)
        self.assertEqual(got[2], "out/content")
        got = self.select(self.client("b", 3619879), EVE_BUILDS_TSV=real)
        self.assertEqual(got[2], "out/content-3619879")


class Wiring(unittest.TestCase):
    """up.sh and client-ci.sh must go through the library."""

    def read(self, name):
        with open(os.path.join(HERE, name)) as fh:
            return fh.read()

    def test_up_sh_selects_and_passes_the_choice_down(self):
        s = self.read("up.sh")
        self.assertIn("eve_content_select", s)
        self.assertIn('--out "/work/$CONTENT_REL"', s)
        self.assertIn("EVE_CONTENT_TOOL_ARGS", s)
        self.assertIn("EVE_SERVER_BUILD_ARGS", s)
        self.assertNotIn("/work/out/content ", s)
        self.assertNotIn("CONTENT=$REPO/out/content\n", s)

    def test_client_ci_selects_and_its_rootless_line_uses_it(self):
        s = self.read("client-ci.sh")
        self.assertIn("eve_content_select", s)
        self.assertIn('--content "$CONTENT"', s)
        self.assertNotIn('--content "$REPO/out/content"', s)
        self.assertIn("EVE_SERVER_BUILD_ARGS", s)


if __name__ == "__main__":
    unittest.main()
