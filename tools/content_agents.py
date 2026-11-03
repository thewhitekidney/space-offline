"""The `agents` section: `agents/agents.tsv`, `agents/missions.tsv`,
`agents/mission-messages.tsv`, `agents/epic-arcs.tsv`, `agents/schools.tsv`,
`agents/races.tsv` and `agents/bloodlines.tsv`.

Packet CP13 (`docs/content/plan.md` §6.2, W12). The columns are C4 §3.1's,
in the house snake_case, exactly as `docs/content/schema.md` §2 freezes them
(C4 wins over C1 §2.7). The consumer is `mission_pool.rs`, whose
`parse_agents` and `parse_missions` compare the header line byte for byte.

    tools/analysis.sh python3 -I /work/tools/content.py --sections agents
    tools/analysis.sh python3 -I /work/tools/content.py --sections agents --check

Everything that turns client rows into bundle rows is a pure function over
plain dicts (`agent_rows`, `mission_rows` and their helpers), so the unit
tests (`tools/test_content_agents.py`) run on invented rows. `section(ctx)`
only loads the client tables and hands them over. Counts and the client's
own ids are asserted in `check(ctx)` alone (plan §1.6).

FX5 (W16) adds `mission-messages.tsv` and `epic-arcs.tsv`, which
`epic_arc.rs` (CP41) reads (`epic_arc::ARCS`, `epic_arc::MESSAGES`).

P-START-X adds `schools.tsv` (the start-location table `start.rs` reads:
a school's first starting station is where a new character of that school
starts, `characterdata/schools.py` [V]) and `races.tsv` (each race's rookie
ship and starter skills, for a later fresh-start packet).

Lane start-skills adds `bloodlines.tsv` (each bloodline's race and
corporation, `bloodlines` cFSD [V]): `start.rs` refuses a
`CreateCharacterWithDoll` whose bloodline is not of the chosen race.

Not written here yet: `npc-characters.tsv`, `agent-types.tsv` and
`divisions.tsv` (plan §1.2 lists them under this section; no packet reads
them).
"""

import collections
import os
import re
import sys

SECTION = "agents"
#: `check` reads `universe/celestials.tsv` (every agent's station exists).
DEPENDS = ("universe",)
#: The cFSD stems the schema guard re-derives before this section runs.
CFSD = ("npccharacters", "npccorporations", "missions", "dungeons", "types",
        "epicarcs", "schools", "schoolmap", "races", "bloodlines")

RES = "res:/staticdata/"
LOC_EN = "res:/localizationfsd/localization_fsd_en-us.pickle"
SSC = RES + "solarsystemcontent.static"


def _bin(stem):
    return RES + stem + ".fsdbinary"


# The header lines, copied from `schema.md` §2 (and equal, byte for byte, to
# `mission_pool.rs`'s `AGENTS_HEADER` / `MISSIONS_HEADER`).
AGENTS_HEADER = tuple(
    "agent_id agent_type_id division_id level corp_id faction_id station_id "
    "system_id is_locator storyline_missions".split())
MISSIONS_HEADER = tuple(
    "mission_id name_id template family level level_src offer_faction_id "
    "enemy_faction_id dungeon_id obj_type_id obj_qty obj_volume_total "
    "obj_group_id drop_item_type_id expire_minutes agent_type_id corp_id "
    "chain_key chain_index chain_len has_next_slot gift_type_id gift_qty "
    "reward_type_id reward_qty bonus_type_id bonus_qty bonus_minutes fixed_lp "
    "has_standing_rewards extra_standings content_tags messages".split())
#: `schema.md` `agents/mission-messages.tsv` (FX5), equal to `epic_arc.rs`'s
#: `MESSAGES` header.
MESSAGES_HEADER = ("mission_id", "slot", "message_id")
#: `schema.md` `agents/epic-arcs.tsv` (FX5), equal to `epic_arc.rs`'s `ARCS`.
EPIC_ARCS_HEADER = tuple(
    "arc_id mission_id agent_id next_missions fail_mission_id restart_minutes "
    "faction_id name_id".split())

