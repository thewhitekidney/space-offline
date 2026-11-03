"""Unit tests for `tools/content_deploy.py` (lane hand-data, #109).

Invented rows only (ids in the 99xxxx band). The bundle-backed run is the
`deploy` section of `tools/content.py` (`tools/up.sh`).
Run: `python3 -I tools/test_content_deploy.py -q`.
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

import content_deploy as D  # noqa: E402

DEPOT_A, DEPOT_B, UNIT_A, SKILL, INH_A, INH_B = 990801, 990802, 990803, 990804, 990805, 990806


def depot(decay=100):
    return {"cargoBay": {"accessRange": 7, "allowUserAdd": 1},
            "activate": {"durationSeconds": 3},
            "decay": {"durationSeconds": decay},
            "deploy": {"deployAtRange": 11.5, "minDistanceFromOwnGroup": 900.0}}


def looter():
    c = depot(40)
    c["cargoBay"]["allowUserAdd"] = 0
    c["autoTractorBeam"] = {"cycleTimeSeconds": 2, "maxRange": 5000, "minRange": 60}
    c["autoLooter"] = {"cycleTimeSeconds": 2}
    return c


TYPES = {DEPOT_A: {"groupID": 1246}, DEPOT_B: {"groupID": 1246},
         UNIT_A: {"groupID": 1250}, INH_A: {"groupID": 1249},
         INH_B: {"groupID": 1249}, SKILL: {"groupID": 990821, "volume": 0.5,
                                            "typeNameID": 1}}


class UnitsTest(unittest.TestCase):
    def test_a_group_reads_its_lowest_type_with_every_component(self):
        comps = {DEPOT_A: {"cargoBay": {}}, DEPOT_B: depot(), UNIT_A: looter()}
        rows, problems = D.derive_units(comps, TYPES)
        self.assertEqual(problems, [])
        self.assertEqual(rows[0], (1246, 3, 100, 7, 1, 11.5, 900.0, "", "", "", "", "", ""))
        self.assertEqual(rows[1], (1250, 3, 40, 7, 0, 11.5, 900.0, 5000, 60, 2, "", "", ""))

    def test_the_inhibitor_row_carries_its_range_and_its_placement_figures(self):
        # Invented numbers throughout.
        inh = {"activate": {"durationSeconds": 7}, "decay": {"durationSeconds": 900},
               "cynoInhibitor": {"range": 25000},
               "deploy": {"minDistanceFromOwnGroup": 41000,
                          "minDistanceFromStargatesAndStations": 16000,
                          "maxSecurityLevel": 0.3}}
        comps = {DEPOT_A: depot(), UNIT_A: looter(), INH_B: inh,
                 INH_A: {"activate": {"durationSeconds": 1}, "cynoInhibitor": {"range": 0}}}
        rows, problems = D.derive_units(comps, TYPES)
        self.assertEqual(problems, [])
        self.assertEqual(rows[2], (1249, 7, 900, 0, 0, 0, 41000, "", "", "", 25000, 16000, 0.3))

    def test_an_inhibitor_without_a_deploy_component_has_empty_placement(self):
        inh = {"activate": {"durationSeconds": 7}, "cynoInhibitor": {"range": 25000}}
        rows, _ = D.derive_units({DEPOT_A: depot(), UNIT_A: looter(), INH_B: inh}, TYPES)
        self.assertEqual(rows[2], (1249, 7, 0, 0, 0, 0, 0, "", "", "", 25000, "", ""))

    def test_no_inhibitor_type_is_no_row_and_no_problem(self):
        rows, problems = D.derive_units({DEPOT_A: depot(), UNIT_A: looter()}, TYPES)
        self.assertEqual(problems, [])
        self.assertEqual([r[0] for r in rows], [1246, 1250])

    def test_a_group_with_no_such_type_is_a_problem(self):
        _, problems = D.derive_units({DEPOT_A: depot()}, TYPES)
        self.assertEqual(len(problems), 1)
        self.assertIn("1250", problems[0])

    def test_a_beam_without_a_looter_is_no_tractor(self):
        c = looter()
        del c["autoLooter"]
        rows, _ = D.derive_units({DEPOT_A: depot(), UNIT_A: c}, TYPES)
        self.assertEqual(rows[1][7:], ("", "", "", "", "", ""))


class InjectorsTest(unittest.TestCase):
    def test_rows_come_from_the_client_tables(self):
        saved = D.INJECTOR_TYPES
        D.INJECTOR_TYPES = (SKILL, 990805)
        try:
            rows, problems = D.derive_injectors(
                TYPES, {990821: 17}, lambda i: "Fixture\tInjector")
        finally:
            D.INJECTOR_TYPES = saved
        self.assertEqual(rows, [(SKILL, 990821, 17, 0, 0.5, "Fixture Injector")])
        self.assertEqual(len(problems), 1)

    def test_a_missing_group_is_a_problem(self):
        saved = D.INJECTOR_TYPES
        D.INJECTOR_TYPES = (SKILL,)
        try:
            rows, problems = D.derive_injectors(TYPES, {}, str)
        finally:
            D.INJECTOR_TYPES = saved
        self.assertEqual((rows, len(problems)), ([], 1))


if __name__ == "__main__":
    unittest.main()
