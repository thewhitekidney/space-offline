"""Unit tests for the `dbuffs` content section, `tools/content_dbuffs.py` (K-BOOST).

Invented rows only (plan §1.6): ids in the 99xxxx band. The bundle-backed check
is `content.py --check`, recorded in `docs/lanes/k-boost.md`.
Run: `python3 -I tools/test_content_dbuffs.py -q`.
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_dbuffs as D  # noqa: E402

C_ITEM, C_LOC, C_GROUP, C_SKILL, GHOST = 990701, 990702, 990703, 990704, 990799
ATTR_A, ATTR_B, ATTR_C = 990711, 990712, 990713
GROUP_G, SKILL_S = 990721, 990731
NAME = {990741: "Alpha", 990742: "Beta"}.get


def _collections():
    return {
        C_ITEM: {"displayNameID": 990741, "operationName": "PostPercent",
                 "aggregateMode": "Minimum", "showOutputValueInUI": "ShowInverted",
                 "developerDescription": "item kind",
                 "itemModifiers": [{"dogmaAttributeID": ATTR_B}, {"dogmaAttributeID": ATTR_A}],
                 "locationModifiers": [], "locationGroupModifiers": [],
                 "locationRequiredSkillModifiers": []},
        C_LOC: {"operationName": "ModAdd", "aggregateMode": "Maximum",
                "showOutputValueInUI": "Hide", "developerDescription": "no name",
                "itemModifiers": [],
                "locationModifiers": [{"dogmaAttributeID": ATTR_C}],
                "locationGroupModifiers": [], "locationRequiredSkillModifiers": []},
        C_GROUP: {"displayNameID": 990742, "operationName": "PostMul",
                  "aggregateMode": "Maximum", "showOutputValueInUI": "ShowNormal",
                  "developerDescription": "group kind", "itemModifiers": [],
                  "locationModifiers": [],
                  "locationGroupModifiers": [{"dogmaAttributeID": ATTR_A, "groupID": GROUP_G}],
                  "locationRequiredSkillModifiers": []},
        C_SKILL: {"operationName": "PostPercent", "aggregateMode": "Minimum",
                  "showOutputValueInUI": "ShowNormal", "developerDescription": "skill kind",
                  "itemModifiers": [], "locationModifiers": [], "locationGroupModifiers": [],
                  "locationRequiredSkillModifiers": [
                      {"dogmaAttributeID": ATTR_B, "skillID": SKILL_S},
                      {"dogmaAttributeID": ATTR_A, "skillID": SKILL_S}]},
    }


class Collections(unittest.TestCase):
    def test_rows_are_sorted_and_named(self):
        rows = D.collections_rows(_collections(), NAME)
        self.assertEqual([r[0] for r in rows], sorted(_collections()))
        by = {r[0]: r for r in rows}
        self.assertEqual(by[C_ITEM], (C_ITEM, "PostPercent", "Minimum", "ShowInverted",
                                      "Alpha", "item kind"))
        self.assertEqual(by[C_LOC][4], "", "no displayNameID is an empty name")

    def test_a_row_round_trips_through_the_encoder(self):
        for r in D.collections_rows(_collections(), NAME):
            self.assertEqual(len(content.encode_row(r).split("\t")),
                             len(D.COLLECTIONS_HEADER))


class Modifiers(unittest.TestCase):
    def test_each_kind_has_its_columns(self):
        rows = D.modifiers_rows(_collections())
        by = {(r[0], r[1], r[2]): r for r in rows}
        self.assertEqual(by[(C_ITEM, "item", ATTR_A)], (C_ITEM, "item", ATTR_A, "", ""))
        self.assertEqual(by[(C_LOC, "location", ATTR_C)], (C_LOC, "location", ATTR_C, "", ""))
        self.assertEqual(by[(C_GROUP, "group", ATTR_A)],
                         (C_GROUP, "group", ATTR_A, GROUP_G, ""))
        self.assertEqual(by[(C_SKILL, "skill", ATTR_B)],
                         (C_SKILL, "skill", ATTR_B, "", SKILL_S))

    def test_rows_are_sorted_within_a_collection(self):
        rows = D.modifiers_rows(_collections())
        item = [r[2] for r in rows if r[0] == C_ITEM]
        self.assertEqual(item, [ATTR_A, ATTR_B])
        self.assertEqual([r[0] for r in rows], sorted(r[0] for r in rows))

    def test_a_row_round_trips_through_the_encoder(self):
        for r in D.modifiers_rows(_collections()):
            self.assertEqual(len(content.encode_row(r).split("\t")),
                             len(D.MODIFIERS_HEADER))


class Check(unittest.TestCase):
    def _good(self):
        c = _collections()
        return (D.collections_rows(c, NAME), D.modifiers_rows(c),
                {ATTR_A, ATTR_B, ATTR_C}, {SKILL_S}, {C_ITEM, C_GROUP})

    def test_good_rows_pass(self):
        self.assertEqual(D.problems(*self._good()), [])

    def test_an_unknown_operation_aggregate_or_show_is_refused(self):
        co, mo, a, s, b = self._good()
        for col, bad in ((1, "Sideways"), (2, "Median"), (3, "Maybe")):
            row = list(co[0])
            row[col] = bad
            self.assertTrue(D.problems([tuple(row)] + co[1:], mo, a, s, b), col)

    def test_a_modifier_on_an_unknown_attribute_is_refused(self):
        co, mo, a, s, b = self._good()
        self.assertTrue(any("not in typedata" in x for x in D.problems(co, mo, set(), s, b)))

    def test_a_skill_modifier_needs_a_known_skill(self):
        co, mo, a, s, b = self._good()
        self.assertTrue(any("skill" in x for x in D.problems(co, mo, a, set(), b)))

    def test_a_modifier_of_no_collection_is_refused(self):
        co, mo, a, s, b = self._good()
        mo = mo + [(GHOST, "item", ATTR_A, "", "")]
        self.assertTrue(any("not in collections.tsv" in x for x in D.problems(co, mo, a, s, b)))

    def test_a_burst_charge_naming_no_collection_is_refused(self):
        co, mo, a, s, b = self._good()
        self.assertTrue(any("lacks" in x for x in D.problems(co, mo, a, s, b | {GHOST})))

    def test_a_burst_collection_with_no_modifier_is_refused(self):
        co, mo, a, s, b = self._good()
        mo = [r for r in mo if r[0] != C_GROUP]
        self.assertTrue(any("no modifier" in x for x in D.problems(co, mo, a, s, b)))

    def test_a_group_row_without_its_group_is_refused(self):
        co, mo, a, s, b = self._good()
        mo = mo + [(C_GROUP, "group", ATTR_B, "", "")]
        self.assertTrue(any("group" in x for x in D.problems(co, mo, a, s, b)))


class TypedataFacts(unittest.TestCase):
    def test_burst_collections_are_the_charges_buff_ids(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "typedata.tsv")
            with open(p, "w", encoding="utf-8", newline="") as fh:
                fh.write("attribute\t%d\t0.0\tx\n" % ATTR_A)
                # A charge with the warfare effect names two collections ...
                fh.write("type\t990751\t1\t8\t1\t1\t0.0\t0.0\t0.0\t1.0\t%d\t%d\t"
                         "%d=%d.0;%d=%d.0;%d=0.0\tCharge\n"
                         % (D.CHARGE_EFFECT, D.CHARGE_EFFECT,
                            D.BUFF_SLOTS[0][0], C_ITEM, D.BUFF_SLOTS[1][0], C_GROUP,
                            D.BUFF_SLOTS[2][0]))
                # ... a module naming one (a titan generator) does not count.
                fh.write("type\t990752\t1\t7\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t12\t"
                         "%d=%d.0\tGenerator\n" % (D.BUFF_SLOTS[0][0], C_SKILL))
            attrs, types, buffs = D._typedata_facts(p)
        self.assertEqual(attrs, {ATTR_A})
        self.assertEqual(types, {990751, 990752})
        self.assertEqual(buffs, {C_ITEM, C_GROUP})


if __name__ == "__main__":
    unittest.main()