#: `schema.md` `agents/schools.tsv` (P-START-X adds `starter_space`), equal
#: to `start.rs`'s `HEADER`.
SCHOOLS_HEADER = tuple(
    "school_id corp_id race_id career_id system_id starting_stations "
    "career_agents name_id starter_space".split())
#: `schema.md` `agents/races.tsv` (P-START-X).
RACES_HEADER = ("race_id", "ship_type_id", "skills", "name_id")
#: `schema.md` `agents/bloodlines.tsv` (lane start-skills), equal to
#: `start.rs`'s `BLOODLINES` header.
BLOODLINES_HEADER = ("bloodline_id", "race_id", "corp_id", "name_id")

TEMPLATE_PREFIX = "agent.missionTemplatizedContent_"
TEMPLATE_SUFFIX = "Mission"
#: `schema.md` `family`: `EpicArcTalkToAgent` and `EpicArcAgentInteraction`
#: are `talk`; anything with no kill/courier/mining/trade suffix is `other`.
TALK_TEMPLATES = ("EpicArcTalkToAgent", "EpicArcAgentInteraction")
#: The slot that says "a next part follows" (C4 §2.5).
NEXT_SLOT_SUFFIX = "completed.nextmission.agentsays"

#: [A] C4 §2.3: a courier's level is a cargo-volume band. At most
#: `VOLUME_BANDS[i]` m3 is level i+1; anything larger is
#: `len(VOLUME_BANDS) + 1`. Falsified by a Tranquility capture of a level-N
#: agent offering a courier outside its band.
VOLUME_BANDS = (250.0, 1000.0, 4000.0)

#: "Series - Part (k of n)", with the client's spacing slips ("( 2 of 2)")
#: and trailing text ("(3 of 13) (part 2)", `"Ruse"`) allowed [V titles].
_CHAIN = re.compile(r"^(.*?)\(\s*(\d+)\s+of\s+(\d+)\s*\)")
#: The series key ends at the first dash that has whitespace before it
#: ("Sign-up" is not a separator; "Ruemin -Serpentis" is) [V titles].
_SERIES_SEP = re.compile(r"\s[-–]")


# --- pure helpers -------------------------------------------------------------

def template_of(content_template):
    """`agent.missionTemplatizedContent_BasicKillMission` -> `BasicKill`."""
    t = content_template or ""
    if t.startswith(TEMPLATE_PREFIX):
        t = t[len(TEMPLATE_PREFIX):]
    if t.endswith(TEMPLATE_SUFFIX):
        t = t[:-len(TEMPLATE_SUFFIX)]
    return t


def family_of(template):
    if template in TALK_TEMPLATES:
        return "talk"
    for suffix, fam in (("Kill", "kill"), ("Courier", "courier"),
                        ("Mining", "mining"), ("Trade", "trade")):
        if template.endswith(suffix):
            return fam
    return "other"


def parse_chain(title):
    """`(chain_key, k, n)` for a "(k of n)" title, else `(None, None, None)`.

    The key is the series name (the text before the first spaced dash, or
    before the parenthesis), with runs of whitespace collapsed, lowercased.
    """
    m = _CHAIN.match(title or "")
    if not m:
        return None, None, None
    head = _SERIES_SEP.split(m.group(1), 1)[0]
    key = " ".join(head.split()).lower()
    if not key:
        return None, None, None
    return key, int(m.group(2)), int(m.group(3))


def volume_level(volume):
    """C4 §2.3's [A] cargo-volume band (`VOLUME_BANDS`)."""
    for i, top in enumerate(VOLUME_BANDS):
        if volume <= top:
            return i + 1
    return len(VOLUME_BANDS) + 1


def canonical_name_ids(missions, title):
    """Ruling 14: `{name_id: lowest name_id with the same title text}`, and
    `{title: sorted name_ids}` for every title that had more than one.
    `missions` is `{mission_id: row}`; `title(name_id)` is the en-us text."""
    by_title = collections.defaultdict(set)
    for row in missions.values():
        nid = row.get("nameID")
        if nid is not None and title(nid):
            by_title[title(nid)].add(int(nid))
    canon, multi = {}, {}
    for text, ids in by_title.items():
        low = min(ids)
        for nid in ids:
            canon[nid] = low
        if len(ids) > 1:
            multi[text] = sorted(ids)
    return canon, multi


