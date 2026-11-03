"""The `dungeons` section of the content bundle (packet CP10).

Writes the nine `dungeons/*.tsv` files of `docs/content/schema.md` §2 from
`dungeons.fsdbinary` (decoded with K1's `fsd.rows`, rooms and objects as
dicts, `docs/re/dungeon-model.md` §4), `archetypes.fsdbinary`,
`agentsinspace.fsdbinary`, the three `*typesbydungeonid.static` tables and
`asteroidcountbytypeidroomiddungeonid.static`. Spec: C1 §2.5, K4; C3 §7.2.

`resources.tsv` carries `ore_band`, C3 §7.2 rule R4: an ore type's band is
the lowest band in which it holds more than `BAND_SHARE` of its belt rocks,
and an ore site's band is the highest band among its ores, or `special`
when any of them occurs in no belt. The belts are the whole corpus's, never
the `--systems` sandbox, so the band does not depend on the restriction.

The dungeon files are not restricted by `--systems`: a dungeon has no
system (`dungeon-model.md` §2).

Every function above `section` is pure over plain mappings, so
`tools/test_content_dungeons.py` drives it with invented rows. Counts and
closures are in `check` (plan §1.6). CCP's data: never commit the output.
"""

# `tools/` goes LAST on `sys.path` (`tools/dis.py` shadows the stdlib's).
import sys as _sys
import os as _os

_HERE = _os.path.dirname(_os.path.abspath(__file__))
if _HERE in _sys.path:
    _sys.path.remove(_HERE)
_sys.path.append(_HERE)

import content as _content  # noqa: E402

SECTION = "dungeons"
DEPENDS = ()
CFSD = ("dungeons", "archetypes", "agentsinspace")

RES = "res:/staticdata/"
DUNGEONS = RES + "dungeons.fsdbinary"
ARCHETYPES = RES + "archetypes.fsdbinary"
AGENTS_IN_SPACE = RES + "agentsinspace.fsdbinary"
ROOM_ROCKS = RES + "asteroidcountbytypeidroomiddungeonid.static"
#: kind -> the `*typesbydungeonid` table, in `resources.tsv` order.
RESOURCES = (("ore", RES + "oretypesbydungeonid.static"),
             ("gas", RES + "gastypesbydungeonid.static"),
             ("ice", RES + "icetypesbydungeonid.static"))
SYSTEMS = RES + "systems.static"
SSC = RES + "solarsystemcontent.static"
BELT_ROCKS = RES + "asteroidcountbytypeidbeltid.static"

# --- headers (`schema.md` §2, byte for byte) ----------------------------------

H_DUNGEONS = tuple("dungeon_id archetype_id faction_id difficulty entry_object_id "
                   "entry_type_id objectives_id name_id content_tags".split())
H_ROOMS = tuple("dungeon_id room_id empty_reset_delay message_header_id "
                "message_text_id short_name".split())
H_OBJECTS = tuple("dungeon_id room_id object_id type_id x y z radius yaw pitch roll "
                  "object_min_respawn object_max_respawn object_delay_spawn "
                  "object_trigger_spawn name_id".split())
H_GATES = tuple("dungeon_id from_object_id allowed_ships_list "
                "disallowed_fitted_types_list allowed_ship_races".split())
H_SPAWNS = tuple("dungeon_id room_id object_id entity_id npc_grouping_id behavior_name "
                 "group_behavior_name trigger_spawn additional_loot_id "
                 "entity_min_respawn entity_max_respawn spawn_once delay_spawn "
                 "aggression_range orbit_range jitter_radius entity_name_id".split())
H_ARCHETYPES = tuple("archetype_id title_id content_tags location".split())
H_RESOURCES = tuple("dungeon_id kind type_ids ore_band".split())
H_ROOM_ROCKS = tuple("dungeon_id room_id type_id count".split())
H_AGENTS = tuple("key dungeon_id system_id spawn_point_id type_id".split())

# --- ore bands (C3 §3, §7.2 rule R4) ------------------------------------------

