"""Tests for `tools/celestials.py`'s station rows (packet CP2).

**Invented rows only.** Every system here is built by `fixture()` below in
the shape `solarsystemcontent.static` has; nothing reads the client, and no
id, name or count from the client is in this file. The client-backed check
(the unique station count of the `--all` output) is recorded in
`docs/lanes/cp2.md`.

Run::

    python3 -I tools/test_celestials.py -q
"""

from __future__ import annotations

import copy
import os
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import celestials  # noqa: E402

SYSTEM = 39000001
STAR = 49000001
PLANET = 49000010
MOON = 49000011
MOON_2 = 49000012
BELT = 49000013
PLANET_2 = 49000020
GATE = 59000001
GATE_FAR = 59000002
STATION_STAR = 69000001
STATION_PLANET = 69000002
STATION_MOON = 69000003


def pos(x, y, z):
    return {"x": x, "y": y, "z": z}


def station(type_id, p):
    return {"typeID": type_id, "position": p}


def fixture():
    """A planet-only system: one star, two planets, moons, a belt, one
    planet station and one gate. Every id and value is invented."""
    return {
        "solarSystemID": SYSTEM,
        "star": {"id": STAR, "typeID": 990001, "radius": 500000.0},
        "planets": {
            PLANET: {
                "typeID": 990002,
                "radius": 6000.0,
                "position": pos(1000.0, 0.0, -1000.0),
                "moons": {
                    MOON: {
                        "typeID": 990003,
                        "radius": 100.0,
                        "position": pos(1100.0, 0.0, -1000.0),
                    },
                    MOON_2: {
                        "typeID": 990003,
                        "radius": 120.0,
                        "position": pos(1200.0, 0.0, -1000.0),
                    },
                },
                "asteroidBelts": {
                    BELT: {"typeID": 990004, "position": pos(900.0, 5.0, -1000.0)}
                },
                "npcStations": {
                    STATION_PLANET: station(990005, pos(1001.5, 2.0, -999.0))
                },
            },
            PLANET_2: {
                "typeID": 990002,
                "radius": 7000.0,
                "position": pos(-5000.0, 10.0, 5000.0),
            },
        },
        "stargates": {GATE: {"typeID": 990006, "position": pos(0.0, 0.0, 9000.0), "destination": GATE_FAR}},
    }


def with_star_and_moon_stations():
    s = copy.deepcopy(fixture())
    s["star"]["npcStations"] = {STATION_STAR: station(990007, pos(10.0, 20.0, 30.0))}
    s["planets"][PLANET]["moons"][MOON]["npcStations"] = {
        STATION_MOON: station(990008, pos(1100.0, 50.0, -1000.0))
    }
    return s


def render(system):
    return celestials.render(
        {SYSTEM: system},
        {SYSTEM: {"constellationID": 29000001, "regionID": 19000001}},
        [SYSTEM],
        {990006: 15000.0},
        {GATE_FAR: 39000002},
    )


# The planet-only fixture's file, byte for byte, as the extractor wrote it
# before CP2. Planet-only output must not move.
PLANET_ONLY = (
    "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n"
    "#system\t39000001\t29000001\t19000001\n"
    "star\t49000001\t990001\t500000.0\t0.0\t0.0\t0.0\t39000001\t0\t0\n"
    "planet\t49000010\t990002\t6000.0\t1000.0\t0.0\t-1000.0\t39000001\t0\t0\n"
    "moon\t49000011\t990003\t100.0\t1100.0\t0.0\t-1000.0\t39000001\t0\t0\n"
    "moon\t49000012\t990003\t120.0\t1200.0\t0.0\t-1000.0\t39000001\t0\t0\n"
    "belt\t49000013\t990004\t0.0\t900.0\t5.0\t-1000.0\t39000001\t0\t0\n"
    "station\t69000002\t990005\t0.0\t1001.5\t2.0\t-999.0\t39000001\t0\t0\n"
    "planet\t49000020\t990002\t7000.0\t-5000.0\t10.0\t5000.0\t39000001\t0\t0\n"
    "gate\t59000001\t990006\t15000.0\t0.0\t0.0\t9000.0\t39000001\t59000002\t39000002\n"
)


class Stations(unittest.TestCase):
    def test_star_planet_and_moon_stations_follow_their_parents(self):
        rows = celestials.rows_for(
            with_star_and_moon_stations(), SYSTEM, {}.get, {}.get
        )
        ids = [r[1] for r in rows]
        stations = [r for r in rows if r[0] == "station"]
        self.assertEqual(
            sorted(r[1] for r in stations),
            [STATION_STAR, STATION_PLANET, STATION_MOON],
        )
        # The star station directly follows the star, the moon station its
        # moon, and the planet station sits inside its planet's block.
        self.assertEqual(ids[ids.index(STATION_STAR) - 1], STAR)
        self.assertEqual(ids[ids.index(STATION_MOON) - 1], MOON)
        self.assertLess(ids.index(PLANET), ids.index(STATION_PLANET))
        self.assertLess(ids.index(STATION_PLANET), ids.index(PLANET_2))
        for r in stations:
            self.assertEqual(r[7], SYSTEM)
            self.assertEqual((r[8], r[9]), (0, 0))
        moon_station = rows[ids.index(STATION_MOON)]
        self.assertEqual(moon_station[2], 990008)
        self.assertEqual(moon_station[4:7], (1100.0, 50.0, -1000.0))

    def test_planet_only_output_is_unchanged_byte_for_byte(self):
        text, n = render(fixture())
        self.assertEqual(text, PLANET_ONLY)
        self.assertEqual(n, PLANET_ONLY.count("\n") - 2)

    def test_new_stations_only_insert_lines(self):
        text, n = render(with_star_and_moon_stations())
        added = [
            line
            for line in text.splitlines(True)
            if line.startswith("station\t%d\t" % STATION_STAR)
            or line.startswith("station\t%d\t" % STATION_MOON)
        ]
        self.assertEqual(len(added), 2)
        rest = "".join(line for line in text.splitlines(True) if line not in added)
        self.assertEqual(rest, PLANET_ONLY)
        self.assertEqual(n, PLANET_ONLY.count("\n") - 2 + 2)

    def test_a_system_with_no_star_still_emits_its_stations(self):
        s = with_star_and_moon_stations()
        del s["star"]
        rows = celestials.rows_for(s, SYSTEM, {}.get, {}.get)
        self.assertEqual(
            sorted(r[1] for r in rows if r[0] == "station"),
            [STATION_PLANET, STATION_MOON],
        )


if __name__ == "__main__":
    unittest.main()
