"""Unit tests for `tools/content_npc_regions.py` (packet CP11b).

Synthetic data only (plan §1.6): an invented universe with invented ids
(factions 599001+, regions 49000001+, systems 39000001+, types 990001+).
The client-backed checks (Curse, Venal, Stain; the four empires' home
pirates) are `content.py --check`, recorded in `docs/lanes/cp11b.md`.
Run: `python3 -I tools/test_content_npc_regions.py -q`.
"""

import collections
import os
import sys
import tempfile
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_npc_regions as npr  # noqa: E402

P1, P2, P3 = 599001, 599002, 599003  # pirates
E, E2, V = 599010, 599011, 599012    # an empire, its enemy, a vassal


def universe():
    """Regions RA..RF, one step each; `neighbours` empty unless a test adds."""
    regions = {
        49000001: (None, []),  # RA: P1's HQ, and a P2 station -> step 0
        49000002: (P2, []),    # RB: sovereign pirate -> step 1
        49000003: (None, []),  # RC: pirate stations -> step 2
        49000004: (E, []),     # RD: empire -> step 3, despite a P3 station
        49000005: (None, []),  # RE: null, no stations -> step 4
        49000006: (None, []),  # RF: null, unreachable -> no rows
    }
    systems = {
        39000001: (49000001, None, []),
        39000002: (49000002, P2, []),
        39000003: (49000003, None, [39000005]),
        39000004: (49000004, E, []),
        39000014: (49000004, V, []),
        39000005: (49000005, None, [39000003, 39000006]),
        39000006: (49000005, None, [39000005]),
        39000007: (49000006, None, []),
    }
    stations = {39000001: collections.Counter({P2: 2}),
                39000003: collections.Counter({P3: 3, P2: 1, E: 5}),
                39000004: collections.Counter({P3: 1})}
    home = {E: P1, V: P2}
    hq = {P1: 39000001, E: 39000004}
    return regions, systems, stations, {P1, P2, P3}, home, hq


def by_region(table):
    return {r: {f: rule for f, (_w, rule) in rows.items()} for r, rows in table.items()}


class Pirates(unittest.TestCase):

    def test_a_pirate_has_the_floor_executor_and_is_a_mission_enemy(self):
        factions = {P1: {"corporationID": 1999001}, P2: {"corporationID": 1999002},
                    P3: {"corporationID": 1999003}, E: {"corporationID": 1999004}}
        corps = {1999001: {"minSecurity": -10.0}, 1999002: {"minSecurity": -10.0},
                 1999003: {"minSecurity": 0.0}, 1999004: {"minSecurity": 0.0}}
        kills = [(E, P1), (E, P3), (None, P2), (P2, P2)]
        # P2 is never the enemy of a faction-locked mission; P3's executor
        # is above the floor.
        self.assertEqual(npr.pirates(factions, corps, kills), {P1})

    def test_home_pirate_is_the_most_killed_pirate(self):
        kills = [(E, P1)] * 3 + [(E, P2)] + [(E, E2)] * 5 + [(None, P2)] * 9
        self.assertEqual(npr.home_pirates(kills, {P1, P2}), {E: P1})

    def test_a_faction_without_pirate_enemies_takes_its_allys_home_pirate(self):
        kills = [(E, P1)] * 3 + [(E, E2)] * 5 + [(V, E2)] * 2 + [(E2, P2)] * 4 + [(E2, E)] * 6
        self.assertEqual(npr.home_pirates(kills, {P1, P2}), {E: P1, E2: P2, V: P1})

    def test_pirates_offer_no_home_pirate_and_ties_go_to_the_lowest_id(self):
        kills = [(P1, P2), (E, P2), (E, P1)]
        self.assertEqual(npr.home_pirates(kills, {P1, P2}), {E: P1})


