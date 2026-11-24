"""The `achievements` content section: the Career Program achievements that count agent missions.

AIR scene 2.5.01 (#205) waits on `IsAnyCareerAgentMissionCompleted`, which reads the
character's achievement state for the NPE career achievements [V
`nodegraph/client/conditions/achievement.py`]. The server keeps that state
(`server/crates/eve-server/src/achievements.rs`) and needs each achievement's
milestones, their category points and their rewards.

**The definitions are not in the client install.** The client downloads them from
CCP's CDN through the public gateway (`achievement.definition.api.cdn.GetLatest`) and
keeps the flatbuffer in its user cache:

    %LOCALAPPDATA%/CCP/EVE/<install>_<server>/cache/Storage/achievements/data.bin
    (version.txt beside it; file identifier `ACHV`)

So this section reads that file, from a client that has entered the game on
Tranquility once, and only from where it is told: `EVE_ACHIEVEMENTS_BIN` if set, else
`out/cdn/achievements/data.bin`, where `tools/copy-cdn-achievements.sh CACHE_DIR` copies
it. It never looks through a client's or a user's folders on its own. The `out/` copy is
the one `tools/analysis.sh` can reach: the container sees the repo as `/work`, but not
the host's environment. With neither, the section writes an empty table and says how to
fix it. The manifest records the file as a `cdn:achievements@<version>` source, so a new
download re-extracts.

The flatbuffer layout is the client's own generated accessors [V decompiled
`achievement_system/client/model/flatbuffer/generated.pyj`]: each field below is the
vtable slot its accessor reads (``self._tab.Offset(slot)``).

Written into the bundle:

    achievements/agent-missions.tsv   achievement name category divisions levels
                                      milestone milestone_name target points reward
                                      kind amount type_id
    (one row per reward; divisions and levels are comma lists, levels may be empty)

An achievement is kept when it is under the Career Program root and its contribution
method takes an `agent_division` parameter and counts missions: either a SET of `mission`
values (definitions 1.7.3) or a cumulative count (1.8.0, where all nine career-agent
achievements moved to one `CumulativeMode` method with `agent_division`, `agent_type`,
`agent_level` and `agent_faction` parameters [V-TQ 20NOV2026, data.bin 1.8.0]).
**[A]** the server counts distinct missions for both: a career agent offers each of its
missions once, so a count of completions and a count of distinct missions agree;
falsifier: a career achievement on TQ that advances on a repeated mission.
`derive(tree)` is pure; the unit tests feed it invented trees. `gather` is the only
client-data reader.
"""
import os
import struct
import sys

SECTION = "achievements"
DEPENDS = ()

_HERE = os.path.dirname(os.path.abspath(__file__))

FILE = "achievements/agent-missions.tsv"
HEADER = ("achievement", "name", "category", "divisions", "levels", "milestone",
          "milestone_name", "target", "points", "reward", "kind", "amount", "type_id")
ENV = "EVE_ACHIEVEMENTS_BIN"
#: Where `tools/copy-cdn-achievements.sh` puts the file (gitignored `out/`).
LOCAL = os.path.join(_HERE, "..", "out", "cdn", "achievements", "data.bin")
COPY = "tools/copy-cdn-achievements.sh"

#: `RewardPayload` union tags [V generated.pyj].
REWARD_KINDS = {1: "ISK", 2: "SP", 3: "ITEM", 4: "TITLE"}
#: `ProgressConfig` union tags [V generated.pyj, progress_config.extract_from_flatbuffer]:
#: 1 CumulativeMode, 2 ChecklistMode, 3 SetMode.
PROGRESS_CUMULATIVE = 1
PROGRESS_SET = 3

# vtable slots [V generated.pyj], by table.
DATA_CAREER, DATA_METHODS = 4, 10
CAT_ID, CAT_CATEGORIES, CAT_ACHIEVEMENTS, CAT_NAME = 4, 14, 16, 18
ACH_ID, ACH_CONTRIBUTION, ACH_MILESTONES, ACH_NAME = 4, 16, 18, 30
CFG_METHOD, CFG_PARAMS = 4, 6
PARAM_KEY, PARAM_MATCHERS = 4, 6
MATCH_TYPE, MATCH_VALUES = 4, 6
MS_ID, MS_TARGET, MS_POINTS, MS_REWARDS, MS_NAME = 4, 6, 8, 10, 12
RW_ID, RW_TYPE, RW_PAYLOAD = 4, 6, 8
ISK_QTY, SP_QTY, ITEM_TYPE, ITEM_QTY, TITLE_ID = 4, 4, 4, 6, 4
M_ID, M_PROGRESS_TYPE, M_PROGRESS, M_PARAMS = 4, 18, 20, 16
MPARAM_KEY, SET_TARGET_TYPE = 4, 4


