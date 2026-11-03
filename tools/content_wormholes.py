"""The `wormholes` content section: the J-space tables (Y-WH, issue #155 WH-0).

Two derived files, both read by `wormhole_sites` in the server:

    wormholes/sleepers.tsv   the Sleeper NPC types, derived from the bundle's
                             `types/typedata.tsv` and `types/market.tsv` (the
                             logic of the old `tools/wh_sleepers.py`, which
                             is now a thin wrapper over `derive` here).
    wormholes/sites.tsv      which dungeons exist in J-space, by class, derived
                             from the client's own dungeon rows and names.

The site derivation reads `dungeons/dungeons.tsv` (archetype, difficulty,
faction) and each dungeon's en-us name, both the client's. A site is a faction
500021 (Sleeper) dungeon of archetype 24 (combat), 44 (relic), 45 (data),
30 (gas) or 27 (ore) with difficulty 3, 4 or 5 (Perimeter, Frontier, Core).
The family comes from the client's name (`Reservoir` gas, `Deposit` ore,
`Forgotten` relic, `Unsecured` data); the class range from the name stem and
the difficulty. The combat anomalies carry no stem, so their class is a short
per-NAME table, `content/wormholes/combat-sites.tsv` (authored, `[A]`).

The derivation is pure (plain rows in, lines out), so the unit tests feed it
invented rows; `gather` and `section` are the only bundle readers.
"""
import os
import sys

SECTION = "wormholes"
DEPENDS = ("types", "types2", "dungeons")

_HERE = os.path.dirname(os.path.abspath(__file__))

#: Authored: the combat anomalies' class by name (the client has none).
COMBAT_FILE = os.path.join(os.path.dirname(_HERE), "content", "wormholes", "combat-sites.tsv")
SOURCES = ["res:/staticdata/dungeons.fsdbinary",
           "res:/localizationfsd/localization_fsd_en-us.pickle"]

SLEEPER_FACTION = 500021
#: Dungeon archetype -> the site family it can be (the client's archetypes).
FAMILY_ARCHETYPES = {24: "combat", 44: "relic", 45: "data", 30: "gas", 27: "ore"}
#: Name stem -> (typical difficulty, first class, last class) for the wormhole
#: classes. The client's difficulty mostly agrees with the stem (Perimeter 3,
#: Frontier 4, Core 5) but not always (some deposits and data sites differ), so
#: the stem decides the class and the difficulty only has to be 3, 4 or 5.
STEMS = {"Perimeter": (3, 1, 2), "Frontier": (4, 3, 4), "Core": (5, 5, 6)}
#: Gas and ore deposits reach further than their tier's own pair of classes.
WIDE = {"gas": {"Perimeter": (1, 6), "Frontier": (3, 6), "Core": (5, 6)},
        "ore": {"Perimeter": (1, 6), "Frontier": (3, 6), "Core": (5, 6)}}
SITES_HEADER = "dungeon_id\tclass_lo\tclass_hi\tfamily\twhy"
SITES_COMMENTS = (
    "# Wormhole sites: which of the client's dungeons exist in J-space, and in",
    "# which classes. Read by the server (`wormhole_sites::TABLE`, Y-WH); header",
    "# frozen in docs/content/schema.md §4. Written by tools/content_wormholes.py;",
    "# do not edit, change the tool or content/wormholes/combat-sites.tsv.",
    "#",
    "# Derived from the client's dungeons.tsv (faction 500021, archetype, difficulty)",
    "# and each dungeon's en-us name. The client has no wormhole class, so the",
    "# class comes from the name stem (Perimeter, Frontier, Core) and difficulty",
    "# 3, 4, 5 [A]; the combat anomalies have no stem and take their class from",
    "# content/wormholes/combat-sites.tsv by name [A].",
    "# - dungeon_id: a `dungeons.tsv` id. A population row of band `wormhole`",
    "#   draws only from the dungeons this file lists for the system's class.",
    "# - class_lo, class_hi: the J-space classes (1-6) whose systems may hold it.",
    "# - family: combat, relic, data, gas or ore (informational; the draw is by",
    "#   the population row's archetype).",
    "# Not listed: the Shattered Debris Field (class 13), Thera, the Drifter",
    "# classes. Falsified by a J-space site a human calls the wrong class.",
)