def _int(v):
    return None if v is None else int(v)


def station_systems(content):
    """`{station_id: system_id}` over `solarsystemcontent`: stations listed
    under the star, a planet or a moon (ruling 6, C4 §11.5)."""
    out = {}
    for sid in content:
        system = content.get(sid)
        parents = []
        star = system.get("star")
        if star:
            parents.append(star)
        for _pid, planet in (system.get("planets") or {}).items():
            parents.append(planet)
            parents.extend((planet.get("moons") or {}).values())
        for parent in parents:
            for st in (parent.get("npcStations") or {}):
                out[int(st)] = int(sid)
    return out


def agent_rows(characters, corp_faction, station_system):
    """`agents.tsv` rows, sorted by `agent_id`: every `npccharacters` row
    with an `agent` struct. `corp_faction` is `{corp: factionID}`."""
    for aid in sorted(int(k) for k in characters):
        r = characters[aid]
        a = r.get("agent")
        if not a:
            continue
        corp = int(r["corporationID"])
        station = _int(r.get("stationID"))
        yield (
            aid, int(a["agentType"]), int(a["agentDivisionID"]), int(a["agentLevel"]),
            corp, _int(corp_faction.get(corp)), station,
            station_system.get(station) if station is not None else None,
            bool(a.get("agentLocateCharacter")),
            [int(x) for x in (a.get("storyLineMissions") or ())],
        )


def level_of(family, difficulty, volume_total):
    """`(level, level_src)` (C4 §2.3, §3.1): a dungeon's `difficulty`, else
    a courier's volume band, else nothing."""
    if difficulty is not None:
        return int(difficulty), "difficulty"
    if family == "courier" and volume_total is not None:
        return volume_level(volume_total), "volume_band"
    return None, "none"


def mission_rows(missions, dungeons, volumes, canon, title):
    """`missions.tsv` rows, sorted by `mission_id`.

    `dungeons` is `{dungeonID: row}` (`difficulty`, `factionID`), `volumes`
    is `{typeID: m3}`, `canon` is `canonical_name_ids`'s first value and
    `title(name_id)` the en-us text of a title.
    """
    for mid in sorted(int(k) for k in missions):
        r = missions[mid]
        template = template_of(r.get("contentTemplate"))
        family = family_of(template)
        kill = r.get("killMission") or {}
        courier = r.get("courierMission") or {}
        obj = courier or kill
        dungeon_id = _int(kill.get("dungeonID"))
        dungeon = (dungeons.get(dungeon_id) if dungeon_id is not None else None) or {}
        obj_type, obj_qty = _int(obj.get("objectiveTypeID")), _int(obj.get("objectiveQuantity"))
        vol = volumes.get(obj_type) if obj_type is not None else None
        volume_total = (float(obj_qty * vol)
                        if obj_qty is not None and vol is not None else None)
        level, level_src = level_of(family, dungeon.get("difficulty"), volume_total)
        name_id = int(r["nameID"])
        chain_key, k, n = parse_chain(title(name_id))
        msgs = r.get("messages") or {}
        rewards = r.get("missionRewards") or {}
        reward = rewards.get("reward") or {}
        bonus = rewards.get("bonusReward") or {}
        yield (
            mid, canon.get(name_id, name_id), template, family, level, level_src,
            _int(r.get("factionID")), _int(dungeon.get("factionID")), dungeon_id,
            obj_type, obj_qty, volume_total,
            _int(kill.get("objectiveGroupID")), _int(kill.get("dropItemInMissionContainer")),
            _int(r.get("expirationTime")), _int(r.get("agentTypeID")),
            _int(r.get("corporationID")),
            chain_key, k, n,
            any(s.endswith(NEXT_SLOT_SUFFIX) for s in msgs),
            _int(r.get("initialAgentGiftTypeID")), _int(r.get("initialAgentGiftQuantity")),
            _int(reward.get("rewardTypeID")), _int(reward.get("rewardQuantity")),
            _int(bonus.get("rewardTypeID")), _int(bonus.get("rewardQuantity")),
            _int(rewards.get("bonusTimeInterval")),
            # [A] erratum 13: the client keeps an Alpha and an Omega value;
            # eve-offline has no clone states, so the Omega (full) reward.
            # Falsified by a capture of an Omega pilot paid the Alpha number.
            int(r.get("fixedLpRewardOmega") or 0),
            bool(r.get("hasStandingRewards")),
            {int(f): float(v) for f, v in sorted(
                (r.get("extraStandings") or {}).items(), key=lambda kv: int(kv[0]))},
            [str(t) for t in (r.get("contentTags") or ())],
            {str(s): int(msgs[s]) for s in sorted(msgs)},
        )


