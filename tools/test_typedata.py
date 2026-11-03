"""Unit tests for `tools/typedata.py`'s type selection (FX7, W17).

Synthetic ids only (plan §1.6): the client-backed check is
`content.py --sections types --check`, recorded in `docs/lanes/fx7.md`.
Run: `python3 -I tools/test_typedata.py -q`.
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

import typedata as T  # noqa: E402

#: Invented group ids, outside any real range the tests could collide with.
G_ANY = 99001


class Selection(unittest.TestCase):
    def test_every_listed_category_is_extracted_whatever_its_group(self):
        for cat in T.CATEGORIES:
            self.assertTrue(T.wanted(G_ANY, cat), cat)

    def test_the_fx7_categories_are_listed(self):
        # categoryStation, categoryMaterial, categoryBlueprint,
        # categoryImplant [V inventorycommon/const.pyc].
        for cat in (3, 4, 9, 20):
            self.assertIn(cat, T.CATEGORIES)

    def test_a_listed_group_is_extracted_from_a_category_left_out(self):
        # groupWormhole lives in categoryCelestial [V], which stays out.
        self.assertIn(988, T.GROUPS)
        self.assertNotIn(2, T.CATEGORIES)
        self.assertTrue(T.wanted(988, 2))
        self.assertFalse(T.wanted(G_ANY, 2), "the rest of category 2 stays out")

    def test_the_effect_beacons_are_extracted_sys_1(self):
        self.assertIn(920, T.GROUPS)
        self.assertTrue(T.wanted(920, 2))
        self.assertNotIn(2, T.CATEGORIES)

    def test_the_gas_clouds_are_extracted_soc_b2(self):
        # groupHarvestableCloud lives in categoryCelestial [V]: the cloud
        # type is the gas a harvester cuts (server mining.rs).
        self.assertIn(711, T.GROUPS)
        self.assertTrue(T.wanted(711, 2))

    def test_the_sovereignty_hub_is_extracted_d_hub(self):
        # groupInfrastructureHub lives in categorySovereigntyStructure [V-data],
        # which stays out; the hub's own row was missing from the bundle.
        self.assertIn(1012, T.GROUPS)
        self.assertTrue(T.wanted(1012, 40))
        self.assertNotIn(40, T.CATEGORIES)

    def test_the_structure_categories_are_extracted_u0(self):
        self.assertIn(65, T.CATEGORIES)
        self.assertIn(66, T.CATEGORIES)
        self.assertTrue(T.wanted(G_ANY, 65))
        self.assertTrue(T.wanted(G_ANY, 66))

    def test_the_deployable_category_is_extracted_nw2(self):
        # categoryDeployable [V]: the mobile warp disruptors, group 361.
        self.assertIn(22, T.CATEGORIES)
        self.assertTrue(T.wanted(G_ANY, 22))

    def test_the_deed_group_is_extracted_u0(self):
        self.assertIn(4086, T.GROUPS)
        self.assertTrue(T.wanted(4086, 2))

    def test_the_container_groups_are_extracted_57(self):
        # The four player-deployed container groups live in categoryCelestial.
        for group in (12, 340, 448, 649):
            self.assertIn(group, T.GROUPS)
            self.assertTrue(T.wanted(group, 2))
        self.assertNotIn(2, T.CATEGORIES)

    def test_the_deployable_groups_are_extracted_82(self):
        # The Mobile Depot (1246) and the Mobile Tractor Unit (1250) live in
        # categoryDeployable (22), which NW-2 then took whole.
        for group in (1246, 1250):
            self.assertIn(group, T.GROUPS)
            self.assertTrue(T.wanted(group, 22))
        self.assertIn(22, T.CATEGORIES)

    def test_the_crate_groups_are_extracted_w6d(self):
        # The geode candidates' groups, whatever their category.
        for group in (314, 526, 1194, 1818, 1886, 1977):
            self.assertIn(group, T.GROUPS)
            self.assertTrue(T.wanted(group, 2))

    def test_an_unlisted_category_or_no_category_is_left_out(self):
        self.assertFalse(T.wanted(G_ANY, None), "a group with no groups row")
        unlisted = max(T.CATEGORIES) + 1
        self.assertFalse(T.wanted(G_ANY, unlisted))


#: Invented ids for the subsystem tests (T3-A). Attribute ids are the
#: client's [V]; every type id, group id and value below is made up.
T_SUB, T_HULL = 990101, 990102
G_SUB_ANY = 99010


def _sub_line(t, published, attrs, group=G_SUB_ANY, cat=32, name="Fixture Subsystem"):
    row = {"groupID": group, "published": published, "volume": 5.0}
    return T.type_line(t, row, cat, attrs, [], 0, name)


GOOD_ATTRS = {T.ATTR_SUBSYSTEM_SLOT: 127.0, T.ATTR_FITS_TO_SHIP_TYPE: float(T_HULL),
              1374: 7.0, 1368: 6.0}


class Subsystems(unittest.TestCase):
    def test_typedata_extracts_subsystem_category(self):
        self.assertIn(32, T.CATEGORIES)
        self.assertEqual(T.CATEGORY_SUBSYSTEM, 32)
        # Whatever its group, a category 32 type is wanted ...
        self.assertTrue(T.wanted(G_SUB_ANY, 32))
        # ... and the neighbouring categories stay as they were.
        self.assertTrue(T.wanted(G_SUB_ANY, 34) and T.wanted(G_SUB_ANY, 35))
        self.assertFalse(T.wanted(G_SUB_ANY, 33))

    def test_a_subsystem_row_carries_slot_and_fit_attributes(self):
        line = _sub_line(T_SUB, True, GOOD_ATTRS)
        f = line.split("\t")
        self.assertEqual(f[0], "type")
        self.assertEqual((int(f[1]), int(f[2]), int(f[3]), f[4]), (T_SUB, G_SUB_ANY, 32, "1"))
        attrs = dict(p.split("=") for p in f[12].split(";"))
        self.assertEqual(float(attrs["1366"]), 127.0)
        self.assertEqual(float(attrs["1380"]), float(T_HULL))
        self.assertEqual(float(attrs["1374"]), 7.0)
        self.assertEqual(T.problems(line + "\n"), [])

    def test_the_check_names_each_way_a_subsystem_can_be_wrong(self):
        self.assertTrue(any("no category 32" in b for b in T.problems("# nothing\n")))
        nothing_else = _sub_line(990103, True, {}, cat=6)
        self.assertTrue(T.problems(nothing_else + "\n"), "a ship is not a subsystem")
        # Not in one of the four live flags.
        wrong_flag = dict(GOOD_ATTRS)
        wrong_flag[T.ATTR_SUBSYSTEM_SLOT] = 129.0
        bad = T.problems(_sub_line(T_SUB, True, wrong_flag) + "\n")
        self.assertEqual(len(bad), 1)
        self.assertIn("subSystemSlot", bad[0])
        # No hull it fits.
        no_hull = {k: v for k, v in GOOD_ATTRS.items() if k != T.ATTR_FITS_TO_SHIP_TYPE}
        self.assertIn("fitsToShipType", T.problems(_sub_line(T_SUB, True, no_hull) + "\n")[0])
        # No slot modifier at all.
        no_slots = {k: v for k, v in GOOD_ATTRS.items() if k not in T.ATTR_SLOT_MODIFIERS}
        self.assertIn("slot modifier", T.problems(_sub_line(T_SUB, True, no_slots) + "\n")[0])

    def test_a_deprecated_unpublished_subsystem_is_extracted_not_held_to_the_rule(self):
        good = _sub_line(T_SUB, True, GOOD_ATTRS)
        old = _sub_line(990104, False, {}, name="OLD Fixture")
        self.assertEqual(T.problems(good + "\n" + old + "\n"), [])

    def test_the_slot_attributes_are_the_clients_ids(self):
        # subSystemSlot, fitsToShipType and the five modifiers [V dogmaattributes].
        self.assertEqual((T.ATTR_SUBSYSTEM_SLOT, T.ATTR_FITS_TO_SHIP_TYPE), (1366, 1380))
        self.assertEqual(sorted(T.ATTR_SLOT_MODIFIERS), [1368, 1369, 1374, 1375, 1376])
        self.assertEqual(T.SUBSYSTEM_FLAGS, (125, 126, 127, 128))

    def test_an_effect_line_carries_is_warp_safe_before_the_usage_chance(self):
        safe = T.effect_line(990001, {"effectName": "fixtureRep", "isWarpSafe": True})
        unsafe = T.effect_line(990002, {"effectName": "fixtureBurn"})
        self.assertEqual(safe.split("\t")[-2], "1")
        self.assertEqual(unsafe.split("\t")[-2], "0")
        self.assertEqual(len(safe.split("\t")), 13)


if __name__ == "__main__":
    unittest.main()