class ContentError(ValueError):
    """A refusal: the definitions file is not one this section can read."""


# ---- the flatbuffer reader (gather) ------------------------------------------------------

class _Table:
    """One flatbuffer table: a position and its vtable."""

    def __init__(self, buf, pos):
        self.b, self.pos = buf, pos
        self.vt = pos - struct.unpack_from("<i", buf, pos)[0]
        self.vlen = struct.unpack_from("<H", buf, self.vt)[0]

    def _off(self, slot):
        if slot >= self.vlen:
            return 0
        return struct.unpack_from("<H", self.b, self.vt + slot)[0]

    def scalar(self, slot, fmt, default=0):
        o = self._off(slot)
        return struct.unpack_from(fmt, self.b, self.pos + o)[0] if o else default

    def _deref(self, p):
        return p + struct.unpack_from("<I", self.b, p)[0]

    def string(self, slot):
        o = self._off(slot)
        if not o:
            return None
        p = self._deref(self.pos + o)
        n = struct.unpack_from("<I", self.b, p)[0]
        return self.b[p + 4:p + 4 + n].decode("utf-8")

    def table(self, slot):
        o = self._off(slot)
        return _Table(self.b, self._deref(self.pos + o)) if o else None

    def _vector(self, slot):
        o = self._off(slot)
        if not o:
            return []
        p = self._deref(self.pos + o)
        n = struct.unpack_from("<I", self.b, p)[0]
        return [p + 4 + 4 * i for i in range(n)]

    def tables(self, slot):
        return [_Table(self.b, self._deref(e)) for e in self._vector(slot)]

    def strings(self, slot):
        out = []
        for e in self._vector(slot):
            p = self._deref(e)
            n = struct.unpack_from("<I", self.b, p)[0]
            out.append(self.b[p + 4:p + 4 + n].decode("utf-8"))
        return out


def _reward(t):
    kind = REWARD_KINDS.get(t.scalar(RW_TYPE, "<B"), "NONE")
    p = t.table(RW_PAYLOAD)
    r = {"id": t.string(RW_ID), "kind": kind, "amount": 0, "type_id": None}
    if p is not None:
        if kind == "ISK":
            r["amount"] = p.scalar(ISK_QTY, "<Q")
        elif kind == "SP":
            r["amount"] = p.scalar(SP_QTY, "<Q")
        elif kind == "ITEM":
            r["type_id"] = p.scalar(ITEM_TYPE, "<I")
            r["amount"] = p.scalar(ITEM_QTY, "<Q")
        elif kind == "TITLE":
            r["title"] = p.string(TITLE_ID)
    return r


def _achievement(t):
    cfg = t.table(ACH_CONTRIBUTION)
    params = {}
    method = None
    if cfg is not None:
        method = cfg.string(CFG_METHOD)
        for p in cfg.tables(CFG_PARAMS):
            params[p.string(PARAM_KEY)] = {m.string(MATCH_TYPE): m.strings(MATCH_VALUES)
                                           for m in p.tables(PARAM_MATCHERS)}
    return {
        "id": t.string(ACH_ID),
        "name": t.string(ACH_NAME),
        "method": method,
        "params": params,
        "milestones": [{
            "id": m.string(MS_ID),
            "name": m.string(MS_NAME),
            "target": m.scalar(MS_TARGET, "<Q"),
            "points": m.scalar(MS_POINTS, "<I"),
            "rewards": [_reward(r) for r in m.tables(MS_REWARDS)],
        } for m in t.tables(ACH_MILESTONES)],
    }


def _category(t):
    return {
        "id": t.string(CAT_ID),
        "name": t.string(CAT_NAME),
        "categories": [_category(c) for c in t.tables(CAT_CATEGORIES)],
        "achievements": [_achievement(a) for a in t.tables(CAT_ACHIEVEMENTS)],
    }


def read_definitions(buf):
    """The decoded tree of a definitions flatbuffer: the Career Program category and
    every contribution method."""
    if len(buf) < 8 or buf[4:8] != b"ACHV":
        raise ContentError("not an achievements flatbuffer (no ACHV identifier)")
    root = _Table(buf, struct.unpack_from("<I", buf, 0)[0])
    methods = {}
    for m in root.tables(DATA_METHODS):
        prog = m.table(M_PROGRESS)
        mode = m.scalar(M_PROGRESS_TYPE, "<B") if prog is not None else None
        target = prog.string(SET_TARGET_TYPE) if mode == PROGRESS_SET else None
        methods[m.string(M_ID)] = {
            "params": [p.string(MPARAM_KEY) for p in m.tables(M_PARAMS)],
            "set_of": target,
            "mode": mode,
        }
    career = root.table(DATA_CAREER)
    return {"methods": methods, "career": _category(career) if career else None}


