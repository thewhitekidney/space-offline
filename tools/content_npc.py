"""The `npc` content section, part one: factions, corporations and types.

Packet CP11a (`docs/content/plan.md` §6.2, W12). Spec: C6 R1, R2, §4 and N1,
C1 §2.6; every header is `docs/content/schema.md`'s. Writes six bundle files:

    npc/factions.tsv       one row per `factions` key; C6 R1's columns on
                           the factions the group grammar parses
    npc/corporations.tsv   one row per `npccorporations` key
    npc/types.tsv          one row per category-11 type in a parsed group
    npc/components.tsv     `spacecomponentsbytype` presence index
    npc/reactive-spawn.tsv every `reactiveSpawnComponent`, by scope
    npc/bounty.tsv         attribute 481 plus `bountyMultipliersByOwner`

The derivation is pure (`derive(data)` over plain dicts, so the unit tests
feed it invented rows); `gather(ctx)` is the only client reader. Regions,
the home pirate, trades and loot are CP11b's (`content_npc_regions.py`).

Every `[A]` below is shipped on purpose and names its falsifier.
"""

import collections
import re
import sys

SECTION = "npc"
DEPENDS = ()
CFSD = ("types", "groups", "typedogma", "dogmaeffects", "graphicids", "factions",
        "npccorporations", "spacecomponentsbytype")

# The header lines, copied from `schema.md` §2.
FACTIONS_HEADER = tuple(
    "faction_id corporation_id militia_corporation_id system_id member_races "
    "owner_corp_id groups wrecks damage_profile ammo_groups tank tags base_prefix "
    "elite_prefix commander_prefix deadspace_stems officers sof_faction name".split())
CORPORATIONS_HEADER = tuple(
    "corp_id faction_id station_id system_id friend_id enemy_id tax_rate "
    "min_security main_activity_id size extent lp_offer_tables ticker name".split())
TYPES_HEADER = tuple(
    "type_id group_id faction_id faction_src context class tier alias_of needs role".split())
COMPONENTS_HEADER = ("type_id", "component")
REACTIVE_HEADER = tuple(
    "type_id scope key spawn_table_id initial_fleet_type_id owner_id "
    "fleet_counter_table_id bounty_payout_multiplier friendly_response_threshold".split())
BOUNTY_HEADER = ("type_id", "bounty", "multipliers")

CATEGORY_ENTITY = 11   # `categoryEntity` [V]
CATEGORY_CHARGE = 8    # `categoryCharge` [V]

# Dogma attribute ids [V: `dogmaattributes` names, build 3619879].
A_HP, A_ARMOR, A_SHIELD = 9, 265, 263
A_ATTACK_RANGE = 247          # entityAttackRange
A_BOUNTY = 481                # entityKillBounty
A_DAMAGE_MULT = 64            # damageMultiplier
A_DAMAGE = (114, 116, 117, 118)  # em, explosive, kinetic, thermal
A_CHARGE_SIZE = 128           # chargeSize
A_MISSILE_TYPE = 507          # entityMissileTypeID

# C6 R2 capabilities and R8 roles, from dogma effect ids [V: `dogmaeffects`
# names; the effects that occur on category-11 types]. Which id joins which
# capability is [I] from the effect name; falsifier: an NPC whose behaviour
# in the client contradicts its capability.
NEEDS = (
    ("TURRET", {10, 6995}),                       # targetAttack, targetDisintegratorAttack
    ("MISSILE", {569}),                           # missileLaunchingForEntity (+ attr 507 > 0)
    ("SCRAM", {563, 6745, 6744, 5928, 39, 3713}),  # warpScrambleForEntity, behaviorWarp*, ...
    ("WEB", {575, 6743, 3714}),                   # modifyTargetSpeed2, npcBehaviorWebifier, ...
    ("EWAR", {1879, 6754, 1878, 6755, 6695, 6757, 3710, 1752, 4656, 6846, 6747, 6746}),
    ("NEUT", {6691, 6756, 6882, 6187}),
    ("SELF_REP", {2192, 2193, 2194, 876, 5371, 6990, 2195, 2196, 2197, 878, 5370, 6884}),
    ("REMOTE_REP", {6741, 6742, 3852, 3855, 6165, 6687, 4686, 4689}),
)
SHIELD_REP = {2192, 2193, 2194, 876, 5371, 6990}
ARMOR_REP = {2195, 2196, 2197, 878, 5370, 6884}

