"""Unit tests for the `explore` content section, `tools/content_explore.py` (HK3).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, and no [A] value asserted (the tests name `CLASSES`,
`DATACORE_EXCLUDED`, `FILLER_MAX_PRICE` and `COMPONENT_SHARE`, never their
numbers). The bundle-backed check is `content.py --sections explore --check`,
recorded in `docs/lanes/hk3.md`.
Run: `python3 -I tools/test_content_explore.py -q`.
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
import content_explore as X  # noqa: E402

# Invented type ids (`schema.md` §1.1).
CORE, CORE_FACTION, DECRYPT, IFACE = 990001, 990002, 990003, 990004
SALV_CHEAP, SALV_MID, SALV_DEAR = 990011, 990012, 990013
ANC_SALV, T3_RELIC = 990021, 990022
MOD, MOD_BP, RIG, RIG_BP, AMMO, AMMO_BP = 990031, 990131, 990032, 990132, 990033, 990133
FACTION_MOD, FACTION_BP, UNPUB_BP, UNPUB_BP_PRODUCT = 990034, 990134, 990135, 990035
BOOK_CHEAP, BOOK_DEAR, BOOK_FREE = 990041, 990042, 990043
DATA_CAN, RELIC_CAN, ANCIENT_CAN, PLAIN_CAN, NOT_A_CAN = 990051, 990052, 990053, 990054, 990055
DATA_SKILL = 990999
SUBSYS_CORE, SUBSYS_CORE_UNPUB = 990061, 990062


def _m(group, category, published=True, meta=None, tech=None, base=0.0):
    return {"group_id": group, "category_id": category, "base_price": base,
            "meta_group_id": meta, "tech_level": tech, "published": published}


def _world():
    market = {
        CORE: _m(X.GROUP_DATACORE, 17),
        CORE_FACTION: _m(X.GROUP_DATACORE, 17),
        SUBSYS_CORE: _m(X.GROUP_DATACORE, 17),
        SUBSYS_CORE_UNPUB: _m(X.GROUP_DATACORE, 17, published=False),
        DECRYPT: _m(X.GROUP_DECRYPTOR, 35),
        IFACE: _m(X.GROUP_DATA_INTERFACE, 17),
        SALV_CHEAP: _m(X.GROUP_SALVAGE, 4, base=1.0),
        SALV_MID: _m(X.GROUP_SALVAGE, 4, base=50.0),
        SALV_DEAR: _m(X.GROUP_SALVAGE, 4, base=900.0),
        ANC_SALV: _m(X.GROUP_ANCIENT_SALVAGE, 4),
        T3_RELIC: _m(99971, X.CATEGORY_ANCIENT_RELIC),
        MOD: _m(99053, X.CATEGORY_MODULE, meta=1, tech=1),
        MOD_BP: _m(99105, 9),
        RIG: _m(99774, X.CATEGORY_MODULE, meta=1, tech=1),
        RIG_BP: _m(99105, 9),
        AMMO: _m(99083, X.CATEGORY_CHARGE, tech=1),
        AMMO_BP: _m(99105, 9),
        FACTION_MOD: _m(99053, X.CATEGORY_MODULE, meta=4, tech=1),
        FACTION_BP: _m(99105, 9),
        UNPUB_BP: _m(99105, 9, published=False),
        UNPUB_BP_PRODUCT: _m(99053, X.CATEGORY_MODULE, meta=1, tech=1),
        BOOK_CHEAP: _m(99255, X.CATEGORY_SKILL),
        BOOK_DEAR: _m(99255, X.CATEGORY_SKILL),
        BOOK_FREE: _m(99255, X.CATEGORY_SKILL),
    }
    prices = {
        BOOK_CHEAP: X.FILLER_MAX_PRICE / 2,
        BOOK_DEAR: X.FILLER_MAX_PRICE * 2,
        SALV_DEAR: 1_000.0,
    }

    def t(name, attrs=None):
        return {"name": name, "attrs": attrs or {}}
    typedata = {
        CORE: t("Datacore - Fixture Physics"),
        CORE_FACTION: t("Datacore - %s Fixture" % X.DATACORE_EXCLUDED[0]),
        SUBSYS_CORE: t("Datacore - Fixture %s" % X.T3_DATACORE_NAMES[0]),
        SUBSYS_CORE_UNPUB: t("Datacore - Retired %s" % X.T3_DATACORE_NAMES[0]),
        RIG: t("Fixture Rig I", {X.ATTR_RIG_SIZE: 1.0}),
        MOD: t("Fixture Module I"),
        DATA_CAN: t("Fixture Data Cache",
                    {X.ATTR_HACKABLE: 1.0, X.ATTR_REQUIRED_SKILL_1: float(DATA_SKILL)}),
        RELIC_CAN: t("Fixture Debris",
                     {X.ATTR_HACKABLE: 1.0, X.ATTR_REQUIRED_SKILL_1: float(X.SKILL_ARCHAEOLOGY)}),
        ANCIENT_CAN: t("%s Fixture Debris" % X.ANCIENT_NAMES[0],
                       {X.ATTR_HACKABLE: 1.0, X.ATTR_REQUIRED_SKILL_1: float(X.SKILL_ARCHAEOLOGY)}),
        PLAIN_CAN: t("Fixture Vault", {X.ATTR_HACKABLE: 1.0}),
        NOT_A_CAN: t("Fixture Beacon", {X.ATTR_HACKABLE: 0.0}),
    }
    products = {MOD_BP: MOD, RIG_BP: RIG, AMMO_BP: AMMO, FACTION_BP: FACTION_MOD,
                UNPUB_BP: UNPUB_BP_PRODUCT}
    return market, prices, typedata, products


def _by_class(rows):
    out = {}
    for r in rows:
        out.setdefault((r[0], r[1]), []).append(r[2])
    return out


class ExploreLoot(unittest.TestCase):
    def setUp(self):
        self.rows = X.derive_loot(*_world())
        self.by = _by_class(self.rows)

    def test_every_class_takes_its_group_or_category(self):
        self.assertEqual(self.by[("data", "datacore")], [CORE])
        self.assertEqual(self.by[("data", "decryptor")], [DECRYPT])
        self.assertEqual(self.by[("data", "data_interface")], [IFACE])
        self.assertEqual(self.by[("relic", "salvage")], [SALV_CHEAP, SALV_MID, SALV_DEAR])
        self.assertEqual(self.by[("ancient", "ancient_salvage")], [ANC_SALV])
        self.assertEqual(self.by[("ancient", "t3_relic")], [T3_RELIC])

    def test_an_excluded_datacore_is_left_out(self):
        self.assertNotIn(CORE_FACTION, self.by[("data", "datacore")])

    def test_ancient_data_cans_roll_t3_datacores(self):
        # T3-A / R-T3 §6.1: the Subsystems Engineering cores are the ancient
        # cans' own class, published ones only, and the class is ancient's.
        self.assertEqual(self.by[("ancient", "t3_datacore")], [SUBSYS_CORE])
        terms = [c for c in X.CLASSES if c[1] == "t3_datacore"]
        self.assertEqual(terms, [("ancient", "t3_datacore", X.T3_DATACORE_WEIGHT, 1, 3, False)])
        rows = [r for r in self.rows if r[1] == "t3_datacore"]
        self.assertEqual([(r[0], r[3], r[4], r[5], r[6]) for r in rows],
                         [("ancient", X.T3_DATACORE_WEIGHT, 1, 3, False)])

    def test_ordinary_data_cans_still_exclude_subsystems(self):
        # The exclusion that kept the cores out of data cans is untouched:
        # no other kind lists them, and "Subsystems" is still excluded.
        self.assertIn("Subsystems", X.DATACORE_EXCLUDED)
        for kind_class, ids in self.by.items():
            if kind_class != ("ancient", "t3_datacore"):
                self.assertNotIn(SUBSYS_CORE, ids, kind_class)
                self.assertNotIn(SUBSYS_CORE_UNPUB, ids, kind_class)
        self.assertNotIn(SUBSYS_CORE_UNPUB, self.by[("ancient", "t3_datacore")])

    def test_copies_are_published_t1_blueprints_split_on_rigs(self):
        self.assertEqual(self.by[("data", "bpc")], [MOD_BP, AMMO_BP])
        self.assertEqual(self.by[("relic", "rig_bpc")], [RIG_BP])
        copies = {r[2] for r in self.rows if r[6]}
        self.assertEqual(copies, {MOD_BP, AMMO_BP, RIG_BP})

    def test_the_filler_is_the_priced_books_under_the_cap(self):
        self.assertEqual(self.by[("data", "filler")], [BOOK_CHEAP])

    def test_the_component_is_the_cheapest_share_of_the_salvage(self):
        n = int(3 * X.COMPONENT_SHARE)
        self.assertEqual(self.by[("relic", "relic_component")], [SALV_CHEAP, SALV_MID][:n])

    def test_rows_carry_the_class_terms_and_the_value(self):
        terms = {(k, c): (w, lo, hi, cp) for k, c, w, lo, hi, cp in X.CLASSES}
        for kind, cls, t, w, lo, hi, cp, value in self.rows:
            if cls == X.CAN:
                continue
            self.assertEqual((w, lo, hi, cp), terms[(kind, cls)])
        value = {r[2]: r[7] for r in self.rows if r[1] == "salvage"}
        self.assertEqual(value[SALV_DEAR], 1_000.0, "P_ref wins over basePrice")
        self.assertEqual(value[SALV_MID], 50.0, "basePrice without P_ref")

    def test_every_hackable_type_gets_one_can_row_of_its_kind(self):
        self.assertEqual(self.by[("data", X.CAN)], [DATA_CAN, PLAIN_CAN])
        self.assertEqual(self.by[("relic", X.CAN)], [RELIC_CAN])
        self.assertEqual(self.by[("ancient", X.CAN)], [ANCIENT_CAN])
        self.assertFalse(any(r[2] == NOT_A_CAN for r in self.rows))

    def test_the_rows_round_trip_through_the_file_encoding(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "loot.tsv")
            with open(p, "w", encoding="utf-8") as fh:
                fh.write("#" + "\t".join(X.LOOT_HEADER) + "\n")
                for r in self.rows:
                    fh.write(content.encode_row(r) + "\n")
            back = list(content.read_rows(p))
        self.assertEqual(len(back), len(self.rows))
        self.assertTrue(all(len(f) == len(X.LOOT_HEADER) for f in back))
        self.assertEqual(back[0][6], "1" if self.rows[0][6] else "0")

    def test_the_derivation_is_deterministic(self):
        self.assertEqual(X.derive_loot(*_world()), self.rows)


# --- CAP-LOOT: site loot ---------------------------------------------------------

# Invented ids; the sites are found by can names and faction names.
PIRATE_A, PIRATE_B, OTHER_FACTION = 599101, 599102, 599103
ITEM_NAMES = X.MAINFRAME_ITEMS + (X.SIGNALLER, X.SIGNALLER_ENHANCED, X.DEPOSITION_GUIDE)
ITEM_ID = {n: 991000 + i for i, n in enumerate(ITEM_NAMES)}
COND_ID = {n: 991100 + i for i, n in enumerate(sorted(X.CONDENSER_EMPIRE))}
CAN_ID = {n: 991200 + i for i, n in enumerate(
    X.GHOST_CANS + (X.AEGIS_VAULT_CAN, "Dented Storage Depot", X.SUPERIOR_DEPOT_CAN, "Junk Can"))}
FACTIONS = {PIRATE_A: "Guristas Pirates", PIRATE_B: "Angel Cartel", OTHER_FACTION: "Fixture Folk"}
D_LESSER, D_STANDARD, D_IMPROVED, D_SUPERIOR = 998001, 998002, 998003, 998004
D_DATA, D_NONPIRATE_DATA, D_AEGIS = 998011, 998012, 998021
D_CACHE_LO, D_CACHE_HI = 998031, 998032


def _site_world():
    typedata = {}
    for n, t in {**ITEM_ID, **COND_ID, **CAN_ID}.items():
        typedata[t] = {"name": n, "attrs": {X.ATTR_HACKABLE: 1.0} if t in CAN_ID.values() else {}}
    c = CAN_ID
    dungeons = {
        D_LESSER: (PIRATE_A, 3, [c["Secure Depot"], c["Secure Lab"]]),
        D_STANDARD: (PIRATE_A, 3, [c["Secure Lab"], c["Secure Databank"]]),
        D_IMPROVED: (PIRATE_B, 3, [c["Secure Databank"], c["Secure Mainframe"]]),
        D_SUPERIOR: (PIRATE_B, 3, [c["Secure Mainframe"], c["Secure Vault"]]),
        D_DATA: (PIRATE_B, 2, [c["Junk Can"]]),
        D_NONPIRATE_DATA: (OTHER_FACTION, 2, [c["Junk Can"]]),
        D_AEGIS: (OTHER_FACTION, 5, [c[X.AEGIS_VAULT_CAN], c["Junk Can"]]),
        D_CACHE_LO: (OTHER_FACTION, 3, [c["Dented Storage Depot"]]),
        D_CACHE_HI: (OTHER_FACTION, 4, [c["Dented Storage Depot"], c[X.SUPERIOR_DEPOT_CAN]]),
    }
    return ({d: {"faction_id": f, "difficulty": diff} for d, (f, diff, _c) in dungeons.items()},
            {d: cans for d, (_f, _diff, cans) in dungeons.items()}, typedata, FACTIONS)


class SiteLoot(unittest.TestCase):
    def setUp(self):
        self.rows = X.derive_site_loot(*_site_world())
        self.by_dungeon = {}
        for r in self.rows:
            self.by_dungeon.setdefault(r[0], []).append(r)

    def items(self, d):
        return {r[2] for r in self.by_dungeon.get(d, [])}

    def test_the_mainframe_of_improved_and_superior_drops_the_four(self):
        four = {ITEM_ID[n] for n in X.MAINFRAME_ITEMS}
        for d in (D_IMPROVED, D_SUPERIOR):
            frame = [r for r in self.by_dungeon[d] if r[2] in four]
            self.assertEqual({r[2] for r in frame}, four)
            self.assertTrue(all(r[1] == CAN_ID["Secure Mainframe"] for r in frame))
            self.assertTrue(all(r[3] == X.SITE_MAINFRAME_PPM and r[6] == "" for r in frame))

    def test_lesser_and_standard_drop_them_from_any_can_more_rarely(self):
        four = {ITEM_ID[n] for n in X.MAINFRAME_ITEMS}
        for d in (D_LESSER, D_STANDARD):
            frame = [r for r in self.by_dungeon[d] if r[2] in four]
            self.assertEqual({r[2] for r in frame}, four)
            self.assertTrue(all(r[1] == 0 and r[3] == X.SITE_GHOST_LOW_PPM for r in frame))
        self.assertLess(X.SITE_GHOST_LOW_PPM, X.SITE_MAINFRAME_PPM)

    def test_condensers_follow_the_pirate_faction_in_ghost_and_data_sites(self):
        guristas = COND_ID["AG-Composite Molecular Condenser"]
        angel = COND_ID["AV-Composite Molecular Condenser"]
        self.assertEqual(X.PIRATE_EMPIRE["Guristas Pirates"], X.CONDENSER_EMPIRE[
            "AG-Composite Molecular Condenser"])
        for d in (D_LESSER, D_STANDARD):
            self.assertIn(guristas, self.items(d))
            self.assertNotIn(angel, self.items(d))
        for d in (D_IMPROVED, D_SUPERIOR, D_DATA):
            self.assertIn(angel, self.items(d))
        data = [r for r in self.by_dungeon[D_DATA] if r[2] == angel]
        self.assertEqual([(r[1], r[3], r[6]) for r in data],
                         [(0, X.SITE_DATA_CONDENSER_PPM, X.DATA_CONDENSER_BANDS)])
        self.assertNotIn(D_NONPIRATE_DATA, self.by_dungeon, "only pirate data sites")

    def test_aegis_vault_always_drops_one_signaller_and_rarely_the_enhanced(self):
        rows = {r[2]: r for r in self.by_dungeon[D_AEGIS]}
        sig, enh = rows[ITEM_ID[X.SIGNALLER]], rows[ITEM_ID[X.SIGNALLER_ENHANCED]]
        self.assertEqual((sig[1], sig[3], sig[4], sig[5]),
                         (CAN_ID[X.AEGIS_VAULT_CAN], X.PPM, 1, 1))
        self.assertEqual(enh[3], X.SITE_AEGIS_ENHANCED_PPM)
        self.assertLess(enh[3], X.PPM)
        self.assertEqual(len(self.by_dungeon[D_AEGIS]), 2, "no condenser from an AEGIS site")

    def test_the_superior_cache_always_drops_and_the_others_rarely(self):
        guide = ITEM_ID[X.DEPOSITION_GUIDE]
        hi = {r[1]: r[3] for r in self.by_dungeon[D_CACHE_HI] if r[2] == guide}
        self.assertEqual(hi[CAN_ID[X.SUPERIOR_DEPOT_CAN]], X.SITE_SLEEPER_SUPERIOR_PPM)
        self.assertEqual(hi[CAN_ID["Dented Storage Depot"]], X.SITE_SLEEPER_DEPOT_PPM)
        lo = {r[1]: r[3] for r in self.by_dungeon[D_CACHE_LO] if r[2] == guide}
        self.assertEqual(lo, {CAN_ID["Dented Storage Depot"]: X.SITE_SLEEPER_DEPOT_PPM})
        self.assertLess(X.SITE_SLEEPER_DEPOT_PPM, X.SITE_SLEEPER_SUPERIOR_PPM)

    def test_rows_are_sorted_unique_and_in_range(self):
        self.assertEqual(self.rows, sorted(set(self.rows)))
        for r in self.rows:
            self.assertTrue(0 < r[3] <= X.PPM and 1 <= r[4] <= r[5], r)
            self.assertTrue(set(r[6]) <= set("hlnw"))

    def test_the_header_and_the_encoding_match_the_file(self):
        self.assertEqual("#" + "\t".join(X.SITE_LOOT_HEADER),
                         "#dungeon_id\tcan_type_id\ttype_id\tchance_ppm\tqty_min\tqty_max\tbands")
        self.assertEqual(content.encode_row(self.rows[0]).count("\t"), 6)

    def test_the_variants_are_the_four_condensers_and_every_pirate_maps(self):
        self.assertEqual(set(X.PIRATE_EMPIRE), set(X.PIRATE_FACTIONS))
        self.assertTrue(set(X.PIRATE_EMPIRE.values()) <= set(X.CONDENSER_EMPIRE.values()))
        self.assertEqual(len(set(X.CONDENSER_EMPIRE.values())), 4)

    def test_it_is_deterministic(self):
        self.assertEqual(X.derive_site_loot(*_site_world()), self.rows)

    def test_the_section_depends_on_npc_and_dungeons(self):
        names = [s.name for s in content.order(content.discover(), [X.SECTION])]
        for dep in ("npc", "dungeons"):
            self.assertLess(names.index(dep), names.index(X.SECTION))


if __name__ == "__main__":
    unittest.main()