def find_definitions(environ=None, local=LOCAL):
    """`(path, version)` of the definitions file to read, or `None`: `EVE_ACHIEVEMENTS_BIN`
    if set, else the `out/` copy. Nothing else is looked at."""
    environ = os.environ if environ is None else environ
    p = environ[ENV] if environ.get(ENV) else local
    if not os.path.isfile(p):
        return None
    vpath = os.path.join(os.path.dirname(p), "version.txt")
    if os.path.isfile(vpath):
        with open(vpath) as fh:
            v = fh.read().strip()
    else:
        v = "0.0.0"
    return p, v


def gather(ctx):
    """The decoded definitions and the `cdn:` source they came from, or `(None, None)`."""
    found = find_definitions()
    if found is None:
        return None, None
    path, version = found
    with open(path, "rb") as fh:
        tree = read_definitions(fh.read())
    return tree, ctx.cdn("achievements@%s" % version, path)


# ---- the derivation (pure) ----------------------------------------------------------------

def _counts_agent_missions(method):
    if method is None or "agent_division" not in method.get("params", ()):
        return False
    return method.get("set_of") == "mission" or method.get("mode") == PROGRESS_CUMULATIVE


def _walk(cat, top):
    for a in cat["achievements"]:
        yield a, cat["id"]
    for c in cat["categories"]:
        yield from _walk(c, top)


def derive(tree):
    """Bundle rows for every Career Program achievement that counts agent missions by
    division: one row per milestone reward, in file order."""
    if not tree or not tree.get("career"):
        return []
    methods = tree["methods"]
    rows = []
    for a, cat in _walk(tree["career"], tree["career"]["id"]):
        method = methods.get(a["method"])
        if not _counts_agent_missions(method):
            continue
        divisions = a["params"].get("agent_division", {}).get("division", [])
        if not divisions:
            if method.get("mode") == PROGRESS_CUMULATIVE:
                # 1.8.0's shared method makes the division optional; an achievement that
                # filters on something else (acp_sof_factional_warfare_iii: agent_type)
                # is one the server cannot count yet, so it is left out, not refused.
                continue
            raise ContentError("%s: an agent-mission achievement with no division" % a["name"])
        levels = a["params"].get("agent_level", {}).get("level", [])
        for m in a["milestones"]:
            if not m["rewards"]:
                rows.append((a["id"], a["name"], cat, ",".join(divisions), ",".join(levels),
                             m["id"], m["name"], m["target"], m["points"], "", "NONE", 0, ""))
            for r in m["rewards"]:
                rows.append((a["id"], a["name"], cat, ",".join(divisions), ",".join(levels),
                             m["id"], m["name"], m["target"], m["points"], r["id"], r["kind"],
                             r["amount"], "" if r["type_id"] is None else r["type_id"]))
    return rows


def section(ctx):
    tree, source = gather(ctx)
    if tree is None:
        ctx.note("achievements: NO DEFINITIONS FILE, the table is empty: the Career Program "
                 "achievements (and AIR scene 2.5) will not work. Enter the game once on "
                 "Tranquility, then run %s CACHE_DIR (or set %s)" % (COPY, ENV))
        return [(FILE, HEADER, [], [])]
    try:
        rows = derive(tree)
    except ContentError as e:
        raise ContentError("achievements: %s" % e) from None
    ctx.note("achievements: %d reward rows from %s" % (len(rows), source))
    return [(FILE, HEADER, rows, [source])]


def check(ctx):
    path = ctx.path(FILE)
    if not os.path.exists(path):
        return [FILE + ": missing from the bundle"]
    tree, _ = gather(ctx)
    if tree is None:
        return []
    want = derive(tree)
    with open(path, encoding="utf-8") as fh:
        have = [l for l in fh.read().splitlines() if l and not l.startswith("#")]
    if len(have) != len(want):
        return ["%s: differs from a fresh derive (%d vs %d rows)" % (FILE, len(have), len(want))]
    ctx.note("achievements: %d rows match a fresh derive" % len(want))
    return []


if __name__ == "__main__":
    sys.exit("run through tools/content.py --sections achievements")
