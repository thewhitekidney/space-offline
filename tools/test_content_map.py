"""Unit tests for `universe/centers.tsv` (MAP-A), `tools/content.py`'s
`center_rows`. Invented ids and positions only (`schema.md` §1.1's unused
ranges). Run: `python3 -I tools/test_content_map.py -q`."""

from __future__ import annotations

import os
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402


class Table(dict):
    """A table as the readers give it: iterable keys, `.get(key)`."""


class Ctx:
    def __init__(self, systems=None):
        self.systems = systems
        self._t = {
            content.SYSTEMS: Table({
                39000001: {"constellationID": 29000001, "regionID": 19000001,
                           "center": {"x": 1.0, "y": -2.0, "z": 3.5}},
                39000002: {"constellationID": 29000002, "regionID": 19000002,
                           "center": {"x": 9.46e15, "y": 0.0, "z": 0.0}},
            }),
            content.CONSTELLATIONS: Table({
                29000001: {"center": {"x": 4.0, "y": 5.0, "z": 6.0}},
                29000002: {"center": {"x": 7.0, "y": 8.0, "z": 9.0}},
            }),
            # A region's centre may come by index.
            content.REGIONS: Table({
                19000001: {"center": [10.0, 11.0, 12.0]},
                19000002: {"center": [13.0, 14.0, 15.0]},
            }),
        }

    def load(self, res):
        return self._t[res]


class Centers(unittest.TestCase):
    def test_three_kinds_in_order(self):
        rows = list(content.center_rows(Ctx()))
        self.assertEqual([r[:2] for r in rows], [
            ("s", 39000001), ("s", 39000002), ("c", 29000001), ("c", 29000002),
            ("r", 19000001), ("r", 19000002)])
        self.assertEqual(rows[0][2:], (1.0, -2.0, 3.5))

    def test_a_region_by_index_and_a_system_by_key_agree_on_shape(self):
        rows = {r[:2]: r[2:] for r in content.center_rows(Ctx())}
        self.assertEqual(rows[("r", 19000001)], (10.0, 11.0, 12.0))
        self.assertEqual(rows[("s", 39000002)], (9.46e15, 0.0, 0.0))

    def test_the_sandbox_keeps_the_selected_systems_and_their_parents(self):
        rows = list(content.center_rows(Ctx([39000002])))
        self.assertEqual([r[:2] for r in rows],
                         [("s", 39000002), ("c", 29000002), ("r", 19000002)])

    def test_a_missing_center_is_refused(self):
        ctx = Ctx()
        ctx._t[content.CONSTELLATIONS][29000001] = {"center": None}
        with self.assertRaises(content.ContentError):
            list(content.center_rows(ctx))

    def test_floats_are_written_by_repr(self):
        self.assertEqual(content.encode(9.46e15), "9460000000000000.0")
        self.assertEqual(content.CENTERS_HEADER, ("kind", "id", "x", "y", "z"))
        self.assertEqual(content.SYSTEM_EFFECTS_HEADER[:2], ("system_id", "beacon_type_id"))
        self.assertEqual(content.FORMAT, "1.5")


class SystemEffects(unittest.TestCase):
    """`effect_rows` (SYS-1): the rows are read from `secondarySun`, never
    computed from the system's class. Invented ids."""

    CONTENT = {
        39000001: {"secondarySun": {"effectBeaconTypeID": 990101, "typeID": 990102,
                                    "itemID": 990103,
                                    "position": {"x": 1.0, "y": -2.0, "z": 3.5}}},
        39000002: {},
        39000003: {"secondarySun": {"typeID": 990102, "itemID": 990104}},
        39000004: {"secondarySun": {"effectBeaconTypeID": 990105, "typeID": 990106,
                                    "itemID": 990107}},
    }

    def test_only_systems_with_a_beacon_have_a_row(self):
        got = list(content.effect_rows(Table(self.CONTENT), sorted(self.CONTENT)))
        self.assertEqual(got, [
            (39000001, 990101, 990102, 990103, 1.0, -2.0, 3.5),
            (39000004, 990105, 990106, 990107, 0.0, 0.0, 0.0),
        ])

    def test_a_row_has_a_cell_per_header_column(self):
        for row in content.effect_rows(Table(self.CONTENT), sorted(self.CONTENT)):
            self.assertEqual(len(row), len(content.SYSTEM_EFFECTS_HEADER))
            self.assertEqual(content.encode_row(row).count("\t"),
                             len(content.SYSTEM_EFFECTS_HEADER) - 1)


if __name__ == "__main__":
    unittest.main()