class RegionRule(unittest.TestCase):

    def test_r3_steps_0_to_4_in_order(self):
        got = by_region(npr.region_pirates(*universe()))
        self.assertEqual(got, {
            49000001: {P1: "0"},
            49000002: {P2: "1"},
            49000003: {P3: "2", P2: "2"},
            49000004: {P1: "3", P2: "3"},
            49000005: {P3: "4", P2: "4"},
        })

    def test_shares_are_station_counts_votes_and_bfs_ties(self):
        t = npr.region_pirates(*universe())
        self.assertEqual(t[49000003], {P3: (0.75, "2"), P2: (0.25, "2")})
        self.assertEqual(t[49000004], {P1: (0.5, "3"), P2: (0.5, "3")})
        # Both RE systems reach RC's station system first, where P3 and P2
        # tie: each pirate gets half of each system's vote.
        self.assertEqual(t[49000005], {P3: (0.5, "4"), P2: (0.5, "4")})

    def test_an_hq_in_empire_space_does_not_take_step_0(self):
        regions, systems, stations, pset, home, hq = universe()
        hq[P2] = 39000004
        got = by_region(npr.region_pirates(regions, systems, stations, pset, home, hq))
        self.assertEqual(got[49000004], {P1: "3", P2: "3"})

    def test_the_secondary_pirate_is_named_secondary_weight(self):
        regions, systems, stations, pset, home, hq = universe()
        regions[49000005] = (None, [49000001, 49000002])
        t = npr.region_pirates(regions, systems, stations, pset, home, hq)
        self.assertEqual(t[49000005][P1], (npr.SECONDARY_WEIGHT, "secondary"))
        # P2 is already there on its own step: the derived row stays.
        self.assertEqual(t[49000005][P2][1], "4")
        self.assertNotIn(49000006, t)

    def test_an_override_replaces_adds_and_removes(self):
        t = npr.region_pirates(*universe())
        npr.apply_overrides(t, [(49000003, P3, 2.0), (49000003, P2, 0.0),
                                (49000006, P1, 1.0), (49000002, P2, 0.0)])
        self.assertEqual(t[49000003], {P3: (2.0, "override")})
        self.assertEqual(t[49000006], {P1: (1.0, "override")})
        self.assertNotIn(49000002, t)

    def test_rows_are_sorted_and_typed(self):
        rows = npr.region_rows(npr.region_pirates(*universe()))
        self.assertEqual(rows, sorted(rows))
        self.assertTrue(all(isinstance(r[2], float) for r in rows))


class Overrides(unittest.TestCase):

    def test_comments_header_rows_and_a_skipped_row(self):
        lines = ["# preamble\n", "\n", "region_id\tfaction_id\tweight\twhy\n",
                 "# a comment between rows\n", "49000001\t599001\t0.5\t[A] invented\n",
                 "49000001\t599001\n", "x\t599001\t1\twhy\n"]
        rows, problems = npr.parse_overrides(lines)
        self.assertEqual(rows, [(49000001, 599001, 0.5)])
        self.assertEqual(len(problems), 2)

    def test_a_wrong_header_is_refused(self):
        with self.assertRaises(content.ContentError):
            npr.parse_overrides(["region_id\tfaction_id\tweight\n"])

    def test_the_shipped_file_parses(self):
        with open(npr.OVERRIDES, encoding="utf-8") as fh:
            _rows, problems = npr.parse_overrides(fh)
        self.assertEqual(problems, [])


class Files(unittest.TestCase):

    def test_headers_are_schema_md(self):
        with open(os.path.join(_HERE, os.pardir, "docs", "content", "schema.md"),
                  encoding="utf-8") as fh:
            lines = set(fh.read().splitlines())
        for h in (npr.REGIONS_HEADER, npr.HOME_HEADER, npr.TRADES_HEADER, npr.LOOT_HEADER):
            self.assertIn("#" + "\t".join(h), lines)
        self.assertIn("\t".join(npr.OVERRIDE_HEADER), lines)

    def test_trades_are_one_row_per_corp_and_type(self):
        corps = {1999002: {"corporationTrades": {"990002": -1.0, "990001": 0.25}},
                 1999001: {"corporationTrades": None}}
        self.assertEqual(npr.trade_rows(corps),
                         [(1999002, 990001, 0.25), (1999002, 990002, -1.0)])

    def test_the_factions_example_row_decodes(self):
        # `schema.md`'s invented `npc/factions.tsv` example row, verbatim.
        row = ("599001\t1999001\t\t39000001\t1\t1999002\t99101,99102\t"
               "Frigate=990080;Cruiser=990081\t0.1,0.4,0.4,0.1\t1=99201;2=99202\tshield\t"
               "990090,990091\tExample\tArch Example\tDread Example\tExi,Exior,Exum,Exatis,Ex\t"
               "Exa\texamplecorp\tExample Cartel")
        with tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False,
                                         encoding="utf-8") as fh:
            path = fh.name
            fh.write("#header\n" + row + "\n" + row.replace("99101,99102", "", 1)
                     .replace("599001", "599009", 1) + "\n")
        try:
            got = npr.read_factions_tsv(path)
        finally:
            os.remove(path)
        self.assertEqual(got, {599001: {
            "ammo_groups": {1: 99201, 2: 99202}, "tags": [990090, 990091],
            "commander_prefix": "Dread Example",
            "deadspace_stems": ["Exi", "Exior", "Exum", "Exatis", "Ex"], "officers": ["Exa"]}})


