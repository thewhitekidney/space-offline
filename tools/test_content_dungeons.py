"""Unit tests for `tools/content_dungeons.py` (packet CP10).

**Invented rows only** (plan §1.6): every id below is in a range the build
does not use (`schema.md` §1.1), and no client count or name appears. The
client-backed counts and closures are `content.py --check --sections
dungeons`, pasted into `docs/lanes/cp10.md`.

    python3 -I tools/test_content_dungeons.py -q
"""

import os
import sys
import unittest
from unittest import mock

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_dungeons as cd  # noqa: E402

ORE_A, ORE_B, ORE_C, ROCK = 990003, 990006, 990009, 990060


def obj(room, oid, tid, guard=None, entities=None, **kw):
    o = {"roomID": room, "objectID": oid, "typeID": tid, "position": (1.0, -2.5, 0.0),
         "radius": None, "yaw": None, "pitch": None, "roll": None,
         "guardCommand": guard, "entities": entities, "objectNameID": None}
    o.update(kw)
    return o


def fixture():
    """Two dungeons: 999101 (two rooms, a gate, a spawn) and 999102 (ore)."""
    guard = {"aggressionRange": 30000.0, "entityMaxRespawnDelay": 120,
             "entityMinRespawnDelay": 60, "jitterRadius": None,
             "objectMaxRespawnDelay": 7, "objectMinRespawnDelay": 5,
             "orbitRange": 5000.0, "entityDelaySpawn": 1, "entitySpawnOnce": 0,
             "objectDelaySpawn": 0, "objectTriggerSpawn": 1}
    ent = {70001: {"entityID": 70001, "npcGroupingID": 9901, "behaviorName": None,
                   "groupBehaviorName": "Guard", "triggerSpawn": None,
                   "additionalLootId": None, "entityNameID": 999950}}
    return {
        999101: {
            "archetypeID": 24, "factionID": 599001, "difficulty": 2,
            "entryObjectID": 8001, "entryTypeID": 990070, "objectivesID": None,
            "dungeonNameID": 999901, "contentTags": ["activity_combat"],
            "connections": [{"fromObjectID": 8002, "allowedShipsList": 9901,
                             "disallowedFittedTypesList": None,
                             "allowedShipRaces": [1, 8]}],
            "rooms": {
                2: {"shortName": "R1", "emptyResetDelay": None, "messageHeaderID": 999960,
                    "messageTextID": None,
                    "objects": {8003: obj(2, 8003, ROCK, guard=guard, entities=ent)}},
                1: {"shortName": "R0", "emptyResetDelay": 30, "messageHeaderID": None,
                    "messageTextID": None,
                    "objects": {8002: obj(1, 8002, 990071), 8001: obj(1, 8001, 990070,
                                                                     radius=2500.0)}},
            },
        },
        999102: {
            "archetypeID": 27, "factionID": None, "difficulty": None,
            "entryObjectID": 8999, "entryTypeID": None, "objectivesID": None,
            "dungeonNameID": None, "contentTags": None, "connections": None,
            "rooms": {3: {"shortName": "R0", "objects": {
                8101: obj(3, 8101, ORE_A), 8102: obj(3, 8102, ORE_A),
                8103: obj(3, 8103, ORE_B)}}},
        },
    }


