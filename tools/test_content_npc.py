"""Unit tests for the `npc` content section, `tools/content_npc.py` (CP11a).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, invented faction names fed to the grammar through `grammar()`. The
client-backed checks are `content.py --sections npc --check`, recorded in
`docs/lanes/cp11a.md`. Run: `python3 -I tools/test_content_npc.py -q`.
"""

from __future__ import annotations

import os
import re
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_npc as N  # noqa: E402

EX, OTHER, PLAIN = 599001, 599002, 599003      # factions
EXEC, SPAWN_OWNER, OTHER_CORP = 1999001, 1999002, 1999003
G_FRIG, G_CMDR, G_DS, G_OTHER, G_TAGS = 99101, 99102, 99103, 99104, 99105
TARGET_ATTACK, SCRAM_EFFECT, REMOTE_REP = 10, 563, 6741   # [V] effect ids


def _type(group, name, faction=None, graphic=None, wreck=None, price=0.0):
    return {"group": group, "faction": faction, "graphic": graphic, "wreck": wreck,
            "price": price, "published": 1, "meta": None, "name": name}


def fixture():
    """One grammar faction (`Example Cartel`), one other grammar faction
    (`Sample`) and one faction outside the grammar."""
    types, dog = {}, {}

    def add(tid, group, name, bounty, effects=(TARGET_ATTACK,), extra=None, **kw):
        types[tid] = _type(group, name, **kw)
        attrs = {N.A_BOUNTY: bounty, N.A_HP: 100.0, N.A_ATTACK_RANGE: 5000.0,
                 114: 0.0, 116: 0.0, 117: 2.0, 118: 1.0}
        attrs.update(extra or {})
        dog[tid] = (attrs, list(effects))

    for i, b in enumerate((1000.0, 2000.0, 3000.0)):
        add(990101 + i, G_FRIG, "Example Hull%d" % i, b, graphic=7001, wreck=990901)
        add(990111 + i, G_FRIG, "Arch Example Hull%d" % i, b * 10, graphic=7001, wreck=990901)
        add(990121 + i, G_FRIG, "Alias Hull%d" % i, b, graphic=7001, wreck=990901)
        add(990131 + i, G_CMDR, "Dread Example Hull%d" % i, b * 30, graphic=7001)
        add(990141 + i, G_DS, "Exii Hull%d" % i, b * 2, graphic=7001)
    add(990151, G_OTHER, "Sample Hull", 500.0, graphic=7002)
    add(990152, G_OTHER, "Sample Tackle", 500.0, effects=(TARGET_ATTACK, SCRAM_EFFECT),
        graphic=7002)
    types[990201] = _type(G_TAGS, "Example Copper Tag", price=1000.0)
    types[990202] = _type(G_TAGS, "Example Gold Tag", price=500.0)
    types[990203] = _type(G_TAGS, "Example Retired Tag", price=0.0)
    types[990301] = _type(7, "Zarzakh Example Roaming Spawner")
    groups = {G_FRIG: (11, "Asteroid Example Cartel Frigate"),
              G_CMDR: (11, "Asteroid Example Cartel Commander Frigate"),
              G_DS: (11, "Deadspace Example Cartel Frigate"),
              G_OTHER: (11, "Asteroid Samples Cruiser"),
              G_TAGS: (17, N.CRIMINAL_TAGS), 7: (2, "Cloud")}
    factions = {
        EX: {"corporationID": EXEC, "militiaCorporationID": None, "solarSystemID": 39000001,
             "memberRaces": [1], "name": "Example Cartel"},
        OTHER: {"corporationID": OTHER_CORP, "militiaCorporationID": None,
                "solarSystemID": 39000002, "memberRaces": [], "name": "Samples"},
        PLAIN: {"corporationID": OTHER_CORP, "militiaCorporationID": 1999004,
                "solarSystemID": 39000003, "memberRaces": [2, 4], "name": "Plain State"},
    }
    corps = {
        EXEC: {"factionID": EX, "stationID": 69000001, "solarSystemID": 39000001,
               "taxRate": 0.1, "minSecurity": 0.0, "mainActivityID": 5, "size": "M",
               "extent": "R", "lpOfferTables": [9901], "tickerName": "EXC",
               "name": "Example Corporation"},
        SPAWN_OWNER: {"factionID": EX, "size": "S", "extent": "L", "tickerName": "EXO",
                      "taxRate": 0.0, "name": "Example Owners"},
        OTHER_CORP: {"factionID": OTHER, "size": "S", "extent": "L", "tickerName": "SMP",
                     "taxRate": 0.0, "name": "Sample Corporation"},
    }
    components = {
        990101: {"entityStandings": {"friendlyResponseThreshold": 0.0}},
        990301: {"reactiveSpawnComponent": {
            "spawnTableID": 9001, "ownerID": SPAWN_OWNER, "fleetCounterTableID": 901,
            "bountyPayoutMultiplier": 1.0, "friendlyResponseThreshold": 0.0,
            "spawnDataByDungeonID": {3999902: {"spawnTableID": 9003, "ownerID": EXEC},
                                     3999901: {"spawnTableID": 9002, "ownerID": EXEC}},
            "spawnDataByFactionID": {EX: {"spawnTableID": 9004}}}},
        990111: {"entityBounty": {"bountyMultipliersByOwner": {EXEC: 0.5, 0: 1.0}}},
    }
    return {"types": types, "groups": groups, "dogma": dog, "factions": factions,
            "corps": corps, "components": components, "sof": {7001: "exbase", 7002: "smp"},
            "charges": {990401: (99201, 1, [0.0, 0.0, 2.0, 1.0]),
                        990402: (99202, 1, [1.0, 0.0, 0.0, 1.0])},
            "tag_types": [990201, 990202, 990203]}