class Loot(unittest.TestCase):

    def test_pools_by_kind_tier_and_size(self):
        f = {P1: {"ammo_groups": {1: 99201, 2: 99202}, "tags": [990009],
                  "commander_prefix": "Dread Example", "officers": ["Exa"],
                  "deadspace_stems": ["Exi", "Ex"]}}
        mod, chg = npr.CATEGORY_MODULE, npr.CATEGORY_CHARGE
        types = {
            990001: {"category": mod, "metaGroupID": npr.META_TECH1, "metaLevel": 0,
                     "charge_groups": (99201,)},                      # small weapon
            990002: {"category": mod, "metaGroupID": npr.META_TECH1, "metaLevel": 0,
                     "charge_groups": (99209,)},                      # other family
            990003: {"category": chg, "metaGroupID": None, "groupID": 99202},  # M ammo
            990004: {"category": chg, "metaGroupID": npr.META_FACTION, "groupID": 99202},
            990005: {"category": mod, "metaGroupID": npr.META_FACTION, "metaLevel": 7},
            990006: {"category": mod, "metaGroupID": npr.META_OFFICER, "metaLevel": 11},
            990007: {"category": mod, "metaGroupID": npr.META_DEADSPACE, "metaLevel": 10},
            990008: {"category": mod, "metaGroupID": npr.META_DEADSPACE, "metaLevel": 10},
            990009: {"category": chg, "metaGroupID": None},              # the tag
        }
        sizes = {990001: 1, 990002: 1, 990003: 2, 990004: 2, 990005: 3}
        names = {990005: "Dread Example Gun", 990006: "Exa's Booster",
                 990007: "Exi Hardener", 990008: "Exia Thing"}
        rows = npr.loot_rows(f, types, sizes, lambda t: names.get(t, ""))
        got = collections.defaultdict(set)
        for fid, size, kind, tier, tid, _meta in rows:
            self.assertEqual(fid, P1)
            got[(kind, tier)].add((size, tid))
        all3 = lambda t: {(s, t) for s in npr.SIZES}  # noqa: E731
        self.assertEqual(dict(got), {
            ("weapon", "base"): {(1, 990001)},
            ("ammo", "base"): {(2, 990003)},
            ("faction", "commander"): {(3, 990005)},
            ("officer", "officer"): all3(990006),
            ("deadspace", "deadspace"): all3(990007),
            ("tag", "base"): all3(990009),
        })


class CloneTags(unittest.TestCase):
    """SEC-1c, on invented ids and names."""

    TYPES = {990501: {"category": 11}, 990502: {"category": 11},
             990503: {"category": 11}, 990504: {"category": 17},
             990505: {"category": 17}, 990506: {"category": 7}}
    NAMES = {990501: "Fixa Clone Soldier Cadet", 990502: "Fixb Clone Soldier Cadet",
             990503: "Fixa Clone Soldier Orphan", 990504: "Clone Soldier Cadet Tag",
             990505: "Clone Soldier Spare Tag", 990506: "Clone Soldier Cadet Tag"}

    def test_each_rat_gets_the_tag_of_its_rank(self):
        rows = npr.clone_tag_rows(self.TYPES, self.NAMES.get)
        self.assertEqual(rows, [(990501, 0, "clone-tag", "base", 990504, None),
                                (990502, 0, "clone-tag", "base", 990504, None)])

    def test_the_check_names_a_missing_a_stray_and_a_wrong_row(self):
        types = {i: (t["category"], self.NAMES[i]) for i, t in self.TYPES.items()}
        self.assertEqual(npr.clone_tag_problems(types, [(990501, 990504), (990502, 990504)]), [])
        bad = npr.clone_tag_problems(types, [(990501, 990505), (990999, 990504)])
        self.assertEqual(len(bad), 3, bad)


if __name__ == "__main__":
    unittest.main()
