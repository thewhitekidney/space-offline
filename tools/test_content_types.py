"""Unit tests for the `types2` content section, `tools/content_types.py` (CP12).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges. The client-backed checks are `content.py --sections types2 --check`,
recorded in `docs/lanes/cp12.md`. Run: `python3 -I tools/test_content_types.py -q`.
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
import content_types as T  # noqa: E402

G_FRIG, G_CRUISER, G_ORE, G_TAG = 99001, 99002, 99003, 99004
C_SHIP, C_ASTEROID, C_COMMODITY = 6, 25, 17
FRIG_A, FRIG_B, CRUISER, ORE, ORE_COMPRESSED, MINERAL, TAG = (
    990001, 990002, 990003, 990010, 990011, 990030, 990040)


def _types():
    def row(group, **kw):
        r = {"groupID": group, "basePrice": 0.0, "portionSize": 1, "published": 1}
        r.update(kw)
        return r
    return {
        FRIG_A: row(G_FRIG, marketGroupID=99501, basePrice=250000.0, metaGroupID=1,
                    metaLevel=0, techLevel=1, raceID=1, wreckTypeID=990020),
        FRIG_B: row(G_FRIG, factionID=599001, variationParentTypeID=FRIG_A),
        CRUISER: row(G_CRUISER, published=0),
        ORE: row(G_ORE, portionSize=100),
        ORE_COMPRESSED: row(G_ORE),
        MINERAL: row(G_ORE),
        TAG: row(G_TAG),
    }


GROUP_CAT = {G_FRIG: C_SHIP, G_CRUISER: C_SHIP, G_ORE: C_ASTEROID, G_TAG: C_COMMODITY}


class Headers(unittest.TestCase):
    def test_headers_are_schema_md_lines(self):
        # Copied from `docs/content/schema.md` §2, byte for byte.
        want = {
            "market": "#type_id\tgroup_id\tcategory_id\tmarket_group_id\tbase_price\t"
                      "meta_group_id\tmeta_level\ttech_level\tvariation_parent_type_id\t"
                      "faction_id\trace_id\twreck_type_id\tportion_size\tpublished",
            "marketgroups": "#market_group_id\tparent_id\thas_types\ticon_id\tname",
            "typelists": "#list_id\tinc_types\tinc_groups\tinc_cats\texc_types\t"
                         "exc_groups\texc_cats",
            "materials": "#type_id\tmaterial_type_id\tquantity",
            "random": "#type_id\tmaterial_type_id\tqmin\tqmax",
            "compress": "#type_id\tcompressed_type_id",
        }
        got = {
            "market": T.MARKET_HEADER, "marketgroups": T.MARKETGROUPS_HEADER,
            "typelists": T.TYPELISTS_HEADER, "materials": T.MATERIALS_HEADER,
            "random": T.RANDOM_HEADER, "compress": T.COMPRESS_HEADER,
        }
        for k, line in want.items():
            self.assertEqual("#" + "\t".join(got[k]), line, k)

    def test_the_section_is_discovered_beside_the_builtin_types(self):
        names = [s.name for s in content.discover()]
        self.assertIn(T.SECTION, names)
        self.assertIn("types", names)
        self.assertNotIn("compressibletypes", T.CFSD)   # no committed field layout


class Market(unittest.TestCase):
    def test_every_type_has_one_row_and_absent_fields_are_empty(self):
        rows = T.market_rows(_types(), GROUP_CAT)
        self.assertEqual([r[0] for r in rows], sorted(_types()))
        lines = {r[0]: content.encode_row(r).split("\t") for r in rows}
        self.assertEqual(lines[FRIG_A], [
            "990001", "99001", "6", "99501", "250000.0", "1", "0", "1", "", "", "1",
            "990020", "1", "1"])
        b = lines[FRIG_B]
        self.assertEqual((b[3], b[4], b[8], b[9]), ("", "0.0", str(FRIG_A), "599001"))
        self.assertEqual(lines[CRUISER][13], "0")
        self.assertEqual(lines[ORE][12], "100")
        self.assertEqual(len(T.MARKET_HEADER), len(rows[0]))

    def test_a_group_without_a_category_leaves_the_column_empty(self):
        rows = T.market_rows({FRIG_A: {"groupID": 99999, "basePrice": 1.0}}, GROUP_CAT)
        self.assertIsNone(rows[0][2])
        self.assertEqual(rows[0][12], 1, "portionSize absent reads as one")

    def test_market_groups_name_through_the_text_table(self):
        rows = T.marketgroup_rows(
            {99501: {"parentGroupID": 99500, "hasTypes": 1, "iconID": None, "nameID": 7},
             99500: {"hasTypes": 0, "nameID": 8}},
            {7: "Example Frigates", 8: "Example Ships"}.get)
        self.assertEqual(rows, [(99500, None, False, None, "Example Ships"),
                                (99501, 99500, True, None, "Example Frigates")])


class Materials(unittest.TestCase):
    TABLE = {
        ORE: {"materials": [{"materialTypeID": MINERAL, "quantity": 250}],
              "randomizedMaterials": None},
        TAG: {"materials": [],
              "randomizedMaterials": [{"materialTypeID": MINERAL, "quantityMin": 10,
                                       "quantityMax": 40}]},
    }

    def test_synthetic_materials_close_into_synthetic_types(self):
        plain, rnd = T.material_rows(self.TABLE)
        self.assertEqual(plain, [(ORE, MINERAL, 250)])
        self.assertEqual(rnd, [(TAG, MINERAL, 10, 40)])
        known = set(_types())
        self.assertEqual(T.unclosed(plain, (0, 1), known), [])
        self.assertEqual(T.unclosed(rnd, (0, 1), known), [])
        stray = dict(self.TABLE)
        stray[990099] = {"materials": [{"materialTypeID": 990098, "quantity": 1}]}
        plain, _ = T.material_rows(stray)
        self.assertEqual([m[2] for m in T.unclosed(plain, (0, 1), known)], [990099, 990098])

    def test_compress_keys_on_the_source(self):
        rows = T.compress_rows({ORE: ORE_COMPRESSED})
        self.assertEqual(rows, [(ORE, ORE_COMPRESSED)])
        self.assertEqual(T.unclosed(rows, (0, 1), set(_types())), [])

    def test_derive_writes_all_six_files(self):
        data = {"types": _types(), "groups": {g: {"categoryID": c} for g, c in GROUP_CAT.items()},
                "marketgroups": {}, "typelist": {}, "typematerials": self.TABLE,
                "compressibletypes": {ORE: ORE_COMPRESSED}, "text": lambda _m: ""}
        rows = T.derive(data)
        self.assertEqual(sorted(rows), sorted([
            "types/market.tsv", "types/marketgroups.tsv", "types/typelists.tsv",
            "types/materials.tsv", "types/materials-random.tsv", "types/compress.tsv"]))
        self.assertEqual(rows["types/market.tsv"][3][2], C_ASTEROID)


class TypeLists(unittest.TestCase):
    TYPE_GROUP = {t: r["groupID"] for t, r in _types().items()}

    def _row(self, **kw):
        lst = {f: kw.get(f, []) for f in T.LIST_FIELDS}
        return T.typelist_rows({9901: lst})[0]

    def test_included_types_groups_and_categories(self):
        r = self._row(includedTypeIDs=[TAG], includedGroupIDs=[G_CRUISER],
                      includedCategoryIDs=[C_ASTEROID])
        self.assertEqual(T.expand(r, self.TYPE_GROUP, GROUP_CAT),
                         {TAG, CRUISER, ORE, ORE_COMPRESSED, MINERAL})

    def test_exclusions_remove_from_the_inclusions(self):
        r = self._row(includedCategoryIDs=[C_SHIP, C_ASTEROID], excludedTypeIDs=[FRIG_B],
                      excludedGroupIDs=[G_CRUISER])
        self.assertEqual(T.expand(r, self.TYPE_GROUP, GROUP_CAT),
                         {FRIG_A, ORE, ORE_COMPRESSED, MINERAL})
        r = self._row(includedTypeIDs=[FRIG_A, TAG], excludedCategoryIDs=[C_SHIP])
        self.assertEqual(T.expand(r, self.TYPE_GROUP, GROUP_CAT), {TAG})

    def test_the_row_round_trips_through_the_list_encoding(self):
        r = self._row(includedGroupIDs=[G_FRIG, G_CRUISER], excludedTypeIDs=[FRIG_B])
        line = content.encode_row(r)
        self.assertEqual(line, "9901\t\t99001,99002\t\t990002\t\t")
        back = line.split("\t")
        self.assertEqual((int(back[0]),) + tuple(content.decode_list(x) for x in back[1:]), r)


class ExpectedCounts(unittest.TestCase):
    def test_pinned_and_old_builds_are_verified(self):
        counts, ok = T.expected_counts(3662168)
        self.assertTrue(ok)
        self.assertEqual(counts["types/market.tsv"], T.EXPECT_BY_BUILD[3662168]["types/market.tsv"])
        for b in (3619879, 3628513):
            counts, ok = T.expected_counts(b)
            self.assertTrue(ok)
            self.assertEqual(counts, T.EXPECT)

    def test_no_build_means_the_reference(self):
        self.assertEqual(T.expected_counts(None), T.expected_counts(3662168))

    def test_an_unlisted_build_is_unverified_and_hinted_with_the_base(self):
        counts, ok = T.expected_counts(1234)
        self.assertFalse(ok)
        self.assertEqual(counts, T.EXPECT)


if __name__ == "__main__":
    unittest.main()
