"""Unit tests for the content driver, `tools/content.py` (packet CP1).

Synthetic data only (plan §1.6): fixture section modules in a temp dir and
invented ids. The client-backed checks are `content.py --check`, recorded in
`docs/lanes/cp1.md`. Run: `python3 -I tools/test_content.py -q`.
"""

from __future__ import annotations

import glob
import io
import os
import sys
import tempfile
import textwrap
import types
import unittest
from contextlib import redirect_stderr

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402


def _read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


ZZ = '''
SECTION = "zz"
DEPENDS = ()

def section(ctx):
    rows = [(39000001, 1.5, [1, 2], {990001: 0.25}, None, "Example Prime"),
            (39000002, -3.0, [], {}, True, "Example\\tSecond")]
    lines = ["# a comment, not a row", "kind\\t1"]
    return [("zz/systems.tsv", ("system_id", "x", "l", "m", "o", "name"), rows),
            ("zz/raw.tsv", None, lines)]

def check(ctx):
    return []
'''

AA = '''
SECTION = "aa"
DEPENDS = ("zz",)

def section(ctx):
    with open(ctx.path("zz/systems.tsv")) as fh:
        n = sum(1 for ln in fh if not ln.startswith("#"))
    return [("aa/derived.tsv", ("n",), [(n,)])]

def check(ctx):
    return []
'''

BOOM = '''
SECTION = "boom"
DEPENDS = ()

def section(ctx):
    def rows():
        yield (1,)
        raise RuntimeError("mid-section")
    return [("boom/ok.tsv", ("a",), [(1,)]), ("boom/bad.tsv", ("a",), rows())]

def check(ctx):
    return []
'''


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.sections = os.path.join(self.root, "tools")
        os.makedirs(self.sections)
        self.client = os.path.join(self.root, "client")
        os.makedirs(self.client)
        with open(os.path.join(self.client, "start.ini"), "w") as fh:
            fh.write("[main]\nbuild = 1234567\n")
        self.out = os.path.join(self.root, "out", "content")

    def tearDown(self):
        self.tmp.cleanup()

    def module(self, name, text):
        with open(os.path.join(self.sections, "content_%s.py" % name), "w") as fh:
            fh.write(textwrap.dedent(text))

    def run_driver(self, *extra):
        argv = ["--client", self.client, "--out", self.out] + list(extra)
        with redirect_stderr(io.StringIO()):
            return content.main(argv, section_dir=self.sections)

    def manifest(self):
        return content.read_manifest(self.out)


class Discovery(Fixture):
    def test_a_fixture_module_is_discovered_with_no_driver_edit(self):
        self.module("zz", ZZ)
        names = [s.name for s in content.discover(self.sections)]
        self.assertIn("zz", names)
        self.assertEqual(names[:4], list(content.BUILTIN_ORDER))
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        self.assertTrue(os.path.isfile(os.path.join(self.out, "zz", "systems.tsv")))

    def test_depends_orders_sections_and_pulls_in_dependencies(self):
        self.module("aa", AA)
        self.module("zz", ZZ)
        picked = [s.name for s in content.order(content.discover(self.sections), ["aa"])]
        self.assertEqual(picked, ["zz", "aa"])
        self.assertEqual(self.run_driver("--sections", "aa"), 0)
        _single, files = self.manifest()
        self.assertEqual([f[0] for f in files],
                         ["zz/systems.tsv", "zz/raw.tsv", "aa/derived.tsv"])
        with open(os.path.join(self.out, "aa", "derived.tsv")) as fh:
            self.assertEqual(fh.read(), "#n\n2\n")

    def test_a_cycle_or_unknown_dependency_refuses(self):
        a = content.Section("a", ("b",), None, None)
        b = content.Section("b", ("a",), None, None)
        with self.assertRaises(content.ContentError):
            content.order([a, b])
        with self.assertRaises(content.ContentError):
            content.order([content.Section("c", ("nope",), None, None)])


