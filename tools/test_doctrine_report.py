#!/usr/bin/env python3
"""Tests for tools/doctrine-report.py (DOC-9): invented lines only.

    tools/analysis.sh python3 -I tools/test_doctrine_report.py
"""
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("doctrine_report", os.path.join(HERE, "doctrine-report.py"))
dr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dr)

K1 = "armour/short/brawl/cruiser/hybrid"
K2 = "shield/long/kite/cruiser/missile"
K3 = "shield/medium/skirmish/frigate/laser"


def roll(groups, span=6.0, switches=4, flaps=0, min_gap="2.0", fighters=20, committed=8):
    """One `[doc] roll` block: `groups` is [(group, key)]."""
    lines = [
        f"[doc] roll group={g} current={k} since=1.5 fancy=armour/short/hybrid/cruiser switches=1 flaps=0 "
        f"min_gap=- fighters=5 theatres=0 committed=0 open=-"
        for g, k in groups
    ]
    heads = {dr.layer_range(k) for _, k in groups}
    lines.append(
        f"[doc] roll-all leaders={len(groups)} doctrines={len({k for _, k in groups})} layer_range={len(heads)} "
        f"switches={switches} flaps={flaps} min_gap={min_gap} span={span} fighters={fighters} "
        f"theatres=1 committed={committed} calls=0"
    )
    return lines


def log_of(lines):
    log = dr.Log()
    log.feed(lines)
    return log


class Parse(unittest.TestCase):
    def test_tokens_and_prefixes(self):
        self.assertEqual(dr.kv("[doc] roll group=7 current=a/b open=watch,commit")["open"], "watch,commit")
        log = log_of(["Bot12 [doc] roll group=1 current=x since=1.0", "noise", "[soc] bots 3 online 3 switches_per_bot_day 0.10 flap_max 0.50"])
        self.assertEqual(len(log.soc), 1)
        self.assertEqual(log.last_roll(), None, "a roll block is closed by its roll-all")

    def test_a_block_is_its_leaders_then_the_total(self):
        log = log_of(roll([(1, K1), (2, K2)]) + roll([(1, K1), (2, K3)], span=9.0))
        self.assertEqual(len(log.roll), 2)
        self.assertEqual([l["current"] for l in log.last_roll()["leaders"]], [K1, K3])
        self.assertEqual(log.last_roll()["all"]["span"], "9.0")

    def test_the_lead_line_decisions(self):
        first = '[doc] lead group=98000001 current=- fancy=armour/short/hybrid/cruiser decision=First { key: "%s" }' % K1
        sw = '[doc] lead group=98000001 current=%s fancy=armour/short/hybrid/cruiser decision=Switch { from: "%s", to: "%s" } can=1.00' % (K1, K1, K2)
        log = log_of([first, sw, "[doc] lead group=98000001 current=x fancy=a/b/c/d decision=Hold"])
        self.assertEqual(log.leads[0][1]["_first"], K1)
        self.assertEqual(log.leads[1][1]["_switch"], (K1, K2))
        self.assertIsNone(log.leads[2][1]["_first"])

    def test_fancy_is_layer_range_family_class(self):
        self.assertTrue(dr.fancy_matches("armour/short/hybrid/cruiser", K1))
        self.assertFalse(dr.fancy_matches("armour/short/laser/cruiser", K1))
        self.assertFalse(dr.fancy_matches("bad", K1))


