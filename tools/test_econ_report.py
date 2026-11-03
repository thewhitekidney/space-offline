#!/usr/bin/env python3
"""Tests for tools/econ-report.py (FE-MEASURE): invented rows only.

    tools/analysis.sh python3 tools/test_econ_report.py
"""
import importlib.util
import io
import os
import contextlib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("econ_report", os.path.join(HERE, "econ-report.py"))
er = importlib.util.module_from_spec(spec)
spec.loader.exec_module(er)

HEADER = "day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h"


def row(day, kind, key, n=0, hours=24, isk=0, isk2=0, per_h=0, scope="all"):
    return dict(zip(HEADER.split("\t"), map(str, (day, scope, kind, key, n, hours, isk, isk2, per_h))))


def basket(day, idx):
    return row(day, "pbasket", "armour", n=3, isk=idx, isk2=idx, per_h=10)


class Shock(unittest.TestCase):
    def test_a_step_with_overshoot(self):
        rows = [basket(d, v) for d, v in
                [(1, 1000), (2, 1000), (3, 1000), (4, 1400), (5, 1300), (6, 1200), (7, 1100), (8, 1100), (9, 1100)]]
        s = er.shock(rows, "armour", 4)
        self.assertEqual(s["pre"], 1000)
        self.assertEqual(s["settled"], 1100)
        self.assertEqual(s["peak"], 1400)
        self.assertAlmostEqual(s["overshoot"], 3.0)  # (1400 - 1100) / (1100 - 1000)
        self.assertEqual(s["days_to_half"], 0)
        self.assertEqual(s["half_life"], 2)  # 1200 is within half the peak gap of 1100

    def test_too_short_is_none_not_a_crash(self):
        s = er.shock([basket(1, 1000)], "armour", 1)
        self.assertIsNone(s["pre"])
        self.assertIsNone(er.shock([], "armour", 1)["peak"])

    def test_a_fall_mirrors_a_rise(self):
        rows = [basket(d, v) for d, v in [(1, 1000), (2, 1000), (3, 600), (4, 700), (5, 800), (6, 800), (7, 800)]]
        s = er.shock(rows, "armour", 3)
        self.assertEqual((s["pre"], s["settled"], s["peak"]), (1000, 800, 600))
        self.assertAlmostEqual(s["overshoot"], 1.0)


class Tape(unittest.TestCase):
    def test_the_tables_print_and_a_missing_day_is_a_dash(self):
        rows = [basket(1, 1000), basket(2, 1100), row(2, "book", "armour", n=5, isk=7, per_h=-167),
                row(1, "level", "all", isk=1000, isk2=900, per_h=500),
                row(2, "audit", "pinned", n=1)]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            er.tape(rows, "all", None)
        text = out.getvalue()
        self.assertIn("basket price index, history", text)
        self.assertIn("1100", text)
        self.assertIn("-167", text)
        self.assertIn("day 2 pinned 1", text)
        self.assertIn("day 1 level 1000 bots 900", text)

    def test_the_old_tables_still_read_the_old_rows(self):
        path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "econ-report-test.tsv")
        with open(path, "w") as f:
            f.write(HEADER + "\n" + "\t".join(["1", "all", "isk_src", "bounties", "4", "0", "400", "0", "0"]) + "\n")
        try:
            self.assertEqual(er.read(path)[0]["key"], "bounties")
        finally:
            os.remove(path)


if __name__ == "__main__":
    unittest.main()
