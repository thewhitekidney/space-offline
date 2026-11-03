"""Unit tests for the `lp` content section, `tools/content_lp.py` (CP37a).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, and no [A] value asserted (the tests name `ISK_PER_LP`, `ISK_SHARE`,
`LP_FLOOR` and the class thresholds, never their numbers). The client-backed
check is `content.py --sections lp --check`, recorded in `docs/lanes/cp37a.md`.
Run: `python3 -I tools/test_content_lp.py -q`.
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
import content_lp as L  # noqa: E402

# Invented ids (`schema.md` §1.1).
FAC_A, FAC_B, FAC_P = 599001, 599002, 599003
RACE_A = 99
T_BULK, T_NAVY, T_PIRATE, T_UNI, T_SPECIAL, T_FW = 9901, 9902, 9903, 9904, 9905, 9906
PARENT, NAVY_MOD, PIRATE_MOD, IMPLANT, BOOSTER, SHIP, SHIP_BP, OTHER_SHIP_BP = (
    990001, 990002, 990003, 990004, 990005, 990006, 990106, 990107)
OTHER_SHIP, AMMO, UNBRANDED, UNPRICED_NAVY = 990007, 990008, 990009, 990010
MG = 99501


def _corps():
    """FAC_A: `BULK_MIN_CORPS` civilian corps on T_BULK and a military pair on
    T_NAVY; FAC_P: one pirate corp; FAC_B: civilians; a mixed pair on
    T_SPECIAL; a militia on T_FW; and every one of them on T_UNI."""
    corps = {}
    cid = 1999000

    def add(faction, activity, tables, name):
        nonlocal cid
        cid += 1
        corps[cid] = {"faction_id": faction, "main_activity_id": activity,
                      "lp_offer_tables": list(tables), "name": name}
        return cid
    for i in range(L.BULK_MIN_CORPS):
        add(FAC_A, 1, [T_BULK], "Alpha Works %d" % i)
    add(FAC_A, L.ACTIVITY_MILITARY, [T_NAVY], "Alpha Navy")
    add(FAC_A, L.ACTIVITY_MILITARY, [T_NAVY], "Alpha Guard")
    add(FAC_P, L.ACTIVITY_MILITARY, [T_PIRATE], "Pirate Corp")
    add(FAC_B, 1, [T_SPECIAL], "Brandco")
    add(FAC_A, 1, [T_SPECIAL], "Alpha Special")
    add(FAC_A, L.ACTIVITY_MILITARY, [T_FW], "Alpha Militia")
    while len(corps) < L.UNIVERSAL_MIN_CORPS:
        add(FAC_B, 1, [], "Beta Filler %d" % len(corps))
    for c in corps.values():
        c["lp_offer_tables"].append(T_UNI)
    return corps


def _factions(corps):
    militia = next(c for c, r in corps.items() if r["name"] == "Alpha Militia")
    return {FAC_A: {"militia_corporation_id": militia, "member_races": [RACE_A],
                    "prefixes": [], "name": "Alpha State"},
            FAC_B: {"militia_corporation_id": None, "member_races": [1, 2],
                    "prefixes": [], "name": "The Beta Union"},
            FAC_P: {"militia_corporation_id": None, "member_races": [],
                    "prefixes": ["Dread", "Grim Raiders"], "name": "Raiders"}}


def _market():
    def row(cat=7, meta=None, parent=None, race=None, tech=1, group=99001, mg=MG):
        return {"group_id": group, "category_id": cat, "market_group_id": mg,
                "meta_group_id": meta, "tech_level": tech, "parent": parent,
                "race_id": race, "published": True}
    return {PARENT: row(meta=1), NAVY_MOD: row(meta=L.META_FACTION, parent=PARENT),
            PIRATE_MOD: row(meta=L.META_FACTION, parent=PARENT),
            IMPLANT: row(cat=L.CATEGORY_IMPLANT),
            BOOSTER: row(cat=L.CATEGORY_IMPLANT, group=L.GROUP_BOOSTER),
            UNBRANDED: row(cat=L.CATEGORY_IMPLANT),
            SHIP: row(cat=L.CATEGORY_SHIP, race=RACE_A), SHIP_BP: row(cat=9),
            OTHER_SHIP: row(cat=L.CATEGORY_SHIP, race=1), OTHER_SHIP_BP: row(cat=9),
            AMMO: row(cat=L.CATEGORY_CHARGE, meta=L.META_FACTION),
            UNPRICED_NAVY: row(meta=L.META_FACTION, parent=PARENT)}


NAMES = {NAVY_MOD: "Alpha Navy Gyro", PIRATE_MOD: "Dread Gyro", IMPLANT: "Brandco Booster X",
         BOOSTER: "Brandco Drug", UNBRANDED: "Neural Thing", AMMO: "Grim Raiders Slug",
         UNPRICED_NAVY: "Alpha Navy Unpriced"}
PRICES = {PARENT: 50000.0, NAVY_MOD: 4.0e6, PIRATE_MOD: 9.0e6, IMPLANT: 2.0e6,
          BOOSTER: 1.0e6, UNBRANDED: 1.0e6, SHIP_BP: 3.0e6, OTHER_SHIP_BP: 3.0e6,
          AMMO: 300.0}
PRODUCTS = {SHIP_BP: SHIP, OTHER_SHIP_BP: OTHER_SHIP}


def _derive(overrides=None):
    corps = _corps()
    return L.derive_offers(corps, _factions(corps), _market(), PRICES, NAMES, PRODUCTS,
                           overrides or {})


def _by_table(rows):
    out = {}
    for r in rows:
        out.setdefault(r[0], {})[r[2]] = r
    return out


class Shape(unittest.TestCase):
    def test_headers_are_schema_md_lines(self):
        # Copied from `docs/content/schema.md` §2 and §4, byte for byte.
        self.assertEqual("#" + "\t".join(L.OFFERS_HEADER),
                         "#table_id\toffer_id\ttype_id\tqty\tlp_cost\tisk_cost\treq_items")
        self.assertEqual("\t".join(L.OVERRIDES_HEADER),
                         "table_id\ttype_id\taction\tlp_cost\tisk_cost\twhy")

    def test_the_section_is_discovered_after_npc_types2_and_econ(self):
        sections = content.discover()
        self.assertIn(L.SECTION, [s.name for s in sections])
        names = [s.name for s in content.order(sections, [L.SECTION])]
        for dep in ("npc", "types2", "econ"):
            self.assertLess(names.index(dep), names.index(L.SECTION))

    def test_the_shipped_authored_file_parses(self):
        # Well-formedness only (plan §1.4): no value in it is asserted.
        with open(L.OVERRIDES, encoding="utf-8") as fh:
            _rows, problems = L.parse_overrides(fh)
        self.assertEqual(problems, [])

    def test_a_row_encodes_its_required_items_as_a_map(self):
        rows, _ = _derive()
        navy = _by_table(rows)[T_NAVY][NAVY_MOD]
        self.assertEqual(content.encode_row(navy).split("\t")[6], "%d=1" % PARENT)


class CapitalCondensers(unittest.TestCase):
    """CAP-LOOT: a militia's store sells its condenser, found by name."""

    CONDENSER = 990901

    def setUp(self):
        self._was = L.FW_CONDENSER_MILITIA
        L.FW_CONDENSER_MILITIA = (("Alpha Militia", "Fixture Condenser"),
                                  ("No Such Militia", "Fixture Condenser"))

    def tearDown(self):
        L.FW_CONDENSER_MILITIA = self._was

    def _rows(self, condensers, overrides=None):
        corps = _corps()
        return L.derive_offers(corps, _factions(corps), _market(), PRICES, NAMES, PRODUCTS,
                               overrides or {}, condensers)[0]

    def test_the_militias_fw_table_sells_its_condenser_for_the_named_cost(self):
        rows = self._rows({"Fixture Condenser": self.CONDENSER})
        mine = [r for r in rows if r[2] == self.CONDENSER]
        self.assertEqual([r[0] for r in mine], [T_FW], "a militia the bundle lacks adds nothing")
        _t, offer, _ty, qty, lp, isk, req = mine[0]
        self.assertEqual((qty, lp, isk, req),
                         (1, L.FW_CONDENSER_LP, L.FW_CONDENSER_ISK, {}))

    def test_no_condenser_in_the_bundle_adds_nothing(self):
        self.assertEqual(self._rows({}), _derive()[0])
        self.assertEqual(self._rows(None), _derive()[0])

    def test_an_authored_override_wins(self):
        rows = self._rows({"Fixture Condenser": self.CONDENSER},
                          {(T_FW, self.CONDENSER): ("price", 1, 2)})
        mine = [r for r in rows if r[2] == self.CONDENSER]
        self.assertEqual([(r[4], r[5]) for r in mine], [(1, 2)])

    def test_the_four_militias_and_condensers_are_by_name_only(self):
        self.assertEqual(len(self._was), 4)
        self.assertEqual(len({c for _m, c in self._was}), 4)
        for corp, cond in self._was:
            self.assertIn("Condenser", cond)
            self.assertFalse(corp.isdigit() or cond.isdigit())


