"""The `npc_regions` content section (packet CP11b): who owns a region's belts,
each empire's home pirate, the NPC corporations' trades and the loot pools.

Writes `npc/regions.tsv` (C6 R3, with `content/npc/region-pirates.tsv`
applied), `npc/home-pirate.tsv` (C4 §2.4), `npc/trades.tsv` (C5 §2.3) and
`npc/loot.tsv` (C6 R7). Headers are `docs/content/schema.md` §2. The rules
are pure functions over plain dicts, so `tools/test_content_npc_regions.py`
runs them on an invented universe; `section` only reads the client into
those dicts. Every row is a query over the client: the repo holds the rules,
the `[A]` constants below and the sparse override file, never a row.
"""

import collections
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402

#: The driver's own `ContentError` when it runs as `__main__`, so a refusal
#: here is reported as one and not as a traceback.
_ERR = getattr(sys.modules.get("__main__"), "ContentError", content.ContentError)

SECTION = "npc_regions"
#: CP11a's `content_npc.py` owns `npc/factions.tsv` (prefixes, ammo groups,
#: tags), which the loot pools read. Until it lands there is no such section
#: and `loot.tsv` is written empty (a note says so) [A: the section is named
#: `npc`, after its file, as every section is].
DEPENDS = ("npc",) if os.path.isfile(os.path.join(_HERE, "content_npc.py")) else ()
CFSD = ("factions", "npccorporations", "missions", "dungeons", "types", "groups")

REGIONS_HEADER = ("region_id", "faction_id", "weight", "rule")
HOME_HEADER = ("faction_id", "pirate_faction_id")
TRADES_HEADER = ("corp_id", "type_id", "value")
LOOT_HEADER = ("faction_id", "size", "kind", "tier", "type_id", "meta_level")
OVERRIDE_HEADER = ("region_id", "faction_id", "weight", "why")
OVERRIDES = os.path.join(_HERE, os.pardir, "content", "npc", "region-pirates.tsv")

#: [A] C6 R3: a region within one region hop of a pirate's HQ or sovereign
#: region also spawns that pirate at this weight, relative to the region's
#: own pirates (which sum to 1). Falsifier: a pilot who knows Tash-Murkon or
#: Derelik says the second pirate is wrong or too common there.
SECONDARY_WEIGHT = 1.0 / 3.0

# Class-A vocabulary [V: `inventorycommon/const.pyc`, `dogmaattributes`].
CATEGORY_MODULE, CATEGORY_CHARGE = 7, 8
CATEGORY_ENTITY, CATEGORY_COMMODITY = 11, 17
META_TECH1, META_FACTION, META_OFFICER, META_DEADSPACE = 1, 4, 5, 6
ATTR_CHARGE_SIZE = 128
ATTR_CHARGE_GROUPS = (604, 605, 606)
SIZES = (1, 2, 3)

#: C6 R7 equipment groups [V: `dogma` group ids, the type names of each group
#: in `typedata.tsv`]. Propulsion is one group; a faction's tank is the booster
#: and extender groups (shield) or the repairer and plate groups (armor);
#: the ewar a faction carries is read from its NPCs' `needs`
#: (`npc/types.tsv`): SCRAM/WEB/NEUT name one module group each, and the
#: generic EWAR need is every other ewar group [A: the data holds no per-faction
#: choice among TP/ECM/damp/TD. Falsifier: a faction's rats on TQ that jam and
#: never paint].
GROUP_PROP, GROUP_PLATE = 46, 329
TANK_GROUPS = {"shield": (40, 38), "armor": (62, GROUP_PLATE)}
EWAR_GROUPS = {"SCRAM": (52,), "WEB": (65,), "NEUT": (71,),
               "EWAR": (379, 201, 208, 291)}
_WORD_SIZE = {"Micro": 1, "Heavy": 3, "Small": 1, "Medium": 2, "Large": 3, "X-Large": 3}
_MN = re.compile(r"(?:^|\s)(\d+)MN(?:\s|$)")
_MM = re.compile(r"(?:^|\s)(\d+)mm(?:\s|$)")


# --- R3 inputs: pirates and home pirates ------------------------------------------

