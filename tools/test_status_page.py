"""Tests for tools/status-page.py, on invented rows.

The last class checks the real table, so the gate fails when
`docs/status/progress.tsv` and the page beside it drift. `tools/gate.sh` runs
every `tools/test_*.py`, which is all the wiring `--check` needs.

Run::

    python3 -I tools/test_status_page.py
"""

from __future__ import annotations

import importlib.util
import os
import re
import tempfile
import unittest
import xml.dom.minidom

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

_spec = importlib.util.spec_from_file_location("status_page", os.path.join(HERE, "status-page.py"))
sp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sp)

THEMES = {"3.1": "Prove things", "3.2": "Build things"}
KNOWN = {"fci_alpha", "fci_beta", "fci_gamma", "smoke_row"}


def row(feature="A thing", state="client-proved", evidence="fci_alpha W2", system="Space", issue="", theme="", note=""):
    return {
        "system": system, "feature": feature, "state": state, "evidence": evidence,
        "issue": issue, "theme": theme, "note": note, "line": 1,
    }


def sample():
    return [
        row("One", "client-proved"),
        row("Two", "done", "tools/thing.sh", system="Tools"),
        row("Three", "built-unproved", "fci_beta red in W2", theme="3.1"),
        row("Four", "partial", "half of it", theme="3.2", system="Tools"),
        row("Five", "stubbed", "an empty answer", theme="3.2"),
        row("Six", "missing", "nothing", theme="3.2"),
        row("Seven", "unknown", "no record", theme="3.1"),
        row("Eight", "parked", "owner", issue="#7", system="Tools", theme="3.2"),
    ]


WAVES = [
    {"wave": "1", "date": "2000-01-01", "sha": "abc1234", "rows": "10", "green": "4", "red": "1",
     "no_verdict": "5", "scope": "partial", "note": "Stopped early"},
    {"wave": "2", "date": "2000-01-02", "sha": "", "rows": "12", "green": "11", "red": "1",
     "no_verdict": "0", "scope": "full", "note": ""},
]


class Counting(unittest.TestCase):
    def test_parked_rows_stay_out_of_the_percentages(self):
        h = sp.headline(sample())
        self.assertEqual(h["counted"], 7)
        self.assertEqual(h["parked"], 1)
        self.assertEqual((h["proved"], h["built"]), (2, 3))
        self.assertEqual((h["proved_pct"], h["built_pct"]), (29, 43))

    def test_every_state_but_parked_has_a_group(self):
        self.assertEqual(set(sp.GROUP_OF) | {"parked"}, set(sp.STATES))

    def test_an_empty_table_is_zero_not_a_crash(self):
        self.assertEqual(sp.headline([])["proved_pct"], 0)


class Validation(unittest.TestCase):
    def check(self, *rows):
        return sp.validate(list(rows), KNOWN, THEMES)

    def test_sound_rows_pass(self):
        self.assertEqual(sp.validate(sample(), KNOWN, THEMES), [])

    def test_a_bad_state_fails(self):
        self.assertTrue(self.check(row(state="nearly")))

    def test_client_proved_needs_a_scenario(self):
        self.assertTrue(self.check(row(evidence="I saw it work")))

    def test_a_census_counts_as_client_evidence(self):
        self.assertEqual(self.check(row(evidence="a census of the build")), [])

    def test_an_unknown_scenario_fails(self):
        self.assertTrue(self.check(row(evidence="fci_nowhere W2")))

    def test_a_scenario_label_is_allowed(self):
        self.assertEqual(self.check(row(evidence="fci_alpha@on W2")), [])

    def test_a_theme_must_be_in_the_plan(self):
        self.assertTrue(self.check(row(state="missing", evidence="x", theme="9.9")))

    def test_a_finished_row_has_no_theme(self):
        self.assertTrue(self.check(row(theme="3.1")))

    def test_the_same_feature_twice_fails(self):
        self.assertTrue(self.check(row(), row()))

    def test_an_issue_reads_hash_n(self):
        self.assertTrue(self.check(row(issue="12")))

    def test_a_client_id_is_refused(self):
        self.assertTrue(self.check(row(note="type 123456789")))