# C6 §2.1's grammar. A faction's key is its token's first word, lowercased,
# apostrophes dropped, so `Sansha's`/`Sanshas` and `Blood Raider(s)` agree.
CONTEXTS = ("Asteroid", "Deadspace", "Mission", "Ghost Sites", "Roaming",
            "Incursion", "Insurgency")
#: C6 §2.1's faction alternatives, as written in the group names.
FACTION_TOKENS = (r"Angel Cartel", r"Blood Raiders?", r"Guristas", r"Sansha'?s(?: Nation)?",
                  r"Serpentis", r"Rogue Drones?", r"Mordu'?s(?: Legion)?")


def grammar(tokens=FACTION_TOKENS):
    """`^(<context>) (<faction>) (<class>)$`; the tests pass invented tokens."""
    return re.compile(r"^(%s) (%s) (.+)$" % ("|".join(CONTEXTS), "|".join(tokens)))


GRAMMAR = grammar()
#: The five human pirates of C6 R1's falsifier (Rogue Drones reuse empire
#: hulls, 66% [V]; Mordu has three groups). Used by `check` only.
HUMAN_PIRATES = ("angel", "blood", "guristas", "sanshas", "serpentis")
#: C6 R1 `deadspace_stems`, frigate to battleship.
DEADSPACE_CLASSES = ("frigate", "destroyer", "cruiser", "battlecruiser", "battleship")
#: `groups` name of the tag group, category 17 (C6 §2.6 [V]).
CRIMINAL_TAGS = "Criminal Tags"
ROAMING_SPAWNER = re.compile(r"^Zarzakh (\S+) .*Roaming Spawner$")


def key_of(token):
    """A faction key from a grammar token or a faction name."""
    return token.split(" ")[0].replace("'", "").lower()


def parse_group(name):
    """`(context, key, class)` for a group the §2.1 grammar parses, else None."""
    m = GRAMMAR.match(name or "")
    return (m.group(1), key_of(m.group(2)), m.group(3)) if m else None


def prefix_of(name):
    """The name minus its last word (C6 §2.2)."""
    return name.rsplit(" ", 1)[0] if " " in name else ""


def size_of(cls):
    """R7's size: frigate/destroyer 1, cruiser/BC 2, battleship 3; else None."""
    last = cls.split(" ")[-1].lower()
    return {"frigate": 1, "destroyer": 1, "cruiser": 2, "battlecruiser": 2,
            "battleship": 3}.get(last)


def needs_of(attrs, effects):
    """C6 R2's capability list, in `NEEDS` order."""
    eff = set(effects)
    out = [n for n, ids in NEEDS if eff & ids]
    if "MISSILE" not in out and (attrs.get(A_MISSILE_TYPE) or 0) > 0:
        out.insert(1 if "TURRET" in out else 0, "MISSILE")
    return out


def role_of(needs):
    """C6 R8, first match (CCP's roles [V], the order [A]: falsifier, a type
    whose client-side role differs)."""
    if "REMOTE_REP" in needs:
        return "LOGISTIC"
    if "SCRAM" in needs or "WEB" in needs:
        return "TACKLER"
    if "EWAR" in needs or "NEUT" in needs:
        return "EWAR"
    return "DPS"


def _mode(values):
    """The most common value; ties go to the smallest. None when empty."""
    c = collections.Counter(v for v in values if v is not None and v != "")
    if not c:
        return None
    top = max(c.values())
    return min(v for v, n in c.items() if n == top)


def faction_keys(factions):
    """`{key: faction_id}` from each faction's name, lowest id on a clash."""
    out = {}
    for fid in sorted(factions):
        out.setdefault(key_of(factions[fid].get("name") or ""), fid)
    return out