def message_rows(missions):
    """`mission-messages.tsv` rows: every `messages` pair of every mission,
    sorted by `(mission_id, slot)`; the same pairs as `missions.tsv`'s
    `messages` column, in long form (`schema.md`)."""
    for mid in sorted(int(k) for k in missions):
        msgs = missions[mid].get("messages") or {}
        for slot in sorted(msgs):
            yield mid, str(slot), int(msgs[slot])


def epic_arc_rows(arcs):
    """`epic-arcs.tsv` rows, sorted by `(arc_id, mission_id)`: one per
    mission of each `epicarcs` row (C4 §7.1), with the arc's own columns
    repeated on every row (`schema.md`)."""
    for aid in sorted(int(k) for k in arcs):
        arc = arcs[aid]
        nodes = arc.get("missions") or {}
        for mid in sorted(int(k) for k in nodes):
            node = nodes[mid]
            yield (
                aid, mid, int(node["agentID"]),
                [int(x) for x in (node.get("nextMissions") or ())],
                _int(node.get("failMissionID")),
                int(arc["arcRestartInterval"]), _int(arc.get("factionID")),
                int(arc["epicArcNameID"]),
            )


def school_rows(schools, schoolmap):
    """`schools.tsv` rows, sorted by `school_id`. `schoolmap` is the client
    table as read: its rows carry `schoolID` and `solarSystemID` (its keys
    are row indices, not school ids). A school with no `schoolmap` row has
    an empty `system_id`; an absent list is the empty list."""
    system_of = {int(r["schoolID"]): int(r["solarSystemID"]) for r in schoolmap.values()}
    for sid in sorted(int(k) for k in schools):
        r = schools[sid]
        yield (
            sid, int(r["corporationID"]), int(r["raceID"]), _int(r.get("careerID")),
            system_of.get(sid),
            [int(x) for x in (r.get("startingStations") or ())],
            [int(x) for x in (r.get("careerAgents") or ())],
            int(r["nameID"]),
            bool(r.get("is_starter_space_school")),
        )


def race_rows(races):
    """`races.tsv` rows, sorted by `race_id`: the rookie ship (`shipTypeID`,
    absent for most races) and the starter skills `{type_id: level}`,
    sorted by type id."""
    for rid in sorted(int(k) for k in races):
        r = races[rid]
        skills = r.get("skills") or {}
        yield (
            rid, _int(r.get("shipTypeID")),
            {int(k): int(v) for k, v in sorted(skills.items(), key=lambda kv: int(kv[0]))},
            int(r["nameID"]),
        )


def bloodline_rows(bloodlines):
    """`bloodlines.tsv` rows, sorted by `bloodline_id`: the race
    (`raceID`), the corporation (`corporationID`) and `nameID`."""
    for bid in sorted(int(k) for k in bloodlines):
        r = bloodlines[bid]
        yield bid, int(r["raceID"]), int(r["corporationID"]), int(r["nameID"])


# --- the section ----------------------------------------------------------------

def _driver(ctx):
    """The driver module that made `ctx` (`content`, or `__main__` when
    `content.py` is the script): its `_imp` imports a reader safely."""
    return sys.modules[type(ctx).__module__]