class Audit(unittest.TestCase):
    def run_audit(self, rows, latest, listed=()):
        return sp.audit(rows, KNOWN, latest, list(listed))

    def test_proved_with_a_newer_red_is_flagged(self):
        self.assertTrue(self.run_audit([row()], {"fci_alpha": ("3", False)}))

    def test_unproved_with_a_green_is_flagged(self):
        r = row(state="built-unproved", evidence="fci_beta red in W2", theme="3.1")
        self.assertTrue(self.run_audit([r], {"fci_beta": ("3", True)}))

    def test_an_older_green_wave_in_the_evidence_is_not_a_finding(self):
        self.assertEqual(self.run_audit([row()], {"fci_alpha": ("w14", True)}), [])

    def test_proved_with_no_verdict_is_flagged(self):
        self.assertTrue(self.run_audit([row()], {}))

    def test_a_stub_with_a_green_is_silent(self):
        r = row(state="stubbed", evidence="fci_beta proves the empty answer", theme="3.1")
        self.assertEqual(self.run_audit([r], {"fci_beta": ("3", True)}), [])

    def test_regress_dirs_sort_by_mtime_and_take_every_run(self):
        with tempfile.TemporaryDirectory() as d:
            for name, when, code in (("w14", 2000, "0"), ("wave11", 1000, "1"), ("owed1103a", 3000, "245")):
                os.makedirs(os.path.join(d, f"regress-{name}"))
                path = os.path.join(d, f"regress-{name}/regress.tsv")
                with open(path, "w", encoding="utf-8") as f:
                    f.write(f"fci_alpha\t{code}\n")
                os.utime(path, (when, when))
            runs = sp.regress_dirs(d)
            self.assertEqual([n for n, _ in runs], ["wave11", "w14", "owed1103a"])
            self.assertEqual(sp.newest_verdicts(runs), {"fci_alpha": ("w14", True)})

    def test_agreement_is_silent(self):
        self.assertEqual(self.run_audit([row()], {"fci_alpha": ("2", True)}), [])

    def test_a_row_no_feature_cites_is_flagged(self):
        self.assertTrue(self.run_audit([row()], {"fci_alpha": ("2", True)}, ["fci_gamma"]))

    def test_a_scenario_with_a_verdict_and_no_row_is_flagged(self):
        found = self.run_audit([row()], {"fci_alpha": ("2", True), "fci_gamma": ("2", True)})
        self.assertEqual(len(found), 1)
        self.assertIn("fci_gamma", found[0])

    def test_a_verdict_for_a_non_feature_is_silent(self):
        name = sorted(sp.NOT_FEATURES)[0]
        self.assertEqual(self.run_audit([row()], {"fci_alpha": ("2", True), name: ("2", True)}), [])

    def test_no_verdict_does_not_replace_a_verdict(self):
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for wave, code in (("1", "0"), ("2", "245")):
                path = os.path.join(d, wave)
                with open(path, "w", encoding="utf-8") as f:
                    f.write(f"fci_alpha\t{code}\n")
                paths.append((wave, path))
            self.assertEqual(sp.newest_verdicts(paths), {"fci_alpha": ("1", True)})


class Output(unittest.TestCase):
    def test_every_svg_parses_and_carries_both_themes(self):
        for image in (
            sp.overall_svg(sample()), sp.systems_svg(sample()),
            sp.left_svg(sample(), THEMES), sp.regress_svg(WAVES),
        ):
            xml.dom.minidom.parseString(image)
            self.assertIn("prefers-color-scheme:dark", image)
            self.assertIn('class="bg"', image)

    def test_an_svg_stays_inside_its_width(self):
        for image in (sp.overall_svg(sample()), sp.systems_svg(sample())):
            for x, w in re.findall(r'<rect x="([\d.]+)" y="\d+" width="([\d.]+)"', image):
                self.assertLessEqual(float(x) + float(w), sp.WIDTH)

    def test_the_readme_says_what_the_table_says(self):
        text = sp.readme(sample(), WAVES, THEMES, "abc1234")
        self.assertIn("**29% proved** (2 of 7", text)
        self.assertIn("**43% built** (3 of 7", text)
        self.assertIn("- [x] One", text)
        self.assertIn("- [ ] Six", text)
        self.assertIn("### 3.2 Build things", text)
        self.assertIn("- Eight: owner", text)
        self.assertNotIn("#7", text)
        self.assertIn("Stopped early", text)

    def test_issues_render_only_with_eve_issues_url(self):
        old = os.environ.pop("EVE_ISSUES_URL", None)
        try:
            self.assertEqual(sp.issue_text("#7, #8"), "")
            os.environ["EVE_ISSUES_URL"] = "https://tracker.invalid/issues/"
            self.assertEqual(sp.issue_text("#7, #8"),
                             "[#7](https://tracker.invalid/issues/7), [#8](https://tracker.invalid/issues/8)")
            self.assertIn("(https://tracker.invalid/issues/7)", sp.readme(sample(), WAVES, THEMES, "abc1234"))
        finally:
            os.environ.pop("EVE_ISSUES_URL", None)
            if old is not None:
                os.environ["EVE_ISSUES_URL"] = old

    def test_an_open_row_needs_a_theme(self):
        self.assertTrue(sp.validate([row("Odd", "missing", "nothing")], KNOWN, THEMES))

    def test_two_issues_are_allowed(self):
        self.assertEqual(sp.validate([row(issue="#1, #2")], KNOWN, THEMES), [])

    def test_a_row_with_no_theme_is_listed_as_unplanned(self):
        rows = [row("Odd", "missing", "nothing")]
        self.assertIn(sp.NO_THEME, sp.readme(rows, [], THEMES, "abc1234"))

    def test_nothing_is_left_when_all_is_proved(self):
        self.assertEqual(sp.left_rows([row()]), [])