def _by_key(rows):
    return {r[0]: r for r in rows}


class NpcSection(unittest.TestCase):

    def setUp(self):
        self._grammar = N.GRAMMAR
        N.GRAMMAR = N.grammar((r"Example Cartel", r"Samples?"))

    def tearDown(self):
        N.GRAMMAR = self._grammar

    def derive(self, data=None):
        rows, _npcs = N.derive(data or fixture())
        return rows

    def types(self, data=None):
        return _by_key(self.derive(data)["npc/types.tsv"])

    def test_a_present_faction_id_wins_over_sof_faction_name(self):
        data = fixture()
        # 990102 sits in an Example group and is drawn with Example's sof,
        # but its `types.factionID` says Sample: the type field wins.
        data["types"][990102]["faction"] = OTHER
        t = self.types(data)
        self.assertEqual(t[990102][2:4], (OTHER, "type"))
        self.assertEqual(t[990101][2:4], (EX, "group"))

    def test_a_faction_id_naming_a_corporation_resolves_through_it(self):
        data = fixture()
        data["types"][990151]["faction"] = EXEC
        data["types"][990152]["faction"] = 1999999   # neither faction nor corp
        t = self.types(data)
        self.assertEqual(t[990151][2:4], (EX, "type"))
        self.assertEqual(t[990152][2:4], (OTHER, "group"))

    def test_the_alias_collapse_keeps_the_lowest_type_id(self):
        t = self.types()
        for i in range(3):
            self.assertIsNone(t[990101 + i][7])
            self.assertEqual(t[990121 + i][7], 990101 + i)
        self.assertIsNone(t[990111][7])   # a different bounty is no alias

    def test_the_alias_collapse_ignores_insertion_order(self):
        data = fixture()
        data["types"] = dict(reversed(list(data["types"].items())))
        self.assertEqual(self.types(data)[990121][7], 990101)

    def test_the_bounty_row_comes_from_attribute_481(self):
        data = fixture()
        b = _by_key(self.derive(data)["npc/bounty.tsv"])
        self.assertEqual(b[990102][1], data["dogma"][990102][0][N.A_BOUNTY])
        self.assertEqual(N.A_BOUNTY, 481)
        self.assertEqual(b[990111][2], {0: 1.0, EXEC: 0.5})
        data["dogma"][990101][0].pop(N.A_BOUNTY)
        data["dogma"][990101][0][482] = 99.0
        self.assertNotIn(990101, _by_key(self.derive(data)["npc/bounty.tsv"]))

    def test_needs_and_roles_follow_the_effects(self):
        t = self.types()
        self.assertEqual(t[990152][8:], (["TURRET", "SCRAM"], "TACKLER"))
        self.assertEqual(t[990151][8:], (["TURRET"], "DPS"))
        self.assertIn("MISSILE", N.needs_of({N.A_MISSILE_TYPE: 990999.0}, []))
        self.assertNotIn("MISSILE", N.needs_of({N.A_MISSILE_TYPE: 0.0}, []))
        self.assertEqual(N.role_of(N.needs_of({}, [REMOTE_REP, SCRAM_EFFECT])), "LOGISTIC")

    def test_the_grammar_gives_context_key_and_verbatim_class(self):
        self.assertEqual(N.parse_group("Asteroid Example Cartel Commander Frigate"),
                         ("Asteroid", "example", "Commander Frigate"))
        self.assertEqual(N.parse_group("Ghost Sites Samples Cruiser")[:2],
                         ("Ghost Sites", "samples"))
        self.assertIsNone(N.parse_group("Example Cartel Frigate"))
        self.assertIsNone(N.parse_group("Asteroid Unknown Frigate"))

    def test_the_prefix_ladder_and_tiers(self):
        f = _by_key(self.derive()["npc/factions.tsv"])[EX]
        cols = dict(zip(N.FACTIONS_HEADER, f))
        self.assertEqual((cols["base_prefix"], cols["elite_prefix"], cols["commander_prefix"]),
                         ("Example", "Arch Example", "Dread Example"))
        self.assertEqual(cols["deadspace_stems"], ["Exii", "", "", "", ""])
        t = self.types()
        self.assertEqual([t[x][6] for x in (990101, 990111, 990121, 990131, 990141)],
                         ["base", "elite", "base", "commander", "deadspace"])

    def test_r1_columns(self):
        rows = _by_key(self.derive()["npc/factions.tsv"])
        cols = dict(zip(N.FACTIONS_HEADER, rows[EX]))
        self.assertEqual(cols["owner_corp_id"], SPAWN_OWNER)
        self.assertEqual(cols["groups"], [G_FRIG, G_CMDR, G_DS])
        self.assertEqual(cols["wrecks"], {"Frigate": 990901})
        self.assertEqual(cols["tags"], [990202, 990201])   # by price, price > 0 only
        self.assertEqual(cols["ammo_groups"], {1: 99201})
        self.assertEqual(cols["damage_profile"], [0.0, 0.0, round(2 / 3, 4), round(1 / 3, 4)])
        self.assertEqual(cols["sof_faction"], "exbase")
        other = dict(zip(N.FACTIONS_HEADER, rows[OTHER]))
        self.assertEqual(other["owner_corp_id"], OTHER_CORP)   # no spawner: the executor
        self.assertIsNone(other["tank"])

    def test_a_faction_outside_the_grammar_has_empty_r1_columns(self):
        row = _by_key(self.derive()["npc/factions.tsv"])[PLAIN]
        self.assertEqual(row[:5], (PLAIN, OTHER_CORP, 1999004, 39000003, [2, 4]))
        self.assertTrue(all(v in (None, [], {}) for v in row[5:-1]))
        self.assertEqual(row[-1], "Plain State")

    def test_a_faction_with_no_executor_is_left_out(self):
        data = fixture()
        data["factions"][599099] = {"name": "Nobody", "solarSystemID": 39000009,
                                    "memberRaces": [1]}
        rows = _by_key(N.derive(data)[0]["npc/factions.tsv"])
        self.assertNotIn(599099, rows)
        self.assertIn(PLAIN, rows)

    def test_corporation_rows_keep_absent_fields_empty(self):
        c = _by_key(self.derive()["npc/corporations.tsv"])
        line = content.encode_row(c[SPAWN_OWNER])
        self.assertEqual(line, "1999002\t%d\t\t\t\t\t0.0\t\t\tS\tL\t\tEXO\tExample Owners" % EX)
        self.assertEqual(c[EXEC][11], [9901])

    def test_components_and_reactive_spawn_rows(self):
        rows = self.derive()
        self.assertIn((990101, "entityStandings"), rows["npc/components.tsv"])
        self.assertIn((990301, "reactiveSpawnComponent"), rows["npc/components.tsv"])
        spawn = [r for r in rows["npc/reactive-spawn.tsv"] if r[0] == 990301]
        self.assertEqual([r[1:3] for r in spawn], [("type", None), ("dungeon", 3999901),
                                                   ("dungeon", 3999902), ("faction", EX)])
        self.assertEqual(spawn[0][3:], (9001, None, SPAWN_OWNER, 901, 1.0, 0.0))
        self.assertEqual(spawn[1][3:], (9002, None, EXEC, None, None, None))

    def test_a_component_on_every_row_of_a_big_table_is_refused(self):
        rows = {900000 + i: {"destinyServerBallClass": {}, "staleBit": {}} for i in range(120)}
        for k in list(rows)[::2]:
            rows[k]["partial"] = {}
        with self.assertRaises(content.ContentError) as cm:
            N.refuse_universal_components(rows)
        self.assertIn("staleBit", str(cm.exception))
        self.assertNotIn("destinyServerBallClass", str(cm.exception))
        self.assertNotIn("partial", str(cm.exception))

    def test_the_known_universal_component_and_small_tables_pass(self):
        rows = {900000 + i: {"destinyServerBallClass": {}} for i in range(120)}
        N.refuse_universal_components(rows)
        # A component that is not on every row passes at any size.
        rows[900000]["staleBit"] = {}
        N.refuse_universal_components(rows)
        # 100 rows or fewer are not judged (a fixture, a sandbox).
        small = {900000 + i: {"staleBit": {}} for i in range(100)}
        N.refuse_universal_components(small)
        N.refuse_universal_components({})
        # ...and `known` is the caller's to widen.
        big = {900000 + i: {"another": {}} for i in range(101)}
        N.refuse_universal_components(big, known={"another"})
        with self.assertRaises(content.ContentError):
            N.refuse_universal_components(big)

    def test_sof_agreement_counts_types_with_a_graphic(self):
        data = fixture()
        data["sof"][7003] = "elsewhere"
        data["types"][990141]["graphic"] = 7003
        rows, npcs = N.derive(data)
        agree, total, sof = N.sof_agreement(data, npcs)["example"]
        self.assertEqual((agree, total, sof), (total - 1, total, "exbase"))

    def test_headers_equal_schema_md(self):
        path = os.path.join(_HERE, os.pardir, "docs", "content", "schema.md")
        with open(path, encoding="utf-8") as fh:
            doc = fh.read()
        for rel, header in (("factions", N.FACTIONS_HEADER),
                            ("corporations", N.CORPORATIONS_HEADER),
                            ("types", N.TYPES_HEADER), ("components", N.COMPONENTS_HEADER),
                            ("reactive-spawn", N.REACTIVE_HEADER),
                            ("bounty", N.BOUNTY_HEADER)):
            m = re.search(r"### `npc/%s\.tsv`.*?```\n(#[^\n]*)\n" % re.escape(rel), doc, re.S)
            self.assertIsNotNone(m, rel)
            self.assertEqual(m.group(1), "#" + "\t".join(header), rel)

    def test_section_is_discovered_by_the_driver(self):
        names = [s.name for s in content.discover(_HERE)]
        self.assertIn(N.SECTION, names)


if __name__ == "__main__":
    unittest.main()