def _inputs(ctx):
    fsd = _driver(ctx)._imp("fsd")
    c = ctx.client
    corps = fsd.rows(c, "npccorporations")
    types = fsd.cfsd_rows(fsd.CfsdTable(c.path(_bin("types")), "types"), "types")
    return {
        "characters": fsd.rows(c, "npccharacters"),
        "corp_faction": {int(k): r.get("factionID") for k, r in corps.items()},
        "station_system": station_systems(ctx.load(SSC)),
        "missions": fsd.rows(c, "missions"),
        "epicarcs": fsd.rows(c, "epicarcs"),
        "dungeons": fsd.rows(c, "dungeons"),
        "volumes": {int(t): float(r["volume"]) for t, r in types.items()
                    if r.get("volume") is not None},
        "schools": fsd.rows(c, "schools"),
        "schoolmap": fsd.rows(c, "schoolmap"),
        "races": fsd.rows(c, "races"),
        "bloodlines": fsd.rows(c, "bloodlines"),
    }


def section(ctx):
    i = _inputs(ctx)
    canon, multi = canonical_name_ids(i["missions"], ctx.text)
    ctx.note("agents/missions.tsv: %d titles had more than one name_id; "
             "each now carries its lowest (ruling 14, listed by --check)" % len(multi))
    return [
        ("agents/agents.tsv", AGENTS_HEADER,
         agent_rows(i["characters"], i["corp_faction"], i["station_system"]),
         [_bin("npccharacters"), _bin("npccorporations"), SSC]),
        ("agents/missions.tsv", MISSIONS_HEADER,
         mission_rows(i["missions"], i["dungeons"], i["volumes"], canon, ctx.text),
         [_bin("missions"), _bin("dungeons"), _bin("types"), LOC_EN]),
        ("agents/mission-messages.tsv", MESSAGES_HEADER,
         message_rows(i["missions"]), [_bin("missions")]),
        ("agents/epic-arcs.tsv", EPIC_ARCS_HEADER,
         epic_arc_rows(i["epicarcs"]), [_bin("epicarcs")]),
        ("agents/schools.tsv", SCHOOLS_HEADER,
         school_rows(i["schools"], i["schoolmap"]), [_bin("schools"), _bin("schoolmap")]),
        ("agents/races.tsv", RACES_HEADER, race_rows(i["races"]), [_bin("races")]),
        ("agents/bloodlines.tsv", BLOODLINES_HEADER, bloodline_rows(i["bloodlines"]),
         [_bin("bloodlines")]),
    ]


# --- the check (client-backed; counts live only here) ---------------------------

def _read_list(ctx, rel, header):
    path = ctx.path(rel)
    with open(path, encoding="utf-8") as fh:
        first = fh.readline().rstrip("\n")
    want = "#" + "\t".join(header)
    if first != want:
        return None, ["%s: header %r, expected %r" % (rel, first, want)]
    return list(_driver(ctx).read_rows(path)), []


def _read(ctx, rel, header):
    rows, bad = _read_list(ctx, rel, header)
    return (None, bad) if rows is None else ({int(r[0]): r for r in rows}, [])


