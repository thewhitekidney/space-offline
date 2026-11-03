"""Unit tests for the `econ` content section, `tools/content_econ.py` (CP24).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, and no [A] value asserted (the tests name `SALVAGE_TIER_BASE` and
read the multipliers from the fixture, never the shipped file). The
client-backed checks are `content.py --sections econ --check`, recorded in
`docs/lanes/cp24.md`. Run: `python3 -I tools/test_content_econ.py -q`.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_econ as E  # noqa: E402

# Invented ids (`schema.md` §1.1).
MIN_A, MIN_B, ORE, FRIG, FRIG_BP, MOD, MOD_BP, VARIANT = (
    990030, 990031, 990010, 990001, 990101, 990002, 990102, 990003)
GOO, GOO_BP, P1, P2, UNPRICED, SALVAGE, RIG_T1, RIG_T2 = (
    990040, 990103, 990050, 990051, 990060, 990070, 990080, 990081)
MG = 99501  # a market group
META_FAKE = 99


def _market():
    def row(base=0.0, meta=None, parent=None, portion=1, group=99001, mg=MG):
        return {"group_id": group, "category_id": 7, "market_group_id": mg,
                "base_price": base, "meta_group_id": meta, "parent": parent,
                "portion_size": portion}
    return {
        MIN_A: row(base=2.0), MIN_B: row(base=8.0), ORE: row(base=1.0, portion=100),
        FRIG: row(base=1.0, meta=1), FRIG_BP: row(base=500.0), MOD: row(meta=1),
        VARIANT: row(meta=META_FAKE, parent=MOD), GOO: row(), P1: row(base=5.0),
        P2: row(), UNPRICED: row(mg=None), SALVAGE: row(group=E.GROUP_SALVAGE),
        RIG_T1: row(meta=1), RIG_T2: row(meta=E.META_T2),
    }


def _bp(time, runs, **acts):
    return {"blueprintTypeID": 0, "maxProductionLimit": runs,
            "activities": {k: dict(v, time=time) for k, v in acts.items()}}


def _blueprints():
    frig = _bp(600, 30,
               manufacturing={"materials": [{"typeID": MIN_A, "quantity": 100},
                                            {"typeID": MIN_B, "quantity": 10}],
                              "products": [{"typeID": FRIG, "quantity": 1}],
                              "skills": [{"typeID": 990900, "level": 1}]},
               copying={},
               invention={"materials": [{"typeID": MIN_A, "quantity": 1}],
                          "products": [{"typeID": MOD_BP, "quantity": 10,
                                        "probability": 0.25}]})
    frig["blueprintTypeID"] = FRIG_BP
    mod = _bp(60, 10, manufacturing={
        "materials": [{"typeID": MIN_A, "quantity": 5}, {"typeID": UNPRICED, "quantity": 1}],
        "products": [{"typeID": MOD, "quantity": 1}]})
    mod["blueprintTypeID"] = MOD_BP
    goo = _bp(3600, 1, reaction={"materials": [{"typeID": MIN_B, "quantity": 30}],
                                 "products": [{"typeID": GOO, "quantity": 3}]})
    goo["blueprintTypeID"] = GOO_BP
    rigs = {}
    for bp, rig in ((990104, RIG_T1), (990105, RIG_T2)):
        rigs[bp] = _bp(60, 10, manufacturing={
            "materials": [{"typeID": SALVAGE, "quantity": 2}],
            "products": [{"typeID": rig, "quantity": 1}]})
        rigs[bp]["blueprintTypeID"] = bp
    return {FRIG_BP: frig, MOD_BP: mod, GOO_BP: goo, **rigs}


MATERIALS = {ORE: [(MIN_A, 400)], FRIG: [(MIN_A, 100), (MIN_B, 10)],
             MOD: [(MIN_B, 50)]}
PI = {P2: ([(P1, 40)], 5)}
META = {1: 1.0, META_FAKE: 3.0}


def _oracle(overrides=None, salvage=True):
    m = _market()
    rmap = E.recipes(E.blueprint_rows(_blueprints()))
    rule = E.salvage_prices(m, rmap) if salvage else {}
    return E.Oracle(m, rmap, PI, MATERIALS, META, overrides, rule)


class Headers(unittest.TestCase):
    def test_headers_are_schema_md_lines(self):
        # Copied from `docs/content/schema.md` §2 and §4, byte for byte.
        want = {
            "blueprints": "#bp_type_id\tactivity_id\tkind\ttype_id\tquantity\t"
                          "probability\ttime\tmax_runs",
            "stations": "#station_id\tsystem_id\tregion_id\towner_id\toperation_id\t"
                        "services\treproc_eff\treproc_take",
            "operations": "#operation_id\tactivity_id\tservices\tstation_types\t"
                          "manufacturing_factor\tresearch_factor",
            "prices": "#type_id\tref_price\tsource\tfloor_price\tcategory_id\tgroup_id\t"
                      "meta_group_id\tportion_size",
            "overrides": "#type_id\tprice\twhy",
            "meta": "#meta_group_id\tmult\twhy",
        }
        got = {"blueprints": E.BLUEPRINTS_HEADER, "stations": E.STATIONS_HEADER,
               "operations": E.OPERATIONS_HEADER, "prices": E.PRICES_HEADER,
               "overrides": E.OVERRIDES_HEADER, "meta": E.META_MULT_HEADER}
        for k, line in want.items():
            self.assertEqual("#" + "\t".join(got[k]), line, k)

    def test_the_section_is_discovered_after_types2(self):
        sections = content.discover()
        self.assertIn(E.SECTION, [s.name for s in sections])
        names = [s.name for s in content.order(sections, [E.SECTION])]
        self.assertLess(names.index("types2"), names.index(E.SECTION))

    def test_the_shipped_authored_files_parse(self):
        # Well-formedness only (plan §1.4): no value in them is asserted.
        for path, parse in ((E.META_MULT, E.parse_meta_mult),
                            (E.OVERRIDES, E.parse_overrides)):
            with open(path, encoding="utf-8") as fh:
                _rows, problems = parse(fh)
            self.assertEqual(problems, [], path)


class Oracle(unittest.TestCase):
    def test_a_leaf_is_its_base_price(self):
        o = _oracle()
        self.assertEqual(o.price(MIN_A)[:2], (2.0, "base"))
        self.assertEqual(o.price(MIN_B)[:2], (8.0, "base"))

    def test_a_manufactured_type_is_the_recursion(self):
        o = _oracle()
        want = (100 * o.price(MIN_A)[0] + 10 * o.price(MIN_B)[0]) / 1
        self.assertEqual(o.price(FRIG)[:2], (want, "recipe"))
        self.assertNotEqual(want, _market()[FRIG]["base_price"])
        goo = o.price(GOO)
        self.assertEqual(goo[:2], (30 * o.price(MIN_B)[0] / 3, "reaction"))

    def test_pi_divides_by_the_output_quantity(self):
        o = _oracle()
        self.assertEqual(o.price(P2)[:2], (40 * o.price(P1)[0] / 5, "pi"))

    def test_an_unpriced_material_falls_through_to_the_parent_rule(self):
        o = _oracle()
        self.assertIsNone(o.price(UNPRICED))
        # MOD's recipe needs UNPRICED and it has no basePrice: only its floor
        # is known, and nothing prices it. VARIANT's parent is MOD.
        self.assertIsNone(o.price(MOD))
        self.assertIsNone(o.price(VARIANT))
        o = _oracle(overrides={MOD: 1000.0})
        self.assertEqual(o.price(VARIANT)[:2], (1000.0 * META[META_FAKE], "parent"))

    def test_every_priced_type_is_at_least_its_floor(self):
        o = _oracle(overrides={MOD: 1.0})
        rows = E.price_rows(_market(), o)
        self.assertTrue(rows)
        for r in rows:
            self.assertGreaterEqual(r[1], r[3], r)
        ore = o.price(ORE)
        self.assertEqual(ore[2], 400 * o.price(MIN_A)[0] / 100)
        self.assertEqual(ore[0], ore[2], "the floor lifts a base price under it")
        mod = o.price(MOD)
        self.assertEqual(mod, (50 * o.price(MIN_B)[0], "override", 50 * o.price(MIN_B)[0]))

    def test_an_override_replaces_the_derived_price(self):
        o = _oracle(overrides={FRIG: 123456.0})
        self.assertEqual(o.price(FRIG)[:2], (123456.0, "override"))
        self.assertEqual(_oracle(overrides={P2: 7.5e6}).price(P2)[:2], (7.5e6, "override"))

    def test_salvage_is_priced_by_tier_where_nothing_else_prices_it(self):
        o = _oracle()
        self.assertEqual(o.price(SALVAGE)[:2], (E.SALVAGE_TIER_BASE * 3, "override"))
        self.assertEqual(o.price(RIG_T1)[:2], (2 * o.price(SALVAGE)[0], "recipe"))
        self.assertIsNone(_oracle(salvage=False).price(RIG_T1))
        self.assertEqual(_oracle(overrides={SALVAGE: 5.0}).price(SALVAGE)[0], 5.0)

    def test_a_cycle_and_the_depth_cap_leave_a_type_unpriced(self):
        m = {990500 + i: {"base_price": 0.0, "portion_size": 1} for i in range(40)}
        cyc = {990500: [(1, 1, [(990501, 1)], 1)], 990501: [(1, 1, [(990500, 1)], 1)]}
        self.assertIsNone(E.Oracle(m, cyc, {}, {}, {}).price(990500))
        m[990539]["base_price"] = 1.0
        chain = {990500 + i: [(1, 1, [(990501 + i, 1)], 1)] for i in range(39)}
        o = E.Oracle(m, chain, {}, {}, {})
        self.assertIsNone(o.price(990500), "deeper than DEPTH_CAP")
        self.assertEqual(o.price(990539 - E.DEPTH_CAP)[0], 1.0)
        self.assertEqual(E.Oracle(m, chain, {}, {}, {}).price(990539 - E.DEPTH_CAP)[0], 1.0,
                         "the answer does not depend on what was asked first")

    def test_price_rows_are_market_types_only(self):
        rows = E.price_rows(_market(), _oracle())
        self.assertNotIn(UNPRICED, [r[0] for r in rows])
        self.assertEqual([r[0] for r in rows], sorted(r[0] for r in rows))
        self.assertEqual(len(rows[0]), len(E.PRICES_HEADER))


class Blueprints(unittest.TestCase):
    def test_rows_round_trip_the_documents(self):
        bps = _blueprints()
        rows = E.blueprint_rows(bps)
        lines = [content.encode_row(r).split("\t") for r in rows]
        decoded = [(int(f[0]), int(f[1]), f[2], int(f[3]), int(f[4]),
                    float(f[5]) if f[5] else None, int(f[6]), int(f[7])) for f in lines]
        self.assertEqual(E.blueprints_from_rows(decoded),
                         {k: E.normalise(v) for k, v in bps.items()})

    def test_an_activity_with_no_lists_keeps_its_row(self):
        rows = [r for r in E.blueprint_rows(_blueprints()) if r[0] == FRIG_BP]
        copying = [r for r in rows if r[1] == E.ACTIVITY_IDS["copying"]]
        self.assertEqual(copying, [(FRIG_BP, 5, "activity", 0, 0, None, 600, 30)])
        self.assertEqual([r[1] for r in rows], sorted(r[1] for r in rows))
        inv = [r for r in rows if r[2] == "product" and r[1] == 8]
        self.assertEqual(inv, [(FRIG_BP, 8, "product", MOD_BP, 10, 0.25, 600, 30)])

    def test_an_unknown_activity_is_refused(self):
        with self.assertRaises(E.EconError):
            E.blueprint_rows({FRIG_BP: _bp(1, 1, smelting={})})

    def test_the_blueprint_cache_decodes_from_synthetic_sqlite_bytes(self):
        # `blueprints.static` is an fsdlite cache: `cache(key, value, time)`
        # with one JSON document per blueprint (`fsd.load_sqlite`).
        fsd = content._imp("fsd")  # noqa: SLF001
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "blueprints.static")
            con = sqlite3.connect(path)
            con.execute("CREATE TABLE cache (key, value, time)")
            con.execute("CREATE TABLE indexes (key, value)")
            for k, v in _blueprints().items():
                con.execute("INSERT INTO cache VALUES (?, ?, 0)", (str(k), json.dumps(v)))
            con.commit()
            con.close()
            got = {int(k): v for k, v in fsd.sqlite_rows(path)}
        self.assertEqual(E.blueprint_rows(got), E.blueprint_rows(_blueprints()))


class Stations(unittest.TestCase):
    OPS = {99: {"activityID": 1, "services": [7, 21, 25], "stationTypes": {2: 990002, 1: 990001},
                "manufacturingFactor": 1.0, "researchFactor": 0.98}}

    def test_one_sorted_row_per_station_with_its_operation_services(self):
        st = [(69000002, 39000001, 1999001, 99, 0.5, 0.05),
              (69000001, 39000002, 1999002, 99, 0.4, 0.025)]
        rows = E.station_rows(st, {39000001: 19000001, 39000002: 19000002}, self.OPS)
        self.assertEqual([r[0] for r in rows], [69000001, 69000002])
        self.assertEqual(content.encode_row(rows[0]),
                         "69000001\t39000002\t19000002\t1999002\t99\t7,21,25\t0.4\t0.025")

    def test_operations_write_the_station_type_map(self):
        self.assertEqual(content.encode_row(E.operation_rows(self.OPS)[0]),
                         "99\t1\t7,21,25\t1=990001;2=990002\t1.0\t0.98")


class Authored(unittest.TestCase):
    def test_a_bad_header_is_refused_and_a_bad_row_skipped(self):
        with self.assertRaises(E.EconError):
            E.parse_overrides(["# preamble\n", "type_id\tprice\n"])
        rows, problems = E.parse_overrides([
            "# preamble\n", "type_id\tprice\twhy\n", "990300\t18000.0\t[A] x\n",
            "990301\t-1\t[A] negative\n", "990302\t5\n"])
        self.assertEqual(rows, {990300: 18000.0})
        self.assertEqual(len(problems), 2)
        mult, _ = E.parse_meta_mult(["meta_group_id\tmult\twhy\n", "99\t2.0\t[A] x\n"])
        self.assertEqual(mult, {99: 2.0})


if __name__ == "__main__":
    unittest.main()
