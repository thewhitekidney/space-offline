"""Unit tests for the `salvage` content section, `tools/content_salvage.py` (X-SALV).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, and no [A] value asserted (the tests name `SIZE_OF_GROUP`,
`BAND_WEIGHTS`, `BAND_QTY`, `FAVOUR_MOD` and `FAVOUR_BOOST`, never their
numbers). The bundle-backed check is `content.py --sections salvage --check`,
recorded in `docs/lanes/x-salv.md`.
Run: `python3 -I tools/test_content_salvage.py -q`.
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
import content_salvage as X  # noqa: E402

# Invented type ids (`schema.md` §1.1).
S1, S2, S3, S4, S5, S6 = 990301, 990302, 990303, 990304, 990305, 990306
UNPUB, OTHER_GROUP = 990311, 990312
FRIG_A, FRIG_B, CRUISER, BS, ODD = 990321, 990322, 990323, 990324, 990325
W_FRIG, W_CRUISER, W_BS, W_ODD, W_MIXED = 990331, 990332, 990333, 990334, 990335
RACE_A, RACE_B = 990341, 990342
GROUP_OF = {size: next(g for g, s in sorted(X.SIZE_OF_GROUP.items()) if s == size)
            for size in X.SIZES}
UNLISTED_GROUP = 990399


def _salv(base, published=True):
    return {"group_id": X.GROUP_SALVAGE, "category_id": 17, "base_price": base,
            "faction_id": None, "race_id": None, "wreck_type_id": None,
            "published": published}


def _hull(size_group, wreck, race=None, faction=None):
    return {"group_id": size_group, "category_id": X.CATEGORY_SHIP, "base_price": 0.0,
            "faction_id": faction, "race_id": race, "wreck_type_id": wreck, "published": True}


def _world():
    market = {
        S1: _salv(1.0), S2: _salv(2.0), S3: _salv(3.0), S4: _salv(4.0),
        S5: _salv(5.0), S6: _salv(6.0),
        UNPUB: _salv(9.0, published=False),
        OTHER_GROUP: dict(_salv(7.0), group_id=990398),
        FRIG_A: _hull(GROUP_OF[1], W_FRIG, race=RACE_A),
        FRIG_B: _hull(GROUP_OF[1], W_FRIG, race=RACE_A),
        CRUISER: _hull(GROUP_OF[2], W_CRUISER, race=RACE_B),
        BS: _hull(GROUP_OF[3], W_BS, faction=RACE_B),
        ODD: _hull(UNLISTED_GROUP, W_ODD),
        # One wreck type, two sizes and two races.
        990326: _hull(GROUP_OF[1], W_MIXED, race=RACE_A),
        990327: _hull(GROUP_OF[3], W_MIXED, race=RACE_B),
        # A hull with no wreck leaves none.
        990328: _hull(GROUP_OF[1], None, race=RACE_A),
    }
    prices = {S6: 100.0}
    return market, prices


class Wrecks(unittest.TestCase):
    def setUp(self):
        self.rows = {r[0]: r for r in X.derive_wrecks(_world()[0])}

    def test_a_wreck_type_takes_its_hulls_size_group(self):
        self.assertEqual(self.rows[W_FRIG][1], 1)
        self.assertEqual(self.rows[W_CRUISER][1], 2)
        self.assertEqual(self.rows[W_BS][1], 3)

    def test_an_unlisted_group_is_the_default_size(self):
        self.assertEqual(self.rows[W_ODD][1], X.DEFAULT_SIZE)

    def test_hulls_sharing_a_wreck_take_the_largest_size_and_no_race_when_they_differ(self):
        self.assertEqual(self.rows[W_MIXED][1:], (3, 0))

    def test_race_is_race_else_faction_else_none(self):
        self.assertEqual(self.rows[W_FRIG][2], RACE_A)
        self.assertEqual(self.rows[W_BS][2], RACE_B)
        self.assertEqual(self.rows[W_ODD][2], 0)

    def test_a_hull_without_a_wreck_leaves_no_row(self):
        self.assertEqual(sorted(self.rows), sorted([W_FRIG, W_CRUISER, W_BS, W_ODD, W_MIXED]))

    def test_every_listed_group_names_a_size_in_range(self):
        self.assertTrue(set(X.SIZE_OF_GROUP.values()) <= set(X.SIZES))
        self.assertEqual(set(GROUP_OF), set(X.SIZES))


class Yields(unittest.TestCase):
    def setUp(self):
        market, prices = _world()
        self.wrecks = X.derive_wrecks(market)
        self.rows = X.derive_yields(market, prices, self.wrecks)

    def of(self, size, race):
        return [r for r in self.rows if r[0] == size and r[1] == race]

    def test_only_published_salvage_is_yielded(self):
        types = {r[2] for r in self.rows}
        self.assertEqual(types, {S1, S2, S3, S4, S5, S6})

    def test_every_size_has_race_zero_rows_and_every_named_race_has_its_own(self):
        for size in X.SIZES:
            self.assertEqual({r[2] for r in self.of(size, 0)}, {S1, S2, S3, S4, S5, S6})
            for race in (RACE_A, RACE_B):
                self.assertEqual({r[2] for r in self.of(size, race)},
                                 {S1, S2, S3, S4, S5, S6})
        self.assertFalse(self.of(1, 990999))

    def test_value_is_p_ref_else_base_price_and_orders_the_bands(self):
        row = {r[2]: r for r in self.of(1, 0)}
        self.assertEqual(row[S6][6], 100.0)
        self.assertEqual(row[S1][6], 1.0)
        # S6 is dearest by P_ref, so it sits in the dear band: with six types
        # the bands are two each.
        self.assertEqual(row[S1][3:6], (X.BAND_WEIGHTS[1][0],) + X.BAND_QTY[0])
        self.assertEqual(row[S3][3:6], (X.BAND_WEIGHTS[1][1],) + X.BAND_QTY[1])
        self.assertEqual(row[S6][3:6], (X.BAND_WEIGHTS[1][2],) + X.BAND_QTY[2])

    def test_bigger_wrecks_lean_to_dearer_bands(self):
        for size in (1, 2):
            self.assertGreater(X.BAND_WEIGHTS[size + 1][2], X.BAND_WEIGHTS[size][2])
            self.assertLess(X.BAND_WEIGHTS[size + 1][0], X.BAND_WEIGHTS[size][0])

    def test_a_race_favours_every_nth_type_of_the_value_order(self):
        plain = {r[2]: r[3] for r in self.of(2, 0)}
        for rank, race in enumerate((RACE_A, RACE_B)):
            row = {r[2]: r[3] for r in self.of(2, race)}
            order = [S1, S2, S3, S4, S5, S6]
            for pos, t in enumerate(order):
                favoured = pos % X.FAVOUR_MOD == rank % X.FAVOUR_MOD
                self.assertEqual(row[t], plain[t] * (X.FAVOUR_BOOST if favoured else 1), t)

    def test_rows_are_sorted_and_deterministic(self):
        keys = [(r[0], r[1], r[2]) for r in self.rows]
        self.assertEqual(keys, sorted(keys))
        market, prices = _world()
        self.assertEqual(X.derive_yields(market, prices, X.derive_wrecks(market)), self.rows)

    def test_fewer_than_three_types_leave_the_cheap_bands_empty(self):
        market = {S1: _salv(1.0)}
        rows = X.derive_yields(market, {}, [])
        self.assertEqual([(r[0], r[1], r[2]) for r in rows], [(s, 0, S1) for s in X.SIZES])
        self.assertEqual(rows[0][3], X.BAND_WEIGHTS[1][2])

    def test_no_salvage_no_rows(self):
        self.assertEqual(X.derive_yields({}, {}, []), [])


class Files(unittest.TestCase):
    def test_the_rows_round_trip_through_the_file_encoding(self):
        market, prices = _world()
        wrecks, yields = X.derive(market, prices)
        with tempfile.TemporaryDirectory() as d:
            for name, header, rows in (("w.tsv", X.WRECKS_HEADER, wrecks),
                                       ("y.tsv", X.YIELDS_HEADER, yields)):
                p = os.path.join(d, name)
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write("#" + "\t".join(header) + "\n")
                    for r in rows:
                        fh.write(content.encode_row(r) + "\n")
                back = list(content.read_rows(p))
                self.assertEqual(len(back), len(rows))
                self.assertTrue(all(len(f) == len(header) for f in back))


if __name__ == "__main__":
    unittest.main()