#: The bands, lowest first. "Lowest band" in R4 is the first of these.
BANDS = ("high", "low", "null")
#: C3 §3: `securityStatus >= HIGH_SEC` is high, `> 0` low, else null. [I]
#: the client's displayed rounding to 0.5, not a constant read. Falsifier:
#: a 0.45-0.4999 system the client's map shows as low-sec.
HIGH_SEC = 0.45
#: R4's threshold: a band counts for an ore when it holds more than this
#: share of the ore's belt rocks (C3 §7.2, [V-data] `content-C3-bands.tsv`).
BAND_SHARE = 0.01
#: An ore site with an ore that occurs in no belt (R4).
SPECIAL = "special"


def _f(v):
    return None if v is None else float(v)


def _i(v):
    return None if v is None else int(v)


def _b(v):
    return bool(v)


def _items(m):
    """`(int key, value)` pairs of a mapping (or none), sorted by key."""
    if m is None:
        return []
    return sorted((int(k), v) for k, v in m.items())


def _list(v):
    return [] if v is None else list(v)


def band_of(security):
    """C3 §3's band of one system's `securityStatus`."""
    if security >= HIGH_SEC:
        return "high"
    if security > 0:
        return "low"
    return "null"


def type_bands(belt_rocks, belt_band):
    """R4 per ore type: `{type_id: band}` from `{belt: {type: rocks}}` and
    `{belt: band}`. A belt with no band (no system) is skipped. A type whose
    rocks are all in skipped belts has no band, as if it were in no belt."""
    per = {}
    for bid, counts in belt_rocks.items():
        band = belt_band.get(int(bid))
        if band is None:
            continue
        for tid, n in counts.items():
            row = per.setdefault(int(tid), dict.fromkeys(BANDS, 0))
            row[band] += int(n)
    out = {}
    for tid, row in per.items():
        total = sum(row.values())
        for band in BANDS:
            if total and row[band] > BAND_SHARE * total:
                out[tid] = band
                break
    return out


def ore_band(type_ids, bands):
    """R4 per ore site: the highest band among its ores, or `SPECIAL` when
    any ore has none (occurs in no belt). An empty list has no band."""
    if not type_ids:
        return None
    got = [bands.get(int(t)) for t in type_ids]
    if any(b is None for b in got):
        return SPECIAL
    return max(got, key=BANDS.index)


# --- the dungeon files ----------------------------------------------------------

def dungeon_row(did, d):
    return (did, _i(d.get("archetypeID")), _i(d.get("factionID")),
            _i(d.get("difficulty")), _i(d.get("entryObjectID")),
            _i(d.get("entryTypeID")), _i(d.get("objectivesID")),
            _i(d.get("dungeonNameID")), _list(d.get("contentTags")))


def room_row(did, rid, r):
    return (did, rid, _i(r.get("emptyResetDelay")), _i(r.get("messageHeaderID")),
            _i(r.get("messageTextID")), r.get("shortName") or "")


def _objects(did, d):
    """`(room_id, object_id, object)` for one dungeon, by room then object."""
    for rid, room in _items(d.get("rooms")):
        for oid, o in _items(room.get("objects")):
            yield rid, oid, o


def object_row(did, oid, o):
    pos = o.get("position") or (0.0, 0.0, 0.0)
    g = o.get("guardCommand") or {}
    return (did, int(o.get("roomID")), oid, int(o.get("typeID")),
            float(pos[0]), float(pos[1]), float(pos[2]),
            _f(o.get("radius")), _f(o.get("yaw")), _f(o.get("pitch")), _f(o.get("roll")),
            _i(g.get("objectMinRespawnDelay")), _i(g.get("objectMaxRespawnDelay")),
            _b(g.get("objectDelaySpawn")), _b(g.get("objectTriggerSpawn")),
            _i(o.get("objectNameID")))


def spawn_rows(did, oid, o):
    g = o.get("guardCommand") or {}
    for eid, e in _items(o.get("entities")):
        yield (did, int(o.get("roomID")), oid, eid, _i(e.get("npcGroupingID")),
               e.get("behaviorName"), e.get("groupBehaviorName"),
               _b(e.get("triggerSpawn")), _i(e.get("additionalLootId")),
               _i(g.get("entityMinRespawnDelay")), _i(g.get("entityMaxRespawnDelay")),
               _b(g.get("entitySpawnOnce")), _b(g.get("entityDelaySpawn")),
               _f(g.get("aggressionRange")), _f(g.get("orbitRange")),
               _f(g.get("jitterRadius")), _i(e.get("entityNameID")))