class Files(unittest.TestCase):
    def setUp(self):
        self.f = cd.dungeon_files(fixture())

    def test_headers_are_schema_md_byte_for_byte(self):
        want = {
            cd.H_DUNGEONS: "#dungeon_id\tarchetype_id\tfaction_id\tdifficulty\t"
            "entry_object_id\tentry_type_id\tobjectives_id\tname_id\tcontent_tags",
            cd.H_ROOMS: "#dungeon_id\troom_id\tempty_reset_delay\tmessage_header_id\t"
            "message_text_id\tshort_name",
            cd.H_OBJECTS: "#dungeon_id\troom_id\tobject_id\ttype_id\tx\ty\tz\tradius\tyaw\t"
            "pitch\troll\tobject_min_respawn\tobject_max_respawn\tobject_delay_spawn\t"
            "object_trigger_spawn\tname_id",
            cd.H_GATES: "#dungeon_id\tfrom_object_id\tallowed_ships_list\t"
            "disallowed_fitted_types_list\tallowed_ship_races",
            cd.H_SPAWNS: "#dungeon_id\troom_id\tobject_id\tentity_id\tnpc_grouping_id\t"
            "behavior_name\tgroup_behavior_name\ttrigger_spawn\tadditional_loot_id\t"
            "entity_min_respawn\tentity_max_respawn\tspawn_once\tdelay_spawn\t"
            "aggression_range\torbit_range\tjitter_radius\tentity_name_id",
            cd.H_ARCHETYPES: "#archetype_id\ttitle_id\tcontent_tags\tlocation",
            cd.H_RESOURCES: "#dungeon_id\tkind\ttype_ids\tore_band",
            cd.H_ROOM_ROCKS: "#dungeon_id\troom_id\ttype_id\tcount",
            cd.H_AGENTS: "#key\tdungeon_id\tsystem_id\tspawn_point_id\ttype_id",
        }
        for cols, line in want.items():
            self.assertEqual("#" + "\t".join(cols), line)

    def test_rows_are_in_key_order_and_absent_is_empty(self):
        self.assertEqual([r[:2] for r in self.f["rooms"]], [(999101, 1), (999101, 2),
                                                             (999102, 3)])
        self.assertEqual([r[2] for r in self.f["objects"]],
                         [8001, 8002, 8003, 8101, 8102, 8103])
        line = content.encode_row(self.f["dungeons"][1])
        self.assertEqual(line, "999102\t27\t\t\t8999\t\t\t\t")
        self.assertEqual(content.encode_row(self.f["rooms"][0]),
                         "999101\t1\t30\t\t\tR0")

    def test_object_row_carries_position_and_the_objects_guard(self):
        o = {r[2]: r for r in self.f["objects"]}
        self.assertEqual(o[8001][4:8], (1.0, -2.5, 0.0, 2500.0))
        self.assertEqual(o[8003][11:15], (5, 7, False, True))
        self.assertEqual(o[8002][11:15], (None, None, False, False))

    def test_spawn_row_joins_entity_and_guard(self):
        (s,) = self.f["spawns"]
        self.assertEqual(s[:5], (999101, 2, 8003, 70001, 9901))
        self.assertEqual(s[5:], (None, "Guard", False, None, 60, 120, False, True,
                                 30000.0, 5000.0, None, 999950))

    def test_gate_row(self):
        self.assertEqual(self.f["gates"], [(999101, 8002, 9901, None, [1, 8])])
        self.assertEqual(content.encode_row(self.f["gates"][0]), "999101\t8002\t9901\t\t1,8")

    def test_a_bad_entry_object_id_is_reported(self):
        self.assertEqual(cd.bad_entry_objects(self.f["dungeons"], self.f["objects"]),
                         [(999102, 8999)])

    def test_an_entry_object_in_another_dungeon_is_bad(self):
        d = fixture()
        d[999102]["entryObjectID"] = 8001
        f = cd.dungeon_files(d)
        self.assertEqual(cd.bad_entry_objects(f["dungeons"], f["objects"]), [(999102, 8001)])


class RoomRocks(unittest.TestCase):
    def test_room_rocks_equals_the_room_objects(self):
        f = cd.dungeon_files(fixture())
        table = {999102: {3: {ORE_A: 2, ORE_B: 1}}, 999101: {2: {ROCK: 1}}}
        rows = list(cd.room_rock_rows(table))
        self.assertEqual(rows[0], (999101, 2, ROCK, 1))
        self.assertEqual(cd.room_rock_disagreements(rows, f["objects"]), [])

    def test_a_wrong_cell_or_a_wrong_room_disagrees(self):
        f = cd.dungeon_files(fixture())
        bad = cd.room_rock_disagreements(
            [(999102, 3, ORE_A, 3), (999101, 1, ROCK, 1)], f["objects"])
        self.assertEqual(len(bad), 2)
        self.assertIn("table 3, objects 2", bad[0])
        self.assertIn("room 1", bad[1])