def pirates(factions, corps, kills):
    """[I] A pirate is a faction whose executor corporation's `minSecurity` is
    the lowest any corporation has (CCP lets outlaws join it) **and** which is
    the enemy of some faction-locked kill mission. Either signal alone lets in
    event or Zarzakh factions. `factions` is `{id: {corporationID}}`, `corps`
    `{id: {minSecurity}}`, `kills` `[(offering, enemy)]`. Falsifier: CP11a's
    `factions.tsv` gives belt groups to a faction this misses (the check)."""
    floor = min((c.get("minSecurity") or 0.0) for c in corps.values())
    enemies = {e for o, e in kills if o is not None and e is not None and o != e}
    return {f for f, r in factions.items() if f in enemies
            and (corps.get(r.get("corporationID")) or {}).get("minSecurity") == floor}


def _argmax(counter):
    """The most common key; ties go to the lowest id, so a rerun is stable."""
    return min(counter, key=lambda k: (-counter[k], k)) if counter else None


def home_pirates(kills, pirate_set):
    """C4 §2.4: an offering faction's home pirate is the pirate it most often
    sends you to kill. A non-pirate faction with no pirate enemy takes the home
    pirate of the faction whose most frequent enemy is the same as its own
    [A: an ally shares its enemy; this gives Ammatar and Khanid Amarr's, C4
    §3.4's vassal rule, without naming them. Falsifier: a vassal agent whose
    missions on TQ favour another pirate]."""
    by_pirate = collections.defaultdict(collections.Counter)
    by_enemy = collections.defaultdict(collections.Counter)
    for o, e in kills:
        if o is None or e is None or o == e or o in pirate_set:
            continue
        (by_pirate if e in pirate_set else by_enemy)[o][e] += 1
    home = {o: _argmax(c) for o, c in by_pirate.items()}
    top = {o: _argmax(c) for o, c in by_enemy.items()}
    for o in sorted(set(by_enemy) - set(home)):
        allies = collections.Counter({a: by_enemy[a][top[o]] for a in home
                                      if top.get(a) == top[o]})
        if allies:
            home[o] = home[_argmax(allies)]
    return home


# --- R3: region -> [(pirate, weight, rule)] ---------------------------------------

def _share(counter):
    total = float(sum(counter.values()))
    return {k: v / total for k, v in counter.items()}


def nearest_pirates(systems, stations, pirate_set):
    """Gate BFS from every system with a pirate-owned station: for each reached
    system, the pirates whose stations are nearest (ties all count)."""
    near, frontier = {}, {}
    for sid in sorted(stations):
        own = {f for f in stations[sid] if f in pirate_set}
        if own:
            frontier[sid] = own
    while frontier:
        near.update(frontier)
        nxt = collections.defaultdict(set)
        for sid, own in frontier.items():
            for n in systems[sid][2]:
                if n in systems and n not in near:
                    nxt[n] |= own
        frontier = dict(nxt)
    return near


def region_pirates(regions, systems, stations, pirate_set, home, hq):
    """C6 R3 with C3 R6's null-space rules, one step per region, first match:

    0. a region with no `factionID` that holds a pirate's HQ;
    1. a region whose `factionID` is a pirate;
    2. a region with no `factionID` and pirate-owned stations, by station share;
    3. a region with a `factionID`: each system's faction (else the region's)
       votes for its home pirate;
    4. a region with no `factionID`: each system votes for its nearest
       pirate-station's owner by gate hops. Unreachable systems (wormhole,
       Jove) vote for nothing, so such a region has no belt pirates [A, C3 §8.1].

    Then SECONDARY_WEIGHT for a pirate whose HQ or step-0/1 region is the
    region itself or one hop away. `regions` is `{id: (faction, neighbours)}`,
    `systems` `{id: (region, faction, neighbours)}`, `stations`
    `{system: Counter(owner faction)}`, `hq` `{pirate: system}`.
    Returns `{region: {pirate: (weight, rule)}}`."""
    members = collections.defaultdict(list)
    for sid in sorted(systems):
        members[systems[sid][0]].append(sid)
    hq_regions = collections.defaultdict(set)
    for p, sid in hq.items():
        if p in pirate_set and sid in systems:
            hq_regions[systems[sid][0]].add(p)
    near = None
    out = {}
    for rid in sorted(regions):
        faction = regions[rid][0]
        if faction is None and hq_regions.get(rid):
            got = ({p: 1.0 for p in hq_regions[rid]}, "0")
        elif faction in pirate_set:
            got = ({faction: 1.0}, "1")
        else:
            owned = collections.Counter()
            for sid in members[rid]:
                for f, n in stations.get(sid, {}).items():
                    if f in pirate_set:
                        owned[f] += n
            votes = collections.Counter()
            if faction is None and owned:
                got = (_share(owned), "2")
            elif faction is not None:
                for sid in members[rid]:
                    p = home.get(systems[sid][1] or faction)
                    if p is not None:
                        votes[p] += 1
                got = (_share(votes), "3") if votes else None
            else:
                if near is None:
                    near = nearest_pirates(systems, stations, pirate_set)
                for sid in members[rid]:
                    for p in near.get(sid, ()):
                        votes[p] += 1.0 / len(near[sid])
                got = (_share(votes), "4") if votes else None
        if got:
            out[rid] = {p: (w, got[1]) for p, w in got[0].items()}
    homes = collections.defaultdict(set, {r: set(ps) for r, ps in hq_regions.items()})
    for rid, rows in out.items():
        if any(rule in ("0", "1") for _w, rule in rows.values()):
            homes[rid] |= set(rows)
    for rid in sorted(out):
        near_regions = {rid} | set(regions[rid][1])
        for p in sorted({p for r in near_regions for p in homes.get(r, ())}):
            out[rid].setdefault(p, (SECONDARY_WEIGHT, "secondary"))
    return out