def gate_row(did, c):
    return (did, int(c.get("fromObjectID")), _i(c.get("allowedShipsList")),
            _i(c.get("disallowedFittedTypesList")),
            [int(x) for x in _list(c.get("allowedShipRaces"))])


def dungeon_files(dungeons):
    """`{relpath: [rows]}` for the five files `dungeons.fsdbinary` gives, in
    key order (dungeon, room, object, entity; gates in the client's order)."""
    out = {k: [] for k in ("dungeons", "rooms", "objects", "gates", "spawns")}
    for did, d in _items(dungeons):
        out["dungeons"].append(dungeon_row(did, d))
        for rid, room in _items(d.get("rooms")):
            out["rooms"].append(room_row(did, rid, room))
        for _rid, oid, o in _objects(did, d):
            out["objects"].append(object_row(did, oid, o))
            out["spawns"].extend(spawn_rows(did, oid, o))
        for c in _list(d.get("connections")):
            out["gates"].append(gate_row(did, c))
    return out


def archetype_rows(archetypes):
    for aid, a in _items(archetypes):
        yield (aid, _i(a.get("titleID")), _list(a.get("contentTags")),
               a.get("location") or "")


def resource_rows(tables, bands):
    """`tables` is `[(kind, {dungeon: [type]})]`; rows by dungeon, then kind
    in `tables` order. `ore_band` is R4's, on `ore` rows only."""
    rows = []
    for rank, (kind, table) in enumerate(tables):
        for did, tids in _items(table):
            tids = [int(t) for t in tids]
            band = ore_band(tids, bands) if kind == "ore" else None
            rows.append(((did, rank), (did, kind, tids, band)))
    return [r for _k, r in sorted(rows, key=lambda x: x[0])]


def room_rock_rows(table):
    """`{dungeon: {room: {type: count}}}` flattened, in key order."""
    for did, rooms in _items(table):
        for rid, counts in _items(rooms):
            for tid, n in _items(counts):
                yield (did, rid, tid, int(n))


def agent_rows(table):
    for key, a in _items(table):
        yield (key, int(a.get("dungeonID")), int(a.get("solarSystemID")),
               int(a.get("spawnPointID")), int(a.get("typeID")))


# --- self-tests over rows (used by `check`, and by the unit tests) -------------

def room_rock_disagreements(rock_rows, object_rows):
    """C1 §2.5's self-test: every `(dungeon, room, type)` cell of the asteroid
    table equals the number of that room's objects of that type. Returns one
    string per cell that differs."""
    seen = {}
    for r in object_rows:
        k = (r[0], r[1], r[3])
        seen[k] = seen.get(k, 0) + 1
    bad = []
    for did, rid, tid, n in rock_rows:
        got = seen.get((did, rid, tid), 0)
        if got != n:
            bad.append("dungeon %d room %d type %d: table %d, objects %d"
                       % (did, rid, tid, n, got))
    return bad


def bad_entry_objects(dungeon_rows, object_rows):
    """`(dungeon_id, entry_object_id)` for every set `entry_object_id` that is
    not an `object_id` of the same dungeon."""
    have = {(r[0], r[2]) for r in object_rows}
    return [(r[0], r[4]) for r in dungeon_rows
            if r[4] is not None and (r[0], r[4]) not in have]


# --- the section ------------------------------------------------------------------

def _fsd():
    return _content._imp("fsd")  # noqa: SLF001


def belt_bands(ctx):
    """`{belt: band}` over every system with belts (not the sandbox)."""
    asteroids = _content._imp("asteroids")  # noqa: SLF001
    ssc, systems = ctx.load(SSC), ctx.load(SYSTEMS)
    out = {}
    for sid in sorted(int(s) for s in ssc):
        s = systems.get(sid)
        if s is None or s.get("securityStatus") is None:
            continue
        band = band_of(float(s.get("securityStatus")))
        for bid in asteroids.belts_of(ssc.get(sid)):
            out[bid] = band
    return out