class Gates(unittest.TestCase):
    def test_g1_diverse_after_three_days(self):
        ok = log_of(roll([(1, K1), (2, K2), (3, K3)]))
        self.assertEqual(dr.gate_g1(ok)[0], dr.PASS)
        same = log_of(roll([(1, K1), (2, K1), (3, K1)]))
        self.assertEqual(dr.gate_g1(same)[0], dr.FAIL)
        short = log_of(roll([(1, K1), (2, K1), (3, K1)], span=1.0))
        self.assertEqual(dr.gate_g1(short)[0], dr.NA, "too short to judge")
        self.assertEqual(dr.gate_g1(dr.Log())[0], dr.NA)

    def test_g2_first_choice_follows_fancy(self):
        mk = lambda g, key: f'[doc] lead group={g} current=- fancy=armour/short/hybrid/cruiser decision=First {{ key: "{key}" }}'
        good = log_of([mk(1, K1), mk(2, K1), mk(3, K1), mk(4, K2)])
        s, d = dr.gate_g2(good)
        self.assertEqual(s, dr.PASS, d)
        bad = log_of([mk(1, K2), mk(2, K2), mk(3, K1)])
        self.assertEqual(dr.gate_g2(bad)[0], dr.FAIL)
        self.assertEqual(dr.gate_g2(dr.Log())[0], dr.NA)

    def test_g3_rate_gap_and_flaps(self):
        s, d = dr.gate_g3(log_of(roll([(1, K1), (2, K2)], span=14.0, switches=4)), 1.0)
        self.assertEqual(s, dr.PASS, d)  # 4 / 2 / 2 weeks = 1.0
        self.assertEqual(dr.gate_g3(log_of(roll([(1, K1), (2, K2)], span=14.0, switches=4, flaps=1)), 1.0)[0], dr.FAIL)
        self.assertEqual(dr.gate_g3(log_of(roll([(1, K1), (2, K2)], span=14.0, switches=4, min_gap="0.4")), 1.0)[0], dr.FAIL)
        self.assertEqual(dr.gate_g3(log_of(roll([(1, K1), (2, K2)], span=14.0, switches=0)), 1.0)[0], dr.FAIL, "frozen is not deliberate")
        self.assertEqual(dr.gate_g3(log_of(roll([(1, K1)], span=1.0)), 1.0)[0], dr.NA)

    def test_g5_reserve(self):
        self.assertEqual(dr.gate_g5(log_of(roll([(1, K1)], fighters=20, committed=10)))[0], dr.PASS)
        self.assertEqual(dr.gate_g5(log_of(roll([(1, K1)], fighters=20, committed=20)))[0], dr.FAIL)

    def test_g7_estimate_error(self):
        lines = ["[doc] battle 1 sys 5 mine=4 est_theirs=10 true_theirs=12", "[doc] battle 2 sys 5 mine=4 est_theirs=9 true_theirs=12"]
        self.assertEqual(dr.gate_g7(log_of(lines))[0], dr.PASS)
        far = ["[doc] battle 1 sys 5 mine=4 est_theirs=2 true_theirs=12"]
        self.assertEqual(dr.gate_g7(log_of(far))[0], dr.FAIL)
        self.assertEqual(dr.gate_g7(dr.Log())[0], dr.NA)

    def test_g8_counter_works(self):
        pair = (
            "[arena] pair a=%s b=%s predicted=B er=+0.80 range=on n=%d wins_predicted=%d wins_other=%d draws=0 "
            "win_rate=0.80 p_chance=0.05 isk_ratio_median=%s by_damage=0 secs_median=80.0"
        )
        good = log_of([pair % (K1, K2, 10, 8, 2, "2.40"), pair % (K2, K3, 10, 9, 1, "inf"), "[arena] summary pairs=2 fights=20 range=on counter_works=2 median_of_medians=inf"])
        s, d = dr.gate_g8(good, 20)
        self.assertEqual(s, dr.PASS, d)
        self.assertNotIn("small sample", d)
        small = log_of([pair % (K1, K2, 2, 2, 0, "1.50")])
        s, d = dr.gate_g8(small, 20)
        self.assertEqual(s, dr.PASS)
        self.assertIn("small sample", d)
        bad = log_of([pair % (K1, K2, 10, 3, 7, "0.40")])
        self.assertEqual(dr.gate_g8(bad, 20)[0], dr.FAIL)
        self.assertEqual(dr.gate_g8(dr.Log(), 20)[0], dr.NA)

    def test_g9_the_ladder_is_monotone(self):
        def run(err, sw):
            lines = [f"[doc] battle 1 sys 5 mine=4 est_theirs={12 * (1 - err):.2f} true_theirs=12"]
            lines += roll([(1, K1), (2, K2)], span=14.0, switches=sw)
            return log_of(lines)

        good = {1: run(0.5, 1), 2: run(0.3, 3), 4: run(0.1, 6)}
        s, d = dr.gate_g9(good)
        self.assertEqual(s, dr.PASS, d)
        bad = {1: run(0.1, 1), 4: run(0.5, 6)}
        s, d = dr.gate_g9(bad)
        self.assertEqual(s, dr.FAIL, d)
        self.assertEqual(dr.gate_g9({2: run(0.3, 3)})[0], dr.NA)

    def test_g10_hold(self):
        ok = log_of(["[turns] secs=10.0 turns=112 hold_mean=0.1ms hold_max=12.5ms wait_mean=0.0ms"])
        self.assertEqual(dr.gate_g10(ok)[0], dr.PASS)
        slow = log_of(["[turns] secs=10.0 hold_max=40.0ms"])
        self.assertEqual(dr.gate_g10(slow)[0], dr.FAIL)
        self.assertEqual(dr.gate_g10(dr.Log())[0], dr.NA)

    def test_the_report_lists_all_ten_and_exits_by_fail(self):
        res = dr.judge(log_of(roll([(1, K1), (2, K1), (3, K1)])), dr.Log(), {})
        text = dr.render(res)
        for gid, _ in dr.GATES:
            self.assertIn(gid, text)
        self.assertIn("G1  FAIL", text)
        self.assertIn("gates:", text)
        self.assertEqual(len(res), 10)


if __name__ == "__main__":
    unittest.main()