def parse_overrides(lines):
    """`content/npc/region-pirates.tsv` (schema.md §4): `#` lines anywhere, the
    first other line is the header. Returns `(rows, problems)`; a bad header
    is a refusal, a bad row is skipped and reported (content/README.md)."""
    rows, problems, header = [], [], None
    for no, line in enumerate(lines, 1):
        line = line.rstrip("\n")
        if not line.strip() or line.startswith("#"):
            continue
        if header is None:
            header = tuple(line.split("\t"))
            if header != OVERRIDE_HEADER:
                raise _ERR("region-pirates.tsv:%d: header %r, want %r"
                                           % (no, line, "\t".join(OVERRIDE_HEADER)))
            continue
        f = line.split("\t")
        try:
            if len(f) != len(OVERRIDE_HEADER) or float(f[2]) < 0:
                raise ValueError("want 4 fields and weight >= 0")
            rows.append((int(f[0]), int(f[1]), float(f[2])))
        except ValueError as e:
            problems.append("region-pirates.tsv:%d: skipped (%s)" % (no, e))
    return rows, problems


def apply_overrides(table, overrides):
    """An override replaces the derived weight of its pair (or adds the pair);
    weight 0 removes it."""
    for rid, fid, w in overrides:
        rows = table.setdefault(rid, {})
        if w == 0:
            rows.pop(fid, None)
        else:
            rows[fid] = (w, "override")
        if not rows:
            del table[rid]
    return table


def region_rows(table):
    return [(r, f, float(w), rule) for r in sorted(table)
            for f, (w, rule) in sorted(table[r].items())]


# --- C5 §2.3 trades, C6 R7 loot ---------------------------------------------------

def trade_rows(corps):
    return [(c, int(t), float(v)) for c in sorted(corps)
            for t, v in sorted(((int(t), v) for t, v in
                                (corps[c].get("corporationTrades") or {}).items()))]


def module_size(group, nm):
    """The hull size (1-3) a tank, propulsion or sized-ewar module is made
    for, from its name; `None` when the name says none (a size-free module,
    or one this does not know, which the caller treats as it must).

    - propulsion: `<n>MN`, 1 and 5 small, 10 and 50 medium, 100 and 500 large;
      the capital 10000 and 50000 are no pool's [V names; I the pairing];
    - plates: `<n>mm`, up to 200 small, 400 and 800 medium, 1600 large [A:
      plate sizes are not in the data. Falsifier: a plate that fits a hull
      class other than the one this puts it in];
    - everything else: the word `Micro`/`Small`, `Medium`, `Large`/`X-Large`
      [A: Large is a battleship size for repairers and boosters alike.
      Falsifier: a cruiser or battlecruiser fit holding a Large one]."""
    nm = nm or ""
    if group == GROUP_PROP:
        m = _MN.search(nm)
        return {1: 1, 5: 1, 10: 2, 50: 2, 100: 3, 500: 3}.get(int(m.group(1))) if m else None
    if group == GROUP_PLATE:
        m = _MM.search(nm)
        if not m:
            return None
        mm = int(m.group(1))
        return 1 if mm <= 200 else 2 if mm <= 800 else 3 if mm <= 1600 else None
    for word in nm.split():
        if word in _WORD_SIZE:
            return _WORD_SIZE[word]
    return None


