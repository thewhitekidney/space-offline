"""Unit tests for the `agents` section, `tools/content_agents.py` (CP13).

Synthetic rows and invented ids only (plan §1.6, `schema.md` §1.1's ranges);
the client-backed counts are `content.py --sections agents --check`, recorded
in `docs/lanes/cp13.md`. Run: `python3 -I tools/test_content_agents.py -q`.
"""

from __future__ import annotations

import os
import re
import sys
import tempfile
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_agents as ca  # noqa: E402

_REPO = os.path.dirname(_HERE)
_POOL = os.path.join(_REPO, "server", "crates", "eve-server", "src", "mission_pool.rs")
_SCHEMA = os.path.join(_REPO, "docs", "content", "schema.md")
_ARC_RS = os.path.join(_REPO, "server", "crates", "eve-server", "src", "epic_arc.rs")
_START_RS = os.path.join(_REPO, "server", "crates", "eve-server", "src", "start.rs")

TITLES = {
    999901: "Example Sweep",
    999902: "Example Sweep",       # the same title under a second id
    999903: "Example Run - Pickup (1 of 2)",
    999904: "Example Run – Dropoff (2 of 2)",
    999905: "Example Haul",
}


def _kill(dungeon, **kw):
    return dict({"dungeonID": dungeon, "objectiveTypeID": None, "objectiveQuantity": 0,
                 "objectiveGroupID": None, "dropItemInMissionContainer": None}, **kw)


def _mission(name_id, template, **kw):
    row = {"nameID": name_id, "contentTemplate": ca.TEMPLATE_PREFIX + template + "Mission",
           "messages": {"messages.mission.briefing": 999990}, "killMission": None,
           "courierMission": None, "fixedLpRewardAlpha": 0, "fixedLpRewardOmega": 0,
           "hasStandingRewards": 1}
    row.update(kw)
    return row


def fixture_missions():
    return {
        999003: _mission(999902, "BasicKill", killMission=_kill(999101), factionID=599001),
        999001: _mission(999901, "BasicKill", killMission=_kill(999102)),
        999002: _mission(999903, "BasicCourier",
                         courierMission={"objectiveTypeID": 990001, "objectiveQuantity": 3},
                         messages={"messages.mission.briefing": 999991,
                                   "messages.mission.completed.nextmission.agentsays": 999992}),
        999004: _mission(999904, "BasicKill", killMission=_kill(999103),
                         fixedLpRewardAlpha=10, fixedLpRewardOmega=20,
                         extraStandings={"599002": 0.5}, contentTags=["tag_a", "tag_b"],
                         missionRewards={"reward": {"rewardTypeID": 990002, "rewardQuantity": 5},
                                         "bonusReward": {"rewardTypeID": 990003,
                                                         "rewardQuantity": 7},
                                         "bonusTimeInterval": 30}),
        999005: _mission(999905, "EpicArcTalkToAgent"),
    }


DUNGEONS = {999101: {"difficulty": 2, "factionID": 599003},
            999102: {"difficulty": None, "factionID": None},
            999103: {"difficulty": 3, "factionID": 599004}}
VOLUMES = {990001: 100.0}


def _rows():
    ms = fixture_missions()
    canon, multi = ca.canonical_name_ids(ms, TITLES.get)
    return {r[0]: r for r in ca.mission_rows(ms, DUNGEONS, VOLUMES, canon, TITLES.get)}, multi


def _rust_const(name):
    with open(_POOL, encoding="utf-8") as fh:
        m = re.search(r'pub const %s: &str = "([^"]*)";' % name, fh.read())
    return m.group(1).encode().decode("unicode_escape")


def _arc_rs_const(name, path=_ARC_RS):
    """`epic_arc.rs`'s (or `path`'s) `pub const NAME: (&str, &str) =
    ("file", "header")`."""
    with open(path, encoding="utf-8") as fh:
        m = re.search(r'pub const %s: \(&str, &str\) = \(\s*"([^"]*)",\s*"([^"]*)",?\s*\);'
                      % name, fh.read())
    return m.group(1), m.group(2).encode().decode("unicode_escape")


#: Invented `epicarcs` rows (FX5): arc 9901 branches at 999501, arc 9902
#: fails back and has no faction.
ARCS = {
    9902: {"arcRestartInterval": 1, "epicArcNameID": 999921, "factionID": None,
           "missions": {999512: {"agentID": 3999411, "nextMissions": [],
                                 "failMissionID": 999511},
                        999511: {"agentID": 3999411, "nextMissions": [999512],
                                 "failMissionID": None}}},
    9901: {"arcRestartInterval": 1000, "epicArcNameID": 999920, "factionID": 599001,
           "missions": {999001: {"agentID": 3999001, "nextMissions": [999002, 999003],
                                 "failMissionID": None}}},
}