class Prices(unittest.TestCase):
    def test_lp_times_r_plus_isk_plus_req_is_p_ref(self):
        value, req = 7.3e6, 1.1e6
        lp, isk = L.price(value, req)
        self.assertGreater(lp, L.LP_FLOOR)
        self.assertLessEqual(abs(lp * L.ISK_PER_LP + isk + req - value), 0.5)
        # The ISK share moves only by what rounding `lp` moved.
        raw = (value * (1 - L.ISK_SHARE) - req) / L.ISK_PER_LP
        self.assertLessEqual(abs(isk - L.ISK_SHARE * value),
                             abs(lp - raw) * L.ISK_PER_LP + 0.5)

    def test_an_offer_row_closes_to_p_ref(self):
        rows, _ = _derive()
        for r in rows:
            value = PRICES[r[2]] * r[3]
            req = sum(PRICES[k] * n for k, n in r[6].items())
            if r[4] > L.LP_FLOOR:
                self.assertLessEqual(abs(r[4] * L.ISK_PER_LP + r[5] + req - value), 0.5, r)

    def test_the_floor_only_makes_an_offer_dearer(self):
        value = L.LP_FLOOR * L.ISK_PER_LP / 10.0
        lp, isk = L.price(value, 0.0)
        self.assertEqual(lp, L.LP_FLOOR)
        self.assertGreaterEqual(lp * L.ISK_PER_LP + isk, value)

    def test_a_charge_is_batched_to_clear_the_floor_and_a_module_is_not(self):
        qty = L.batch(PRICES[AMMO], L.CATEGORY_CHARGE)
        self.assertGreaterEqual(PRICES[AMMO] * qty * (1 - L.ISK_SHARE),
                                L.LP_FLOOR * L.ISK_PER_LP)
        self.assertGreater(qty, 1)
        self.assertEqual(L.batch(PRICES[AMMO], L.CATEGORY_MODULE), 1)

    def test_round_sig2(self):
        self.assertEqual([L.round_sig2(x) for x in (0, 7.4, 1234.0, 98765.0)],
                         [0, 7, 1200, 99000])