def _equipment_pools(f, group, nm, needs):
    """`(kind, size)` pools of one Tech I module of `group` in faction `f`:
    its tank (a tank module of its size, size known), propulsion (size known)
    and ewar groups (a size-free module serves every size)."""
    size = module_size(group, nm)
    out = []
    if group == GROUP_PROP:
        out.append(("prop", size))
    if group in TANK_GROUPS.get(f.get("tank"), ()):
        out.append(("tank", size))
    if any(group in EWAR_GROUPS[n] for n in needs if n in EWAR_GROUPS):
        out.append(("ewar", size))
    return [(k, s) for k, sz in out for s in ([sz] if sz else SIZES if k == "ewar" else [])]


def loot_rows(factions, types, sizes, name, needs=None):
    """C6 R7's pools from CP11a's `factions.tsv` rows (`{faction_id: {column:
    value}}`, decoded) and the types (`{id: {category, metaGroupID,
    metaLevel, groupID}}` of published types), `sizes` (`{type: chargeSize}`),
    `charge_groups` inside each type row, and `name(type)`. The draw policy
    (counts, chances, meta caps by tier) is code in CP22, not rows.

    - weapon/base: a Tech I module (meta level kept for the tier cap) firing
      the faction's ammo group at that size; tank/prop/ewar/base: a Tech I
      module of the faction's tank groups, of propulsion, or of the ewar
      groups its NPCs need (`needs`, `{faction_id: {need}}` from
      `npc/types.tsv`), at the size [`module_size`] its name gives (a size-free
      ewar module at all three); named and compact modules are metaGroup 1
      with a higher meta level [V: the market table], so they drop up to the
      tier cap; ammo/base: a charge of that
      group and size, not meta 2-6;
    - faction/commander, officer/officer, deadspace/deadspace: published types
      of meta group 4/5/6 named `<commander prefix> `, `<officer>'s `,
      `<stem> `; tag/base: the faction's tags. A type with a charge size
      is in that size's pool, any other in all three."""
    out = []
    for fid in sorted(factions):
        f = factions[fid]
        ammo = f.get("ammo_groups") or {}
        named = [("faction", "commander", META_FACTION,
                  [f["commander_prefix"] + " "] if f.get("commander_prefix") else []),
                 ("officer", "officer", META_OFFICER, [o + "'s " for o in f.get("officers") or ()]),
                 ("deadspace", "deadspace", META_DEADSPACE,
                  [s + " " for s in f.get("deadspace_stems") or ()])]
        tags = set(f.get("tags") or ())
        for tid in sorted(types):
            t = types[tid]
            size = sizes.get(tid)
            fits = [size] if size in SIZES else list(SIZES)
            pools = []
            if t["category"] == CATEGORY_MODULE and t.get("metaGroupID") == META_TECH1:
                pools += [(s, k, "base") for k, s in _equipment_pools(
                    f, t.get("groupID"), name(tid), (needs or {}).get(fid, ()))]
                pools += [(s, "weapon", "base") for s in fits
                          if ammo.get(s) is not None and ammo[s] in t.get("charge_groups", ())]
            if t["category"] == CATEGORY_CHARGE and t.get("metaGroupID") in (None, META_TECH1):
                pools += [(s, "ammo", "base") for s in fits
                          if ammo.get(s) is not None and ammo[s] == t.get("groupID")]
            for kind, tier, meta, prefixes in named:
                if t.get("metaGroupID") == meta and any(name(tid).startswith(p) for p in prefixes):
                    pools += [(s, kind, tier) for s in fits]
            if tid in tags:
                pools += [(s, "tag", "base") for s in SIZES]
            out += [(fid, s, kind, tier, tid, t.get("metaLevel")) for s, kind, tier in pools]
    return sorted(out)


