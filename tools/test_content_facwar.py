"""Unit tests for the `facwar` content section, `tools/content_facwar.py` (FW-3).

Synthetic rows only, invented ids in `schema.md`'s unused ranges. The
client-backed check is `content.py --sections facwar --check`.
Run: `python3 -I tools/test_content_facwar.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_facwar as F  # noqa: E402

SPRITE = {"territoryIconFile": "x", "imageHeight": 1, "imageWidth": 1, "xOffset": 0, "yOffset": 0}


def row(sid, visible=1, sprite=True):
    r = {"solarSystemId": sid, "xpos": 1, "ypos": 1, "isVisible": visible}
    if sprite:
        r["territorySprite"] = SPRITE
    return r


class Derive(unittest.TestCase):
    def test_only_visible_sprite_rows_are_territory(self):
        zones = {98: {"systems": {"39000001": row(39000001),
                                  "39000002": row(39000002, sprite=False),
                                  "39000003": row(39000003, visible=0)}},
                 99: {"systems": {"39000004": row(39000004)}}}
        rows, problems = F.derive(zones)
        self.assertEqual(rows, [(39000001, 98), (39000004, 99)])
        self.assertEqual(problems, [])

    def test_a_system_in_two_warzones_is_a_problem(self):
        zones = {98: {"systems": {"39000001": row(39000001)}},
                 99: {"systems": {"39000001": row(39000001)}}}
        rows, problems = F.derive(zones)
        self.assertEqual(rows, [(39000001, 98)])
        self.assertEqual(len(problems), 1)

    def test_a_key_that_disagrees_with_its_row_is_a_problem(self):
        rows, problems = F.derive({98: {"systems": {"39000001": row(39000002)}}})
        self.assertEqual((rows, len(problems)), ([], 1))

    def test_rows_sort_by_system(self):
        zones = {99: {"systems": {"39000009": row(39000009)}},
                 98: {"systems": {"39000002": row(39000002)}}}
        self.assertEqual(F.derive(zones)[0], [(39000002, 98), (39000009, 99)])


def dict_ops(entries, name="facwarMaxLPPayout"):
    """The bytecode of `name = {key: value, ...}` as `dis` yields it."""
    ops = [("LOAD_CONST", 0, "1"), ("STORE_NAME", 0, "other"), ("BUILD_MAP", 2, "")]
    for key, val in entries:
        ops += [("LOAD_CONST", 1, str(val)), ("LOAD_NAME", 2, key), ("STORE_MAP", None, "")]
    return ops + [("STORE_NAME", 3, name)]


class MaxLp(unittest.TestCase):
    GROUPS = {"groupAlpha": 9001, "groupBeta": 9002}

    def test_the_dict_literal_becomes_sorted_group_rows(self):
        ops = dict_ops([("groupBeta", 700), ("groupAlpha", 300)])
        rows, problems = F.max_lp_rows(ops, self.GROUPS)
        self.assertEqual((rows, problems), ([(9001, 300), (9002, 700)], []))

    def test_an_unknown_group_name_is_a_problem(self):
        rows, problems = F.max_lp_rows(dict_ops([("groupGamma", 5)]), self.GROUPS)
        self.assertEqual((rows, len(problems)), ([], 1))

    def test_a_missing_or_non_literal_dict_is_a_problem(self):
        self.assertEqual(len(F.max_lp_rows([], self.GROUPS)[1]), 1)
        ops = [("LOAD_NAME", 0, "x"), ("STORE_NAME", 1, "facwarMaxLPPayout")]
        self.assertEqual(len(F.max_lp_rows(ops, self.GROUPS)[1]), 1)


if __name__ == "__main__":
    unittest.main()
