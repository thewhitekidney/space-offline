"""Unit tests for the `fighters` content section, `tools/content_fighters.py` (FT-A).

Invented rows only (plan §1.6): ids in `schema.md`'s unused ranges. The
bundle-backed check is `content.py --check`, recorded in `docs/lanes/ft-a.md`.
Run: `python3 -I tools/test_content_fighters.py -q`.
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

import content  # noqa: E402
import content_fighters as F  # noqa: E402
import typedata as T  # noqa: E402

AB_A, AB_B, AB_C, GHOST = 17, 4, 7, 990499
FT_1, FT_2, FT_3 = 990401, 990402, 990403
NAME = {990411: "Alpha", 990412: "Beta", 990413: "Gamma"}.get


def _abilities():
    return {
        AB_A: {"targetMode": "itemTargeted", "displayNameID": 990411, "iconID": 990421,
               "turretGraphicID": 990431},
        AB_B: {"targetMode": "untargeted", "displayNameID": 990412, "iconID": 990422,
               "disallowInHighSec": True},
        AB_C: {"targetMode": "pointTargeted", "displayNameID": 990413, "iconID": 990423,
               "disallowInLowSec": True, "disallowInHighSec": True},
    }


class Abilities(unittest.TestCase):
    def test_all_three_target_modes_come_through(self):
        rows = F.abilities_rows(_abilities(), NAME)
        self.assertEqual([r[0] for r in rows], sorted([AB_A, AB_B, AB_C]))
        self.assertEqual({r[1] for r in rows}, set(F.TARGET_MODES))
        by = {r[0]: r for r in rows}
        self.assertEqual(by[AB_A][2:4], (0, 0))
        self.assertEqual(by[AB_B][2:4], (0, 1))
        self.assertEqual(by[AB_C][2:4], (1, 1))
        self.assertEqual(by[AB_A][4], F.ABILITY_EFFECT[AB_A])
        self.assertEqual(by[AB_A][6], 990431)
        self.assertEqual(by[AB_B][6], "", "no turret graphic is an empty column")
        self.assertEqual(by[AB_B][7], "Beta")

    def test_the_effect_map_has_the_clients_36_and_13_effects(self):
        self.assertEqual(len(F.ABILITY_EFFECT), 36)
        self.assertEqual(len(set(F.ABILITY_EFFECT.values())), 16, "13 used + 3 unused")

    def test_a_row_round_trips_through_the_encoder(self):
        for r in F.abilities_rows(_abilities(), NAME):
            self.assertEqual(len(content.encode_row(r).split("\t")), len(F.ABILITIES_HEADER))


class Slots(unittest.TestCase):
    def test_slot_without_cooldown_has_none(self):
        rows = F.slots_rows({FT_1: {"abilitySlot0": {"abilityID": AB_A}}})
        self.assertEqual(rows, [(FT_1, 0, AB_A, "", "", "")])
        self.assertEqual(content.encode_row(rows[0]), "%d\t0\t%d\t\t\t" % (FT_1, AB_A))

    def test_cooldown_charges_and_rearm(self):
        rows = F.slots_rows({FT_2: {
            "abilitySlot2": {"abilityID": AB_C, "charges": {"chargeCount": 3, "rearmTimeSeconds": 20}},
            "abilitySlot1": {"abilityID": AB_B, "cooldownSeconds": 60},
        }})
        self.assertEqual(rows, [(FT_2, 1, AB_B, 60, "", ""), (FT_2, 2, AB_C, "", 3, 20)])

    def test_rows_are_sorted_and_empty_slots_skipped(self):
        rows = F.slots_rows({FT_3: {"abilitySlot2": {"abilityID": AB_C}},
                             FT_1: {"abilitySlot0": {"abilityID": AB_A}}})
        self.assertEqual([(r[0], r[1]) for r in rows], [(FT_1, 0), (FT_3, 2)])


class Check(unittest.TestCase):
    def _good(self):
        ab = F.abilities_rows(_abilities(), NAME)
        sl = F.slots_rows({FT_1: {"abilitySlot0": {"abilityID": AB_A}}})
        effects = {F.ABILITY_EFFECT[AB_A]}
        return ab, sl, effects, {FT_1}

    def test_good_rows_pass(self):
        self.assertEqual(F.problems(*self._good()), [])

    def test_a_slot_naming_an_unknown_ability_is_refused(self):
        ab, sl, eff, fighters = self._good()
        sl = sl + [(FT_1, 1, GHOST, "", "", "")]
        self.assertTrue(any("not in abilities.tsv" in b for b in F.problems(ab, sl, eff, fighters)))

    def test_a_slot_on_a_non_fighter_type_is_refused(self):
        ab, sl, eff, _ = self._good()
        self.assertTrue(any("category-87" in b for b in F.problems(ab, sl, eff, set())))

    def test_a_used_ability_whose_effect_is_missing_is_refused(self):
        ab, sl, _, fighters = self._good()
        self.assertTrue(any("not in typedata" in b for b in F.problems(ab, sl, set(), fighters)))

    def test_an_unused_ability_needs_no_effect_row(self):
        ab = F.abilities_rows({F.UNUSED_ABILITIES[0]: {"targetMode": "untargeted"}}, NAME)
        self.assertEqual(F.problems(ab, [], set(), set()), [])


class TypedataFighters(unittest.TestCase):
    def _line(self, t, published, attrs, group=99087):
        row = {"groupID": group, "published": published, "volume": 1000.0}
        return T.type_line(t, row, 87, attrs, [], 0, "Fixture Fighter")

    def test_typedata_includes_category_87(self):
        self.assertIn(87, T.CATEGORIES)
        self.assertTrue(T.wanted(99087, 87))

    def test_class_comes_from_the_flag_attribute_not_the_group(self):
        # An invented group that is none of the six real ones: the flag decides.
        a = {2214: 1.0, T.ATTR_FIGHTER_SQUADRON_MAX_SIZE: 6.0}
        self.assertEqual(T.fighter_problems(self._line(FT_1, True, a) + "\n", require=True), [])
        none = {T.ATTR_FIGHTER_SQUADRON_MAX_SIZE: 6.0}
        self.assertTrue(T.fighter_problems(self._line(FT_1, True, none) + "\n"))
        two = {2212: 1.0, 2214: 1.0, T.ATTR_FIGHTER_SQUADRON_MAX_SIZE: 6.0}
        self.assertTrue(T.fighter_problems(self._line(FT_1, True, two) + "\n"))

    def test_an_unpublished_placeholder_is_not_held_to_the_rule(self):
        self.assertEqual(T.fighter_problems(self._line(FT_2, False, {}) + "\n"), [])

    def test_require_demands_a_fighter(self):
        self.assertTrue(T.fighter_problems("# nothing\n", require=True))
        self.assertEqual(T.fighter_problems("# nothing\n"), [])


if __name__ == "__main__":
    unittest.main()