class Tables(unittest.TestCase):
    def make_root(self, d, tsv):
        os.makedirs(os.path.join(d, "docs/status"))
        os.makedirs(os.path.join(d, "docs/plans"))
        os.makedirs(os.path.join(d, "tools/client-ci/scenarios"))
        with open(os.path.join(d, "tools/client-ci/scenarios/fci_alpha.py"), "w", encoding="utf-8"):
            pass
        with open(os.path.join(d, "docs/plans/master-plan.md"), "w", encoding="utf-8") as f:
            f.write("### 3.1 Prove things\n")
        with open(os.path.join(d, "docs/status/themes.tsv"), "w", encoding="utf-8") as f:
            f.write("# themes\n3.1\tProve things\n")
        with open(os.path.join(d, "docs/status/progress.tsv"), "w", encoding="utf-8") as f:
            f.write("# checked-at abc1234\n" + "\t".join(sp.COLUMNS) + "\n" + tsv)
        with open(os.path.join(d, "docs/status/waves.tsv"), "w", encoding="utf-8") as f:
            f.write("\t".join(sp.WAVE_COLUMNS) + "\n1\t\t\t1\t1\t0\t0\tfull\t\n")

    GOOD = "Space\tOne\tclient-proved\tfci_alpha W1\t\t\t\n"

    def test_check_goes_red_then_green_then_red_on_a_change(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, self.GOOD)
            self.assertEqual(sp.main(["--check"], d), 1)
            self.assertEqual(sp.main([], d), 0)
            self.assertEqual(sp.main(["--check"], d), 0)
            with open(os.path.join(d, "docs/status/progress.tsv"), "a", encoding="utf-8") as f:
                f.write("Space\tTwo\tmissing\tnothing\t\t3.1\t\n")
            self.assertEqual(sp.main(["--check"], d), 1)

    def test_audit_without_var_skips_green(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, self.GOOD)
            self.assertEqual(sp.main(["--audit", "--var", os.path.join(d, "var")], d), 0)

    def test_audit_no_stamp_leaves_the_table(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, self.GOOD)
            os.makedirs(os.path.join(d, "tools/client-ci"), exist_ok=True)
            with open(os.path.join(d, "tools/client-ci/regress.list"), "w", encoding="utf-8") as f:
                f.write("fci_alpha\n")
            os.makedirs(os.path.join(d, "var/regress-w1"))
            with open(os.path.join(d, "var/regress-w1/regress.tsv"), "w", encoding="utf-8") as f:
                f.write("fci_alpha\t0\n")
            path = os.path.join(d, "docs/status/progress.tsv")
            with open(path, encoding="utf-8") as f:
                before = f.read()
            sp.main(["--audit", "--no-stamp", "--var", os.path.join(d, "var")], d)
            with open(path, encoding="utf-8") as f:
                self.assertEqual(f.read(), before)
            with open(os.path.join(d, "var/regress-w1/regress.tsv"), "a", encoding="utf-8") as f:
                f.write("fci_orphan\t0\n")
            self.assertEqual(sp.main(["--audit", "--no-stamp", "--var", os.path.join(d, "var")], d), 1)

    def test_a_short_row_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, "Space\tOne\n")
            self.assertEqual(sp.main([], d), 1)

    def test_two_builds_give_the_same_bytes(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, self.GOOD)
            self.assertEqual(sp.build(d), sp.build(d))

    def test_waves_keep_a_note_and_never_empty_the_table(self):
        with tempfile.TemporaryDirectory() as d:
            self.make_root(d, self.GOOD)
            path = os.path.join(d, "docs/status/waves.tsv")
            with open(path, "w", encoding="utf-8") as f:
                f.write("\t".join(sp.WAVE_COLUMNS) + "\n1\t2000-01-01\tabc1234\t0\t0\t0\t0\trerun\tKept\n")
            with self.assertRaises(sp.Bad):
                sp.update_waves(d, os.path.join(d, "nowhere"))
            os.makedirs(os.path.join(d, "var/regress-wave1"))
            with open(os.path.join(d, "var/regress-wave1/regress.tsv"), "w", encoding="utf-8") as f:
                f.write("fci_alpha\t0\nfci_beta\t1\nfci_gamma\t245\n")
            sp.update_waves(d, os.path.join(d, "var"))
            rows, _ = sp.read_table(path, sp.WAVE_COLUMNS)
            self.assertEqual(
                [rows[0][c] for c in ("rows", "green", "red", "no_verdict", "scope", "note")],
                ["3", "1", "1", "1", "rerun", "Kept"],
            )


class TheRealPage(unittest.TestCase):
    def test_themes_match_the_plan_headings(self):
        plan = os.path.join(ROOT, "docs/plans/master-plan.md")
        if not os.path.exists(plan):
            self.skipTest("no master plan in this tree")
        with open(plan, encoding="utf-8") as f:
            headings = dict(re.findall(r"^### (3\.\d+) (.+)$", f.read(), re.M))
        self.assertEqual(sp.plan_themes(ROOT), headings)

    def test_the_page_is_current(self):
        files = sp.build(ROOT)  # raises on an unsound row
        self.assertEqual(sp.stale(ROOT, files), [], "run: python3 -I tools/status-page.py")


if __name__ == "__main__":
    unittest.main()
