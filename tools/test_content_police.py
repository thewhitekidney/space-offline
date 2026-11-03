"""Unit tests for `tools/content_police.py` (NW-7). Invented rows only.

Run: `python3 -I tools/test_content_police.py -q`.
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

import content_police as P  # noqa: E402

FORCES = (("Fixa Police", 599001, "chatFixaWarning"), ("Fixb Guard", 599002, P.DEFAULT_WARNING))


def trow(tid, group, name):
    return ["type", str(tid), str(group), "11", "", "", "", "", "", "", "", "", "", name]


class Roles(unittest.TestCase):
    ROLES = (("Fixa Marshal", "captain"), ("Fixa Deputy", "officer"), ("Fixa Absent", "swat"))

    def test_exact_names_map_to_roles_in_order_lowest_id_wins(self):
        rows = [trow(990812, 301, "Fixa Deputy"), trow(990811, 301, "Fixa Deputy"),
                trow(990810, 301, "Fixa Marshal"), trow(990813, 301, "Fixa Deputy Extra"),
                ["junk"]]
        self.assertEqual(P.derive_roles(rows, self.ROLES),
                         [("captain", 990810), ("officer", 990811)])


class Derive(unittest.TestCase):
    def test_group_and_prefix_select_and_sort(self):
        rows = [trow(990803, 182, "Fixb Guard Major"), trow(990802, 182, "Fixa Police Captain"),
                trow(990801, 182, "Fixa Police Sergeant"), trow(990804, 182, "Other Police"),
                trow(990805, 100, "Fixa Police Decoy"), ["junk"]]
        self.assertEqual(P.derive(rows, FORCES), [
            (599001, "chatFixaWarning", "990801,990802"),
            (599002, P.DEFAULT_WARNING, "990803"),
        ])

    def test_a_force_with_no_type_has_no_row(self):
        self.assertEqual(P.derive([trow(990801, 182, "Fixa Police Sergeant")], FORCES),
                         [(599001, "chatFixaWarning", "990801")])

    def test_the_real_table_has_one_prefix_per_faction(self):
        ids = [f for _p, f, _w in P.FORCES]
        self.assertEqual(len(ids), len(set(ids)))


if __name__ == "__main__":
    unittest.main()