#: [V-data] `<Faction> Clone Soldier <Rank>` rats and `Clone Soldier <Rank>
#: Tag` commodities (category 17); the rank is the whole link between them.
CLONE_SOLDIER, CLONE_TAG_KIND = " Clone Soldier ", "clone-tag"


def clone_tag_rows(types, name):
    """SEC-1c: one `clone-tag` row per Clone Soldier rat, its `faction_id`
    column the rat's **type** and `type_id` the tag of its rank. `types`
    is `{id: {category, groupID, published}}`, `name(type)` its English
    name. A rat is any type named `<x> Clone Soldier <Rank>` of category 11
    (entity) with a tag named `Clone Soldier <Rank> Tag`; a rank with no tag
    gets no row (the `check` fails on it)."""
    tags = {}
    for tid, t in types.items():
        n = name(tid) or ""
        if t.get("category") == CATEGORY_COMMODITY and n.startswith("Clone Soldier ") \
                and n.endswith(" Tag"):
            tags[n[len("Clone Soldier "):-len(" Tag")]] = tid
    out = []
    for tid, t in sorted(types.items()):
        n = name(tid) or ""
        if t.get("category") == CATEGORY_ENTITY and CLONE_SOLDIER in n:
            rank = n.split(CLONE_SOLDIER, 1)[1]
            if rank in tags:
                out.append((tid, 0, CLONE_TAG_KIND, "base", tags[rank], None))
    return out


def read_factions_tsv(path):
    """CP11a's `npc/factions.tsv` rows that carry R1 columns, decoded."""
    out = {}
    for f in content.read_rows(path):
        if not f[6]:  # `groups` empty: not a parsed pirate
            continue
        out[int(f[0])] = {
            "ammo_groups": content.decode_map(f[9], int, int),
            "tank": f[10] or None,
            "tags": content.decode_list(f[11]),
            "commander_prefix": f[14] or None,
            "deadspace_stems": content.decode_list(f[15], str),
            "officers": content.decode_list(f[16], str),
        }
    return out


def read_faction_needs(path):
    """`{faction_id: {need}}` from CP11a's `npc/types.tsv` (`faction_id` col 2,
    `needs` col 8): what each faction's NPCs carry. No file, no needs, so no
    ewar rows."""
    out = collections.defaultdict(set)
    if os.path.isfile(path):
        for f in content.read_rows(path):
            out[int(f[2])] |= set(content.decode_list(f[8], str))
    return dict(out)


# --- reading the client -----------------------------------------------------------

def _stations(system):
    def under(p):
        return [s.get("ownerID") for s in (p.get("npcStations") or {}).values()]
    out = under(system.get("star")) if system.get("star") else []
    for planet in (system.get("planets") or {}).values():
        out += under(planet)
        for moon in (planet.get("moons") or {}).values():
            out += under(moon)
    return out


def _inputs(ctx):
    if "npc_regions" in ctx._cache:  # noqa: SLF001
        return ctx._cache["npc_regions"]  # noqa: SLF001
    fsd = content._imp("fsd")  # noqa: SLF001
    c = ctx.client
    factions, corps = fsd.rows(c, "factions"), fsd.rows(c, "npccorporations")
    dungeons = fsd.rows(c, "dungeons")
    kills = []
    for m in fsd.rows(c, "missions").values():
        km = m.get("killMission")
        did = km.get("dungeonID") if km else None
        if did in dungeons:
            kills.append((m.get("factionID"), dungeons[did].get("factionID")))
    reg, sysst, ssc = ctx.load(content.REGIONS), ctx.load(content.SYSTEMS), ctx.load(content.SSC)
    regions = {int(r): (reg.get(int(r)).get("factionID"),
                        [int(n) for n in (reg.get(int(r)).get("neighbours") or ())])
               for r in reg}
    systems, stations = {}, {}
    for sid in (int(s) for s in sysst):
        s = sysst.get(sid)
        systems[sid] = (s.get("regionID"), s.get("factionID"),
                        [int(n.get("solarSystemID")) for n in (s.get("neighbours") or ())])
        owners = collections.Counter((corps.get(o) or {}).get("factionID")
                                     for o in _stations(ssc.get(sid) or {}))
        owners.pop(None, None)
        if owners:
            stations[sid] = owners
    pset = pirates(factions, corps, kills)
    home = home_pirates(kills, pset)
    hq = {f: r.get("solarSystemID") for f, r in factions.items()}
    got = (factions, corps, dungeons, regions, systems, stations, pset, home, hq)
    ctx._cache["npc_regions"] = got  # noqa: SLF001
    return got


