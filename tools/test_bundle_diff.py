"""Tests for `tools/bundle-diff.py` (lane port-bundle, T3 of r-port-1109).

Every bundle is built here from invented ids and rows; no client byte is read.

Run::

    python3 -I tools/test_bundle_diff.py
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last, so nothing in tools/ shadows the stdlib

_spec = importlib.util.spec_from_file_location("bundle_diff", os.path.join(HERE, "bundle-diff.py"))
bd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bd)

TYPEDATA = [
    "type\t5001\t10\t6\t1\t1\t1.0\t0\t0\t0\t0\t11\t100=1.0\tAlpha",
    "type\t5002\t10\t6\t1\t1\t1.0\t0\t0\t0\t0\t11\t100=2.0\tBeta",
    "attribute\t7001\t0.0\tmass",
    "effect\t8001\t1\t0\t73\t6\t0\t0\t0\tfx\tE\t1\t0",
    "modifier\t8001\tshipID\tItemModifier\t7001\t7001\t2\t\t",
]
COMPONENTS = ["5001\tcompA", "5001\tcompB", "5002\tcompA"]
MARKET = ["5001\t4", "5002\t4"]


def write_bundle(root, files, build="1000", extractor="aa"):
    os.makedirs(root, exist_ok=True)
    recs = []
    for rel, rows in files.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        text = "\n".join(rows) + "\n"
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(text)
        recs.append("file\t%s\t%d\t%s\tres:/x" % (rel, len(rows), hashlib.sha256(text.encode()).hexdigest()))
    with open(os.path.join(root, "MANIFEST.tsv"), "w", encoding="utf-8") as fh:
        fh.write("# invented\nformat\t1.4\nclient_build\t%s\nextractor\t%s\n" % (build, extractor))
        fh.write("\n".join(recs) + "\n")


def base():
    return {
        "types/typedata.tsv": ["# note"] + TYPEDATA,
        "npc/components.tsv": ["#type_id\tcomponent"] + COMPONENTS,
        "types/market.tsv": ["#type_id\tmarket_group"] + MARKET,
    }


class BundleDiff(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.old = os.path.join(self.tmp.name, "old")
        self.new = os.path.join(self.tmp.name, "new")
        write_bundle(self.old, base())

    def tearDown(self):
        self.tmp.cleanup()

    def run_report(self, files, **kw):
        write_bundle(self.new, files, build="1001")
        return bd.report(self.old, self.new, **kw)

    def test_equal_bundles_exit_zero(self):
        text, code = self.run_report(base())
        self.assertEqual(code, 0)
        self.assertIn("0 differ, 3 equal", text)

    def test_added_rows_are_not_flagged(self):
        f = base()
        f["types/market.tsv"] = f["types/market.tsv"] + ["5003\t4"]
        text, code = self.run_report(f)
        self.assertEqual(code, 1)
        self.assertIn("added: 5003", text)
        self.assertIn("FLAGS: none", text)

    def test_removed_id_flagged(self):
        f = base()
        f["types/market.tsv"] = ["#type_id\tmarket_group", "5001\t4"]
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("REMOVED", text)
        self.assertIn("removed: 5002", text)

    def test_removed_file_flagged(self):
        f = base()
        del f["types/market.tsv"]
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("files gone from NEW: types/market.tsv", text)

    def test_type_class_change(self):
        f = base()
        f["types/typedata.tsv"] = ["# note"] + [TYPEDATA[0].replace("\t10\t6\t1\t", "\t11\t6\t0\t")] + TYPEDATA[1:]
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("CLASS", text)
        self.assertIn("groupID x1", text)
        self.assertIn("published x1", text)
        self.assertIn("type/5001", text)

    def test_type_attr_change_is_dogma_not_class(self):
        f = base()
        f["types/typedata.tsv"] = ["# note", TYPEDATA[0], TYPEDATA[1].replace("100=2.0", "100=3.0")] + TYPEDATA[2:]
        text, _code = self.run_report(f)
        self.assertIn("types whose effects or attributes changed: type/5002", text)
        self.assertNotIn("CLASS", text)

    def test_modifier_row_change_is_dogma(self):
        f = base()
        f["types/typedata.tsv"] = ["# note"] + TYPEDATA[:4] + [TYPEDATA[4].replace("ItemModifier", "LocationModifier")]
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("changed or removed dogma rows: modifier/8001", text)

    def test_typedata_keyed_by_kind_and_id(self):
        # attribute 7001 and a type 7001 are different keys
        f = base()
        f["types/typedata.tsv"] = ["# note"] + TYPEDATA + ["type\t7001\t10\t6\t1\t1\t1.0\t0\t0\t0\t0\t\t\tGamma"]
        text, _code = self.run_report(f)
        self.assertIn("added: type/7001", text)
        self.assertNotIn("changed:", text)

    def test_component_counts(self):
        f = base()
        f["npc/components.tsv"] = ["#type_id\tcomponent"] + COMPONENTS + ["5002\tcompB", "5002\tcompC"]
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("compB 1 -> 2", text)
        self.assertIn("compC 0 -> 1", text)
        self.assertNotIn("compA 2", text)

    def test_header_change_not_diffed(self):
        f = base()
        f["types/market.tsv"] = ["#type_id\tgroup"] + MARKET
        text, code = self.run_report(f)
        self.assertEqual(code, 3)
        self.assertIn("HEADER DIFFERS", text)

    def test_extractor_note(self):
        write_bundle(self.new, base(), build="1001", extractor="bb")
        text, _ = bd.report(self.old, self.new)
        self.assertIn("extractors differ", text)

    def test_uses_finds_ids_in_sources(self):
        root = os.path.join(self.tmp.name, "repo")
        os.makedirs(os.path.join(root, "server", "crates", "x"))
        os.makedirs(os.path.join(root, "server", "target"))
        with open(os.path.join(root, "server", "crates", "x", "a.rs"), "w") as fh:
            fh.write("const T: u32 = 5003; // 50030 is another number\nlet short = 12;\n")
        with open(os.path.join(root, "server", "target", "b.rs"), "w") as fh:
            fh.write("5003\n")
        f = base()
        f["types/market.tsv"] = f["types/market.tsv"] + ["5003\t4", "12\t4"]
        text, _ = self.run_report(f, uses=True, root=root)
        self.assertIn("5003: server/crates/x/a.rs x1", text)
        self.assertNotIn("50030:", text)
        self.assertNotIn("target", text)
        self.assertNotIn("  12:", text)

    def test_main_writes_out_and_returns_code(self):
        write_bundle(self.new, base(), build="1001")
        out = os.path.join(self.tmp.name, "r.txt")
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = bd.main([self.old, self.new, "--out", out])
        self.assertEqual(code, 0)
        with open(out) as fh:
            self.assertEqual(fh.read(), buf.getvalue())


if __name__ == "__main__":
    unittest.main()