def _resource_tables(ctx):
    return [(kind, ctx.load(res)) for kind, res in RESOURCES]


def section(ctx):
    fsd = _fsd()
    files = dungeon_files(fsd.rows(ctx.client, "dungeons"))
    bad = bad_entry_objects(files["dungeons"], files["objects"])
    if bad:
        ctx.note("dungeons/dungeons.tsv: %d entry_object_id not an object of their "
                 "dungeon (first: dungeon %d object %d)" % (len(bad), bad[0][0], bad[0][1]))
    bands = type_bands(ctx.load(BELT_ROCKS), belt_bands(ctx))
    d = "dungeons/"
    return [
        (d + "dungeons.tsv", H_DUNGEONS, files["dungeons"], [DUNGEONS]),
        (d + "rooms.tsv", H_ROOMS, files["rooms"], [DUNGEONS]),
        (d + "objects.tsv", H_OBJECTS, files["objects"], [DUNGEONS]),
        (d + "gates.tsv", H_GATES, files["gates"], [DUNGEONS]),
        (d + "spawns.tsv", H_SPAWNS, files["spawns"], [DUNGEONS]),
        (d + "archetypes.tsv", H_ARCHETYPES,
         archetype_rows(fsd.rows(ctx.client, "archetypes")), [ARCHETYPES]),
        (d + "resources.tsv", H_RESOURCES,
         resource_rows(_resource_tables(ctx), bands),
         [res for _k, res in RESOURCES] + [BELT_ROCKS, SSC, SYSTEMS]),
        (d + "room-rocks.tsv", H_ROOM_ROCKS, room_rock_rows(ctx.load(ROOM_ROCKS)),
         [ROOM_ROCKS]),
        (d + "agents-in-space.tsv", H_AGENTS,
         agent_rows(fsd.rows(ctx.client, "agentsinspace")), [AGENTS_IN_SPACE]),
    ]


# --- check (client-backed; [V] counts live only here, plan §1.6) ---------------

#: [V] build 3619879: `dungeon-model.md` §1, C1 §2.5, `schema.md` §2.
EXPECT = {
    "dungeons.tsv": 5516,
    "objects.tsv": 260242,
    "spawns.tsv": 30610,
    "archetypes.tsv": 71,
    "agents-in-space.tsv": 360,
    "resources.tsv": 1120 + 83 + 21,
    "room-rocks.tsv": 3054,
}

#: `dunArchetypeOreAnomaly` [V, C3 §1], and C3 §2's R4 split of its ore
#: sites [V-data]: 7 high, 16 low, 16 null, 83 with an ore in no belt.
ORE_ANOMALY = 27
EXPECT_ORE_ANOMALY_BANDS = {"high": 7, "low": 16, "null": 16, SPECIAL: 83}

#: Counts that differ in another build, ``{build: ({file: rows}, ore_bands)}``,
#: over `EXPECT` and `EXPECT_ORE_ANOMALY_BANDS` (either part may be None: the
#: base). The tables above are 3619879's, and 3628513 shares them. 3662168 is
#: not listed: `check` has held it to the base since the re-pin. Any build in neither place is UNVERIFIED: `expected` says so,
#: `check` notes the counts instead of failing on them, and the closures run.
#: After the first extraction of a new build add its row here.
EXPECT_BY_BUILD = {}
VERIFIED_BASE_BUILDS = (3619879, 3628513, 3662168)


def expected(build):
    """``(counts, ore_bands, verified)`` for a client build (None: reference)."""
    if build is None:
        build = 3662168
    counts, bands = EXPECT, EXPECT_ORE_ANOMALY_BANDS
    if build in EXPECT_BY_BUILD:
        c, b = EXPECT_BY_BUILD[build]
        counts = dict(EXPECT, **(c or {}))
        bands = b or bands
    return counts, bands, build in EXPECT_BY_BUILD or build in VERIFIED_BASE_BUILDS


def _read(ctx, name, conv):
    return [conv(r) for r in _content.read_rows(ctx.path("dungeons/" + name))]