def _loot(ctx):
    path = ctx.path("npc/factions.tsv")
    if not os.path.isfile(path):
        ctx.note("npc/loot.tsv: EMPTY, no npc/factions.tsv in this run (CP11a's section)")
        return []
    fsd, dogma = content._imp("fsd"), content._imp("dogma")  # noqa: SLF001
    factions = read_factions_tsv(path)
    tags = {t for f in factions.values() for t in f["tags"]}  # category 17
    groups = fsd.rows(ctx.client, "groups")
    types = {}
    for tid, t in fsd.rows(ctx.client, "types").items():
        cat = (groups.get(t.get("groupID")) or {}).get("categoryID")
        nm = ctx.text(t.get("typeNameID"))
        if tid in tags or (t.get("published") and cat in (CATEGORY_MODULE, CATEGORY_CHARGE)) \
                or (cat in (CATEGORY_ENTITY, CATEGORY_COMMODITY) and "Clone Soldier" in nm):
            types[tid] = dict(t, category=cat, charge_groups=())
    sizes = {}
    for tid, row in dogma.Table(ctx.client, "typedogma").rows():
        if tid not in types:
            continue
        a = {e["attributeID"]: e["value"] for e in row["dogmaAttributes"]}
        if ATTR_CHARGE_SIZE in a:
            sizes[tid] = int(a[ATTR_CHARGE_SIZE])
        types[tid]["charge_groups"] = tuple(int(a[k]) for k in ATTR_CHARGE_GROUPS if a.get(k))
    names = {tid: ctx.text(t.get("typeNameID")) for tid, t in types.items()}
    needs = read_faction_needs(ctx.path("npc/types.tsv"))
    return loot_rows(factions, types, sizes, names.get, needs) + clone_tag_rows(types, names.get)


def section(ctx):
    _f, corps, _d, regions, systems, stations, pset, home, hq = _inputs(ctx)
    table = region_pirates(regions, systems, stations, pset, home, hq)
    if os.path.isfile(OVERRIDES):
        with open(ctx.input(OVERRIDES), encoding="utf-8") as fh:
            overrides, problems = parse_overrides(fh)
        for p in problems:
            ctx.note(p)
        apply_overrides(table, overrides)
    src = [content.fsdbin(s) for s in ("factions", "npccorporations", "missions", "dungeons")]
    univ = [content.REGIONS, content.SYSTEMS, content.SSC]
    return [
        ("npc/regions.tsv", REGIONS_HEADER, region_rows(table), src + univ),
        ("npc/home-pirate.tsv", HOME_HEADER, sorted(home.items()), src),
        ("npc/trades.tsv", TRADES_HEADER, trade_rows(corps), [content.fsdbin("npccorporations")]),
        ("npc/loot.tsv", LOOT_HEADER, _loot(ctx),
         [content.fsdbin(s) for s in ("types", "groups", "typedogma")] + [content.LOC_EN]),
    ]


def clone_tag_problems(types, rows):
    """SEC-1c's check on the extract: `types` is `{id: (category, name)}`,
    `rows` the `clone-tag` rows of `npc/loot.tsv` as `(npc_type, tag)`.
    Every Clone Soldier rat must have its rank's tag, and nothing else."""
    want = {r[0]: r[4] for r in clone_tag_rows(
        {i: {"category": c} for i, (c, _n) in types.items()}, lambda i: types[i][1])}
    have = dict(rows)
    bad = ["npc/loot.tsv: Clone Soldier %d has no tag row" % t for t in sorted(set(want) - set(have))]
    bad += ["npc/loot.tsv: clone-tag row for %d, which is no Clone Soldier" % t
            for t in sorted(set(have) - set(want))]
    bad += ["npc/loot.tsv: Clone Soldier %d drops %d, its rank's tag is %d" % (t, have[t], want[t])
            for t in sorted(set(want) & set(have)) if have[t] != want[t]]
    return bad