#: group id -> (hull class, rank). The names are the client's group names
#: ("Deadspace Sleeper Sleepless Sentinel", ...) [V]. Sentinels are the
#: battleships, defenders the cruisers, patrollers the frigates [V-data: the
#: hull hit points of the three sets].
GROUPS = {
    959: ("Battleship", "sleepless"), 960: ("Battleship", "awakened"),
    961: ("Battleship", "emergent"),
    982: ("Cruiser", "sleepless"), 984: ("Cruiser", "awakened"),
    986: ("Cruiser", "emergent"),
    983: ("Frigate", "sleepless"), 985: ("Frigate", "awakened"),
    987: ("Frigate", "emergent"),
}
SLEEPER_HEADER = "type_id\tgroup_id\tclass\trank\tneeds\trole\twreck_type_id\twhy"
SLEEPER_COMMENTS = (
    "# Sleeper NPC types (Y-WH). Written by tools/wh_sleepers.py from the",
    "# player's own typedata; read by `wormhole_sites::sleeper_rows` and",
    "# merged into npc_data (faction 500021, context Deadspace, tier",
    "# deadspace) and the salvage table. Header frozen in docs/content/schema.md §4.",
    "# - class: Frigate, Cruiser or Battleship, from the client's group",
    "#   (patroller, defender, sentinel families) [V].",
    "# - rank: emergent, awakened or sleepless, from the group [V].",
    "# - needs, role: content_npc.needs_of / role_of over the type's own effects.",
    "# - wreck_type_id: types/market.tsv; empty when the type has none.",
)
SITES_FILES = ("wormholes/sites.tsv", "wormholes/sleepers.tsv")


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _npc():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content_npc
    return content_npc


def wrecks_of(market_rows):
    """`{type_id: wreck_type_id}` from `types/market.tsv` data rows (split)."""
    return {int(p[0]): int(p[11]) for p in market_rows if len(p) > 11 and p[11]}


def derive(type_rows, wreck, needs_of, role_of):
    """The sleepers file as lines.

    `type_rows` are `types/typedata.tsv` data rows (split on TAB: `type`,
    id, group, ..., effects at 11, `attr=value;...` at 12), `wreck`
    `{type_id: wreck_type_id}`. A Sleeper with no combat effects (a retainer)
    gets no row."""
    rows = []
    for p in type_rows:
        if len(p) < 13 or p[0] != "type" or int(p[2]) not in GROUPS:
            continue
        tid, group = int(p[1]), int(p[2])
        attrs = {int(k): float(v) for k, v in (x.split("=") for x in p[12].split(";") if x)}
        effects = [int(e) for e in p[11].split(",") if e]
        if not effects:
            continue
        needs = needs_of(attrs, effects)
        cls, rank = GROUPS[group]
        rows.append((tid, group, cls, rank, ",".join(needs), role_of(needs),
                     wreck.get(tid, ""), "[V-data] group %d, effects %s" % (
                         group, ",".join(map(str, effects)))))
    rows.sort()
    return (list(SLEEPER_COMMENTS) + [SLEEPER_HEADER]
            + ["\t".join(map(str, r)) for r in rows])