def _ints(n):
    return lambda r: tuple(int(x) if x else None for x in r[:n])


def check(ctx):
    bad = []
    counts = {}
    build = _fsd().client_layout_build(ctx.client)
    want_counts, want_bands, verified = expected(build)
    for name in ("dungeons.tsv", "rooms.tsv", "objects.tsv", "gates.tsv", "spawns.tsv",
                 "archetypes.tsv", "resources.tsv", "room-rocks.tsv",
                 "agents-in-space.tsv"):
        try:
            counts[name] = sum(1 for _ in _content.read_rows(ctx.path("dungeons/" + name)))
        except OSError as e:
            return ["dungeons/%s: %s" % (name, e)]
        want = want_counts.get(name)
        if want is not None and counts[name] != want:
            if verified:
                bad.append("dungeons/%s: %d rows, expected %d [V]" % (name, counts[name], want))
            else:
                ctx.note("dungeons/%s: %d rows, UNVERIFIED build %s (older base says %d): "
                         "add a row to content_dungeons.EXPECT_BY_BUILD"
                         % (name, counts[name], build, want))
        ctx.note("dungeons/%s: %d rows" % (name, counts[name]))

    objects = _read(ctx, "objects.tsv", _ints(4))
    dungeons = _read(ctx, "dungeons.tsv", _ints(5))
    rocks = _read(ctx, "room-rocks.tsv", _ints(4))
    diff = room_rock_disagreements(rocks, objects)
    bad += ["dungeons/room-rocks.tsv: " + x for x in diff[:5]]
    ctx.note("dungeons/room-rocks.tsv: equals the room objects on %d of %d cells"
             % (len(rocks) - len(diff), len(rocks)))
    entry = bad_entry_objects(dungeons, objects)
    with_entry = sum(1 for r in dungeons if r[4] is not None)
    ctx.note("dungeons/dungeons.tsv: entry_object_id is an object of its dungeon on "
             "%d of %d; not: %s" % (with_entry - len(entry), with_entry,
                                    " ".join("%d:%d" % e for e in entry) or "none"))

    types = set(int(k) for k in _fsd().rows(ctx.client, "types"))
    res = _read(ctx, "resources.tsv", lambda r: (int(r[0]), r[1],
                                                   _content.decode_list(r[2]), r[3]))
    rtypes = {t for r in res for t in r[2]}
    miss = sorted(rtypes - types)
    if miss:
        bad.append("dungeons/resources.tsv: %d type ids not in types (first %d)"
                   % (len(miss), miss[0]))
    otypes = {r[3] for r in objects}
    omiss = sorted(otypes - types)
    if omiss:
        bad.append("dungeons/objects.tsv: %d type ids not in types (first %d)"
                   % (len(omiss), omiss[0]))
    ctx.note("dungeons: resource types close into types on %d of %d; object types on "
             "%d of %d" % (len(rtypes) - len(miss), len(rtypes),
                           len(otypes) - len(omiss), len(otypes)))
    arch = {r[0]: r[1] for r in dungeons}
    hist, ore27 = {}, {}
    for r in res:
        if r[1] == "ore":
            hist[r[3] or "-"] = hist.get(r[3] or "-", 0) + 1
            if arch.get(r[0]) == ORE_ANOMALY:
                ore27[r[3] or "-"] = ore27.get(r[3] or "-", 0) + 1
    ctx.note("dungeons/resources.tsv: ore_band %s; archetype %d: %s"
             % (" ".join("%s=%d" % kv for kv in sorted(hist.items())), ORE_ANOMALY,
                " ".join("%s=%d" % kv for kv in sorted(ore27.items()))))
    if ore27 != want_bands:
        if verified:
            bad.append("dungeons/resources.tsv: archetype %d ore bands %r, C3 §2 says %r"
                       % (ORE_ANOMALY, ore27, want_bands))
        else:
            ctx.note("dungeons/resources.tsv: archetype %d ore bands %r differ from the "
                     "older base %r; UNVERIFIED build %s" % (ORE_ANOMALY, ore27, want_bands, build))
    return bad
