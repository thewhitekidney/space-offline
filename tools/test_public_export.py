"""Tests for tools/public-export.py: the scanner and the exported-tree helpers.

Invented strings only; every case builds a throwaway tree. Run::

    python3 -I tools/test_public_export.py
"""

import contextlib
import importlib.util
import io
import os
import shutil
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("public_export", os.path.join(HERE, "public-export.py"))
pe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pe)


class Scan(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_public_export-")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, data):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode("utf-8"))

    def scan(self, *pats):
        self.put("private/export-scan.txt", "# c\n" + "\n".join(pats) + "\n")
        return pe.leaks(self.root, pe.exported(self.root))

    def test_a_non_utf8_file_is_scanned_as_latin1(self):
        self.put("a.bin", b"\xff\xfe zzquux \x80")
        self.assertEqual(self.scan(r"zzquux"), ["a.bin:1: matches /zzquux/"])

    def test_a_file_name_is_matched(self):
        self.put("docs/zzquux-notes.md", "clean text\n")
        hits = self.scan(r"zzquux")
        self.assertEqual(hits, ["docs/zzquux-notes.md: path matches /zzquux/"])

    def test_a_clean_tree_has_no_hits(self):
        self.put("a.md", "nothing here\n")
        self.assertEqual(self.scan(r"zzquux"), [])

    def test_case_insensitive_and_spelling_variants(self):
        self.put("a.md", "Host ZZHOST\nname Fjoelzzz\n")
        hits = self.scan(r"(?i)\bzzhost\b", r"(?i)fj(ø|o|oe)lzzz")
        self.assertEqual(len(hits), 2, hits)

    def test_built_in_home_path_still_matches_but_invented_users_do_not(self):
        home = "/" + "home/"
        self.put("a.md", home + "zed/x\n" + home + "alice/x\n")
        self.assertEqual(self.scan(r"zzquux"), ["a.md:1: matches /" + pe.BUILT_IN[0] + "/"])


OS = "open" + " source"  # split, so this file does not trip its own check


class Policy(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_public_export-")
        for rel in pe.REQUIRED:
            self.put(rel, "invented\n")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, text):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)

    def check(self):
        return pe.policy(self.root, pe.exported(self.root))

    def test_a_clean_tree_passes(self):
        self.put("README.md", "Source available. No money, ever.\n")
        self.assertEqual(self.check(), [])

    def test_a_missing_licence_file_fails(self):
        os.remove(os.path.join(self.root, "LICENSE.md"))
        self.assertEqual(self.check(), ["LICENSE.md: missing from the export (the licence must ship)"])

    def test_missing_contributor_terms_and_notice_fail(self):
        os.remove(os.path.join(self.root, "CONTRIBUTING-terms.md"))
        os.remove(os.path.join(self.root, "NOTICE"))
        self.assertEqual(len(self.check()), 2)

    def test_a_licence_file_kept_private_fails(self):
        self.put("private/export-private-paths.txt", "LICENSE.md\n")
        self.assertEqual(len(self.check()), 1)

    def test_a_funding_file_anywhere_fails(self):
        self.put(".github/FUNDING.yml", "invented: x\n")
        self.put("private/sub/funding.yaml", "invented: x\n")
        hits = self.check()
        self.assertEqual(len(hits), 2, hits)
        self.assertTrue(all("funding file" in h for h in hits))

    def test_calling_the_project_open_source_fails(self):
        self.put("README.md", "An " + OS + " server.\nIt is OSI approved.\nAn " + OS.replace(" ", "-") + " tool\n")
        self.assertEqual(len(self.check()), 3)

    def test_quoted_and_negated_mentions_pass(self):
        self.put("docs/a.md", 'Their README says "fully ' + OS + '".\n'
                 "Leave the `" + OS.replace(" ", "-") + "` topic unset.\n"
                 "Source available, not " + OS + ".\n"
                 "This is not an OSI licence.\n")
        self.assertEqual(self.check(), [])

    def test_open_source_claim_fails_main_check(self):
        self.put("README.md", "An " + OS + " server.\n")
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(pe.main(["--root", self.root, "--check"]), 1)
        self.assertIn("README.md:1:", out.getvalue())


class Docs(unittest.TestCase):
    PRIV = ["docs/secret/", "docs/hidden.md"]

    def test_filter_index_drops_private_rows_only(self):
        text = ("| path | role |\n|---|---|\n| `docs/a.md` | GUIDE |\n"
                "| `docs/hidden.md` | PLAN |\n| `docs/secret/x.md` | PLAN |\n"
                "| `docs/guides/b.md` | other mentions docs/hidden.md |\n\nprose\n")
        out = pe.filter_index(text, self.PRIV).split("\n")
        self.assertIn("| `docs/a.md` | GUIDE |", out)
        self.assertIn("| `docs/guides/b.md` | other mentions docs/hidden.md |", out)
        self.assertEqual([l for l in out if "hidden.md`" in l or "secret" in l], [])

    def test_mentions_lists_a_surviving_prose_mention(self):
        root = tempfile.mkdtemp(prefix="test_public_export-")
        try:
            os.makedirs(os.path.join(root, "private"))
            os.makedirs(os.path.join(root, "docs"))
            with open(os.path.join(root, "private/export-private-paths.txt"), "w") as f:
                f.write("docs/hidden.md\ndocs/secret/\ntools/other.sh\n")
            with open(os.path.join(root, "docs/a.md"), "w") as f:
                f.write("see docs/hidden.md.\nand docs/secret/x.md and docs/public.md\n")
            got = pe.mentions(root, ["docs/a.md"])
            self.assertEqual(got, ["docs/a.md:1: mention: docs/hidden.md",
                                   "docs/a.md:2: mention: docs/secret/x.md"])
        finally:
            shutil.rmtree(root)

    def test_out_writes_the_prefix_file_and_filters_the_index(self):
        root = tempfile.mkdtemp(prefix="test_public_export-")
        try:
            def put(rel, text):
                p = os.path.join(root, rel)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "w") as f:
                    f.write(text)
            put("private/export-private-paths.txt", "docs/hidden.md\ntools/other.sh\n")
            put("docs/README.md", "| `docs/a.md` | x |\n| `docs/hidden.md` | y |\n")
            put("docs/a.md", "a\n")
            put("docs/hidden.md", "h\n")
            for rel in pe.REQUIRED:
                put(rel, "invented\n")
            out = os.path.join(root, "out-tree")
            self.assertEqual(pe.main(["--root", root, "--out", out]), 0)
            self.assertFalse(os.path.exists(os.path.join(out, "docs/hidden.md")))
            with open(os.path.join(out, "docs/README.md")) as f:
                self.assertNotIn("hidden", f.read())
            with open(os.path.join(out, "tools/doc-private-prefixes.txt")) as f:
                lines = [l.strip() for l in f if not l.startswith("#") and l.strip()]
            self.assertEqual(lines, ["docs/hidden.md"])
        finally:
            shutil.rmtree(root)


if __name__ == "__main__":
    unittest.main()
