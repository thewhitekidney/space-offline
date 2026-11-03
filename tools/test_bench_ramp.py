import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("bench_ramp", os.path.join(HERE, "bench_ramp.py"))
br = importlib.util.module_from_spec(spec)
spec.loader.exec_module(br)

LINE = ("[world] tick health: 40 ticks, late mean {lm} ms max {lx} ms, over budget {ob}, "
        "world lock wait mean 0.1 ms max 0.5 ms, world lock held mean {hm} ms max {h} ms, "
        "work max {w} ms, skipped {sk}{tail}")
TAIL = ", work p95 {p} ms, out {of} frames {ob} bytes, destiny {df} frames {db} bytes"


def tick(h=3.0, w=4.0, ob=0, sk=0, late="0.1", tail=True, p=3.5, of=800, nb=409600, df=400, db=204800):
    t = TAIL.format(p=p, of=of, ob=nb, df=df, db=db) if tail else ""
    return LINE.format(lm="0.1", lx=late, ob=ob, hm="2.0", h=h, w=w, sk=sk, tail=t)


def step(idx, bots, lines, pilots=None, **kw):
    live = f"[live] day 1 next-rollover 1h | systems hot 1 warm 0 | pilots {pilots or bots} (in space {bots}) | npcs 0"
    return br.summarize_step(idx, bots, lines + [live], [], {"cpu_pct": 5, "rss_mb": 300, "threads": 100},
                             {"cpu_pct": 2, "rss_mb": 600, "threads": 60}, bots)


class Parse(unittest.TestCase):
    def test_new_and_old_tick_lines(self):
        w = br.parse_ticks([tick(), tick(tail=False)])
        self.assertEqual(len(w), 2)
        self.assertEqual(w[0]["dest_bytes"], 204800.0)
        self.assertIsNone(w[1]["dest_bytes"])

    def test_slice_max_is_optional_and_parsed(self):
        w = br.parse_ticks([tick() + ", slice_max 12.5 ms", tick()])
        self.assertEqual(w[0]["slice_max"], 12.5)
        self.assertIsNone(w[1]["slice_max"])
        r = step(0, 50, [tick() + ", slice_max 12.5 ms", tick() + ", slice_max 3.0 ms"])
        self.assertEqual(r["slice_max_ms"], 12.5)
        self.assertEqual(step(0, 50, [tick()])["slice_max_ms"], 0.0)

    def test_step_rates(self):
        r = step(0, 50, [tick()] * 4)   # 4 windows = 40 s
        self.assertAlmostEqual(r["out_kBps"], 400 / 40.0 * 4 * 1.0, delta=0.01 * 400)  # 400 KB/window / 10 s
        self.assertAlmostEqual(r["destiny_frames_per_tick"], 10.0)
        self.assertAlmostEqual(r["destiny_bytes_per_bot_s"], 20480 / 50)
        self.assertEqual(r["pilots"], 50)


class Scenario(unittest.TestCase):
    def test_split_and_roster(self):
        self.assertEqual(sum(br.split_counts(7, {"miners": 60, "ratters": 25, "fighters": 15}).values()), 7)
        text, names = br.roster(300, br.parse_mix("miners=60,ratters=25,fighters=15"), "same")
        self.assertEqual(len(names), 300)
        self.assertEqual(len(set(names)), 300)
        self.assertEqual(len([l for l in text.splitlines() if not l.startswith("#")]), 3)

    def test_spread_miners_in_fives_and_fleet_cap(self):
        text, names = br.roster(100, {"miners": 1}, "spread")
        self.assertEqual(len(text.splitlines()) - 1, 20)
        text, _ = br.roster(600, {"miners": 1}, "same")
        self.assertEqual(len(text.splitlines()) - 1, 3)   # 250 + 250 + 100

    def test_steps(self):
        self.assertEqual(br.steps_of(50, 50, 300), [50, 100, 150, 200, 250, 300])
        self.assertEqual(br.steps_of(10, 15, 30), [10, 25, 30])

    def test_bad_mix(self):
        with self.assertRaises(ValueError):
            br.parse_mix("pirates=3")


class Knee(unittest.TestCase):
    def rows(self, helds):
        return [step(i, 50 * (i + 1), [tick(h=h, w=h + 1)] * 5) for i, h in enumerate(helds)]

    def test_no_knee_below_half_budget(self):
        k, _ = br.knee(self.rows([3, 20, 60, 120]))
        self.assertIsNone(k)

    def test_knee_on_lock_held_p95(self):
        k, why = br.knee(self.rows([3, 20, 130, 300]))
        self.assertEqual(k["target_bots"], 150)
        self.assertIn("lock held p95", why)

    def test_knee_on_skipped_or_over_budget(self):
        rows = self.rows([3, 3])
        rows.append(step(2, 150, [tick(h=3, ob=1)] * 5))
        k, why = br.knee(rows)
        self.assertEqual(k["target_bots"], 150)
        self.assertIn("over budget", why)
        k, why = br.knee([step(0, 50, [tick(sk=2)] * 3)])
        self.assertIn("skipped", why)

    def test_collapse(self):
        ok = step(0, 50, [tick()] * 5)
        self.assertEqual(br.collapse([ok]), "")
        bad = step(1, 100, [tick(ob=3)] * 5)
        self.assertIn("consecutive", br.collapse([ok, bad]))
        gone = step(1, 100, [tick()] * 5, pilots=40)
        self.assertIn("disconnecting", br.collapse([ok, gone]))
        some = step(1, 100, [tick(ob=3), tick(), tick(ob=3), tick()])
        self.assertEqual(br.collapse([ok, some]), "")

    def test_summary_text(self):
        rows = self.rows([3, 130])
        text = "\n".join(br.final_summary(rows, "stopped"))
        self.assertIn("KNEE at 100 bots", text)
        self.assertIn("STOPPED EARLY", text)


if __name__ == "__main__":
    unittest.main()