class OreBand(unittest.TestCase):
    """C3 §7.2 rule R4, on synthetic belts. Shares are named relative to
    `BAND_SHARE`, never its value."""

    def bands(self):
        belts = {1: "high", 2: "low", 3: "null", 4: None}
        total = 10000
        under = int(cd.BAND_SHARE * total / 2)     # below the share
        over = int(cd.BAND_SHARE * total * 2)      # above it
        rocks = {
            # ORE_A: a trace in high, the rest in low -> low
            1: {ORE_A: under}, 2: {ORE_A: total - under, ORE_B: over},
            # ORE_B: over the share in low, the bulk in null -> low (lowest)
            3: {ORE_B: total - over, ORE_C: 5},
            4: {ROCK: 5},  # a belt in no system: ignored
        }
        return cd.type_bands(rocks, belts)

    def test_an_ore_type_takes_the_lowest_band_over_the_share(self):
        b = self.bands()
        self.assertEqual(b[ORE_A], "low")
        self.assertEqual(b[ORE_B], "low")
        self.assertEqual(b[ORE_C], "null")
        self.assertNotIn(ROCK, b)

    def test_a_site_takes_the_highest_band_of_its_ores(self):
        b = {ORE_A: "high", ORE_B: "low", ORE_C: "null"}
        self.assertEqual(cd.ore_band([ORE_A], b), "high")
        self.assertEqual(cd.ore_band([ORE_A, ORE_B], b), "low")
        self.assertEqual(cd.ore_band([ORE_C, ORE_A], b), "null")

    def test_an_ore_in_no_belt_makes_the_site_special(self):
        self.assertEqual(cd.ore_band([ORE_A, ROCK], {ORE_A: "null"}), cd.SPECIAL)

    def test_resource_rows_band_ore_only(self):
        b = {ORE_A: "low"}
        tables = [("ore", {999102: [ORE_A]}), ("gas", {999102: [ORE_A]}),
                  ("ice", {999101: [ORE_B]})]
        rows = cd.resource_rows(tables, b)
        self.assertEqual(rows, [(999101, "ice", [ORE_B], None),
                                (999102, "ore", [ORE_A], "low"),
                                (999102, "gas", [ORE_A], None)])
        self.assertEqual(content.encode_row(rows[1]), "999102\tore\t990003\tlow")

    def test_security_band_edges(self):
        self.assertEqual(cd.band_of(cd.HIGH_SEC), "high")
        self.assertEqual(cd.band_of(cd.HIGH_SEC / 2), "low")
        self.assertEqual(cd.band_of(0.0), "null")
        self.assertEqual(cd.band_of(-0.5), "null")


class Other(unittest.TestCase):
    def test_archetypes_and_agents_in_space(self):
        a = list(cd.archetype_rows({98: {"titleID": None, "contentTags": ["activity_combat"],
                                         "location": "Dungeon Specific"}}))
        self.assertEqual(content.encode_row(a[0]), "98\t\tactivity_combat\tDungeon Specific")
        g = list(cd.agent_rows({3999901: {"dungeonID": 999103, "solarSystemID": 39000001,
                                          "spawnPointID": 1, "typeID": 990070}}))
        self.assertEqual(content.encode_row(g[0]), "3999901\t999103\t39000001\t1\t990070")

    def test_the_section_is_discovered_by_glob(self):
        names = [s.name for s in content.discover()]
        self.assertIn(cd.SECTION, names)


class ExpectedByBuild(unittest.TestCase):
    """Per-build counts (port day: an unlisted build must not fail on a count)."""

    def test_the_reference_and_the_old_builds_are_verified_against_the_base(self):
        for b in (None, 3619879, 3628513, 3662168):
            counts, bands, ok = cd.expected(b)
            self.assertTrue(ok, b)
            self.assertEqual(counts, cd.EXPECT)
            self.assertEqual(bands, cd.EXPECT_ORE_ANOMALY_BANDS)

    def test_an_unlisted_build_is_unverified_not_refused(self):
        counts, bands, ok = cd.expected(1234)
        self.assertFalse(ok)
        self.assertEqual(counts, cd.EXPECT)

    def test_a_listed_build_overrides_only_what_it_names(self):
        with mock.patch.dict(cd.EXPECT_BY_BUILD,
                             {1234: ({"dungeons.tsv": 7}, {"high": 1})}):
            counts, bands, ok = cd.expected(1234)
        self.assertTrue(ok)
        self.assertEqual(counts["dungeons.tsv"], 7)
        self.assertEqual(counts["objects.tsv"], cd.EXPECT["objects.tsv"])
        self.assertEqual(bands, {"high": 1})


if __name__ == "__main__":
    unittest.main()