def check(ctx):
    bad = []
    agents, b1 = _read(ctx, "agents/agents.tsv", AGENTS_HEADER)
    missions, b2 = _read(ctx, "agents/missions.tsv", MISSIONS_HEADER)
    messages, b3 = _read_list(ctx, "agents/mission-messages.tsv", MESSAGES_HEADER)
    arcs, b4 = _read_list(ctx, "agents/epic-arcs.tsv", EPIC_ARCS_HEADER)
    bad += b1 + b2 + b3 + b4
    if bad:
        return bad
    i = _inputs(ctx)
    want_a = sum(1 for r in i["characters"].values() if r.get("agent"))
    for rel, got, want in (("agents/agents.tsv", len(agents), want_a),
                           ("agents/missions.tsv", len(missions), len(i["missions"]))):
        if got != want:
            bad.append("%s: %d rows, the client has %d" % (rel, got, want))
    # C4 §2.1 / §13 K1 [V]: 10,972 agents, 2,892 missions.
    for rel, got, want in (("agents/agents.tsv", len(agents), 10972),
                           ("agents/missions.tsv", len(missions), 2892)):
        if got != want:
            bad.append("%s: %d rows, C4 measured %d" % (rel, got, want))
    ctx.note("agents/agents.tsv: %d rows; agents/missions.tsv: %d rows" % (len(agents), len(missions)))

    # Every agent's station exists in universe/ (CP2's star/planet/moon stations).
    if ctx.systems is None:
        stations = set()
        with open(ctx.path("universe/celestials.tsv"), encoding="utf-8") as fh:
            for ln in fh:
                f = ln.split("\t")
                if f[0] == "station":
                    stations.add(int(f[1]))
        missing = sorted(a for a, r in agents.items() if r[6] and int(r[6]) not in stations)
        if missing:
            bad.append("agents/agents.tsv: %d agents' stations are not in "
                       "universe/celestials.tsv (first %d)" % (len(missing), missing[0]))
        no_station = sum(1 for r in agents.values() if not r[6])
        no_faction = sum(1 for r in agents.values() if not r[5])
        if no_faction:
            bad.append("agents/agents.tsv: %d agents have no faction" % no_faction)
        ctx.note("agents/agents.tsv: %d with a station, all in universe/celestials.tsv: %s; "
                 "%d without one" % (len(agents) - no_station, not missing, no_station))
    else:
        ctx.note("agents/agents.tsv: station closure skipped (a --systems bundle)")

    # The spot rows C4 §13 K1 names [V].
    spots = [
        (missions, 4832, {3: "courier", 4: "1", 5: "volume_band"}),
        (missions, 2924, {3: "kill", 4: "1", 5: "difficulty", 7: "500011"}),
        (missions, 2504, {4: "1"}), (missions, 3081, {4: "2"}),
        (missions, 3082, {4: "3"}), (missions, 3083, {4: "4"}),
        (agents, 3010110, {5: "500007", 7: "30000001", 8: "0"}),
    ]
    for table, key, cols in spots:
        row = table.get(key)
        for col, want in cols.items():
            got = row[col] if row is not None else None
            if got != want:
                bad.append("spot row %d column %d: %r, C4 says %r" % (key, col, got, want))
    ctx.note("C4 §13 K1 spot rows checked: %d" % len(spots))

    # Ruling 14: one name_id per title, the lowest, and the titles that had several.
    canon, multi = canonical_name_ids(i["missions"], ctx.text)
    off = [m for m, r in missions.items()
           if int(r[1]) != canon[int(i["missions"][m]["nameID"])]]
    if off:
        bad.append("agents/missions.tsv: %d rows not canonicalised (first %d)" % (len(off), off[0]))
    ctx.note("ruling 14: %d titles had more than one name_id:" % len(multi))
    for text in sorted(multi):
        ctx.note("  %r: %s" % (text, ",".join(map(str, multi[text]))))

    fams = collections.Counter(r[3] for r in missions.values())
    srcs = collections.Counter(r[5] for r in missions.values())
    def chain_stats(rows):
        rows = [r for r in rows if r[17]]
        agree = sum(1 for r in rows if (int(r[18]) < int(r[19])) == (r[20] == "1"))
        return len(rows), agree
    all_n, all_ok = chain_stats(missions.values())
    basic_n, basic_ok = chain_stats(r for r in missions.values() if r[2].startswith("Basic"))
    lp = sum(1 for r in missions.values() if r[28] != "0")
    ctx.note("families %s; level_src %s; chain rows %d (next slot agrees with k < n on %d), "
             "Basic chain rows %d (agrees on %d; C4 §2.5 measured 297 of 306); fixed_lp set on %d"
             % (dict(sorted(fams.items())), dict(sorted(srcs.items())),
                all_n, all_ok, basic_n, basic_ok, lp))
    bad += _check_messages(ctx, missions, messages)
    bad += _check_arcs(ctx, i["epicarcs"], agents, missions, arcs)
    bad += _check_schools(ctx, i)
    return bad


def _station_systems_of_bundle(ctx):
    """`{station_id: system_id}` from `universe/celestials.tsv` (a DEPENDS)."""
    out = {}
    with open(ctx.path("universe/celestials.tsv"), encoding="utf-8") as fh:
        for ln in fh:
            f = ln.rstrip("\n").split("\t")
            if f[0] == "station":
                out[int(f[1])] = int(f[7])
    return out


