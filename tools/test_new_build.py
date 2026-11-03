"""Tests for `tools/new-build.sh` (NB-1 gate skip logic, NB-2 arguments).

Synthetic only: a temp tree holding a copy of the script, a registry, a
findings file and fake installs (a start.ini each). The analysis tool is a
fake that records its arguments. No client is read or launched.

    python3 -I tools/test_new_build.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "new-build.sh")
REG = "#hdr\n100\tL1\tpinned\t-\tx\n200\tL1\tequivalent\t-\ty\n300\tL1\tunverified\t-\tz\n"
REG_NO_300 = "#hdr\n100\tL1\tpinned\t-\tx\n200\tL1\tequivalent\t-\ty\n"
FSD = ("OTHER = {\n    999: 1,\n}\n"
       "CFSD_BUILD_LAYOUTS = {\n%s}\n"
       "AFTER = {\n    300: 1,\n}\n")


class Tree:
    def __init__(self, td, installs=(), rc=0, delta=True, reg=REG, layouts=(100, 200, 300)):
        self.root = os.path.join(td, "repo")
        self.prefix = os.path.join(td, "EVE")
        os.makedirs(os.path.join(self.root, "tools"))
        os.makedirs(os.path.join(self.root, "findings"))
        os.makedirs(self.prefix)
        shutil.copy(SCRIPT, os.path.join(self.root, "tools", "new-build.sh"))
        with open(os.path.join(self.root, "tools", "builds.tsv"), "w") as f:
            f.write(reg)
        with open(os.path.join(self.root, "tools", "fsd.py"), "w") as f:
            f.write(FSD % "".join("    %d: (None, {}),\n" % b for b in layouts))
        if delta:
            open(os.path.join(self.root, "findings", "build-delta-100-200.tsv"), "w").close()
        for name, build in installs:
            os.makedirs(os.path.join(self.prefix, name))
            with open(os.path.join(self.prefix, name, "start.ini"), "w") as f:
                f.write("[main]\nbuild = %s\ncodename = L1\n" % build)
        self.log = os.path.join(td, "calls.log")
        self.fake = os.path.join(td, "fake-analysis.sh")
        with open(self.fake, "w") as f:
            f.write('#!/bin/sh\necho "$EVE_CLIENT_DIR $*" >> "%s"\nexit %d\n' % (self.log, rc))
        os.chmod(self.fake, 0o755)

    def run(self, *args):
        env = dict(os.environ, EVE_PREFIX=self.prefix, EVE_ANALYSIS=self.fake)
        p = subprocess.run(["bash", os.path.join(self.root, "tools", "new-build.sh"), *args],
                           capture_output=True, text=True, env=env)
        return p.returncode, p.stdout + p.stderr

    def calls(self):
        if not os.path.exists(self.log):
            return ""
        with open(self.log) as f:
            return f.read()


class GateCheck(unittest.TestCase):
    def tree(self, **kw):
        td = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, td)
        return Tree(td, **kw)

    def test_no_installs_skips_cleanly(self):
        t = self.tree()
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 0)
        self.assertIn("not installed here; skipped", out)
        self.assertEqual(t.calls(), "")

    def test_only_one_install_skips(self):
        t = self.tree(installs=[("tq-offline", 100)])
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 0)
        self.assertIn("build 200 not installed", out)
        self.assertEqual(t.calls(), "")

    def test_both_installs_runs_check_with_the_findings_file(self):
        t = self.tree(installs=[("a", 100), ("b", 200)])
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 0, out)
        c = t.calls()
        self.assertIn("--check findings/build-delta-100-200.tsv", c)
        self.assertIn("/client-root/a /client-root/b", c)

    def test_exit_3_differs_still_passes_the_check(self):
        t = self.tree(installs=[("a", 100), ("b", 200)], rc=3)
        self.assertEqual(t.run("--gate-check")[0], 0)

    def test_mismatch_fails_the_gate(self):
        t = self.tree(installs=[("a", 100), ("b", 200)], rc=1)
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 1)
        self.assertIn("no longer reproduces", out)

    def test_not_an_install_fails_the_gate(self):
        t = self.tree(installs=[("a", 100), ("b", 200)], rc=2)
        self.assertEqual(t.run("--gate-check")[0], 1)

    def test_no_committed_delta_skips(self):
        t = self.tree(installs=[("a", 100), ("b", 200)], delta=False)
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 0)
        self.assertIn("none committed", out)

    def test_every_delta_is_checked_not_only_the_pinned_ones(self):
        t = self.tree(installs=[("a", 100), ("b", 200), ("c", 300)],
                      reg="#hdr\n300\tL1\tpinned\t-\tz\n200\tL1\tunverified\t-\ty\n100\tL1\tunverified\t-\tx\n")
        open(os.path.join(t.root, "findings", "build-delta-100-300.tsv"), "w").close()
        rc, out = t.run("--gate-check")
        self.assertEqual(rc, 0, out)
        self.assertIn("--check findings/build-delta-100-200.tsv", t.calls())
        self.assertIn("--check findings/build-delta-100-300.tsv", t.calls())

    def test_gate_sh_calls_the_check(self):
        with open(os.path.join(HERE, "gate.sh")) as f:
            self.assertIn("tools/new-build.sh --gate-check", f.read())


class Arguments(unittest.TestCase):
    def tree(self, **kw):
        td = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, td)
        return Tree(td, **kw)

    def test_no_args_is_usage(self):
        t = self.tree()
        rc, out = t.run()
        self.assertEqual(rc, 2)
        self.assertIn("new-build.sh DIR", out)

    def test_unknown_flag(self):
        self.assertEqual(self.tree().run("--nope")[0], 2)

    def test_flag_missing_value(self):
        self.assertEqual(self.tree().run("x", "--prefix")[0], 2)

    def test_two_dirs(self):
        self.assertEqual(self.tree().run("a", "b")[0], 2)

    def test_not_an_install(self):
        t = self.tree()
        rc, out = t.run(os.path.join(t.prefix, "nothing"))
        self.assertEqual(rc, 2)
        self.assertIn("not a client install", out)

    def test_the_pinned_build_is_refused(self):
        t = self.tree(installs=[("tq-offline", 100)])
        rc, out = t.run("tq-offline", "--prefix", t.prefix)
        self.assertEqual(rc, 2)
        self.assertIn("pinned build itself", out)

    def test_missing_pinned_install(self):
        t = self.tree(installs=[("new", 300)])
        rc, out = t.run("new", "--prefix", t.prefix)
        self.assertEqual(rc, 2)
        self.assertIn("pinned install", out)

    def test_dry_run_runs_nothing_and_stops_at_patches(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)])
        rc, out = t.run("new", "--prefix", t.prefix, "--dry-run")
        self.assertEqual(rc, 10, out)
        self.assertIn("(dry-run) tools/pin-client.sh --write", out)
        self.assertIn("STOP at step 8", out)
        self.assertIn("locate /client --merge tools/patches/100.json", out)
        self.assertIn("CONTROL COPY", out)
        self.assertIn("tools/patches/300.json", out)
        self.assertEqual(t.calls(), "")

    def stub_pin(self, t):
        p = os.path.join(t.root, "tools", "pin-client.sh")
        with open(p, "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        os.chmod(p, 0o755)

    def delta(self, t, verdict):
        with open(os.path.join(t.root, "findings", "build-delta-100-300.tsv"), "w") as f:
            f.write("layer\titem\tclass\told\tnew\tdetail\n"
                    "verdict\t100->300\t%s\t-\t-\t%s\n" % verdict)

    def test_a_delta_with_static_data_reasons_stops_at_step_5(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], rc=3)
        self.stub_pin(t)
        self.delta(t, ("differs", "static data changed res:/staticdata/x; schema of y not checked"))
        rc, out = t.run("new", "--prefix", t.prefix, "--go-on")
        self.assertEqual(rc, 10, out)
        self.assertIn("STOP at step 7", out)
        self.assertIn("--data-done", out)

    def test_data_done_goes_past_step_5_to_the_patch_stop(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], rc=3)
        self.stub_pin(t)
        self.delta(t, ("differs", "static data changed res:/staticdata/x"))
        rc, out = t.run("new", "--prefix", t.prefix, "--go-on", "--data-done")
        self.assertEqual(rc, 10, out)
        self.assertIn("STOP at step 8", out)

    def test_an_equivalent_verdict_that_says_static_data_unchanged_does_not_stop(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], rc=0)
        self.stub_pin(t)
        self.delta(t, ("equivalent", "protocol, call surface, schemas and static data unchanged"))
        rc, out = t.run("new", "--prefix", t.prefix)
        self.assertIn("STOP at step 8", out)

    def test_a_build_with_no_layout_stops_before_the_registry_and_the_data(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], layouts=(100, 200),
                      reg=REG_NO_300)
        self.stub_pin(t)
        rc, out = t.run("new", "--prefix", t.prefix, "--dry-run")
        self.assertEqual(rc, 10, out)
        self.assertIn("STOP at step 5", out)
        self.assertIn("CFSD_BUILD_LAYOUTS", out)
        self.assertIn("--tables", out)
        self.assertNotIn("step 6", out.split("STOP")[0])

    def test_a_layout_only_outside_the_layout_table_does_not_count(self):
        # 300 appears in the stub's AFTER block, not in CFSD_BUILD_LAYOUTS
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], layouts=(100,))
        rc, out = t.run("new", "--prefix", t.prefix, "--dry-run")
        self.assertIn("STOP at step 5", out)

    def test_an_unregistered_build_stops_at_the_registry_after_the_layout(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], reg=REG_NO_300)
        rc, out = t.run("new", "--prefix", t.prefix, "--dry-run")
        self.assertEqual(rc, 10, out)
        self.assertIn("STOP at step 6", out)
        self.assertIn("builds.tsv", out)

    def test_the_registry_stop_comes_before_the_data_stop(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)], rc=3, reg=REG_NO_300)
        self.stub_pin(t)
        self.delta(t, ("differs", "static data changed res:/staticdata/x"))
        rc, out = t.run("new", "--prefix", t.prefix, "--go-on")
        self.assertIn("STOP at step 6", out)
        self.assertNotIn("STOP at step 7", out)

    def test_a_new_host_stops_the_patch_step(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)])
        self.stub_pin(t)
        os.makedirs(os.path.join(t.root, "tools", "patches"))
        open(os.path.join(t.root, "tools", "patches", "300.json"), "w").close()
        with open(t.fake, "w") as f:  # delta ok (equivalent), hosts exit 1
            f.write('#!/bin/sh\ncase "$*" in *eve_patch_author*) exit 1;; esac\nexit 0\n')
        rc, out = t.run("new", "--prefix", t.prefix)
        self.assertEqual(rc, 10, out)
        self.assertIn("STOP at step 8", out)
        self.assertIn("NEW", out)

    def test_all_steps_done_and_report_names_absent_tools(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)])
        self.stub_pin(t)
        os.makedirs(os.path.join(t.root, "tools", "patches"))
        open(os.path.join(t.root, "tools", "patches", "300.json"), "w").close()
        with open(t.fake, "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        rc, out = t.run("new", "--prefix", t.prefix, "--report")
        self.assertEqual(rc, 0, out)
        self.assertIn("== report", out)
        self.assertIn("surface:", out)
        self.assertIn("recheck: tools/port-recheck.py not present", out)

    def test_report_runs_recheck_in_the_container_with_trees_and_bundles(self):
        t = self.tree(installs=[("tq-offline", 100), ("new", 300)])
        self.stub_pin(t)
        os.makedirs(os.path.join(t.root, "tools", "patches"))
        open(os.path.join(t.root, "tools", "patches", "300.json"), "w").close()
        tool = os.path.join(t.root, "tools", "port-recheck.py")
        open(tool, "w").close()
        os.chmod(tool, 0o755)
        for d in ("old-pyc", "new-pyc", "old-b", "new-b"):
            os.makedirs(os.path.join(t.root, d))
        os.environ.update(EVE_REPORT_OLD_PYC="old-pyc", EVE_REPORT_NEW_PYC="new-pyc",
                          EVE_REPORT_OLD_BUNDLE="old-b", EVE_REPORT_NEW_BUNDLE="new-b")
        self.addCleanup(lambda: [os.environ.pop("EVE_REPORT_%s" % k) for k in
                                 ("OLD_PYC", "NEW_PYC", "OLD_BUNDLE", "NEW_BUNDLE")])
        rc, out = t.run("new", "--prefix", t.prefix, "--report")
        self.assertEqual(rc, 0, out)
        self.assertIn("recheck: exit 0", out)
        self.assertIn("python3 -I tools/port-recheck.py old-pyc new-pyc --old-bundle old-b --new-bundle new-b",
                      t.calls())

    def test_a_bare_name_never_falls_back_to_the_online_prefix(self):
        t = self.tree()
        home = os.path.join(os.path.dirname(t.root), "home")
        online = os.path.join(home, ".local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE")
        os.makedirs(os.path.join(online, "tq-x"))
        with open(os.path.join(online, "tq-x", "start.ini"), "w") as f:
            f.write("[main]\nbuild = 300\n")
        env = {k: v for k, v in os.environ.items() if k != "EVE_PREFIX"}
        env.update(HOME=home, EVE_ANALYSIS=t.fake)
        p = subprocess.run(["bash", os.path.join(t.root, "tools", "new-build.sh"), "tq-x"],
                           capture_output=True, text=True, env=env, cwd=t.root)
        self.assertEqual(p.returncode, 2, p.stdout + p.stderr)
        self.assertIn("not a client install", p.stdout + p.stderr)


if __name__ == "__main__":
    unittest.main()
