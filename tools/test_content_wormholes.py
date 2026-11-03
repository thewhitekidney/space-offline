"""Unit tests for `tools/content_wormholes.py` (WH-0). Invented rows only.

Run: `python3 -I tools/test_content_wormholes.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_wormholes as W  # noqa: E402

S1, S2, S3, OTHER, WRECK = 990801, 990802, 990803, 990804, 990805


def trow(tid, group, effects="1,2", attrs="9=1.5"):
    f = [""] * 13
    f[0], f[1], f[2], f[11], f[12] = "type", str(tid), str(group), effects, attrs
    return f


def needs_of(attrs, effects):
    return ["TURRET"] if effects else []


def role_of(needs):
    return "DPS"


class Derive(unittest.TestCase):
    def test_rows_comments_and_header(self):
        rows = [trow(S2, 984), trow(S1, 959), trow(OTHER, 1), trow(S3, 987, effects="")]
        lines = W.derive(rows, {S1: WRECK}, needs_of, role_of)
        self.assertEqual(lines[:len(W.SLEEPER_COMMENTS)], list(W.SLEEPER_COMMENTS))
        self.assertEqual(lines[len(W.SLEEPER_COMMENTS)], W.SLEEPER_HEADER)
        data = [l.split("\t") for l in lines[len(W.SLEEPER_COMMENTS) + 1:]]
        self.assertEqual([r[0] for r in data], [str(S1), str(S2)])
        self.assertEqual(data[0][1:7], ["959", "Battleship", "sleepless", "TURRET", "DPS", str(WRECK)])
        self.assertEqual(data[1][2:4], ["Cruiser", "awakened"])
        self.assertEqual(data[1][6], "")

    def test_short_and_non_type_rows_are_skipped(self):
        lines = W.derive([["group", "1"], ["type", str(S1), "959"]], {}, needs_of, role_of)
        self.assertEqual(len(lines), len(W.SLEEPER_COMMENTS) + 1)

    def test_wrecks_of(self):
        row = [str(S1)] + [""] * 10 + [str(WRECK)]
        self.assertEqual(W.wrecks_of([row, [str(S2)] + [""] * 11, ["x"]]), {S1: WRECK})


NAMES = {
    1: "Quiet Perimeter Reservoir", 2: "Big Frontier Reservoir", 3: "Odd Core Reservoir",
    4: "Plain Frontier Deposit", 5: "Plain Perimeter Deposit OLD", 6: "Forgotten Core Cellar",
    7: "Unsecured Perimeter Booth", 8: "Unsecured Core Booth", 9: "Alpha Outpost",
    10: "Beta Outpost", 11: "Plain Core Reservoir", 12: "Forgotten Frontier Vault",
    13: "Veldspar Pile", 14: "Unsecured Frontier Booth",
}


def text(i):
    return NAMES.get(i, "") if i is not None else ""


def drow(did, arch, diff, name_id, faction="500021"):
    return [str(did), str(arch), faction, str(diff), "", "", "", str(name_id), ""]


COMBAT = {"Alpha Outpost": 2, "Beta Outpost": 5, "Ghost Site": 3}


def sites(rows, combat=COMBAT):
    lines, problems = W.derive_sites(rows, text, combat)
    data = [l.split("\t") for l in lines[len(W.SITES_COMMENTS) + 1:]]
    return lines, problems, {int(r[0]): r[1:] for r in data}


class DeriveSites(unittest.TestCase):
    def setUp(self):
        self.rows = [
            drow(990901, 30, 3, 1), drow(990902, 30, 4, 2), drow(990903, 30, 5, 3),
            drow(990904, 27, 4, 4), drow(990905, 27, 3, 5), drow(990906, 44, 5, 6),
            drow(990907, 45, 3, 7), drow(990908, 45, 5, 8), drow(990909, 24, 3, 9),
            drow(990910, 24, 5, 10), drow(990911, 24, 3, 7),
        ]

    def test_header_and_comments(self):
        lines, _, _ = sites(self.rows)
        self.assertEqual(lines[:len(W.SITES_COMMENTS)], list(W.SITES_COMMENTS))
        self.assertEqual(lines[len(W.SITES_COMMENTS)], W.SITES_HEADER)
        self.assertFalse(any("EvEJS" in l for l in lines))

    def test_gas_and_ore_by_stem(self):
        _, _, s = sites(self.rows)
        self.assertEqual(s[990901][:3], ["1", "6", "gas"])
        self.assertEqual(s[990902][:3], ["3", "6", "gas"])
        self.assertEqual(s[990903][:3], ["5", "6", "gas"])
        self.assertEqual(s[990904][:3], ["3", "6", "ore"])

    def test_old_name_is_dropped(self):
        _, _, s = sites(self.rows)
        self.assertNotIn(990905, s)

    def test_relic_and_data_pairs(self):
        _, _, s = sites(self.rows)
        self.assertEqual(s[990906][:3], ["5", "6", "relic"])
        self.assertEqual(s[990907][:3], ["1", "2", "data"])
        self.assertEqual(s[990908][:3], ["5", "6", "data"])
        # a data site of the combat archetype is still data
        self.assertEqual(s[990911][2], "data")

    def test_combat_by_name_table(self):
        _, _, s = sites(self.rows)
        self.assertEqual(s[990909][:3], ["2", "2", "combat"])
        self.assertEqual(s[990910][:3], ["5", "5", "combat"])

    def test_unmatched_combat_name_is_a_problem(self):
        _, problems, _ = sites(self.rows)
        self.assertEqual(len(problems), 1)
        self.assertIn("Ghost Site", problems[0])

    def test_filters(self):
        rows = [drow(990921, 30, 2, 1), drow(990922, 30, 3, 1, faction="500010"),
                drow(990923, 20, 3, 1), drow(990925, 30, 3, 13), drow(990926, 30, 3, 99), ["short"]]
        _, _, s = sites(rows, {})
        self.assertEqual(s, {})

    def test_stem_decides_even_when_difficulty_differs(self):
        _, _, s = sites([drow(990931, 27, 3, 4)], {})
        self.assertEqual(s[990931][:3], ["3", "6", "ore"])

    def test_sorted_by_id(self):
        lines, _, _ = sites(list(reversed(self.rows)))
        ids = [int(l.split("\t")[0]) for l in lines[len(W.SITES_COMMENTS) + 1:]]
        self.assertEqual(ids, sorted(ids))


class ParseCombat(unittest.TestCase):
    def test_rows(self):
        got = W.parse_combat(["# c", "name\tclass\twhy", "Alpha Outpost\t2\t[A] x", ""])
        self.assertEqual(got, {"Alpha Outpost": 2})

    def test_bad_inputs(self):
        for lines in (["name\ttier"], ["name\tclass\twhy", "A\t7\t[A]"],
                      ["name\tclass\twhy", "A\tx\t[A]"],
                      ["name\tclass\twhy", "A\t1\t[A]", "A\t2\t[A]"],
                      ["name\tclass\twhy", "A"]):
            with self.assertRaises(ValueError):
                W.parse_combat(lines)

    def test_shipped_file_parses_and_every_row_is_assumed(self):
        with open(W.COMBAT_FILE, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
        self.assertTrue(W.parse_combat(lines))
        for l in lines:
            if l and not l.startswith("#") and not l.startswith("name\t"):
                self.assertIn("[A]", l.split("\t")[2])


class Wiring(unittest.TestCase):
    def test_section_contract(self):
        self.assertEqual(W.SECTION, "wormholes")
        self.assertIn("types", W.DEPENDS)
        self.assertIn("dungeons", W.DEPENDS)
        self.assertTrue(callable(W.section) and callable(W.check))


if __name__ == "__main__":
    unittest.main()