def _check_schools(ctx, i):
    """`schools.tsv`, `races.tsv` and `bloodlines.tsv` against the client
    (R-START §2.3-2.4):
    one row per client row; every school with a `system_id` starts at a
    station in that system (`schoolmap` agrees with `startingStations[0]`);
    every `corp_id` is a client corporation; a starter-space school has no
    `system_id`; every bloodline's race is a `races` row and its
    corporation a client corporation; every school's race has a
    bloodline."""
    bad = []
    missing = [rel for rel in ("agents/schools.tsv", "agents/races.tsv",
                               "agents/bloodlines.tsv")
               if not os.path.isfile(ctx.path(rel))]
    if missing:
        return ["%s: not in the bundle; re-extract the agents section" % rel for rel in missing]
    schools, b1 = _read(ctx, "agents/schools.tsv", SCHOOLS_HEADER)
    races, b2 = _read(ctx, "agents/races.tsv", RACES_HEADER)
    bloodlines, b3 = _read(ctx, "agents/bloodlines.tsv", BLOODLINES_HEADER)
    if b1 or b2 or b3:
        return b1 + b2 + b3
    for rel, got, want in (("agents/schools.tsv", len(schools), len(i["schools"])),
                           ("agents/races.tsv", len(races), len(i["races"])),
                           ("agents/bloodlines.tsv", len(bloodlines), len(i["bloodlines"]))):
        if got != want:
            bad.append("%s: %d rows, the client has %d" % (rel, got, want))
    # R-START §2.1-2.2 [V]: 23 schools, 12 of them in `schoolmap`.
    with_system = {s: r for s, r in schools.items() if r[4]}
    for what, got, want in (("rows", len(schools), 23), ("rows with a system_id", len(with_system), 12)):
        if got != want:
            bad.append("agents/schools.tsv: %d %s, R-START measured %d" % (got, what, want))
    dec = _driver(ctx).decode_list
    if ctx.systems is None:
        st_sys = _station_systems_of_bundle(ctx)
        off = sorted(s for s, r in with_system.items()
                     if not dec(r[5]) or st_sys.get(dec(r[5])[0]) != int(r[4]))
        if off:
            bad.append("agents/schools.tsv: %d schools whose first station is not in their "
                       "system_id (first %d)" % (len(off), off[0]))
        ctx.note("agents/schools.tsv: first station lies in system_id for %d of %d"
                 % (len(with_system) - len(off), len(with_system)))
    else:
        ctx.note("agents/schools.tsv: station closure skipped (a --systems bundle)")
    no_corp = sorted(s for s, r in schools.items() if int(r[1]) not in i["corp_faction"])
    if no_corp:
        bad.append("agents/schools.tsv: %d corp_ids not client corporations (first school %d)"
                   % (len(no_corp), no_corp[0]))
    starter = {s for s, r in schools.items() if r[8] == "1"}
    both = sorted(s for s in starter if schools[s][4])
    if both:
        bad.append("agents/schools.tsv: %d starter-space schools have a system_id (first %d)"
                   % (len(both), both[0]))
    by_race = collections.Counter(int(r[2]) for s, r in schools.items()
                                  if s not in starter and dec(r[5]))
    ctx.note("agents/schools.tsv: %d rows, %d with a system_id, %d starter space; "
             "placeable schools per race %s"
             % (len(schools), len(with_system), len(starter), dict(sorted(by_race.items()))))
    ships = sum(1 for r in races.values() if r[1])
    ctx.note("agents/races.tsv: %d rows, %d with a rookie ship, %d with starter skills"
             % (len(races), ships, sum(1 for r in races.values() if r[2])))
    skill_counts = {rid: len(_driver(ctx).decode_map(r[2], int, int))
                    for rid, r in races.items() if r[2]}
    ctx.note("agents/races.tsv: starter skills per race %s" % dict(sorted(skill_counts.items())))
    no_race = sorted(b for b, r in bloodlines.items() if int(r[1]) not in races)
    if no_race:
        bad.append("agents/bloodlines.tsv: %d bloodlines of no races.tsv race (first %d)"
                   % (len(no_race), no_race[0]))
    bl_no_corp = sorted(b for b, r in bloodlines.items() if int(r[2]) not in i["corp_faction"])
    if bl_no_corp:
        bad.append("agents/bloodlines.tsv: %d corp_ids not client corporations (first %d)"
                   % (len(bl_no_corp), bl_no_corp[0]))
    bl_races = {int(r[1]) for r in bloodlines.values()}
    orphan = sorted(set(by_race) - bl_races)
    if orphan:
        bad.append("agents/bloodlines.tsv: races %s have a school but no bloodline" % orphan)
    per_race = collections.Counter(int(r[1]) for r in bloodlines.values())
    ctx.note("agents/bloodlines.tsv: %d rows; bloodlines per race %s"
             % (len(bloodlines), dict(sorted(per_race.items()))))
    return bad


