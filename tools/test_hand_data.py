"""Unit tests for `tools/hand-data.py` (#105). Invented snippets only.
Run: `python3 -I tools/test_hand_data.py -q`.
"""
from __future__ import annotations

import importlib.util
import os
import sys
import tempfile
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("hand_data", os.path.join(_HERE, "hand-data.py"))
H = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(H)

SRC = """\
/// `groupFoo` **[V]**.
pub const FOO_GROUP: i64 = 1234;
pub const BAR_TYPE: i64 = 5678; // **[A]** guess
const PLAIN_TYPE: u32 = 4321;
const INVENTED_TYPE: i64 = 990_017;
const SMALL_TYPE: i64 = 4;
const TICKS_PER_SECOND: i64 = 1000;
const NOT_AN_ID: f64 = 1.5;

#[cfg(test)]
mod tests {
    const TEST_TYPE: i64 = 7777;
    fn f() { if true { } }
}
const AFTER_TYPE: i64 = 2468;
"""


class Scan(unittest.TestCase):
    def rows(self, src=SRC, path="a.rs"):
        return {n: (m, v) for _, n, m, v in H.scan(src, path)}

    def test_finds_with_marks(self):
        r = self.rows()
        self.assertEqual(r["FOO_GROUP"], ("V", 1234))
        self.assertEqual(r["BAR_TYPE"], ("A", 5678))
        self.assertEqual(r["PLAIN_TYPE"], ("-", 4321))

    def test_skips_invented_small_nonid_and_tests(self):
        r = self.rows()
        for n in ("INVENTED_TYPE", "SMALL_TYPE", "TICKS_PER_SECOND", "NOT_AN_ID", "TEST_TYPE"):
            self.assertNotIn(n, r)

    def test_code_after_test_module_is_seen(self):
        self.assertIn("AFTER_TYPE", self.rows())

    def test_test_files_skipped(self):
        self.assertEqual(self.rows(path="x/tests.rs"), {})
        self.assertEqual(self.rows(path="crate/tests/t.rs"), {})

    def test_invented_ranges(self):
        for v in (990001, 39000001, 69000005, 599001, 1999500):
            self.assertTrue(H.invented(v), v)
        for v in (1234, 30000001, 60000004):
            self.assertFalse(H.invented(v), v)


class Cli(unittest.TestCase):
    def test_collect_and_tsv_and_out_guard(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "c", "src"))
            with open(os.path.join(d, "c", "src", "moon.rs"), "w") as f:
                f.write(SRC)
            rows = H.collect(d)
            self.assertEqual({r[2] for r in rows}, {"FOO_GROUP", "BAR_TYPE", "PLAIN_TYPE", "AFTER_TYPE"})
            self.assertTrue(all(r[4] == "moon" for r in rows))
            self.assertNotIn("1234", H.to_tsv(rows, values=False))
            self.assertIn("1234", H.to_tsv(rows))
            self.assertEqual(H.main(["--root", d, "--out", os.path.join(d, "x.tsv")]), 2)


if __name__ == "__main__":
    unittest.main()
