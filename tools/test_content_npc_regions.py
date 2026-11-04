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
            "ammo_groups": {1: 99201, 2: 99202}, "tank": "shield", "tags": [990090, 990091],
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


NOT_TECH1 = 990  # an invented meta group id that is not META_TECH1


class Equipment(unittest.TestCase):
    """C6 R7's tank, prop and ewar pools (#123), on invented ids, groups and names."""

    def setUp(self):
        # Invented group ids stand in for the client's, which the module
        # constants name; the tests patch them so no real id is pinned.
        self.patches = (npr.GROUP_PROP, npr.GROUP_PLATE, npr.TANK_GROUPS, npr.EWAR_GROUPS)
        npr.GROUP_PROP, npr.GROUP_PLATE = 98001, 98021
        npr.TANK_GROUPS = {"shield": (98010, 98011), "armor": (98020, 98021)}
        npr.EWAR_GROUPS = {"SCRAM": (98030,), "WEB": (98031,), "NEUT": (98032,),
                           "EWAR": (98033, 98034)}

    def tearDown(self):
        npr.GROUP_PROP, npr.GROUP_PLATE, npr.TANK_GROUPS, npr.EWAR_GROUPS = self.patches

    def rows(self, tank, needs, types, names):
        f = {P1: {"tank": tank, "ammo_groups": {}, "tags": []}}
        return {(s, k, t, m) for _f, s, k, tier, t, m in npr.loot_rows(
            f, types, {}, lambda t: names.get(t, ""), {P1: set(needs)}) if tier == "base"}

    def mod(self, group, meta=npr.META_TECH1, level=1):
        return {"category": npr.CATEGORY_MODULE, "metaGroupID": meta, "metaLevel": level,
                "groupID": group}

    def test_prop_goes_to_the_size_of_its_mn_with_the_meta_level_kept(self):
        types = {990601: self.mod(98001, level=0), 990602: self.mod(98001),
                 990603: self.mod(98001), 990604: self.mod(98001),
                 990605: self.mod(98001, meta=npr.META_FACTION, level=8),
                 990606: self.mod(98001)}
        names = {990601: "1MN Example Afterburner I", 990602: "50MN Compact Microwarpdrive",
                 990603: "100MN Enduring Afterburner", 990604: "20000MN Afterburner I",
                 990605: "Dread 5MN Microwarpdrive", 990606: "Afterburner"}
        self.assertEqual(self.rows(None, [], types, names), {
            (1, "prop", 990601, 0), (2, "prop", 990602, 1), (3, "prop", 990603, 1)})

    def test_tank_is_the_factions_own_groups_by_size_word_or_plate_mm(self):
        types = {990611: self.mod(98010), 990612: self.mod(98011), 990613: self.mod(98020),
                 990614: self.mod(98020), 990615: self.mod(98010), 990616: self.mod(98010)}
        names = {990611: "Small Example Booster I", 990612: "Medium Example Extender I",
                 990613: "X-Large Example Repairer II", 990614: "Example Repairer",
                 990615: "Large Example Booster", 990616: "Example Booster"}
        self.assertEqual(self.rows("shield", [], types, names), {
            (1, "tank", 990611, 1), (2, "tank", 990612, 1), (3, "tank", 990615, 1)})
        self.assertEqual(self.rows("armor", [], types, names), {(3, "tank", 990613, 1)})
        self.assertEqual(self.rows(None, [], types, names), set())

    def test_plates_are_sized_by_their_mm(self):
        types = {i: self.mod(98021) for i in range(990641, 990646)}
        names = dict(zip(types, ("100mm Example Plates I", "400mm Example Plates I",
                                 "1600mm Example Plates I", "9000mm Example Plates",
                                 "Example Plates")))
        self.assertEqual(self.rows("armor", [], types, names), {
            (1, "tank", 990641, 1), (2, "tank", 990642, 1), (3, "tank", 990643, 1)})

    def test_ewar_follows_what_the_factions_npcs_need(self):
        types = {990621: self.mod(98030), 990622: self.mod(98031), 990623: self.mod(98032),
                 990624: self.mod(98033), 990625: self.mod(98034), 990626: self.mod(98030)}
        names = {990621: "Scram I", 990622: "Web I", 990623: "Heavy Neut I",
                 990624: "Painter I", 990625: "Jammer I", 990626: "Medium Scram I"}
        all3 = lambda t: {(s, "ewar", t, 1) for s in npr.SIZES}  # noqa: E731
        self.assertEqual(self.rows(None, ["SCRAM"], types, names),
                         all3(990621) | {(2, "ewar", 990626, 1)})
        self.assertEqual(self.rows(None, ["WEB", "NEUT"], types, names),
                         all3(990622) | {(3, "ewar", 990623, 1)})
        self.assertEqual(self.rows(None, ["EWAR"], types, names), all3(990624) | all3(990625))
        self.assertEqual(self.rows(None, ["TURRET"], types, names), set())

    def test_only_tech1_modules_and_no_charge_or_seller(self):
        types = {990631: self.mod(98001, meta=NOT_TECH1, level=5),
                 990632: {"category": npr.CATEGORY_CHARGE, "metaGroupID": npr.META_TECH1,
                          "groupID": 98001}}
        names = {990631: "1MN Example Afterburner II", 990632: "1MN Example Afterburner I"}
        self.assertEqual(self.rows("shield", ["SCRAM"], types, names), set())

    def test_capital_heavy_and_unsized_neuts_are_in_no_pool(self):
        types = {i: self.mod(g) for i, g in ((990651, 98032), (990652, 98032), (990653, 98032),
                                              (990654, 98030), (990655, 98031), (990656, 98010),
                                              (990657, 98001))}
        names = {990651: "Capital Example Neut I", 990652: "Example Neut I",
                 990653: "Small Example Neut I", 990654: "Heavy Example Scram",
                 990655: "Capital Example Web", 990656: "Capital Example Booster",
                 990657: "Capital Example Afterburner"}
        got = self.rows("shield", ["NEUT", "SCRAM", "WEB"], types, names)
        self.assertEqual(got, {(1, "ewar", 990653, 1)})

    def test_the_size_word_ladder(self):
        types = {i: self.mod(98010) for i in range(990661, 990666)}
        names = dict(zip(types, ("Micro Example Booster", "Small Example Booster",
                                 "Medium Example Booster", "Large Example Booster",
                                 "X-Large Example Booster")))
        self.assertEqual(self.rows("shield", [], types, names), {
            (1, "tank", 990661, 1), (1, "tank", 990662, 1), (2, "tank", 990663, 1),
            (3, "tank", 990664, 1), (3, "tank", 990665, 1)})

    def test_the_check_names_each_empty_equipment_kind(self):
        self.assertEqual(npr.equipment_problems(
            {"weapon": 3, "tank": 1, "prop": 1, "ewar": 1, "ammo": 9}), [])
        bad = npr.equipment_problems({"weapon": 3, "tank": 1, "prop": 1, "ammo": 9})
        self.assertEqual(len(bad), 1, bad)
        self.assertIn("no ewar rows", bad[0])
        self.assertIn("re-extract", bad[0])
        self.assertEqual(len(npr.equipment_problems({})), 4)

    def test_an_empty_tank_column_is_no_tank(self):
        row = ("599001\t1999001\t\t39000001\t1\t1999002\t99101\t\t\t1=99201\t\t"
               "\t\t\t\t\t\t\tExample Cartel")
        with tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False,
                                         encoding="utf-8") as fh:
            path = fh.name
            fh.write("#h\n" + row + "\n")
        try:
            got = npr.read_factions_tsv(path)
        finally:
            os.remove(path)
        self.assertIsNone(got[599001]["tank"])

    def test_the_needs_file_is_per_faction(self):
        with tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False,
                                         encoding="utf-8") as fh:
            path = fh.name
            fh.write("#h\n1\t2\t599001\tgroup\tRoaming\tFrigate\tbase\t\tTURRET,SCRAM\tDPS\n"
                     "3\t2\t599001\tgroup\tRoaming\tFrigate\tbase\t\tWEB\tDPS\n"
                     "4\t2\t599002\tgroup\tRoaming\tFrigate\tbase\t\t\tDPS\n")
        try:
            got = npr.read_faction_needs(path)
        finally:
            os.remove(path)
        self.assertEqual(got, {599001: {"TURRET", "SCRAM", "WEB"}, 599002: set()})
        self.assertEqual(npr.read_faction_needs(path), {})


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