def _check_messages(ctx, missions, messages):
    """`mission-messages.tsv` holds exactly `missions.tsv`'s `messages`
    pairs (`schema.md`: "CP13's `check` asserts that the two agree")."""
    bad = []
    dec = _driver(ctx).decode_map
    wide = {(m, s, v) for m, r in missions.items()
            for s, v in dec(r[32], str, int).items()}
    long_ = [(int(r[0]), r[1], int(r[2])) for r in messages]
    if len(set(long_)) != len(long_):
        bad.append("agents/mission-messages.tsv: duplicate (mission_id, slot) rows")
    if set(long_) != wide:
        bad.append("agents/mission-messages.tsv: %d pairs differ from missions.tsv's "
                   "messages column" % len(set(long_) ^ wide))
    epic = {m for m, s, _ in long_ if s.startswith("messages.epicMission.")}
    ctx.note("agents/mission-messages.tsv: %d rows over %d missions, equal to "
             "missions.tsv's messages column: %s; %d missions carry a "
             "messages.epicMission.* slot"
             % (len(long_), len({m for m, _, _ in long_}), set(long_) == wide, len(epic)))
    return bad


def _check_arcs(ctx, client_arcs, agents, missions, arcs):
    """`epic-arcs.tsv` against the client and C4 §7.1 [V]: 21 arcs, 275
    missions, every mission and `next_missions` target closes into
    `missions.tsv` and stays inside its arc, every agent is in `agents.tsv`,
    and the arc columns are the same on each of an arc's rows."""
    bad = []
    want_rows = sum(len(a.get("missions") or {}) for a in client_arcs.values())
    by_arc = collections.defaultdict(list)
    for r in arcs:
        by_arc[int(r[0])].append(r)
    for what, got, want in (("rows", len(arcs), want_rows),
                            ("arcs", len(by_arc), len(client_arcs))):
        if got != want:
            bad.append("agents/epic-arcs.tsv: %d %s, the client has %d" % (got, what, want))
    # C4 §7.1 [V]: 21 arcs, 275 missions.
    for what, got, want in (("arcs", len(by_arc), 21), ("rows", len(arcs), 275)):
        if got != want:
            bad.append("agents/epic-arcs.tsv: %d %s, C4 measured %d" % (got, what, want))
    seen = collections.Counter(int(r[1]) for r in arcs)
    twice = sorted(m for m, n in seen.items() if n > 1)
    if twice:
        bad.append("agents/epic-arcs.tsv: %d missions in two rows (first %d)"
                   % (len(twice), twice[0]))
    dec = _driver(ctx).decode_list
    no_mission = no_agent = outside = 0
    for aid, rows in by_arc.items():
        if len({tuple(r[5:8]) for r in rows}) != 1:
            bad.append("agents/epic-arcs.tsv: arc %d's columns differ between rows" % aid)
        inside = {int(r[1]) for r in rows}
        for r in rows:
            no_mission += int(r[1]) not in missions
            no_agent += int(r[2]) not in agents
            outside += sum(1 for x in dec(r[3]) if x not in inside)
            outside += bool(r[4]) and int(r[4]) not in inside
    for n, what in ((no_mission, "missions not in agents/missions.tsv"),
                    (no_agent, "agents not in agents/agents.tsv"),
                    (outside, "next/fail targets outside their own arc")):
        if n:
            bad.append("agents/epic-arcs.tsv: %d %s" % (n, what))
    valid = sum(1 for rows in by_arc.values() if int(rows[0][5]) > 1)
    ctx.note("agents/epic-arcs.tsv: %d rows, %d arcs (%d with restart_minutes > 1, "
             "which IsValidArc shows); missions, agents and next/fail targets close: %s"
             % (len(arcs), len(by_arc), valid, not (no_mission or no_agent or outside)))
    return bad