class Headers(unittest.TestCase):
    def test_headers_equal_mission_pool_rs_and_schema_md_byte_for_byte(self):
        with open(_SCHEMA, encoding="utf-8") as fh:
            schema = fh.read()
        for header, const in ((ca.AGENTS_HEADER, "AGENTS_HEADER"),
                              (ca.MISSIONS_HEADER, "MISSIONS_HEADER")):
            line = "#" + "\t".join(header)
            self.assertEqual(line, _rust_const(const))
            self.assertIn("\n" + line + "\n", schema)

    def test_the_epic_arc_files_headers_equal_epic_arc_rs_and_schema_md(self):
        with open(_SCHEMA, encoding="utf-8") as fh:
            schema = fh.read()
        for header, const, rel in ((ca.EPIC_ARCS_HEADER, "ARCS", "agents/epic-arcs.tsv"),
                                   (ca.MESSAGES_HEADER, "MESSAGES",
                                    "agents/mission-messages.tsv")):
            line = "#" + "\t".join(header)
            self.assertEqual(_arc_rs_const(const), (rel, line))
            self.assertIn("\n" + line + "\n", schema)

    def test_every_row_has_the_header_width(self):
        rows, _ = _rows()
        for r in rows.values():
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.MISSIONS_HEADER))
        agents = list(ca.agent_rows(_characters(), {1999001: 599002}, {69000001: 39000001}))
        for r in agents:
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.AGENTS_HEADER))


class Parse(unittest.TestCase):
    def test_template_and_family(self):
        cases = {"BasicKill": "kill", "StorylineCourier": "courier", "BasicMining": "mining",
                 "BasicTrade": "trade", "EpicArcTalkToAgent": "talk",
                 "EpicArcAgentInteraction": "talk", "Heraldry": "other"}
        for tpl, fam in cases.items():
            self.assertEqual(ca.template_of(ca.TEMPLATE_PREFIX + tpl + "Mission"), tpl)
            self.assertEqual(ca.family_of(tpl), fam)

    def test_the_chain_k_of_n_parse(self):
        self.assertEqual(ca.parse_chain("Example Run - Pickup (1 of 2)"), ("example run", 1, 2))
        self.assertEqual(ca.parse_chain("Example  Run – Drop (2 of 2)"),
                         ("example run", 2, 2))
        self.assertEqual(ca.parse_chain("Example Hill (3 of 13) (part 2)"),
                         ("example hill", 3, 13))
        self.assertEqual(ca.parse_chain("Example Opp -Other Thing ( 2 of 2)"),
                         ("example opp", 2, 2))
        self.assertEqual(ca.parse_chain("Sign-up Example (1 of 5) \"Word\""),
                         ("sign-up example", 1, 5))
        self.assertEqual(ca.parse_chain("Example Sweep"), (None, None, None))

    def test_level_through_dungeon_else_volume_band_else_none(self):
        rows, _ = _rows()
        self.assertEqual(rows[999003][4:6], (2, "difficulty"))
        self.assertEqual(rows[999001][4:6], (None, "none"))
        self.assertEqual(rows[999002][4:6], (ca.volume_level(300.0), "volume_band"))
        self.assertEqual(rows[999002][11], 300.0)
        self.assertEqual(rows[999005][4:6], (None, "none"))
        # The band rule, by the named constant: at most the top is that level.
        for i, top in enumerate(ca.VOLUME_BANDS):
            self.assertEqual(ca.volume_level(top), i + 1)
        self.assertEqual(ca.volume_level(ca.VOLUME_BANDS[-1] * 2), len(ca.VOLUME_BANDS) + 1)


class Missions(unittest.TestCase):
    def test_name_id_is_canonicalised_per_title(self):
        rows, multi = _rows()
        self.assertEqual(rows[999003][1], 999901)
        self.assertEqual(rows[999001][1], 999901)
        self.assertEqual(rows[999002][1], 999903)
        self.assertEqual(multi, {"Example Sweep": [999901, 999902]})

    def test_columns(self):
        rows, _ = _rows()
        r = rows[999003]
        self.assertEqual((r[2], r[3], r[6], r[7], r[8]), ("BasicKill", "kill", 599001, 599003, 999101))
        c = rows[999002]
        self.assertEqual((c[9], c[10], c[17], c[18], c[19], c[20]),
                         (990001, 3, "example run", 1, 2, True))
        self.assertFalse(rows[999004][20])
        lp = rows[999004]
        self.assertEqual(lp[23:28], (990002, 5, 990003, 7, 30))
        self.assertEqual(lp[28], 20)  # the Omega field (erratum 13)
        self.assertEqual(lp[30], {599002: 0.5})
        self.assertEqual(lp[31], ["tag_a", "tag_b"])
        self.assertEqual(content.encode(c[32]),
                         "messages.mission.briefing=999991;"
                         "messages.mission.completed.nextmission.agentsays=999992")
        self.assertEqual(list(rows), sorted(rows))


