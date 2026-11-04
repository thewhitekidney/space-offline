#!/usr/bin/env python3
"""Tests for tools/society-report.py (SOC-REPORT): invented log lines only.

    tools/analysis.sh python3 tools/test_society_report.py
"""
import contextlib
import importlib.util
import io
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("society_report", os.path.join(HERE, "society-report.py"))
sr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sr)


def log_of(lines):
    log = sr.Log()
    log.feed(lines)
    return log


def summary(corps=3, al=2, blue=1, red=1, day=None):
    d = f" day {day}" if day is not None else ""
    return f"[soc] bots 9 online 9 mix a=9 switches_per_bot_day 0.10 flap_max 1.00 corps {corps} alliances {al} blue {blue} red {red}{d}"


def stance(o, t, a, b):
    return f"[soc] Bot One corp stance {o} -> {t}: {a} -> {b} (R 5.0)"


def rows_of(res):
    return {n: s for n, s, _ in res[2]}


GC_OK = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1),
         "[soc] relation loss victim=1 killer=2 before=3.0 after=1.5", summary(day=2)]


class Gc(unittest.TestCase):
    def test_pass(self):
        res = sr.gate_gc(log_of(GC_OK))
        self.assertEqual(res[0], sr.PASS, res)

    def test_off_is_na(self):
        self.assertEqual(sr.gate_gc(log_of(["[doc] roll x"]))[0], sr.NA)

    def test_corps_alliances_blue_red_fail(self):
        for kw, name in [({"corps": 2}, "corps"), ({"al": 1}, "alliances"), ({"blue": 0}, "blue pair"), ({"red": 0}, "red pair")]:
            res = sr.gate_gc(log_of(GC_OK[:-1] + [summary(day=2, **kw)] if name in ("corps", "alliances") else
                                    [summary(day=0, **kw), summary(day=1, **kw)] + GC_OK[3:4]))
            self.assertEqual(res[0], sr.FAIL, name)
            self.assertEqual(rows_of(res)[name], sr.FAIL, name)

    def test_peak_blue_counts_even_if_the_last_summary_lost_it(self):
        res = sr.gate_gc(log_of([summary(blue=2), summary(blue=0)]))
        self.assertEqual(rows_of(res)["blue pair"], sr.PASS)

    def test_loss_pass_fail_na(self):
        self.assertEqual(rows_of(sr.gate_gc(log_of(GC_OK)))["loss moves R"], sr.PASS)
        flat = [summary(day=0), "[soc] relation loss victim=1 killer=2 before=3.0 after=3.0"]
        self.assertEqual(rows_of(sr.gate_gc(log_of(flat)))["loss moves R"], sr.FAIL)
        none = rows_of(sr.gate_gc(log_of([summary(day=0)])))
        self.assertEqual(none["loss moves R"], sr.NA)

    def test_na_criterion_means_the_gate_is_not_pass(self):
        res = sr.gate_gc(log_of([summary(day=0), summary(day=1)]))
        self.assertEqual(res[0], sr.NA)

    def test_flap_fail_pass_na_with_a_day_field(self):
        fast = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1), stance(1, 2, "blue", "red"),
                stance(1, 2, "red", "blue"), summary(day=1)]
        res = sr.gate_gc(log_of(fast))
        self.assertEqual(rows_of(res)["no flapping"], sr.FAIL)
        slow = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1), stance(1, 2, "blue", "red"),
                summary(day=3), stance(1, 2, "red", "blue"), summary(day=4)]
        self.assertEqual(rows_of(sr.gate_gc(log_of(slow)))["no flapping"], sr.PASS)

    def test_a_neutral_gap_still_counts_as_a_flip(self):
        lines = [summary(day=1), stance(1, 2, "neutral", "blue"), stance(1, 2, "blue", "neutral"),
                 stance(1, 2, "neutral", "red"), stance(1, 2, "red", "blue"), summary(day=1)]
        self.assertEqual(rows_of(sr.gate_gc(log_of(lines)))["no flapping"], sr.FAIL)

    def test_flap_clock_from_ticks_and_without_any_clock(self):
        lines = [summary(), stance(1, 2, "neutral", "blue"), summary(), stance(1, 2, "blue", "red"),
                 summary(), stance(1, 2, "red", "blue"), summary()]
        log = log_of(lines)
        self.assertEqual(rows_of(sr.gate_gc(log))["no flapping"], sr.NA)
        # 60s ticks, a 600s game day: two flips three ticks apart are inside one day.
        self.assertEqual(rows_of(sr.gate_gc(log, 600.0, 60.0))["no flapping"], sr.FAIL)
        self.assertEqual(rows_of(sr.gate_gc(log, 60.0, 60.0))["no flapping"], sr.PASS)

    def test_no_stance_lines_means_flaps_na_and_gc_not_pass(self):
        lines = [summary(day=0), "[soc] relation loss victim=1 killer=2 before=3.0 after=1.5", summary(day=1)]
        res = sr.gate_gc(log_of(lines))
        self.assertEqual(rows_of(res)["no flapping"], sr.NA)
        self.assertEqual(res[0], sr.NA)

    def test_a_prefixed_line_is_read(self):
        log = log_of(["12:00:01 " + summary(corps=4)])
        self.assertEqual(log.summaries[0][0], 4)


