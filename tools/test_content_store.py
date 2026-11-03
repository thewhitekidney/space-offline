"""Unit tests for `tools/content_store.py` (L1 NES-BACKEND).

Invented rows only (ids in the 99xxxx band). The bundle-backed run is
`tools/analysis.sh python3 /work/tools/content_store.py ...`, recorded in
`docs/lanes/l1-nes-backend.md`.
Run: `python3 -I tools/test_content_store.py -q`.
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

import content_store as S  # noqa: E402

PLEX, EXTRACTOR = S.TYPE_PLEX, S.CONSTS["skill_extractor"]
LIC_A, LIC_B, LIC_C, LIC_TEMP, LIC_BIG = 990801, 990802, 990803, 990804, 990805
HULL_S, HULL_B, HULL_X = 990811, 990812, 990813
G_SMALL, G_BIG, G_SKIN, G_PLEX, G_TOOL, G_OTHER = 990821, 990822, 990823, 990824, 990825, 990826
NAMES = {1: "Plex", 2: "Extractor", 3: "A SKIN", 4: "B SKIN", 5: "C SKIN", 6: "Big SKIN", 7: "x\ty"}


def _types():
    return {
        PLEX: {"groupID": G_PLEX, "volume": 0.0002, "typeNameID": 1},
        EXTRACTOR: {"groupID": G_TOOL, "volume": 0.01, "typeNameID": 2, "descriptionID": 7},
        LIC_A: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 3},
        LIC_B: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 4},
        LIC_C: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 5},
        LIC_TEMP: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 3},
        LIC_BIG: {"groupID": G_SKIN, "volume": 0.1, "typeNameID": 6},
        HULL_S: {"groupID": G_SMALL}, HULL_B: {"groupID": G_BIG}, HULL_X: {"groupID": G_OTHER},
    }


GROUPS = {G_SKIN: {"categoryID": 91}, G_PLEX: {"categoryID": 5}, G_TOOL: {"categoryID": 17},
          G_SMALL: {"categoryID": 6}, G_BIG: {"categoryID": 6}, G_OTHER: {"categoryID": 6}}
HULLCLASS = {G_SMALL: "small", G_BIG: "big"}
LICENCES = {
    LIC_A: {"skinID": 100, "duration": -1, "isSingleUse": False},
    LIC_TEMP: {"skinID": 100, "duration": 30},
    LIC_B: {"skinID": 101, "duration": -1},
    LIC_C: {"skinID": 102, "duration": -1},
    LIC_BIG: {"skinID": 103, "duration": -1},
}
SKINS = {
    100: {"types": [HULL_S], "visibleTranquility": True},
    101: {"types": [HULL_S], "visibleTranquility": True},
    102: {"types": [HULL_S, HULL_B], "visibleTranquility": True},   # straddles two classes
    103: {"types": [HULL_B], "visibleTranquility": True},
    104: {"types": [HULL_S], "visibleTranquility": False},
}
CATS = {"skins/small", "skins/big", "services/tools", "services/char"}


def offers(*lines):
    head = "key\tcategory\tselector\tqty\tprice_plx\tbase_plx\tsingle\twhy"
    return S.parse_offers([head] + list(lines))


def run(offs):
    return S.derive(offs, CATS, HULLCLASS, LICENCES, SKINS, _types(), GROUPS, NAMES.get)


class Selectors(unittest.TestCase):
    def test_the_three_kinds_parse(self):
        self.assertEqual(S.parse_selector("const:skill_extractor"), ("const", "skill_extractor"))
        self.assertEqual(S.parse_selector("service:bonus_remap"), ("service", "bonus_remap"))
        k, o = S.parse_selector("skin:hullclass=small:permanent:cap=3")
        self.assertEqual((k, o), ("skin", {"class": "small", "permanent": True, "cap": 3}))
        for bad in ("skin:permanent", "skin:hullclass=a:wat", "nope:x"):
            with self.assertRaises(ValueError):
                S.parse_selector(bad)

    def test_the_header_is_checked(self):
        with self.assertRaises(ValueError):
            S.parse_offers(["wrong"])
        with self.assertRaises(ValueError):
            S.parse_categories(["path\tname"])


class Skins(unittest.TestCase):
    def test_only_permanent_visible_single_class_skins_sell_lowest_licence_first(self):
        got = S.skin_licences(LICENCES, SKINS, _types(), GROUPS, HULLCLASS, "small", True, None)
        self.assertEqual(got, [LIC_A, LIC_B])      # 102 straddles, 104 is hidden, LIC_TEMP is 30 days
        self.assertEqual(S.skin_licences(LICENCES, SKINS, _types(), GROUPS, HULLCLASS, "big", True, None),
                         [LIC_BIG])

    def test_the_cap_keeps_the_lowest_skin_ids(self):
        got = S.skin_licences(LICENCES, SKINS, _types(), GROUPS, HULLCLASS, "small", True, 1)
        self.assertEqual(got, [LIC_A])

    def test_a_non_permanent_selector_keeps_timed_licences_too(self):
        got = S.skin_licences(LICENCES, SKINS, _types(), GROUPS, HULLCLASS, "small", False, None)
        self.assertEqual(got, [LIC_A, LIC_B])      # one licence per SKIN, the lowest id


class Derive(unittest.TestCase):
    def test_rows_types_and_ids(self):
        rows, types, problems = run(offers(
            "extractor\tservices/tools\tconst:skill_extractor\t1\t100\t100\t0\twhy",
            "remap\tservices/char\tservice:bonus_remap\t1\t150\t150\t0\twhy",
            "skin_small\tskins/small\tskin:hullclass=small:permanent\t1\t50\t50\t1\twhy"))
        self.assertEqual(problems, [])
        keys = [r[0] for r in rows]
        self.assertEqual(keys, ["extractor", "remap", "skin_small:100", "skin_small:101"])
        ex = rows[0]
        self.assertEqual((ex[1], ex[5], ex[8], ex[9], ex[10]), (S.fnv31("extractor"), 100, EXTRACTOR, 1, ""))
        self.assertEqual(ex[4], "x y", "tabs and newlines are squashed out of a field")
        self.assertEqual((rows[1][8], rows[1][10]), (0, "bonus_remap"))
        # PLEX is always there; so is every delivered type; services add none.
        self.assertEqual([t[0] for t in types], sorted([PLEX, EXTRACTOR, LIC_A, LIC_B]))
        plex = [t for t in types if t[0] == PLEX][0]
        self.assertEqual((plex[1], plex[2], plex[3]), (G_PLEX, 5, 0))

    def test_no_real_money_and_no_hulls_exist_to_sell(self):
        # The selector grammar has no hull kind: an offer for one is refused.
        with self.assertRaises(ValueError):
            S.parse_selector("hull:frigate")

    def test_problems_are_reported_not_hidden(self):
        _, _, p = run(offers("a\tnowhere\tconst:skill_extractor\t1\t10\t10\t0\twhy"))
        self.assertTrue(any("categories.tsv" in x for x in p))
        _, _, p = run(offers("a\tservices/tools\tconst:nothing\t1\t10\t10\t0\twhy"))
        self.assertTrue(any("unknown" in x for x in p))
        _, _, p = run(offers("a\tservices/char\tservice:nothing\t1\t10\t10\t0\twhy"))
        self.assertTrue(any("service" in x for x in p))
        _, _, p = run(offers("a\tskins/small\tskin:hullclass=ghost:permanent\t1\t10\t10\t0\twhy"))
        self.assertTrue(any("ghost" in x for x in p))
        _, _, p = run(offers("a\tservices/tools\tconst:skill_extractor\t1\t0\t0\t0\twhy"))
        self.assertTrue(any("price" in x for x in p))

    def test_an_id_collision_is_a_problem(self):
        real = S.fnv31
        try:
            S.fnv31 = lambda t: 7
            _, _, p = run(offers(
                "a\tservices/tools\tconst:skill_extractor\t1\t10\t10\t0\twhy",
                "b\tservices/char\tservice:bonus_remap\t1\t10\t10\t0\twhy"))
        finally:
            S.fnv31 = real
        self.assertTrue(any("collide" in x for x in p))

    def test_a_licence_outside_the_skin_category_is_refused(self):
        types = _types()
        types[LIC_A]["groupID"] = G_TOOL
        _, _, p = S.derive(offers("s\tskins/small\tskin:hullclass=small:permanent\t1\t50\t50\t1\twhy"),
                           CATS, HULLCLASS, LICENCES, SKINS, types, GROUPS, NAMES.get)
        self.assertTrue(any("category 91" in x for x in p))

    def test_fnv31_is_stable_and_in_range(self):
        self.assertEqual(S.fnv31("extractor"), S.fnv31("extractor"))
        for k in ("a", "bonus_remap", "skin_frigate:1090"):
            self.assertTrue(1 <= S.fnv31(k) < 2 ** 31)


class Authored(unittest.TestCase):
    """The shipped files parse and agree with each other."""

    def setUp(self):
        d = os.path.join(os.path.dirname(_HERE), "content", "store")
        def lines(name):
            with open(os.path.join(d, name), encoding="utf-8") as fh:
                return fh.read().splitlines()
        self.offers = S.parse_offers(lines("offers.tsv"))
        self.cats = set(S.parse_categories(lines("categories.tsv")))
        self.hull = S.hullclass()

    def test_every_offer_sits_in_a_category_and_names_a_known_selector(self):
        for o in self.offers:
            self.assertIn(o["category"], self.cats, o["key"])
            kind, arg = S.parse_selector(o["selector"])
            if kind == "const":
                self.assertIn(arg, S.CONSTS)
            elif kind == "service":
                self.assertIn(arg, S.SERVICES)
            else:
                self.assertIn(arg["class"], set(self.hull.values()))
                self.assertTrue(arg["permanent"], "SKIN licences are permanent (owner decision)")

    def test_the_owner_prices(self):
        price = {o["key"]: o["price"] for o in self.offers}
        self.assertEqual(
            [price["skin_" + c] for c in ("frigate", "destroyer", "cruiser", "battlecruiser", "battleship", "capital")],
            [50, 75, 125, 175, 250, 500])
        self.assertEqual((price["extractor"], price["bonus_remap"]), (100, 150))

    def test_the_store_sells_no_hulls_no_mct_and_no_injectors(self):
        for o in self.offers:
            kind, arg = S.parse_selector(o["selector"])
            self.assertIn(kind, ("const", "service", "skin"))
            if kind == "const":
                self.assertEqual(S.CONSTS[arg], 40519, "the extractor is the only item const")
        self.assertEqual(set(S.CONSTS), {"skill_extractor"})

    def test_the_hull_class_groups_are_unique_and_the_authored_classes_exist(self):
        self.assertEqual(len(S.hullclass()), sum(len(g) for g in S.HULL_CLASSES.values()))
        for o in self.offers:
            kind, arg = S.parse_selector(o["selector"])
            if kind == "skin":
                self.assertIn(arg["class"], S.HULL_CLASSES)

    def test_the_section_is_a_content_section(self):
        self.assertEqual((S.SECTION, S.DEPENDS), ("store", ()))
        self.assertTrue(callable(S.section) and callable(S.check))

    def test_the_parent_of_every_category_exists(self):
        for c in self.cats:
            if "/" in c:
                self.assertIn(c.rsplit("/", 1)[0], self.cats)


if __name__ == "__main__":
    unittest.main()