class EpicArcs(unittest.TestCase):
    def test_the_schema_example_row_byte_for_byte(self):
        rows = list(ca.epic_arc_rows(ARCS))
        self.assertEqual(content.encode_row(rows[0]),
                         "9901\t999001\t3999001\t999002,999003\t\t1000\t599001\t999920")

    def test_one_row_per_mission_sorted_with_the_arc_columns_repeated(self):
        rows = list(ca.epic_arc_rows(ARCS))
        self.assertEqual([r[:2] for r in rows],
                         [(9901, 999001), (9902, 999511), (9902, 999512)])
        self.assertEqual(rows[1][3:], ([999512], None, 1, None, 999921))
        self.assertEqual(rows[2][3:], ([], 999511, 1, None, 999921))
        for r in rows:
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.EPIC_ARCS_HEADER))


class MissionMessages(unittest.TestCase):
    def test_the_long_form_holds_the_messages_columns_pairs(self):
        ms = fixture_missions()
        rows, _ = _rows()
        wide = {(m, s, v) for m, r in rows.items() for s, v in r[32].items()}
        long_ = list(ca.message_rows(ms))
        self.assertEqual(set(long_), wide)
        self.assertEqual(len(long_), len(wide))
        self.assertEqual(long_, sorted(long_))
        self.assertEqual(content.encode_row(long_[0]),
                         "999001\tmessages.mission.briefing\t999990")


def _characters():
    return {
        3999002: {"corporationID": 1999001, "stationID": None, "agent": {
            "agentType": 8, "agentDivisionID": 22, "agentLevel": 5,
            "agentLocateCharacter": 1, "storyLineMissions": [999002, 999001]}},
        3999001: {"corporationID": 1999001, "stationID": 69000001, "agent": {
            "agentType": 2, "agentDivisionID": 24, "agentLevel": 1,
            "agentLocateCharacter": 0, "storyLineMissions": None}},
        3999003: {"corporationID": 1999001, "stationID": 69000001, "agent": None},
    }


class Agents(unittest.TestCase):
    def test_agent_rows(self):
        rows = list(ca.agent_rows(_characters(), {1999001: 599002}, {69000001: 39000001}))
        self.assertEqual(rows, [
            (3999001, 2, 24, 1, 1999001, 599002, 69000001, 39000001, False, []),
            (3999002, 8, 22, 5, 1999001, 599002, None, None, True, [999002, 999001]),
        ])
        # The schema's invented example row, byte for byte.
        self.assertEqual(content.encode_row(rows[0]),
                         "3999001\t2\t24\t1\t1999001\t599002\t69000001\t39000001\t0\t")

    def test_station_systems_under_star_planet_and_moon(self):
        ssc = {39000001: {"star": {"npcStations": {69000001: {}}},
                          "planets": {1: {"npcStations": {69000002: {}},
                                          "moons": {2: {"npcStations": {69000003: {}}}}}}}}
        self.assertEqual(ca.station_systems(ssc),
                         {69000001: 39000001, 69000002: 39000001, 69000003: 39000001})

#: Invented `schools` / `schoolmap` rows (P-START-X). `schoolmap` is keyed by
#: row index, as the client's is; the school id is a field.
SCHOOLS = {
    98: {"corporationID": 1999002, "raceID": 2, "careerID": None, "nameID": 999002,
         "startingStations": None, "careerAgents": None, "is_starter_space_school": 1},
    99: {"corporationID": 1999001, "raceID": 1, "careerID": None, "nameID": 999001,
         "startingStations": [69000001], "careerAgents": [3999001],
         "is_starter_space_school": None},
    97: {"corporationID": 1999003, "raceID": 1, "careerID": 9, "nameID": 999003,
         "startingStations": [69000003, 69000004], "careerAgents": [3999003, 3999004]},
}
SCHOOLMAP = {0: {"schoolID": 99, "solarSystemID": 39000001},
             1: {"schoolID": 97, "solarSystemID": 39000003}}
RACES = {
    99: {"nameID": 999001, "shipTypeID": 990001, "skills": {990102: 1, 990101: 3}},
    98: {"nameID": 999002, "shipTypeID": None, "skills": None},
}


