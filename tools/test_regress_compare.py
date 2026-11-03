#!/usr/bin/env python3
"""Tests for tools/regress-compare.py (PORT-REPIN, T6) on invented tables."""
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "regress-compare.py")
spec = importlib.util.spec_from_file_location("regress_compare", TOOL)
rc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rc)


def table(d, rows, finished=True):
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "regress.tsv"), "w") as f:
        for k, v in rows:
            f.write("%s\t%s\n" % (k, v))
        if finished:
            f.write("finished\n")
    return d


class Compare(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.t = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def run_tool(self, base, new, *extra):
        r = subprocess.run([sys.executable, "-I", TOOL, base, new, *extra], capture_output=True, text=True, timeout=30)
        return r.returncode, r.stdout, r.stderr

    def test_classes(self):
        b = table(self.t + "/b", [("a", "0"), ("b", "0"), ("c", "1"), ("d", "1"), ("e", "4"), ("f", "0"), ("g", "0")])
        n = table(self.t + "/n", [("a", "0"), ("b", "1"), ("c", "0"), ("d", "6"), ("e", "4"), ("f", "143"), ("h", "3")])
        code, out, _ = self.run_tool(b, n)
        self.assertEqual(code, 3, out)
        res = rc.compare(rc.read_table(b)[0], rc.read_table(n)[0])
        names = lambda k: [r[0] for r in res[k]]
        self.assertEqual(names("new_red"), ["b", "h"])
        self.assertEqual(names("new_green"), ["c"])
        self.assertEqual(names("still_red"), ["d"])
        self.assertEqual(names("still_never"), ["e"])
        self.assertEqual(names("lost"), ["f", "g"])  # f stopped, g absent
        self.assertEqual(names("only_base"), ["g"])
        self.assertEqual(names("only_new"), ["h"])

    def test_identical_runs_are_clean(self):
        rows = [("a", "0"), ("b", "1"), ("c", "4")]
        code, out, _ = self.run_tool(table(self.t + "/b", rows), table(self.t + "/n", rows))
        self.assertEqual(code, 0, out)
        self.assertIn("new reds: 0", out)
        self.assertIn("still red: 1", out)

    def test_all_fours_name_the_trigger_first(self):
        b = table(self.t + "/b", [("a", "0"), ("b", "0")])
        n = table(self.t + "/n", [("a", "4"), ("b", "4")])
        code, out, _ = self.run_tool(b, n)
        self.assertEqual(code, 4)
        self.assertTrue(out.startswith("TRIGGER:"), out)
        self.assertIn("GetSettingsInfo", out)

    def test_a_mixed_run_is_not_the_trigger(self):
        b = table(self.t + "/b", [("a", "0"), ("b", "0")])
        n = table(self.t + "/n", [("a", "4"), ("b", "0")])
        code, out, _ = self.run_tool(b, n)
        self.assertEqual(code, 3)
        self.assertNotIn("TRIGGER", out)

    def test_files_and_dirs_both_work_and_missing_is_never_ran(self):
        b = table(self.t + "/b", [("a", "0")])
        n = table(self.t + "/n", [("a", "missing")])
        code, out, _ = self.run_tool(b + "/regress.tsv", n)
        self.assertEqual(code, 3, out)
        self.assertIn("never ran (ran in BASE): 1", out)

    def test_an_interrupted_run_is_noted(self):
        b = table(self.t + "/b", [("a", "0")])
        n = table(self.t + "/n", [("a", "0")], finished=False)
        code, out, _ = self.run_tool(b, n)
        self.assertEqual(code, 0)
        self.assertIn("NEW has no `finished` line", out)

    def test_bad_input_is_exit_2(self):
        b = table(self.t + "/b", [("a", "0")])
        os.makedirs(self.t + "/n")
        with open(self.t + "/n/regress.tsv", "w") as f:
            f.write("a\tbanana\n")
        self.assertEqual(self.run_tool(b, self.t + "/n")[0], 2)
        self.assertEqual(self.run_tool(b, self.t + "/nothing")[0], 2)
        e = table(self.t + "/e", [])
        self.assertEqual(self.run_tool(b, e)[0], 2)

    def test_a_rerun_row_last_wins(self):
        b = table(self.t + "/b", [("a", "0")])
        n = table(self.t + "/n", [("a", "1"), ("a", "0")])
        self.assertEqual(self.run_tool(b, n)[0], 0)


if __name__ == "__main__":
    unittest.main()