def parse_combat(lines):
    """`{name: class}` from `combat-sites.tsv` lines (`name`, `class`, `why`)."""
    out, header = {}, None
    for n, line in enumerate(lines, 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        f = line.split("\t")
        if header is None:
            if f[:2] != ["name", "class"]:
                raise ValueError("combat-sites.tsv: header must start `name<TAB>class`")
            header = f
            continue
        if len(f) < 3 or not f[0].strip():
            raise ValueError("combat-sites.tsv line %d: %r" % (n, line))
        try:
            cls = int(f[1])
        except ValueError:
            raise ValueError("combat-sites.tsv line %d: class %r" % (n, f[1]))
        if not 1 <= cls <= 6:
            raise ValueError("combat-sites.tsv line %d: class %d is not 1-6" % (n, cls))
        key = " ".join(f[0].split())
        if key in out:
            raise ValueError("combat-sites.tsv line %d: %r twice" % (n, f[0]))
        out[key] = cls
    return out


def _stem(words):
    for w in words:
        if w in STEMS:
            return w
    return None


def derive_sites(dungeons, text, combat):
    """The sites file as lines.

    `dungeons` are `dungeons/dungeons.tsv` data rows (split: id, archetype,
    faction, difficulty, ..., `name_id` at 7), `text` maps a name id to the
    en-us name, `combat` is `parse_combat`'s table. Returns
    `(lines, problems)`; a combat-table name that matches no combat dungeon is
    a problem, so a renamed site does not vanish silently."""
    rows, seen = [], set()
    for p in dungeons:
        if len(p) < 8 or not p[0].lstrip("-").isdigit():
            continue
        did, arch = int(p[0]), int(p[1] or 0)
        fam = FAMILY_ARCHETYPES.get(arch)
        if fam is None or p[2] != str(SLEEPER_FACTION) or p[3] not in ("3", "4", "5"):
            continue
        name = " ".join(text(int(p[7]) if p[7] else None).split())
        words = name.split()
        if not name or "OLD" in words:
            continue
        stem = _stem(words)
        if words[-1:] == ["Reservoir"] and arch == 30 and stem:
            fam = "gas"
        elif words[-1:] == ["Deposit"] and arch == 27 and stem:
            fam = "ore"
        elif words[:1] == ["Forgotten"] and arch in (44, 24) and stem:
            fam = "relic"
        elif words[:1] == ["Unsecured"] and arch in (45, 24) and stem:
            fam = "data"
        elif arch == 24 and name in combat:
            seen.add(name)
            rows.append((did, combat[name], combat[name], "combat",
                         "[A] %s: class %d by name (combat-sites.tsv)" % (name, combat[name])))
            continue
        else:
            continue
        lo, hi = WIDE.get(fam, {}).get(stem) or STEMS[stem][1:]
        rows.append((did, lo, hi, fam, "[A] %s: %s tier, difficulty %s" % (name, stem, p[3])))
    rows.sort()
    problems = ["combat-sites.tsv: %r matches no combat dungeon" % n
                for n in sorted(set(combat) - seen)]
    return (list(SITES_COMMENTS) + [SITES_HEADER]
            + ["\t".join(map(str, r)) for r in rows]), problems


def _bundle_rows(ctx, rel):
    return list(_content().read_rows(ctx.path(rel)))


def _derive(ctx):
    npc = _npc()
    return derive(_bundle_rows(ctx, "types/typedata.tsv"),
                  wrecks_of(_bundle_rows(ctx, "types/market.tsv")),
                  npc.needs_of, npc.role_of)


def _sites(ctx):
    with open(ctx.input(COMBAT_FILE), encoding="utf-8") as fh:
        combat = parse_combat(fh.read().splitlines())
    return derive_sites(_bundle_rows(ctx, "dungeons/dungeons.tsv"), ctx.text, combat)


def section(ctx):
    sites, problems = _sites(ctx)
    if problems:
        raise ValueError("wormholes: " + "; ".join(problems))
    sleepers = _derive(ctx)
    ctx.note("wormholes: %d site lines, %d sleeper types"
             % (len(sites) - len(SITES_COMMENTS) - 1,
                len(sleepers) - len(SLEEPER_COMMENTS) - 1))
    c = _content()
    return [("wormholes/sites.tsv", None, sites, SOURCES),
            ("wormholes/sleepers.tsv", None, sleepers, [c.fsdbin("types")])]


def check(ctx):
    bad = []
    for rel in ("wormholes/sites.tsv", "wormholes/sleepers.tsv"):
        if not os.path.exists(ctx.path(rel)):
            bad.append("%s: missing from the bundle" % rel)
    if bad:
        return bad
    try:
        sites, problems = _sites(ctx)
    except ValueError as e:
        return [str(e)]
    bad += problems
    for rel, want in (("wormholes/sites.tsv", sites), ("wormholes/sleepers.tsv", _derive(ctx))):
        with open(ctx.path(rel), encoding="utf-8") as fh:
            got = fh.read().splitlines()
        if got != want:
            bad.append("%s: differs from a fresh derive (%d vs %d lines)"
                       % (rel, len(got), len(want)))
    return bad