SOV_OK = [
    "[sov] hub 10 launched in 5 by 77 at (0,0,0)",
    "[sov] hub 11 launched in 6 by 78 at (0,0,0)",
    "[sov] hub 10 in 5 captured by alliance 100 (corp 1)",
    "[sov] campaign in 5 begins on hub 10",
    "[sov] command node 3 appears in 5 for the campaign on hub 10",
    "[sov] a node of hub 10 was taken by the Attackers: the bar is X",
    "[sov] hub 12 in 5 captured by alliance 200 (corp 2)",
]


class Gd(unittest.TestCase):
    def test_pass(self):
        res = sr.gate_gd(log_of(SOV_OK))
        self.assertEqual(res[0], sr.PASS, res)

    def test_off_is_na(self):
        self.assertEqual(sr.gate_gd(log_of([summary()]))[0], sr.NA)

    def test_claims_fail(self):
        res = sr.gate_gd(log_of(SOV_OK[:1] + SOV_OK[3:6]))
        self.assertEqual(rows_of(res)["claims"], sr.FAIL)
        self.assertEqual(res[0], sr.FAIL)

    def test_contested_fail_when_no_node_was_taken(self):
        res = sr.gate_gd(log_of([l for l in SOV_OK if "was taken" not in l]))
        self.assertEqual(rows_of(res)["contested campaign"], sr.FAIL)

    def test_a_node_on_another_hub_is_not_a_fight_for_this_campaign(self):
        lines = [l.replace("taken by", "taken by").replace("node of hub 10", "node of hub 99") for l in SOV_OK]
        self.assertEqual(rows_of(sr.gate_gd(log_of(lines)))["contested campaign"], sr.FAIL)

    def test_owner_change_fail_and_na(self):
        same = [l.replace("alliance 200", "alliance 100") for l in SOV_OK]
        self.assertEqual(rows_of(sr.gate_gd(log_of(same)))["change of owner"], sr.FAIL)
        nocap = [l for l in SOV_OK if "captured" not in l]
        res = sr.gate_gd(log_of(nocap))
        self.assertEqual(rows_of(res)["change of owner"], sr.NA)
        self.assertEqual(res[0], sr.NA)

    def test_owner_change_is_per_system(self):
        lines = ["[sov] hub 1 in 5 captured by alliance 100 (corp 1)", "[sov] hub 2 in 6 captured by alliance 200 (corp 2)"]
        self.assertEqual(sr.owner_changes([("1", "5", "100"), ("2", "6", "200")]), 0)
        self.assertEqual(sr.owner_changes([("1", "5", "100"), ("2", "5", "200")]), 1)
        self.assertEqual(len(log_of(lines).captured), 2)


class Report(unittest.TestCase):
    def run_main(self, bots, server, *extra):
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for name, lines in (("bots.log", bots), ("server.log", server)):
                p = os.path.join(d, name)
                with open(p, "w") as f:
                    f.write("\n".join(lines) + "\n")
                paths.append(p)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = sr.main(paths + list(extra))
            return code, out.getvalue()

    def test_exit_by_fail_and_both_gates_listed(self):
        code, text = self.run_main(GC_OK, SOV_OK)
        self.assertEqual(code, 0, text)
        self.assertIn("GC  PASS", text)
        self.assertIn("GD  PASS", text)
        code, text = self.run_main(GC_OK, SOV_OK[:1])
        self.assertEqual(code, 1)
        self.assertIn("GD  FAIL", text)

    def test_na_only_exits_zero(self):
        code, text = self.run_main(["nothing"], ["nothing"])
        self.assertEqual(code, 0)
        self.assertIn("GC  n/a", text)
        self.assertIn("0 PASS, 0 FAIL, 2 n/a", text)

    def test_json(self):
        code, text = self.run_main(GC_OK, SOV_OK, "--json")
        self.assertEqual(code, 0)
        self.assertIn('"GC"', text)


if __name__ == "__main__":
    unittest.main()
