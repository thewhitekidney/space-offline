"""Unit tests for `tools/content_geodes.py` (L6A GEODES).

Invented rows only (ids in the 99xxxx band). The bundle-backed run is the
`geodes` section of `tools/content.py` (`tools/up.sh`).
Run: `python3 -I tools/test_content_geodes.py -q`.
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

import content_geodes as G  # noqa: E402

A, B, C, D, E, GONE = 990701, 990702, 990703, 990704, 990705, 990706
NAMES = {1: "Alpha Box", 2: "Beta Box", 3: "Gamma Box", 4: "Alpha Box", 5: "Fixed Box"}


def text(i):
    return NAMES.get(i, "")


def crate(tid, name_id, kind="hacking", **extra):
    row = {"typeID": tid, "nameID": name_id, "lootPresentationType": kind,
           "fittings": [], "skillBundleID": None, "fixedLootItems": []}
    row.update(extra)
    return row


CRATES = {
    1: crate(A, 1),
    2: crate(B, 2),
    3: crate(C, 3, fittings=[7]),
    4: crate(D, 4),
    5: crate(E, 5, kind="fixed"),
    6: crate(GONE, 1),
}
TYPES = {t: {"groupID": 990721, "volume": 1.0} for t in (A, B, C, D, E)}


class PicksTest(unittest.TestCase):
    def test_comments_the_header_and_rows(self):
        lines = ["# c", "", "name\ttier\twhy", "Alpha Box\t2\tbecause"]
        self.assertEqual(G.parse_picks(lines), [("Alpha Box", 2)])

    def test_the_shipped_file_is_empty(self):
        with open(G.PICKS_FILE, encoding="utf-8") as fh:
            self.assertEqual(G.parse_picks(fh.read().splitlines()), [])

    def test_bad_tier_and_header_are_errors(self):
        with self.assertRaises(ValueError):
            G.parse_picks(["name\ttier", "X\t4"])
        with self.assertRaises(ValueError):
            G.parse_picks(["name\ttier", "X\tx"])
        with self.assertRaises(ValueError):
            G.parse_picks(["kind\ttier"])


class DeriveTest(unittest.TestCase):
    def test_candidates_are_random_loot_crates_the_client_has_a_type_for(self):
        _, cands, problems = G.derive(CRATES, TYPES, text, [])
        self.assertEqual(problems, [])
        # C has a fitting, E is fixed, GONE has no type row.
        self.assertEqual([c[0] for c in cands], [A, B, D])
        self.assertEqual(cands[0][1:], ("Alpha Box", 990721, "1.0"))

    def test_nothing_picked_writes_no_geode(self):
        rows, _, _ = G.derive(CRATES, TYPES, text, [])
        self.assertEqual(rows, [])

    def test_a_name_picks_every_type_of_it_at_the_tier(self):
        rows, _, problems = G.derive(CRATES, TYPES, text, [("Alpha Box", 2), ("Beta Box", 1)])
        self.assertEqual(problems, [])
        self.assertEqual(rows, [(A, 2), (B, 1), (D, 2)])

    def test_a_pick_of_nothing_or_of_a_non_candidate_is_a_problem(self):
        _, _, p = G.derive(CRATES, TYPES, text, [("Gamma Box", 1)])
        self.assertEqual(len(p), 1)
        _, _, p = G.derive(CRATES, TYPES, text, [("Fixed Box", 1), ("Nope", 1)])
        self.assertEqual(len(p), 2)

    def test_one_name_at_two_tiers_is_a_problem(self):
        _, _, p = G.derive(CRATES, TYPES, text, [("Alpha Box", 1), ("Alpha Box", 3)])
        self.assertTrue(any("two tiers" in x for x in p), p)


if __name__ == "__main__":
    unittest.main()