def check_clone_tags(ctx):
    """Read `types/typedata.tsv` (the `type` rows: id, group, category ... name)."""
    path = ctx.path("types/typedata.tsv")
    if not os.path.isfile(path):
        return []
    types = {}
    for line in open(path, encoding="utf-8"):
        f = line.rstrip("\n").split("\t")
        if f[0] == "type" and len(f) > 3 and f[3] in ("11", "17") and "Clone Soldier" in f[-1]:
            types[int(f[1])] = (int(f[3]), f[-1])
    rows = [(int(r[0]), int(r[4])) for r in content.read_rows(ctx.path("npc/loot.tsv"))
            if r[2] == CLONE_TAG_KIND]
    ctx.note("npc/loot.tsv: %d clone-tag rows" % len(rows))
    return clone_tag_problems(types, rows)


# --- check (client-backed; real ids and counts live only here, plan §1.6) ---------

#: [V] C6 §2.7 / R3 step 1: Curse, Venal, Stain -> Angel, Guristas, Sansha.
KNOWN_REGIONS = {10000012: 500011, 10000015: 500010, 10000022: 500019}
#: The four empires and the name suffix of their pirate mission dungeons
#: (C6 §2.7: "… (Amarr)" dungeons carry the pirate `factionID`) [V].
EMPIRES = {500001: "(Caldari)", 500002: "(Minmatar)", 500003: "(Amarr)", 500004: "(Gallente)"}


def check(ctx):
    factions, _c, dungeons, _r, _s, _st, pset, _h, _hq = _inputs(ctx)
    bad = []
    regions = collections.defaultdict(dict)
    for r, f, w, rule in content.read_rows(ctx.path("npc/regions.tsv")):
        regions[int(r)][int(f)] = (float(w), rule)
    home = {int(f): int(p) for f, p in content.read_rows(ctx.path("npc/home-pirate.tsv"))}
    fname = lambda f: ctx.text((factions.get(f) or {}).get("nameID"))  # noqa: E731
    ctx.note("npc_regions: pirates %s" % ", ".join("%d %s" % (p, fname(p)) for p in sorted(pset)))
    for rid, want in KNOWN_REGIONS.items():
        rows = regions.get(rid, {})
        top = max(rows, key=lambda f: rows[f][0]) if rows else None
        if top != want:
            bad.append("npc/regions.tsv: region %d -> %s, want %d" % (rid, top, want))
    for e, suffix in sorted(EMPIRES.items()):
        vote = collections.Counter(d.get("factionID") for d in dungeons.values()
                                   if d.get("factionID") in pset
                                   and ctx.text(d.get("dungeonNameID")).endswith(suffix))
        want = _argmax(vote)
        ctx.note("home pirate %s: %s (mission-dungeon vote %s)"
                 % (fname(e), fname(home.get(e)), dict(vote)))
        if home.get(e) != want:
            bad.append("npc/home-pirate.tsv: %d -> %s, the dungeon vote says %s"
                       % (e, home.get(e), want))
    for f, p in sorted(home.items()):
        ctx.note("home pirate: %d %s -> %d %s" % (f, fname(f), p, fname(p)))
    rules = collections.Counter(rule for rows in regions.values() for _w, rule in rows.values())
    ctx.note("npc/regions.tsv: %d regions with pirates, rows by rule %s"
             % (len(regions), dict(sorted(rules.items()))))
    path = ctx.path("npc/factions.tsv")
    if os.path.isfile(path):
        parsed = {int(f[0]) for f in content.read_rows(path) if f[6]}
        if pset - parsed:
            bad.append("npc_regions: pirates with no belt groups in factions.tsv: %s"
                       % sorted(pset - parsed))
    bad += check_clone_tags(ctx)
    kinds = collections.Counter(r[2] for r in content.read_rows(ctx.path("npc/loot.tsv")))
    ctx.note("npc/loot.tsv: rows by kind %s" % dict(sorted(kinds.items())))
    if os.path.isfile(path):
        bad += ["npc/loot.tsv: no %s rows (C6 R7's equipment pool)" % k
                for k in ("weapon", "tank", "prop", "ewar") if not kinds.get(k)]
    ctx.note("npc/trades.tsv: %d rows; npc/loot.tsv: %d rows" % tuple(
        sum(1 for _ in content.read_rows(ctx.path(p))) for p in ("npc/trades.tsv", "npc/loot.tsv")))
    return bad