# --- the derivation (pure) -----------------------------------------------------

def npc_types(data):
    """One record per category-11 type in a parsed group (C6 R2's faction).

    `types.factionID` wins where it is present and names a faction (K1 fixed
    its offset to +72). [A] When it names a corporation instead, the
    corporation's faction is used, still `faction_src=type`; falsifier: such
    a type shown in the client under another faction. Else the group's."""
    keys = faction_keys(data["factions"])
    out = {}
    for tid in sorted(data["types"]):
        t = data["types"][tid]
        g = data["groups"].get(t["group"])
        if g is None or g[0] != CATEGORY_ENTITY:
            continue
        parsed = parse_group(g[1])
        if parsed is None:
            continue
        context, key, cls = parsed
        fid, src = t.get("faction"), "type"
        if fid is not None and fid not in data["factions"]:
            fid = (data["corps"].get(fid) or {}).get("factionID")
        if fid not in data["factions"]:
            fid, src = keys.get(key), "group"
        if fid is None:
            continue
        attrs, effects = data["dogma"].get(tid, ({}, []))
        out[tid] = {"type_id": tid, "group": t["group"], "faction": fid, "src": src,
                    "key": key, "context": context, "cls": cls, "name": t.get("name") or "",
                    "attrs": attrs, "effects": sorted(effects),
                    "needs": needs_of(attrs, effects)}
    return out


def aliases(npcs):
    """`{type_id: canonical}` for C6 §2.2's alias rows: types of one group
    with equal `(bounty, hp, attackRange, effects)` collapse to the lowest
    type id. [A] the group is part of the key, so no alias crosses a
    context or a faction; falsifier: an alias pair split across groups."""
    first, out = {}, {}
    for tid in sorted(npcs):
        n = npcs[tid]
        a = n["attrs"]
        k = (n["group"], a.get(A_BOUNTY), a.get(A_HP), a.get(A_ARMOR), a.get(A_SHIELD),
             a.get(A_ATTACK_RANGE), tuple(n["effects"]))
        if k in first:
            out[tid] = first[k]
        else:
            first[k] = tid
    return out