class Classes(unittest.TestCase):
    def test_each_table_gets_its_class(self):
        _rows, classes = _derive()
        self.assertEqual(classes, {T_BULK: "bulk", T_NAVY: "faction", T_PIRATE: "faction",
                                   T_UNI: "universal", T_SPECIAL: "special", T_FW: "fw"})

    def test_deferred_tables_derive_nothing(self):
        tables = _by_table(_derive()[0])
        self.assertNotIn(T_SPECIAL, tables)
        self.assertNotIn(T_FW, tables)


class Contents(unittest.TestCase):
    def test_a_faction_table_sells_its_stems_items_with_the_t1_parent(self):
        tables = _by_table(_derive()[0])
        self.assertEqual(sorted(tables[T_NAVY]), [NAVY_MOD])
        self.assertEqual(tables[T_NAVY][NAVY_MOD][6], {PARENT: 1})
        # The pirate's stems come from its NPC prefixes.
        self.assertEqual(sorted(tables[T_PIRATE]), [PIRATE_MOD, AMMO])
        self.assertEqual(tables[T_PIRATE][AMMO][6], {})

    def test_universal_tables_sell_branded_meta0_implants_only(self):
        tables = _by_table(_derive()[0])
        self.assertEqual(sorted(tables[T_UNI]), [IMPLANT])

    def test_a_bulk_table_sells_its_races_t1_blueprints(self):
        tables = _by_table(_derive()[0])
        self.assertEqual(sorted(tables[T_BULK]), [SHIP_BP])

    def test_offer_ids_count_up_by_type_id_within_a_table(self):
        rows, _ = _derive()
        for table, offers in _by_table(rows).items():
            got = [offers[t][1] for t in sorted(offers)]
            self.assertEqual(got, [table * L.OFFER_STRIDE + n for n in range(len(got))])

    def test_every_offer_type_closes_into_the_market(self):
        market = _market()
        for r in _derive()[0]:
            self.assertIn(r[2], market)
            for k in r[6]:
                self.assertIn(k, market)


class Overrides(unittest.TestCase):
    def test_an_override_row_wins(self):
        ov = {(T_NAVY, NAVY_MOD): ("price", 7, 70), (T_PIRATE, PIRATE_MOD): ("remove", None, None),
              (T_FW, PARENT): ("add", 5, 50)}
        tables = _by_table(_derive(ov)[0])
        self.assertEqual(tables[T_NAVY][NAVY_MOD][3:], (1, 7, 70, {PARENT: 1}))
        self.assertNotIn(PIRATE_MOD, tables[T_PIRATE])
        self.assertEqual(tables[T_FW][PARENT][1:], (T_FW * L.OFFER_STRIDE, PARENT, 1, 5, 50, {}))

    def test_a_bad_header_is_refused_and_a_bad_row_skipped(self):
        with self.assertRaises(L.LpError):
            L.parse_overrides(["# preamble\n", "table_id\ttype_id\n"])
        rows, problems = L.parse_overrides([
            "# preamble\n", "\t".join(L.OVERRIDES_HEADER) + "\n",
            "9901\t990001\tprice\t3000\t3000000\t[A] x\n",
            "9901\t990002\tremove\t\t\t[A] x\n",
            "9901\t990003\tprice\t\t\t[A] no costs\n",
            "9901\t990004\tsteal\t1\t1\t[A] bad action\n", "9901\t990005\n"])
        self.assertEqual(rows, {(9901, 990001): ("price", 3000, 3000000),
                                (9901, 990002): ("remove", None, None)})
        self.assertEqual(len(problems), 3)


class Stems(unittest.TestCase):
    def test_a_stem_is_the_first_word_without_the_or_apostrophes(self):
        self.assertEqual([L.stem(n) for n in ("The Beta Union", "Exa's Nation", "Exas Gyro",
                                              "Exa’s Thing", "", None)],
                         ["beta", "exas", "exas", "exas", "", ""])


if __name__ == "__main__":
    unittest.main()