class AtomicWrite(Fixture):
    def leftovers(self):
        return glob.glob(self.out + ".tmp-*") + glob.glob(self.out + ".old-*")

    def test_an_exception_mid_section_leaves_no_bundle_and_no_tmp(self):
        self.module("boom", BOOM)
        with self.assertRaises(RuntimeError):
            self.run_driver("--sections", "boom")
        self.assertFalse(os.path.exists(self.out))
        self.assertEqual(self.leftovers(), [])

    def test_a_failed_run_keeps_the_previous_bundle(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        before = _read(os.path.join(self.out, content.MANIFEST))
        self.module("boom", BOOM)
        with self.assertRaises(RuntimeError):
            self.run_driver("--sections", "boom")
        self.assertEqual(_read(os.path.join(self.out, content.MANIFEST)), before)
        self.assertEqual(self.leftovers(), [])

    def test_a_stale_tmp_dir_of_a_dead_run_is_swept(self):
        self.module("zz", ZZ)
        os.makedirs(self.out + ".tmp-%d" % os.getpid())
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        self.assertEqual(self.leftovers(), [])


class Manifest(Fixture):
    def test_manifest_rows_equal_rows_written(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz", "--systems", "39000002,39000001"), 0)
        single, files = self.manifest()
        self.assertEqual(single["format"], content.FORMAT)
        self.assertEqual(single["client_build"], "1234567")
        self.assertEqual(single["systems"], "39000001,39000002")
        self.assertEqual(len(single["extractor"]), 64)
        for rel, n, digest, _srcs in files:
            path = os.path.join(self.out, rel)
            data = [ln for ln in _read(path).split("\n") if ln and ln[0] != "#"]
            self.assertEqual(n, len(data), rel)
            self.assertEqual(digest, content.sha256_file(path), rel)
        self.assertEqual(content.check_manifest(self.out), [])
        lines = _read(os.path.join(self.out, content.MANIFEST)).splitlines()
        self.assertEqual(lines[0], content.MANIFEST_BANNER)
        self.assertEqual([ln.split("\t")[0] for ln in lines[1:]],
                         ["format", "client_build", "extractor", "systems", "data_hash",
                          "file", "file"])
        self.assertEqual(single["data_hash"], content.data_hash(files))
        text = _read(os.path.join(self.out, "zz", "systems.tsv"))
        self.assertTrue(text.startswith("#system_id\tx\tl\tm\to\tname\n"))
        self.assertIn("39000002\t-3.0\t\t\t1\tExample Second\n", text)

    def test_check_catches_a_tampered_file(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        with open(os.path.join(self.out, "zz", "raw.tsv"), "a") as fh:
            fh.write("kind\t2\n")
        bad = content.check_manifest(self.out)
        self.assertEqual(len(bad), 2, bad)  # the row count and the sha256
        self.assertEqual(self.run_driver("--sections", "zz", "--check"), 1)



YY = '''
SECTION = "yy"
DEPENDS = ()

def section(ctx):
    return [("yy/one.tsv", ("a",), [(%d,)])]

def check(ctx):
    return []
'''


class Merge(Fixture):
    """FX8: `--sections` merges into an existing bundle (port-tracker §76)."""

    def two_sections(self):
        self.module("zz", ZZ)
        self.module("yy", YY % 1)
        self.assertEqual(self.run_driver("--sections", "zz,yy"), 0)

    def rewrite_manifest(self, kind, value):
        path = os.path.join(self.out, content.MANIFEST)
        lines = _read(path).splitlines()
        lines = [kind + "\t" + value if ln.split("\t")[0] == kind else ln for ln in lines]
        with open(path, "w") as fh:
            fh.write("\n".join(lines) + "\n")

    def test_a_sections_run_keeps_the_other_sections_files_and_rows(self):
        self.two_sections()
        _single, before = self.manifest()
        zz = _read(os.path.join(self.out, "zz", "systems.tsv"))
        # Another length: a same-size rewrite in the same second would load
        # the stale `__pycache__` bytecode.
        self.module("yy", YY % 22)
        self.assertEqual(self.run_driver("--sections", "yy"), 0)
        _single, after = self.manifest()
        self.assertEqual([f[0] for f in after], [f[0] for f in before])
        self.assertEqual([f for f in after if f[0].startswith("zz/")],
                         [f for f in before if f[0].startswith("zz/")])
        self.assertEqual(_read(os.path.join(self.out, "zz", "systems.tsv")), zz)
        self.assertEqual(_read(os.path.join(self.out, "yy", "one.tsv")), "#a\n22\n")
        self.assertEqual(content.check_manifest(self.out), [])

    def test_a_new_file_is_appended_after_the_kept_rows(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        self.module("yy", YY % 1)
        self.assertEqual(self.run_driver("--sections", "yy"), 0)
        _single, files = self.manifest()
        self.assertEqual([f[0] for f in files],
                         ["zz/systems.tsv", "zz/raw.tsv", "yy/one.tsv"])
        self.assertEqual(content.check_manifest(self.out), [])

    def test_the_merge_keeps_the_old_extractor_hash_and_old_records(self):
        # up.sh's staleness check: a merged bundle is never fresher than its
        # oldest file, so an extractor change still re-extracts it all.
        self.two_sections()
        self.rewrite_manifest("extractor", "0" * 64)
        with open(os.path.join(self.out, content.MANIFEST), "a") as fh:
            fh.write("input\tcontent/x.tsv\told\nsource\tres:/x.static\told\n")
        self.assertEqual(self.run_driver("--sections", "yy"), 0)
        single, _files = self.manifest()
        self.assertEqual(single["extractor"], "0" * 64)
        inputs, sources = content.read_records(self.out)
        self.assertEqual(inputs, {"content/x.tsv": "old"})
        self.assertEqual(sources, {"res:/x.static": "old"})
        # A full-bundle writer (no merge) records the current hash.
        self.assertEqual(content.extractor_hash(), content.extractor_hash())
        self.assertNotEqual(content.extractor_hash(), "0" * 64)

    def test_a_merge_refuses_another_systems_list_or_build_and_keeps_the_bundle(self):
        self.two_sections()
        before = _read(os.path.join(self.out, content.MANIFEST))
        self.assertEqual(self.run_driver("--sections", "yy", "--systems", "39000001"), 2)
        self.rewrite_manifest("client_build", "1")
        changed = _read(os.path.join(self.out, content.MANIFEST))
        self.assertNotEqual(changed, before)
        self.assertEqual(self.run_driver("--sections", "yy"), 2)
        self.assertEqual(_read(os.path.join(self.out, content.MANIFEST)), changed)

    def test_a_merge_refuses_a_bundle_that_fails_check(self):
        self.two_sections()
        os.remove(os.path.join(self.out, "zz", "raw.tsv"))
        self.assertEqual(self.run_driver("--sections", "yy"), 2)
        self.assertFalse(os.path.exists(os.path.join(self.out, "zz", "raw.tsv")))

    def test_a_failed_merge_leaves_the_old_bundle_byte_identical(self):
        self.two_sections()
        before = {rel: _read(os.path.join(self.out, rel)) for rel, *_ in self.manifest()[1]}
        self.module("boom", BOOM)
        with self.assertRaises(RuntimeError):
            self.run_driver("--sections", "boom")
        for rel, text in before.items():
            self.assertEqual(_read(os.path.join(self.out, rel)), text, rel)
        self.assertFalse(os.path.exists(os.path.join(self.out, "boom")))


class DataHash(Fixture):
    """BA-3: format 1.1's `data_hash`, and 1.0 bundles still read and merge."""

    def downgrade_to_1_0(self):
        path = os.path.join(self.out, content.MANIFEST)
        lines = [ln for ln in _read(path).splitlines() if not ln.startswith("data_hash\t")]
        lines = ["format\t1.0" if ln.startswith("format\t") else ln for ln in lines]
        with open(path, "w") as fh:
            fh.write("\n".join(lines) + "\n")

    def test_data_hash_is_over_sorted_file_records_only(self):
        a = [("b.tsv", 1, "11", []), ("a.tsv", 2, "22", ["res:/x"])]
        b = [("a.tsv", 9, "22", []), ("b.tsv", 3, "11", ["res:/y"])]
        self.assertEqual(content.data_hash(a), content.data_hash(b))
        self.assertNotEqual(content.data_hash(a), content.data_hash([("a.tsv", 2, "23", [])]))

    def test_check_catches_a_data_hash_that_disagrees_with_the_records(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        Merge.rewrite_manifest(self, "data_hash", "0" * 64)
        self.assertEqual(content.check_manifest(self.out),
                         ["%s: data_hash does not match its file records" % content.MANIFEST])

    def test_a_1_0_bundle_checks_and_a_sections_merge_upgrades_it(self):
        self.module("zz", ZZ)
        self.module("yy", YY % 1)
        self.assertEqual(self.run_driver("--sections", "zz,yy"), 0)
        self.downgrade_to_1_0()
        single, files = self.manifest()
        self.assertEqual((single["format"], single.get("data_hash")), ("1.0", None))
        self.assertEqual(content.check_manifest(self.out), [])
        self.assertEqual(content.bundle_data_hash(self.out), content.data_hash(files))
        self.module("yy", YY % 33)
        self.assertEqual(self.run_driver("--sections", "yy"), 0)
        single, files = self.manifest()
        self.assertEqual(single["format"], content.FORMAT)
        self.assertEqual(single["data_hash"], content.data_hash(files))
        self.assertIn("zz/systems.tsv", [f[0] for f in files])
        self.assertEqual(content.check_manifest(self.out), [])

    def test_a_merge_refuses_another_format_major(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        Merge.rewrite_manifest(self, "format", "2.0")
        self.assertEqual(self.run_driver("--sections", "zz"), 2)

    def test_check_against_proves_two_extractions_hold_the_same_data(self):
        self.module("zz", ZZ)
        self.assertEqual(self.run_driver("--sections", "zz"), 0)
        other = os.path.join(self.root, "out", "other")
        # Another build's extraction of the same data: only client_build moves.
        with open(os.path.join(self.client, "start.ini"), "w") as fh:
            fh.write("[main]\nbuild = 1234568\n")
        with redirect_stderr(io.StringIO()):
            self.assertEqual(content.main(["--client", self.client, "--out", other,
                                           "--sections", "zz"], section_dir=self.sections), 0)
        self.assertEqual(content.read_manifest(other)[0]["client_build"], "1234568")
        self.assertEqual(self.run_driver("--check", "--sections", "zz", "--against", other), 0)
        with open(os.path.join(other, "zz", "raw.tsv"), "a") as fh:
            fh.write("kind\t9\n")
        self.assertEqual(self.run_driver("--check", "--sections", "zz", "--against", other), 1)

    def test_against_needs_check(self):
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            content.parse_args(["--against", "x"])


class Builds(unittest.TestCase):
    """BA-3: tools/builds.tsv decides which installs dogma and content accept."""

    REG = ("# comment\n#build\tline\tstatus\tdata_hash\tevidence\n"
           "1000\tL1\tpinned\t-\tpin\n"
           "1001\tL1\tequivalent\t-\tbuild-delta\n"
           "1002\tL1\tunverified\t-\tseen\n")

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.reg = os.path.join(self.tmp.name, "builds.tsv")
        with open(self.reg, "w") as fh:
            fh.write(self.REG)
        self.dogma = content._imp("dogma")

    def tearDown(self):
        self.tmp.cleanup()

    def install(self, build):
        root = os.path.join(self.tmp.name, "client-%s" % build)
        os.makedirs(os.path.join(self.tmp.name, "ResFiles"), exist_ok=True)
        os.makedirs(root)
        with open(os.path.join(root, "start.ini"), "w") as fh:
            fh.write("[main]\nbuild = %s\n" % build)
        with open(os.path.join(root, "resfileindex.txt"), "w") as fh:
            fh.write("")
        return root

    def resolve(self, build, want=None):
        from unittest import mock
        with mock.patch.object(self.dogma, "BUILDS_FILE", self.reg), \
                redirect_stderr(io.StringIO()):
            return self.dogma.resolve_client(self.install(build), want_build=want,
                                             out=io.StringIO())

    def test_the_registry_reads_and_accepts_pinned_and_equivalent(self):
        reg = self.dogma.registry(self.reg)
        self.assertEqual(sorted(reg), ["1000", "1001", "1002"])
        self.assertEqual(reg["1001"]["status"], "equivalent")
        self.assertIsNone(reg["1001"]["data_hash"])
        self.assertEqual(sorted(self.dogma.accepted_builds(self.reg)), ["1000", "1001"])

    def test_no_registry_falls_back_to_the_pin(self):
        missing = os.path.join(self.tmp.name, "none.tsv")
        self.assertEqual(self.dogma.accepted_builds(missing, pin="1000"), ["1000"])

    def test_a_malformed_row_is_an_error(self):
        with open(self.reg, "a") as fh:
            fh.write("1003\tL1\tmaybe\t-\tx\n")
        with self.assertRaises(SystemExit):
            self.dogma.registry(self.reg)

    def test_resolve_accepts_an_equivalent_build_and_refuses_the_rest(self):
        self.assertTrue(self.resolve("1000"))
        self.assertTrue(self.resolve("1001"))
        for build in ("1002", "999"):
            with self.assertRaises(SystemExit) as e:
                self.resolve(build)
            self.assertIn("WRONG BUILD", str(e.exception))

    def test_build_flag_demands_one_build_or_any(self):
        self.assertTrue(self.resolve("1002", want="1002"))
        self.assertTrue(self.resolve("999", want="any"))
        with self.assertRaises(SystemExit):
            self.resolve("1001", want="1000")

    def test_content_passes_build_through(self):
        a = content.parse_args(["--build", "1002"])
        self.assertEqual(a.build, "1002")
        self.assertIsNone(content.parse_args([]).build)

    def test_the_shipped_registry_parses_with_one_pinned_build(self):
        reg = self.dogma.registry()
        self.assertEqual([b for b, e in reg.items() if e["status"] == "pinned"],
                         [self.dogma.pinned_build()])


class Encoding(unittest.TestCase):
    def test_list_map_and_float_round_trip(self):
        for xs in ([], [1], [990001, -2, 3]):
            self.assertEqual(content.decode_list(content.encode(xs)), xs)
        for m in ({}, {990001: 0.1}, {1: 1e-05, 2: -3.0, 3: 1.7976931348623157e308}):
            self.assertEqual(content.decode_map(content.encode(m)), m)
        for f in (0.4567, 1e-05, -3.0, 0.1 + 0.2, 2.0 ** -24, 1e300):
            self.assertEqual(float(content.encode(f)), f)
        self.assertEqual(content.decode_list(content.encode(["a b", "c"]), str), ["a b", "c"])

    def test_scalars(self):
        self.assertEqual(content.encode(None), "")
        self.assertEqual(content.encode(True), "1")
        self.assertEqual(content.encode(False), "0")
        self.assertEqual(content.encode("a\tb\r\nc"), "a b  c")


class Rocks(unittest.TestCase):
    def test_rocks_sort_by_system_then_belt_keeping_in_belt_order(self):
        # (belt_id, ordinal, type_id, count, system); in-belt order deliberately
        # not ascending, so a sort on type_id would show.
        rows = [
            (49000003, 2, 990009, 1, 39000002),
            (49000002, 1, 990005, 4, 39000001),
            (49000001, 0, 990007, 2, 39000001),
            (49000002, 1, 990001, 3, 39000001),
            (49000001, 0, 990002, 5, 39000001),
        ]
        self.assertEqual(content.sort_rocks(rows), [
            (49000001, 0, 990007, 2, 39000001),
            (49000001, 0, 990002, 5, 39000001),
            (49000002, 1, 990005, 4, 39000001),
            (49000002, 1, 990001, 3, 39000001),
            (49000003, 2, 990009, 1, 39000002),
        ])


class ExtractorHash(unittest.TestCase):
    def test_the_hash_changes_when_any_listed_file_changes(self):
        with tempfile.TemporaryDirectory() as d:
            names = ["content.py", "content_zz.py"] + [r + ".py" for r in content.READERS]
            for n in names:
                with open(os.path.join(d, n), "w") as fh:
                    fh.write("# %s\n" % n)
            with open(os.path.join(d, "test_content.py"), "w") as fh:
                fh.write("# not hashed\n")
            base = content.extractor_hash(d)
            seen = {base}
            for n in names:
                with open(os.path.join(d, n), "a") as fh:
                    fh.write("x = 1\n")
                h = content.extractor_hash(d)
                self.assertNotIn(h, seen, n)
                seen.add(h)
            with open(os.path.join(d, "test_content.py"), "a") as fh:
                fh.write("x = 1\n")
            self.assertIn(content.extractor_hash(d), seen)


class SchemaGuard(unittest.TestCase):
    @staticmethod
    def S(stride, delta, fields):
        return types.SimpleNamespace(stride=stride, delta=delta, fields=[
            types.SimpleNamespace(name=n, offset=o) for n, o in fields])

    def test_agreement_is_silent_and_each_disagreement_is_named(self):
        S = self.S
        # The loader's offsets are struct-relative, the TSV's include delta.
        committed = S(24, 8, [("a", 8), ("b", 16)])
        self.assertEqual(content.layout_disagreements(
            "t", S(24, 8, [("a", 0), ("b", 8)]), committed), [])
        bad = content.layout_disagreements(
            "t", S(32, 4, [("a", 8), ("c", 16)]), committed)
        self.assertEqual(len(bad), 5, bad)  # stride, delta, a moved, b gone, c new


class LocatorRows(unittest.TestCase):
    """`locator_rows` takes the dock-locator group and the structure groups (U0)."""

    T_STATION, T_CITADEL, T_ENGCOMPLEX, T_REFINERY, T_SHIP = 990201, 990202, 990203, 990204, 990205
    LOC = {"directionalLocators": [
        {"category": "undockPointSmall", "name": "subcapital1",
         "position": (1.0, 2.0, 3.0), "direction": (0.0, 0.0, 1.0)}], "hull": "h"}

    def _rows(self):
        T = {self.T_STATION: 15, self.T_CITADEL: 1657, self.T_ENGCOMPLEX: 1404,
             self.T_REFINERY: 1406, self.T_SHIP: 25}

        class Fsd:
            CfsdTable = staticmethod(lambda *a: None)
            cfsd_rows = staticmethod(lambda *a: {
                t: {"groupID": g, "graphicID": t} for t, g in T.items()})
            rows = staticmethod(lambda c, n: {t: {"graphicLocationID": t} for t in T})
            graphic_locations = staticmethod(lambda t: {t_: LocatorRows.LOC for t_ in T})

        ctx = types.SimpleNamespace(client=types.SimpleNamespace(path=lambda p: p))
        saved = content._imp
        content._imp = lambda name: Fsd
        try:
            return list(content.locator_rows(ctx))
        finally:
            content._imp = saved

    def test_stations_and_structures_get_rows_ships_do_not(self):
        got = {r[0] for r in self._rows()}
        self.assertEqual(got, {self.T_STATION, self.T_CITADEL, self.T_ENGCOMPLEX,
                               self.T_REFINERY})

    def test_row_format_is_unchanged(self):
        row = [r for r in self._rows() if r[0] == self.T_CITADEL][0]
        self.assertEqual(len(row), 12)
        self.assertEqual(row[3:5], ("undockPointSmall", "subcapital1"))


if __name__ == "__main__":
    unittest.main()