class Schools(unittest.TestCase):
    def test_header_equals_start_rs_and_schema_md_byte_for_byte(self):
        with open(_SCHEMA, encoding="utf-8") as fh:
            schema = fh.read()
        line = "#" + "\t".join(ca.SCHOOLS_HEADER)
        self.assertEqual(_arc_rs_const("FILE", _START_RS), ("agents/schools.tsv", line))
        self.assertIn("\n" + line + "\n", schema)
        self.assertIn("\n#" + "\t".join(ca.RACES_HEADER) + "\n", schema)

    def test_the_schema_example_row_byte_for_byte(self):
        rows = list(ca.school_rows(SCHOOLS, SCHOOLMAP))
        self.assertEqual(content.encode_row(rows[2]),
                         "99\t1999001\t1\t\t39000001\t69000001\t3999001\t999001\t0")

    def test_system_from_schoolmap_by_its_school_id_field_else_empty(self):
        rows = {r[0]: r for r in ca.school_rows(SCHOOLS, SCHOOLMAP)}
        self.assertEqual(rows[97][4], 39000003)
        self.assertEqual(rows[99][4], 39000001)
        self.assertIsNone(rows[98][4])

    def test_lists_keep_their_order_and_none_is_empty(self):
        rows = {r[0]: r for r in ca.school_rows(SCHOOLS, SCHOOLMAP)}
        self.assertEqual(rows[97][5:7], ([69000003, 69000004], [3999003, 3999004]))
        self.assertEqual(rows[98][5:7], ([], []))
        self.assertEqual(content.encode_row(rows[98]),
                         "98\t1999002\t2\t\t\t\t\t999002\t1")

    def test_starter_space_absent_is_false_and_rows_sort(self):
        rows = list(ca.school_rows(SCHOOLS, SCHOOLMAP))
        self.assertEqual([r[0] for r in rows], [97, 98, 99])
        self.assertEqual([r[8] for r in rows], [False, True, False])
        for r in rows:
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.SCHOOLS_HEADER))


class Races(unittest.TestCase):
    def test_the_schema_example_row_and_an_empty_race(self):
        rows = list(ca.race_rows(RACES))
        self.assertEqual([r[0] for r in rows], [98, 99])
        self.assertEqual(content.encode_row(rows[1]), "99\t990001\t990101=3;990102=1\t999001")
        self.assertEqual(content.encode_row(rows[0]), "98\t\t\t999002")
        for r in rows:
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.RACES_HEADER))


#: Invented `bloodlines` rows (lane start-skills).
BLOODLINES = {
    902: {"raceID": 99, "corporationID": 1999002, "nameID": 999012, "iconID": None},
    901: {"raceID": 99, "corporationID": 1999001, "nameID": 999011},
}


class Bloodlines(unittest.TestCase):
    def test_header_equals_start_rs_and_schema_md_byte_for_byte(self):
        with open(_SCHEMA, encoding="utf-8") as fh:
            schema = fh.read()
        line = "#" + "\t".join(ca.BLOODLINES_HEADER)
        self.assertEqual(_arc_rs_const("BLOODLINES", _START_RS), ("agents/bloodlines.tsv", line))
        self.assertIn("\n" + line + "\n", schema)

    def test_the_schema_example_row_and_rows_sort(self):
        rows = list(ca.bloodline_rows(BLOODLINES))
        self.assertEqual([r[0] for r in rows], [901, 902])
        self.assertEqual(content.encode_row(rows[0]), "901\t99\t1999001\t999011")
        for r in rows:
            self.assertEqual(len(content.encode_row(r).split("\t")), len(ca.BLOODLINES_HEADER))


class Written(unittest.TestCase):
    def test_write_file_round_trips_through_the_driver(self):
        class Ctx:
            def __init__(self, d):
                self.out_dir = d

            def path(self, rel):
                return os.path.join(self.out_dir, rel)

        rows, _ = _rows()
        with tempfile.TemporaryDirectory() as d:
            n, _sha = content.write_file(Ctx(d), "agents/missions.tsv", ca.MISSIONS_HEADER,
                                         list(rows.values()))
            with open(os.path.join(d, "agents/missions.tsv"), encoding="utf-8") as fh:
                first = fh.readline().rstrip("\n")
            back = list(content.read_rows(os.path.join(d, "agents/missions.tsv")))
        self.assertEqual(first, _rust_const("MISSIONS_HEADER"))
        self.assertEqual(n, len(rows))
        self.assertEqual([int(r[0]) for r in back], sorted(rows))


if __name__ == "__main__":
    unittest.main()