def _median(xs):
    xs = sorted(xs)
    return xs[len(xs) // 2] if xs else 0.0


def _own(prefix, key):
    """True when one of the prefix's words is the faction's key word."""
    return any(key_of(w) == key for w in prefix.split(" "))


def prefix_bands(members, key):
    """`(base, elite)` from one faction's `Asteroid <F> Frigate` types.

    [A] base: the shortest prefix whose first word is the faction's key word
    (`Angel`, `Sansha's`), else the prefix of the cheapest type. elite: of
    the other prefixes whose median bounty is above base's, the most common
    (C6 §2.2: "the elite prefix starts above the base"); the alias prefixes
    (`Cyber`, `Outlaw`) copy base's bounties, so they never qualify.
    Falsifier: a faction whose tags or faction loot use another prefix."""
    bounty = collections.defaultdict(list)
    for n in members:
        p = prefix_of(n["name"])
        if p:
            bounty[p].append(n["attrs"].get(A_BOUNTY) or 0.0)
    own = sorted((p for p in bounty if key_of(p.split(" ")[0]) == key), key=len)
    if own:
        base = own[0]
    elif bounty:
        base = min(bounty, key=lambda p: (min(bounty[p]), p))
    else:
        return None, None
    floor = _median(bounty[base])
    above = [p for p in bounty if p != base and _median(bounty[p]) > floor]
    if not above:
        return base, None
    return base, max(above, key=lambda p: (len(bounty[p]), -_median(bounty[p]), p))


def commander_prefix(members, key):
    """[A] The `Asteroid <F> Commander Frigate` prefix holding the faction's
    key word (`Dread Guristas`), else the most common (`Domination` over its
    alias `Psycho`). Falsifier: faction modules under another prefix."""
    names = [prefix_of(n["name"]) for n in members]
    own = [p for p in names if p and _own(p, key)]
    return _mode(own) if own else _mode(names)


def deadspace_stem(members):
    """[A] The word most `Deadspace <F> <class>` names carry before their
    last word, ties to the one that comes first more often (`Corelum` over
    `Chief`). Falsifier: deadspace modules named after another stem."""
    score = collections.Counter()
    for n in members:
        words = n["name"].split(" ")[:-1]
        for w in dict.fromkeys(words):
            score[w] += 1
        if words:
            score[(words[0],)] += 1
    if not score:
        return ""
    words = [w for w in score if isinstance(w, str)]
    return max(sorted(words), key=lambda w: (score[w], score[(w,)]))


def _ammo_groups(members, charges):
    """C6 §2.6: per size, the charge group that wins the cosine vote of the
    faction's Asteroid turret types against T1 charges of that size [V for
    the computation; I that NPCs "use" it]."""
    votes = collections.defaultdict(collections.Counter)
    for n in members:
        size = size_of(n["cls"])
        if n["context"] != "Asteroid" or size is None or "TURRET" not in n["needs"]:
            continue
        v = [n["attrs"].get(a) or 0.0 for a in A_DAMAGE]
        nv = sum(x * x for x in v) ** 0.5
        if nv == 0:
            continue
        best = None
        for cid, (grp, csize, cv) in sorted(charges.items()):
            if csize != size:
                continue
            nc = sum(x * x for x in cv) ** 0.5
            if nc == 0:
                continue
            cos = sum(x * y for x, y in zip(v, cv)) / (nv * nc)
            if best is None or cos > best[0]:
                best = (cos, grp)
        if best is not None:
            votes[size][best[1]] += 1
    return {s: _mode(list(votes[s].elements())) for s in sorted(votes)}


def faction_profile(members, data, tags):
    """C6 R1's derived columns for one parsed faction."""
    by = collections.defaultdict(list)
    for n in members:
        by[(n["context"], n["cls"].lower())].append(n)
    key = members[0]["key"] if members else None
    base, elite = prefix_bands(by[("Asteroid", "frigate")], key)
    commander = commander_prefix(by[("Asteroid", "commander frigate")], key)
    stems = [deadspace_stem(by[("Deadspace", c)]) for c in DEADSPACE_CLASSES]
    officers = []
    for n in sorted(members, key=lambda n: n["type_id"]):
        if n["context"] == "Asteroid" and n["cls"].startswith("Officer") and n["name"]:
            first = n["name"].split(" ")[0]
            if first not in officers:
                officers.append(first)
    wrecks = {}
    for cls in sorted({n["cls"] for n in members}):
        w = _mode([data["types"][n["type_id"]].get("wreck")
                   for n in members if n["cls"] == cls])
        if w is not None:
            wrecks[cls] = w
    profile, k = [0.0] * 4, 0
    for n in members:
        if "TURRET" not in n["needs"]:
            continue
        m = n["attrs"].get(A_DAMAGE_MULT) or 1.0
        v = [(n["attrs"].get(a) or 0.0) * m for a in A_DAMAGE]
        s = sum(v)
        if s > 0:
            profile = [p + x / s for p, x in zip(profile, v)]
            k += 1
    damage = [round(p / k, 4) for p in profile] if k else []
    shield = sum(1 for n in members if SHIELD_REP & set(n["effects"]))
    armor = sum(1 for n in members if ARMOR_REP & set(n["effects"]))
    # [A] `shield` when more types self-boost shields than repair armour,
    # `armor` when any repair armour, else empty. Falsifier: a faction the
    # client's info shows with the other tank.
    tank = "shield" if shield > armor else ("armor" if armor else None)
    # [A] `Sansha's` tags read `Sansha <metal> Tag`: the possessive is dropped.
    heads = {base + " ", base.replace("'s", "") + " "} if base else set()
    tag_ids = [tid for _p, tid, nm in tags if any(nm.startswith(h) for h in heads)]
    sof = _mode([data["sof"].get(data["types"][n["type_id"]].get("graphic"))
                 for n in members])
    return {"groups": sorted({n["group"] for n in members}), "wrecks": wrecks,
            "damage": damage, "ammo": _ammo_groups(members, data["charges"]),
            "tank": tank, "tags": tag_ids, "base": base, "elite": elite,
            "commander": commander, "stems": stems if any(stems) else [],
            "officers": officers, "sof": sof}


def tier_of(n, prof):
    """C6 R2's prefix rung [A: the order]: officer and commander by class,
    deadspace by context, elite by prefix, else base."""
    if n["cls"].startswith("Officer"):
        return "officer"
    if n["cls"].startswith("Commander"):
        return "commander"
    if n["context"] == "Deadspace":
        return "deadspace"
    if prof.get("elite") and prefix_of(n["name"]) == prof["elite"]:
        return "elite"
    return "base"


def owner_corps(data):
    """R1 `owner_corp_id`: `{key: corp}` from the `Zarzakh <F> Roaming
    Spawner` types' `ownerID`, when that is a corporation [I]."""
    out = {}
    for tid in sorted(data["components"]):
        rs = data["components"][tid].get("reactiveSpawnComponent")
        m = ROAMING_SPAWNER.match(data["types"].get(tid, {}).get("name") or "")
        if rs is None or m is None:
            continue
        owner = rs.get("ownerID")
        if owner in data["corps"]:
            out.setdefault(key_of(m.group(1)), owner)
    return out


def derive(data):
    """`({relpath: [row, ...]}, npcs)`: every row of the six files, and the
    per-type records `check` reads."""
    npcs = npc_types(data)
    alias = aliases(npcs)
    tags = sorted((data["types"][t].get("price") or 0.0, t, data["types"][t].get("name") or "")
                  for t in data.get("tag_types", ()))
    tags = [x for x in tags if x[0] > 0]
    # A faction's R1 profile is over its grammar key's types, whatever their
    # own `factionID` says.
    members = collections.defaultdict(list)
    for n in npcs.values():
        members[n["key"]].append(n)
    keys = faction_keys(data["factions"])
    owners = owner_corps(data)
    profiles = {fid: faction_profile(members[k], data, tags)
                for k, fid in keys.items() if members.get(k)}

    fac_rows = []
    for fid in sorted(data["factions"]):
        f = data["factions"][fid]
        # **A faction with no executor corporation is not written.**
        # `corporation_id` is a required i64 (schema.md), and such a faction
        # cannot own an NPC. One exists in this build: 500021, "Unknown". The
        # first end-to-end boot on the real bundle refused it as
        # `bad integer ""`.
        if f.get("corporationID") is None:
            continue
        p = profiles.get(fid)
        row = (fid, f.get("corporationID"), f.get("militiaCorporationID"),
               f.get("solarSystemID"), list(f.get("memberRaces") or ()))
        if p is None:
            row += (None, [], {}, [], {}, None, [], None, None, None, [], [], None)
        else:
            owner = owners.get(key_of(f.get("name") or ""), f.get("corporationID"))
            row += (owner, p["groups"], p["wrecks"], p["damage"], p["ammo"], p["tank"],
                    p["tags"], p["base"], p["elite"], p["commander"], p["stems"],
                    p["officers"], p["sof"])
        fac_rows.append(row + (f.get("name") or "",))

    type_rows = []
    for tid in sorted(npcs):
        n = npcs[tid]
        prof = profiles.get(keys.get(n["key"]), {})
        type_rows.append((tid, n["group"], n["faction"], n["src"], n["context"], n["cls"],
                          tier_of(n, prof), alias.get(tid), n["needs"], role_of(n["needs"])))

    corp_rows = []
    for cid in sorted(data["corps"]):
        c = data["corps"][cid]
        min_sec = c.get("minSecurity")
        corp_rows.append((cid, c.get("factionID"), c.get("stationID"), c.get("solarSystemID"),
                          c.get("friendID"), c.get("enemyID"), float(c.get("taxRate") or 0.0),
                          None if min_sec is None else float(min_sec),
                          c.get("mainActivityID"), c.get("size") or "", c.get("extent") or "",
                          list(c.get("lpOfferTables") or ()), c.get("tickerName") or "",
                          c.get("name") or ""))

    comp_rows, spawn_rows, bounty_rows = [], [], []
    for tid in sorted(data["components"]):
        comps = data["components"][tid]
        for name in sorted(k for k, v in comps.items() if v is not None):
            comp_rows.append((tid, name))
        rs = comps.get("reactiveSpawnComponent")
        if rs is not None:
            spawn_rows.append((tid, "type", None) + _spawn(rs) + (
                _f(rs.get("bountyPayoutMultiplier")), _f(rs.get("friendlyResponseThreshold"))))
            for scope, field in (("dungeon", "spawnDataByDungeonID"),
                                 ("faction", "spawnDataByFactionID")):
                entries = rs.get(field) or {}
                for key in sorted(entries):
                    spawn_rows.append((tid, scope, key) + _spawn(entries[key]) + (None, None))

    mults = {tid: (data["components"][tid].get("entityBounty") or {}).get(
        "bountyMultipliersByOwner") or {} for tid in data["components"]}
    for tid in sorted(set(npcs_all(data)) | {t for t, m in mults.items() if m}):
        bounty = float(data["dogma"].get(tid, ({}, []))[0].get(A_BOUNTY) or 0.0)
        m = mults.get(tid) or {}
        if bounty > 0 or m:
            bounty_rows.append((tid, bounty, {k: float(m[k]) for k in sorted(m)}))

    return {"npc/factions.tsv": fac_rows, "npc/corporations.tsv": corp_rows,
            "npc/types.tsv": type_rows, "npc/components.tsv": comp_rows,
            "npc/reactive-spawn.tsv": spawn_rows, "npc/bounty.tsv": bounty_rows}, npcs


def npcs_all(data):
    """Every category-11 type id (bounty rows are not limited to parsed groups)."""
    return [t for t, r in data["types"].items()
            if (data["groups"].get(r["group"]) or (None,))[0] == CATEGORY_ENTITY]


def _f(v):
    return None if v is None else float(v)


def _spawn(e):
    return (e.get("spawnTableID"), e.get("initialSpawnNpcFleetTypeID"), e.get("ownerID"),
            e.get("fleetCounterTableID"))


# --- the client reader ---------------------------------------------------------

def _content():
    return sys.modules.get("content") or __import__("content")


#: Components that really sit on every `spacecomponentsbytype` row. **[V]**
#: 3619879 and 3662168: `destinyServerBallClass` on 9,289 of 9,289 types and
#: nothing else; the wrong-presence-bit run had `underConstruction` on all
#: 11,238 of 11,238 and exited 0 (`r-port-diff` E1).
UNIVERSAL_COMPONENTS = frozenset({"destinyServerBallClass"})


def refuse_universal_components(components, known=UNIVERSAL_COMPONENTS):
    """Raise unless no unexpected component is on every row of a big table.

    The schema guard cannot see `fsd.CFSD_PRESENCE_FIXES`, so a stale fix
    decodes cleanly into a wrong table; its signature is a component present
    on every row (a presence bit read as set where the data has none).
    ``components`` is ``{type_id: {component: value}}`` as `gather` builds it.
    Tables of 100 rows or fewer are not judged.
    """
    n = len(components)
    if n <= 100:
        return
    counts = collections.Counter(k for comp in components.values() for k in comp)
    bad = sorted(k for k, c in counts.items() if c == n and k not in known)
    if bad:
        raise _content().ContentError(
            "spacecomponentsbytype: %s on every one of %d rows, which no real "
            "table has: a stale presence-bit fix for this client build "
            "(fsd.CFSD_BUILD_LAYOUTS). If the component really is universal, "
            "add it to content_npc.UNIVERSAL_COMPONENTS"
            % (", ".join(bad), n))


def gather(ctx):
    """The plain dicts `derive` reads, from the client."""
    if "npc" in ctx._cache:  # noqa: SLF001
        return ctx._cache["npc"]  # noqa: SLF001
    content = _content()
    fsd = content._imp("fsd")  # noqa: SLF001
    dogma = content._imp("dogma")  # noqa: SLF001
    c = ctx.client
    groups = {g: (r["categoryID"], ctx.text(r.get("groupNameID")))
              for g, r in fsd.rows(c, "groups").items()}
    components = {t: {k: v for k, v in dict(r).items() if v is not None}
                  for t, r in fsd.rows(c, "spacecomponentsbytype").items()}
    refuse_universal_components(components)
    tag_groups = {g for g, (cat, nm) in groups.items() if nm == CRIMINAL_TAGS}
    types, want = {}, set()
    for t, r in fsd.rows(c, "types").items():
        cat = (groups.get(r["groupID"]) or (None,))[0]
        if cat not in (CATEGORY_ENTITY, CATEGORY_CHARGE) and r["groupID"] not in tag_groups \
                and t not in components:
            continue
        types[t] = {"group": r["groupID"], "faction": r.get("factionID"),
                    "graphic": r.get("graphicID"), "wreck": r.get("wreckTypeID"),
                    "price": r.get("basePrice"), "published": r.get("published"),
                    "meta": r.get("metaGroupID"),
                    "name": ctx.text(r.get("typeNameID")) if cat != CATEGORY_CHARGE else ""}
        if cat in (CATEGORY_ENTITY, CATEGORY_CHARGE):
            want.add(t)
    dog = {}
    for t, row in dogma.Dogma(c).table("typedogma").rows():
        if t in want:
            dog[t] = ({e["attributeID"]: e["value"] for e in row["dogmaAttributes"]},
                      [e["effectID"] for e in row["dogmaEffects"]])
    # [A] T1 charges: published, `metaGroupID` 1 or absent, with a chargeSize
    # and damage. Falsifier: a faction whose vote lands on a T2/faction group.
    charges = {}
    for t, r in types.items():
        if groups[r["group"]][0] != CATEGORY_CHARGE or not r["published"] \
                or r["meta"] not in (None, 1) or t not in dog:
            continue
        a = dog[t][0]
        if a.get(A_CHARGE_SIZE) is not None:
            charges[t] = (r["group"], int(a[A_CHARGE_SIZE]), [a.get(x) or 0.0 for x in A_DAMAGE])
    factions = {f: dict(r, name=ctx.text(r.get("nameID")))
                for f, r in fsd.rows(c, "factions").items()}
    corps = {k: dict(r, name=ctx.text(r.get("nameID")))
             for k, r in fsd.rows(c, "npccorporations").items()}
    sof = {g: r.get("sofFactionName") for g, r in fsd.rows(c, "graphicids").items()}
    data = {"types": types, "groups": groups, "dogma": dog, "charges": charges,
            "factions": factions, "corps": corps, "components": components, "sof": sof,
            "tag_types": [t for t, r in types.items() if r["group"] in tag_groups]}
    ctx._cache["npc"] = data  # noqa: SLF001
    return data


def _sources(*stems, loc=True):
    content = _content()
    return [content.fsdbin(s) for s in stems] + ([content.LOC_EN] if loc else [])


def section(ctx):
    rows, _npcs = derive(gather(ctx))
    t = ("types", "groups", "typedogma")
    return [
        ("npc/factions.tsv", FACTIONS_HEADER, rows["npc/factions.tsv"],
         _sources("factions", "npccorporations", "spacecomponentsbytype", "graphicids",
                  "dogmaeffects", *t)),
        ("npc/corporations.tsv", CORPORATIONS_HEADER, rows["npc/corporations.tsv"],
         _sources("npccorporations")),
        ("npc/types.tsv", TYPES_HEADER, rows["npc/types.tsv"],
         _sources("factions", "npccorporations", "dogmaeffects", *t)),
        ("npc/components.tsv", COMPONENTS_HEADER, rows["npc/components.tsv"],
         _sources("spacecomponentsbytype", loc=False)),
        ("npc/reactive-spawn.tsv", REACTIVE_HEADER, rows["npc/reactive-spawn.tsv"],
         _sources("spacecomponentsbytype", loc=False)),
        ("npc/bounty.tsv", BOUNTY_HEADER, rows["npc/bounty.tsv"],
         _sources("spacecomponentsbytype", "types", "groups", "typedogma", loc=False)),
    ]


# --- the client-backed check (counts live only here, plan §1.6) ---------------

def sof_agreement(data, npcs):
    """`{key: (agree, total, modal sof)}` over each parsed faction's types
    that have a `sofFactionName` (C6 R1's falsifier)."""
    by = collections.defaultdict(list)
    for n in npcs.values():
        s = data["sof"].get(data["types"][n["type_id"]].get("graphic"))
        if s:
            by[n["key"]].append(s)
    return {k: (v.count(_mode(v)), len(v), _mode(v)) for k, v in by.items()}


def disagreements(data, npcs):
    """Types whose `factionID` names a faction whose modal sof differs from
    the type's own `sofFactionName`."""
    modal = {}
    for fid in {n["faction"] for n in npcs.values()}:
        modal[fid] = _mode([data["sof"].get(data["types"][n["type_id"]].get("graphic"))
                            for n in npcs.values() if n["faction"] == fid])
    out = []
    for tid, n in sorted(npcs.items()):
        s = data["sof"].get(data["types"][tid].get("graphic"))
        if n["src"] == "type" and s and modal.get(n["faction"]) and s != modal[n["faction"]]:
            out.append((tid, n["faction"], s, modal[n["faction"]]))
    return out


def check(ctx):
    content = _content()
    data = gather(ctx)
    rows, npcs = derive(data)
    bad = []
    for rel, want in rows.items():
        got = list(content.read_rows(ctx.path(rel)))
        if [r for r in got] != [content.encode_row(w).split("\t") for w in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)" % (rel, len(got), len(want)))
        ctx.note("%s: %d rows" % (rel, len(got)))
    with_exec = sum(1 for f in data["factions"].values() if f.get("corporationID") is not None)
    if len(rows["npc/factions.tsv"]) != with_exec:
        bad.append("npc/factions.tsv: %d rows, factions with an executor has %d"
                   % (len(rows["npc/factions.tsv"]), with_exec))
    if any(r[1] is None or r[1] == "" for r in rows["npc/factions.tsv"]):
        bad.append("npc/factions.tsv: a row has no corporation_id")
    open_ = sorted({c.get("factionID") for c in data["corps"].values()
                    if c.get("factionID") is not None} - set(data["factions"]))
    if open_:
        bad.append("npc/corporations.tsv: factionID not in factions: %s" % open_)
    ctx.note("npc/corporations.tsv: %d rows, every factionID closes into factions: %s"
             % (len(data["corps"]), not open_))
    agree = sof_agreement(data, npcs)
    for k in HUMAN_PIRATES:
        a, n, s = agree.get(k, (0, 0, None))
        ctx.note("npc sof agreement %s: %d/%d on %s (%.1f%%)"
                 % (k, a, n, s, 100.0 * a / n if n else 0.0))
        if not n or a < 0.95 * n:
            bad.append("npc: %s agrees with its sofFactionName on %d/%d types (< 95%%)" % (k, a, n))
    src = collections.Counter(n["src"] for n in npcs.values())
    ctx.note("npc/types.tsv: faction_src type %d, group %d" % (src["type"], src["group"]))
    keys = faction_keys(data["factions"])
    for tid, n in sorted(npcs.items()):
        if n["src"] == "type" and keys.get(n["key"]) != n["faction"]:
            ctx.note("  type %d: factionID %d, its group says %s"
                     % (tid, n["faction"], keys.get(n["key"])))
    ctx.note("npc/bounty.tsv: %d rows with multipliers"
             % sum(1 for r in rows["npc/bounty.tsv"] if r[2]))
    dis = disagreements(data, npcs)
    ctx.note("npc: %d factionID/sofFactionName disagreements" % len(dis))
    for tid, fid, s, m in dis:
        ctx.note("  type %d factionID %d sof %s (faction modal %s)" % (tid, fid, s, m))
    return bad
