"""Unit tests for `tools/content_skins.py` (L2 SKINS).

Invented rows only (ids in the 99xxxx band). The bundle-backed run is the
`skins` section of `tools/content.py` (`tools/up.sh`).
Run: `python3 -I tools/test_content_skins.py -q`.
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

import content_skins as S  # noqa: E402

LIC_A, LIC_TEMP, LIC_B, LIC_ODD, LIC_GONE = 990801, 990802, 990803, 990804, 990805
G_SKIN, G_OTHER = 990821, 990822
NAMES = {1: "A SKIN", 2: "B\tSKIN"}


def _types():
    return {
        LIC_A: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 1},
        LIC_TEMP: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 1},
        LIC_B: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 2},
        LIC_ODD: {"groupID": G_OTHER, "volume": 1.0, "typeNameID": 1},
    }


GROUPS = {G_SKIN: {"categoryID": 91}, G_OTHER: {"categoryID": 6}}
SKINS = {
    100: {"types": [990902, 990901], "visibleTranquility": True},
    101: {"types": [990903], "visibleTranquility": False, "isStructureSkin": True},
}
LICENCES = {
    LIC_A: {"skinID": 100, "duration": -1, "isSingleUse": False},
    LIC_TEMP: {"skinID": 100, "duration": 30},
    LIC_B: {"skinID": 101, "duration": -1, "isSingleUse": True},
    LIC_GONE: {"skinID": 100, "duration": 7},
}


def text(i):
    return NAMES.get(i, "")


class DeriveTest(unittest.TestCase):
    def test_one_row_per_licence_type_in_id_order(self):
        lic, skins, problems = S.derive(LICENCES, SKINS, _types(), GROUPS, text)
        self.assertEqual(problems, [])
        self.assertEqual([r[0] for r in lic], [LIC_A, LIC_TEMP, LIC_B])
        self.assertEqual(lic[0][2:5], (100, -1, 0))
        self.assertEqual(lic[1][2:5], (100, 30, 0))
        self.assertEqual(lic[2][2:5], (101, -1, 1))

    def test_a_licence_type_missing_from_the_client_types_is_skipped(self):
        lic, _, _ = S.derive(LICENCES, SKINS, _types(), GROUPS, text)
        self.assertNotIn(LIC_GONE, [r[0] for r in lic])

    def test_the_name_is_one_line(self):
        lic, _, _ = S.derive(LICENCES, SKINS, _types(), GROUPS, text)
        self.assertEqual(lic[2][6], "B SKIN")

    def test_skins_carry_sorted_hulls_and_the_optional_structure_key(self):
        _, skins, _ = S.derive(LICENCES, SKINS, _types(), GROUPS, text)
        self.assertEqual(skins, [(100, 1, 0, "990901,990902"), (101, 0, 1, "990903")])

    def test_a_licence_that_is_not_a_skin_licence_is_a_problem(self):
        lic = dict(LICENCES)
        lic[LIC_ODD] = {"skinID": 100, "duration": -1}
        _, _, problems = S.derive(lic, SKINS, _types(), GROUPS, text)
        self.assertEqual(len(problems), 1)

    def test_a_licence_for_an_unknown_skin_is_a_problem(self):
        lic = {LIC_A: {"skinID": 999, "duration": -1}}
        _, _, problems = S.derive(lic, SKINS, _types(), GROUPS, text)
        self.assertEqual(len(problems), 1)

    def test_only_skins_with_a_licence_are_written(self):
        skins = dict(SKINS)
        skins[102] = {"types": [990904], "visibleTranquility": True}
        _, rows, _ = S.derive(LICENCES, skins, _types(), GROUPS, text)
        self.assertEqual([r[0] for r in rows], [100, 101])

    def test_the_section_is_a_content_section(self):
        self.assertEqual((S.SECTION, S.DEPENDS), ("skins", ()))
        self.assertTrue(callable(S.section) and callable(S.check))


if __name__ == "__main__":
    unittest.main()
