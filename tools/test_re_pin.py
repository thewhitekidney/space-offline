#!/usr/bin/env python3
"""Tests for tools/re-pin.sh (PORT-REPIN, T5) on an invented prefix.

A fake checkout holds the real re-pin.sh and prefix-lib.sh and stubs for the
tools it calls; the prefix is a temporary tree of invented installs. Nothing
here needs a client, Docker or the owner's prefix.
"""
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
STUB = '#!/usr/bin/env bash\necho "$(basename "$0") $*" >> "$FAKE_LOG"\n%s'


def put(path, text, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if mode:
        os.chmod(path, mode)


class RePin(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-re-pin-")
        self.repo = os.path.join(self.tmp, "repo")
        self.prefix = os.path.join(self.tmp, "prefix")
        self.eve = os.path.join(self.prefix, "pfx/drive_c/CCP/EVE")
        self.log = os.path.join(self.tmp, "calls.log")
        open(self.log, "w").close()
        os.makedirs(os.path.join(self.repo, "tools/client-ci"))
        for f in ("re-pin.sh", "prefix-lib.sh"):
            shutil.copy(os.path.join(HERE, f), os.path.join(self.repo, "tools", f))
        put(self.repo + "/tools/builds.tsv", "# registry\n1000001\tL\tunverified\t-\told\n1000002\tL\tpinned\t-\tnew\n")
        put(self.repo + "/tools/eve_patch.py", "import os, sys\nopen(os.environ['FAKE_LOG'], 'a').write('eve_patch ' + ' '.join(sys.argv[1:]) + '\\n')\n")
        for f in ("pin-client.sh", "client2-setup.sh", "client-ci/slot-prefix.sh"):
            put(self.repo + "/tools/" + f, STUB % ('echo "  PREFIX=${PREFIX:-} SRC=${EVE_SLOT_SRC_PREFIX:-}" >> "$FAKE_LOG"\n'), 0o755)
        self.install("tq-offline", 1000001, marker="old")
        self.install("tq-1102", 1000002, marker="new")
        self.install("tq-1000002-control", 1000002, marker="control")
        os.makedirs(self.repo + "/out/pyc")
        os.makedirs(self.repo + "/out/content")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def install(self, name, build, marker=""):
        put("%s/%s/start.ini" % (self.eve, name), "[main]\nbuild = %d\ncodename = V1\n" % build)
        put("%s/%s/marker" % (self.eve, name), marker)

    def marker(self, name):
        with open("%s/%s/marker" % (self.eve, name)) as f:
            return f.read()

    def run_pin(self, *args, prefix=True):
        a = ["bash", os.path.join(self.repo, "tools/re-pin.sh"), *args]
        if prefix:
            a += ["--prefix", self.prefix]
        env = dict(os.environ, FAKE_LOG=self.log, EVE_REPIN_REPO=self.repo, HOME=self.tmp)
        return subprocess.run(a, capture_output=True, text=True, timeout=60, env=env)

    def calls(self):
        with open(self.log) as f:
            return f.read()

    def test_swap_renames_and_runs_the_steps_in_order(self):
        r = self.run_pin("tq-1102")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.marker("tq-offline"), "new")
        self.assertEqual(self.marker("tq-1000001"), "old")
        self.assertFalse(os.path.exists(self.eve + "/tq-1102"))
        self.assertTrue(os.path.isdir(self.repo + "/out/pyc-1000001"))
        self.assertTrue(os.path.isdir(self.repo + "/out/content-1000001"))
        self.assertFalse(os.path.exists(self.repo + "/out/pyc"))
        lines = self.calls().splitlines()
        self.assertTrue(lines[0].startswith("eve_patch apply --client " + self.eve + "/tq-offline"), lines)
        self.assertTrue(lines[1].startswith("pin-client.sh --write"), lines)
        self.assertIn("PREFIX=" + self.eve, lines[2])
        self.assertTrue(any(l.startswith("slot-prefix.sh remove all") for l in lines), lines)
        self.assertIn("tq-1000002-control", r.stdout)

    def test_a_scratch_prefix_skips_the_second_client(self):
        self.run_pin("tq-1102")
        self.assertNotIn("client2-setup", self.calls())

    def test_an_existing_old_name_is_not_nested_into(self):
        # the defect: tq-<old> already there, a plain mv would nest tq-offline in it
        self.install("tq-1000001", 1000001, marker="earlier")
        r = self.run_pin("tq-1102")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.marker("tq-1000001"), "earlier")
        self.assertEqual(self.marker("tq-1000001-a"), "old")
        self.assertFalse(os.path.exists(self.eve + "/tq-1000001/tq-offline"))
        self.assertEqual(self.marker("tq-offline"), "new")

    def test_an_explicit_old_name_that_exists_is_refused_and_nothing_moves(self):
        self.install("kept", 1000001, marker="kept")
        r = self.run_pin("tq-1102", "--old-name", "kept")
        self.assertEqual(r.returncode, 2)
        self.assertEqual(self.marker("tq-offline"), "old")
        self.assertTrue(os.path.isdir(self.eve + "/tq-1102"))
        self.assertEqual(self.calls(), "")

    def test_dry_run_changes_nothing_and_prints_the_plan(self):
        r = self.run_pin("tq-1102", "--dry-run")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("dry run", r.stdout)
        self.assertEqual(self.marker("tq-offline"), "old")
        self.assertTrue(os.path.isdir(self.repo + "/out/pyc"))
        self.assertEqual(self.calls(), "")
        self.assertIn("tq-1000001", r.stdout)

    def test_refuses_without_the_stock_control_copy(self):
        shutil.rmtree(self.eve + "/tq-1000002-control")
        r = self.run_pin("tq-1102")
        self.assertEqual(r.returncode, 2)
        self.assertIn("control", r.stderr)
        self.assertEqual(self.marker("tq-offline"), "old")

    def test_refuses_when_the_registry_pins_another_build(self):
        put(self.repo + "/tools/builds.tsv", "1000001\tL\tpinned\t-\told\n1000002\tL\tunverified\t-\tnew\n")
        r = self.run_pin("tq-1102")
        self.assertEqual(r.returncode, 2)
        self.assertIn("builds.tsv", r.stderr)
        self.assertEqual(self.marker("tq-offline"), "old")

    def test_refuses_the_same_build(self):
        self.install("tq-same", 1000001)
        r = self.run_pin("tq-same")
        self.assertEqual(r.returncode, 2)
        self.assertIn("nothing to re-pin", r.stderr)

    def test_refuses_when_the_old_corpus_is_in_the_way(self):
        os.makedirs(self.repo + "/out/pyc-1000001")
        r = self.run_pin("tq-1102")
        self.assertEqual(r.returncode, 2)
        self.assertEqual(self.marker("tq-offline"), "old")
        self.assertTrue(os.path.isdir(self.repo + "/out/pyc"))

    def test_refuses_bad_names_and_the_online_prefix(self):
        self.assertEqual(self.run_pin("tq").returncode, 2)
        self.assertEqual(self.run_pin("tq-offline").returncode, 2)
        self.assertEqual(self.run_pin("../x").returncode, 2)
        online = os.path.join(self.tmp, "Steam/steamapps/compatdata/8500")
        os.makedirs(os.path.join(online, "pfx/drive_c/CCP/EVE"))
        env = dict(os.environ, FAKE_LOG=self.log, EVE_REPIN_REPO=self.repo)
        r = subprocess.run(["bash", self.repo + "/tools/re-pin.sh", "tq-1102", "--prefix", online],
                           capture_output=True, text=True, timeout=30, env=env)
        self.assertEqual(r.returncode, 2)
        self.assertIn("online prefix", r.stderr)


if __name__ == "__main__":
    unittest.main()
