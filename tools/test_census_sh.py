"""Tests for `tools/census.sh`'s own logic (lane PHONE-HOME, review items 1, 2, 6).

The script runs in a fake repository: its own copy of census.sh, a fake
`tools/client-ci.sh` that records its arguments and writes the segment rows the
real one writes, and a stub `tools/census_report.py`. No client, no jail.

    python3 -I tools/test_census_sh.py              # the script in this checkout
    CENSUS_SH=/path/to/old/census.sh python3 -I tools/test_census_sh.py
"""

import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPT = pathlib.Path(os.environ.get("CENSUS_SH", HERE / "census.sh"))

FAKE_CI = r"""#!/usr/bin/env bash
echo "$*" >> "$FAKE_LOG"
echo "$EVE_CLIENT_DIR" >> "$FAKE_LOG.dirs"
scen=$2 run= check=0
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
    [[ ${args[i]} == --census ]] && run=${args[i+1]}
    [[ ${args[i]} == --check ]] && check=1
done
(( check )) && exit 0
code=$(sed -n "s/^$scen //p" "$FAKE_CODES" | head -n 1)
code=${code:-0}
mkdir -p "$run"
printf '%s\t%s\n' "$scen" "$code" >> "$run/segments.tsv"
if grep -qx -- "$scen" "$FAKE_NOCLIENT" 2>/dev/null; then printf '%s\n' "$scen" >> "$run/noclient.txt"; fi
exit "$code"
"""

STUB_REPORT = r"""#!/usr/bin/env python3
import json, os, sys
run = sys.argv[1]
open(os.environ["FAKE_LOG"], "a").write("report " + " ".join(sys.argv[1:]) + "\n")
open(os.path.join(run, "summary.txt"), "w").write("VERDICT V1: stub\n")
open(os.path.join(run, "report.json"), "w").write(json.dumps({"verdict": "CONTROL"}))
print("VERDICT V1: stub")
"""


class CensusSh(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="test-census-sh-"))
        (self.tmp / "tools").mkdir()
        shutil.copy(SCRIPT, self.tmp / "tools" / "census.sh")
        (self.tmp / "tools" / "client-ci.sh").write_text(FAKE_CI)
        (self.tmp / "tools" / "client-ci.sh").chmod(0o755)
        (self.tmp / "tools" / "census_report.py").write_text(STUB_REPORT)
        self.log = self.tmp / "calls.log"
        self.codes = self.tmp / "codes"
        self.noclient = self.tmp / "noclient"
        for f in (self.log, self.codes, self.noclient):
            f.write_text("")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_census(self, *argv):
        env = dict(os.environ, FAKE_LOG=str(self.log), FAKE_CODES=str(self.codes), FAKE_NOCLIENT=str(self.noclient))
        return subprocess.run(["bash", str(self.tmp / "tools" / "census.sh"), *argv, "--out", str(self.tmp / "out")],
                              capture_output=True, text=True, timeout=60, env=env)

    def calls(self):
        return self.log.read_text().splitlines()

    # -- item 1: the role survives parsing

    def test_the_noenv_role_reaches_client_ci(self):
        r = self.run_census("--copies", "c-ctl:control,c-sub:noenv", "--check")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        calls = self.calls()
        self.assertTrue(any("--census-noenv" in c for c in calls), calls)
        self.assertTrue(any("--census-control" in c for c in calls), calls)
        self.assertFalse(any("--census-noenv" in c and "--census-control" in c for c in calls))

    def test_noenv_has_its_own_scenario_list(self):
        self.run_census("--copies", "c-ctl:control,c-sub:noenv", "--check")
        noenv = [c for c in self.calls() if "--census-noenv" in c]
        names = {c.split()[1] for c in noenv}
        self.assertEqual(names, {"census_canary", "census_idle", "census_cc"})
        ctl = {c.split()[1] for c in self.calls() if "--census-control" in c}
        self.assertIn("census_urls", ctl)

    def test_a_plain_subject_gets_no_noenv_flag(self):
        self.run_census("--copies", "c-ctl:control,c-sub:subject", "--check")
        subject = [c for c in self.calls() if "--census-control" not in c]
        self.assertTrue(subject)
        self.assertFalse(any("--census-noenv" in c for c in subject))

    def test_the_noenv_run_has_its_own_directory_and_report_flag(self):
        r = self.run_census("--copies", "c-ctl:control,c-sub:noenv")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        out = self.tmp / "out"
        self.assertTrue((out / "c-sub-noenv").is_dir())
        self.assertEqual((out / "c-sub-noenv" / "role.txt").read_text().strip(), "noenv")
        self.assertTrue(any(c.startswith("report") and "--noenv" in c and "c-sub-noenv" in c for c in self.calls()), self.calls())

    # -- item 2: a segment that did not run is a fault

    def test_a_clean_run_is_clean_and_needs_no_noclient_file(self):
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for d in ("c-ctl", "c-sub"):
            self.assertFalse((self.tmp / "out" / d / "faults.txt").exists())

    def test_a_failing_segment_makes_the_copy_invalid(self):
        self.codes.write_text("census_urls 1\n")
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        faults = (self.tmp / "out" / "c-sub" / "faults.txt").read_text()
        self.assertIn("census_urls: client-ci exit 1", faults)
        self.assertIn("INVALID", (self.tmp / "out" / "SUMMARY.md").read_text())

    def test_a_segment_with_no_client_is_a_fault(self):
        self.noclient.write_text("census_cdn\n")
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("census_cdn: no client was launched", (self.tmp / "out" / "c-sub" / "faults.txt").read_text())

    def test_crash_and_hang_may_end_with_the_client_dead(self):
        self.codes.write_text("census_crash 4\ncensus_hang 4\n")
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertFalse((self.tmp / "out" / "c-sub" / "faults.txt").exists())

    def test_a_missing_segment_is_a_fault(self):
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject", "--scenarios", "census_idle,census_cc")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        (self.tmp / "out" / "c-sub" / "segments.tsv").write_text("census_idle\t0\n")  # census_cc's row lost
        # re-running the check alone is not possible, so assert on a fresh run with a client-ci that records one fewer
        shutil.rmtree(self.tmp / "out")
        (self.tmp / "tools" / "client-ci.sh").write_text(FAKE_CI.replace('printf \'%s\\t%s\\n\' "$scen" "$code" >> "$run/segments.tsv"', '[[ $scen == census_cc ]] || printf \'%s\\t%s\\n\' "$scen" "$code" >> "$run/segments.tsv"'))
        r = self.run_census("--copies", "c-ctl:control,c-sub:subject", "--scenarios", "census_idle,census_cc")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("census_cc: no segment was recorded", (self.tmp / "out" / "c-sub" / "faults.txt").read_text())


    # -- PORT-REPIN: the default control follows the registry

    def default_dirs(self):
        d = pathlib.Path(str(self.log) + ".dirs")
        return sorted(set(d.read_text().split())) if d.exists() else []

    def test_the_default_control_is_named_for_the_pinned_build(self):
        (self.tmp / "tools" / "builds.tsv").write_text(
            "# registry\n1111111\tL1\tunverified\t-\told\n2222222\tL1\tpinned\t-\tnow\n")
        r = self.run_census("--check")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("tq-2222222-control", self.default_dirs())
        self.assertNotIn("tq-3662168-control", self.default_dirs())

    def test_without_a_registry_the_old_name_is_the_fallback(self):
        r = self.run_census("--check")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("tq-3662168-control", self.default_dirs())


if __name__ == "__main__":
    unittest.main()
