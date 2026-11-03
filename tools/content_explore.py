"""The `explore` content section: what a hacked data or relic can holds.

Packet HK3 (R-HACK §3, §4 row HK3). The client ships **no** loot table for
data and relic cans [V: `resfileindex.txt` has none; `grouprewards` is login
rewards], so this table is ours [A]: built **by class** from the bundle,
never as hand-written type ids. Every header is `docs/content/schema.md`'s.
Writes one bundle file:

    explore/loot.tsv   one row per (kind, class, type): the loot classes of
                       each can kind, and one `can` row per hackable type
                       naming the kind it rolls
    explore/site-loot.tsv
                       CAP-LOOT: what particular sites and containers add
                       to a hacked can (the eleven capital commodities),
                       one row per (dungeon, can type, item)

It reads earlier sections' files from the run's own bundle:
`types/typedata.tsv` (CP1: names, `attrs`), `types/market.tsv` (CP12:
group, category, meta group, tech level, published, `basePrice`),
`industry/blueprints.tsv` and `econ/prices.tsv` (CP24: products, `P_ref`).

**Kinds** (the `can` rows, by the can's own attributes [V ids]):

- `relic`: `requiredSkill1` (182) is Archaeology (13278);
- `ancient`: a relic can whose name names an ancient race
  (`ANCIENT_NAMES`) [A, R-HACK §3];
- `data`: every other hackable (`hackable` 1927 = 1) type.

**Classes** (R-HACK §3's table; groups and categories [V], from
`inventorycommon/const`; weights and quantities [A]):

- `data`: `datacore` (group 333 minus `DATACORE_EXCLUDED` names),
  `decryptor` (group 1304), `data_interface` (group 716), `bpc` (copies of
  T1 module and charge blueprints, not rigs), `filler` (skillbooks priced at
  most `FILLER_MAX_PRICE`);
- `relic`: `salvage` (group 754), `rig_bpc` (copies of T1 rig blueprints),
  `relic_component` (the cheapest `COMPONENT_SHARE` of group 754, in bulk);
- `ancient`: `ancient_salvage` (group 966), `t3_relic` (category 34),
  `t3_datacore` (the datacores whose name is in `T3_DATACORE_NAMES`: the
  four *Subsystems Engineering* cores that every Tech 3 relic needs and no
  other source yields, T3-A / `R-T3.md` §6.1 [A]).

`weight` is the class's [A] weight, the same on every row of the class:
the server picks a class by it, then a type within the class weighted by
`value` and the can's tier (`hacking_loot.rs`). `value` is `P_ref`, else
`basePrice`, else 0. A `copy` row's `qty_min..qty_max` is the copy's
**runs**, and one copy is minted.

The derivation is pure (plain dicts in, rows out), so the unit tests feed it
invented rows; `gather(ctx)` is the only bundle reader.

**Site loot** (CAP-LOOT, `docs/lanes/cap-loot.md`). The client ships no drop
table for the capital commodities either. The sites are found **by the names
of their hackable cans and their faction**, never by id: no client dungeon
id is written in this file. The rates are the `SITE_*_PPM` constants [A].
"""

import os
import sys

SECTION = "explore"
DEPENDS = ("types", "types2", "econ", "npc", "dungeons")

_HERE = os.path.dirname(os.path.abspath(__file__))

# The header line, copied from `schema.md` §2.
LOOT_HEADER = tuple("kind class type_id weight qty_min qty_max copy value".split())

KINDS = ("data", "relic", "ancient")
CAN = "can"

#: Attribute ids [V `dogmaattributes`]: `requiredSkill1`, `hackable`,
#: `rigSize`.
ATTR_REQUIRED_SKILL_1 = 182
ATTR_HACKABLE = 1927
ATTR_RIG_SIZE = 1547
#: `Archaeology` [V]: a relic can's skill.
SKILL_ARCHAEOLOGY = 13278
#: Group and category ids [V `inventorycommon/const.pyc`].
GROUP_DATACORE = 333
GROUP_DECRYPTOR = 1304
GROUP_DATA_INTERFACE = 716
GROUP_SALVAGE = 754
GROUP_ANCIENT_SALVAGE = 966
CATEGORY_MODULE, CATEGORY_CHARGE, CATEGORY_SKILL = 7, 8, 16
CATEGORY_ANCIENT_RELIC = 34
#: `activityManufacturing` [V].
MANUFACTURING = 1
#: `metaGroupID` Tech I [V].
META_TECH_ONE = 1

#: [A] R-HACK §3: the relic cans of these races roll ancient salvage and T3
#: relics. Falsifier: TQ Sleeper/Talocan relic sites that drop plain salvage.
ANCIENT_NAMES = ("Sleeper", "Talocan")
#: [A] R-HACK §3: datacores a data can never holds (faction tech, and the
#: subsystem / Triglavian / Upwell lines, which come from other content).
#: Falsifier: TQ data-site loot lists with one of these.
DATACORE_EXCLUDED = ("Takmahl", "Sleeper", "Talocan", "Yan Jung", "Jove", "Civilian",
                     "Subsystems", "Triglavian", "Upwell")
#: [A] R-T3 §6.1: the datacores an ancient data can holds beyond its salvage
#: and relics, by name. The relics' invention needs one Subsystems Engineering
#: core per family (Defensive, Offensive, Propulsion, Core) and nothing else
#: in the game yields them: `DATACORE_EXCLUDED` keeps them out of ordinary
#: data cans (which stay as they were), no loyalty offer sells them and the
#: price file has no row. Falsifier: retail Sleeper data sites that drop
#: something else, or an NPC seller of these cores.
T3_DATACORE_NAMES = ("Subsystems Engineering",)
#: [A] R-T3 §6.1: the weight of the `t3_datacore` class among the ancient
#: classes (salvage 70, relics 30). An ancient can rolls two to three entries
#: at tier 1, so 20 of 120 is about one entry in six: about one core per
#: three cans. Falsifier: a week of hacking that floods the market with them
#: or never yields one.
T3_DATACORE_WEIGHT = 20
#: [A] the skillbook filler: books priced at most this. Falsifier: data cans
#: whose filler is worth more than their datacores.
FILLER_MAX_PRICE = 1_000_000.0
#: [A] R-HACK §3: the cheapest third of the salvage is the bulk component.
COMPONENT_SHARE = 1.0 / 3.0

#: [A] R-HACK §3, per class: (kind, class, weight, qty_min, qty_max, copy).
#: Falsifier: a week of hacking that floods one class or never yields one.
CLASSES = (
    ("data", "datacore", 40, 1, 3, False),
    ("data", "decryptor", 15, 1, 1, False),
    ("data", "data_interface", 10, 1, 1, False),
    ("data", "bpc", 20, 1, 10, True),
    ("data", "filler", 15, 1, 1, False),
    ("relic", "salvage", 60, 1, 5, False),
    ("relic", "rig_bpc", 25, 1, 10, True),
    ("relic", "relic_component", 15, 2, 8, False),
    ("ancient", "ancient_salvage", 70, 1, 6, False),
    ("ancient", "t3_relic", 30, 1, 1, False),
    ("ancient", "t3_datacore", T3_DATACORE_WEIGHT, 1, 3, False),
)


# --- CAP-LOOT: the capital commodities by site and container ----------------------

SITE_LOOT_HEADER = tuple("dungeon_id can_type_id type_id chance_ppm qty_min qty_max bands".split())
PPM = 1_000_000

#: The pirate factions that own ghost sites and pirate data sites [V names].
PIRATE_FACTIONS = ("Guristas Pirates", "Angel Cartel", "Blood Raider Covenant",
                   "Sansha's Nation", "Serpentis")
#: [A] the empire whose condenser a pirate faction's site yields: the
#: faction's own origin (Blood Raiders and Sansha are Amarr splinters; Angel
#: are Minmatar, Guristas Caldari, Serpentis Gallente). Owner: "the variant
#: follows the site's pirate faction". Falsifier: one TQ site fingerprint
#: (e.g. Sansha sites dropping a Caldari condenser).
PIRATE_EMPIRE = {
    "Guristas Pirates": "Caldari State",
    "Angel Cartel": "Minmatar Republic",
    "Blood Raider Covenant": "Amarr Empire",
    "Sansha's Nation": "Amarr Empire",
    "Serpentis": "Gallente Federation",
}
#: [I] R-CAPLOOT (ESI (CCP)): each condenser's empire, from its militia's LP store
#: (`content_lp.FW_CONDENSER_MILITIA`: Amarr 24th Imperial Crusade, Caldari
#: State Protectorate, Gallente Federal Defense Union, Minmatar Tribal
#: Liberation Force). Falsifier: a retail condenser that fits another hull
#: race than its store.
CONDENSER_EMPIRE = {
    "LM-Composite Molecular Condenser": "Amarr Empire",
    "AG-Composite Molecular Condenser": "Caldari State",
    "CV-Composite Molecular Condenser": "Gallente Federation",
    "AV-Composite Molecular Condenser": "Minmatar Republic",
}
#: The four commodities the Secure Mainframe holds [owner].
MAINFRAME_ITEMS = ("Counter-Subversion Sensor Array", "Meta-Molecular Combiner",
                   "Nanoscale Filter Plate", "Nano Regulation Gate")
SIGNALLER = "Electro-Neural Signaller"
SIGNALLER_ENHANCED = "Enhanced Electro-Neural Signaller"
DEPOSITION_GUIDE = "Isotropic Deposition Guide"
#: The container names that fingerprint a site [V type names].
GHOST_CANS = ("Secure Depot", "Secure Lab", "Secure Databank", "Secure Mainframe",
              "Secure Vault")
MAINFRAME_CAN, VAULT_CAN, DEPOT_CAN = "Secure Mainframe", "Secure Vault", "Secure Depot"
AEGIS_VAULT_CAN = "AEGIS Security Vault"
STORAGE_DEPOT_SUFFIX = "Storage Depot"
SUPERIOR_DEPOT_CAN = "Pristine Storage Depot"

#: [A] per hacked Secure Mainframe of an Improved or Superior ghost site
#: (null and wormhole space), the chance of EACH of the four mainframe
#: commodities. 5 % each: a Superior site has three mainframes, so about
#: 0.6 of the four per site. Falsifier: TQ players reporting a mainframe
#: drop well under or over one in twenty hacks.
SITE_MAINFRAME_PPM = 50_000
#: [A] per hacked can of a Lesser or Standard ghost site (high and low
#: sec), the chance of each mainframe commodity: "more rarely still".
#: 0.5 %. Falsifier: as above for low-sec ghost sites.
SITE_GHOST_LOW_PPM = 5_000
#: [A] per hacked can of any ghost site, the chance of the faction's
#: condenser. 3 %. Falsifier: condensers far commoner or rarer than the
#: mainframe commodities on the market.
SITE_GHOST_CONDENSER_PPM = 30_000
#: [A] per hacked data can of a pirate data site, the chance of the
#: faction's condenser; high and low sec only (`DATA_CONDENSER_BANDS`).
#: 0.5 %: data sites have many cans. Falsifier: condenser supply from data
#: sites dwarfing the ghost sites'.
SITE_DATA_CONDENSER_PPM = 5_000
DATA_CONDENSER_BANDS = "hl"
#: [A] the AEGIS Security Vault: one 57450-line item always [owner], the
#: enhanced one with this chance (10 %: "rarely"). Falsifier: enhanced
#: signallers that capital builders never lack, or never see.
SITE_AEGIS_SIGNALLER_PPM = PPM
SITE_AEGIS_ENHANCED_PPM = 100_000
#: [A] a Storage Depot of a Sleeper Cache that is not the Superior one
#: (3 %, "rarely"), and the Superior cache's Pristine Storage Depot
#: (always [owner]; a Superior site holds several, so a site yields that
#: many). Falsifier: deposition guides flooding, or too few for a titan.
SITE_SLEEPER_DEPOT_PPM = 30_000
SITE_SLEEPER_SUPERIOR_PPM = PPM


def _first_id(typedata):
    """`{name: lowest type id}`."""
    out = {}
    for t in sorted(typedata):
        out.setdefault(typedata[t].get("name", ""), t)
    return out


def derive_site_loot(dungeons, dungeon_cans, typedata, factions):
    """The rows of `explore/site-loot.tsv`.

    `dungeons` is `{id: {"faction_id", "difficulty"}}`, `dungeon_cans`
    `{id: [hackable can type ids placed in it]}`, `factions` `{id: name}`.
    A dungeon whose faction is not a pirate one never gets a row except an
    AEGIS or Sleeper Cache (found by their can names alone)."""
    ids = _first_id(typedata)
    name = lambda t: typedata.get(t, {}).get("name", "")  # noqa: E731
    variant = {}
    for pirate, empire in PIRATE_EMPIRE.items():
        for v, e in CONDENSER_EMPIRE.items():
            if e == empire and v in ids:
                variant[pirate] = ids[v]
    rows = set()

    def add(d, can, item, ppm, bands=""):
        if item in ids:
            rows.add((d, can, ids[item], ppm, 1, 1, bands))
    caches = {}
    for d, cans in dungeons_items(dungeons, dungeon_cans):
        names = {name(t) for t in cans}
        pirate = factions.get(dungeons[d]["faction_id"])
        pirate = pirate if pirate in PIRATE_FACTIONS else None
        if AEGIS_VAULT_CAN in names:
            vault = next(t for t in cans if name(t) == AEGIS_VAULT_CAN)
            add(d, vault, SIGNALLER, SITE_AEGIS_SIGNALLER_PPM)
            add(d, vault, SIGNALLER_ENHANCED, SITE_AEGIS_ENHANCED_PPM)
            continue
        if pirate and names & set(GHOST_CANS):
            if MAINFRAME_CAN in names:
                frame = next(t for t in cans if name(t) == MAINFRAME_CAN)
                for item in MAINFRAME_ITEMS:
                    add(d, frame, item, SITE_MAINFRAME_PPM)
            else:
                for item in MAINFRAME_ITEMS:
                    add(d, 0, item, SITE_GHOST_LOW_PPM)
            if pirate in variant:
                rows.add((d, 0, variant[pirate], SITE_GHOST_CONDENSER_PPM, 1, 1, ""))
            continue
        depots = {t for t in cans if name(t).endswith(STORAGE_DEPOT_SUFFIX)}
        if depots:
            caches[d] = depots
            continue
        if (pirate in variant
                and any(can_kind(typedata[t].get("attrs", {}), name(t)) == "data" for t in cans)):
            rows.add((d, 0, variant[pirate], SITE_DATA_CONDENSER_PPM, 1, 1,
                      DATA_CONDENSER_BANDS))
    top = max((dungeons[d]["difficulty"] or 0 for d in caches), default=0)
    lowest = min((dungeons[d]["difficulty"] or 0 for d in caches), default=0)
    for d, depots in caches.items():
        superior = top > lowest and (dungeons[d]["difficulty"] or 0) == top
        for t in depots:
            always = superior and name(t) == SUPERIOR_DEPOT_CAN
            add(d, t, DEPOSITION_GUIDE, SITE_SLEEPER_SUPERIOR_PPM if always
                else SITE_SLEEPER_DEPOT_PPM)
    return sorted(rows)


def dungeons_items(dungeons, dungeon_cans):
    for d in sorted(dungeons):
        yield d, dungeon_cans.get(d, [])


def value_of(t, market, prices):
    """`P_ref`, else `basePrice`, else 0."""
    if t in prices:
        return prices[t]
    return float(market.get(t, {}).get("base_price") or 0.0)


def is_t1(m):
    return m.get("meta_group_id") in (None, META_TECH_ONE) and m.get("tech_level") in (None, 1)


def can_kind(attrs, name):
    """The kind a hackable can rolls, or `None` for a type that is not one."""
    if attrs.get(ATTR_HACKABLE, 0.0) < 0.5:
        return None
    if int(attrs.get(ATTR_REQUIRED_SKILL_1, 0)) == SKILL_ARCHAEOLOGY:
        return "ancient" if any(n in name for n in ANCIENT_NAMES) else "relic"
    return "data"


def blueprint_classes(market, products, typedata):
    """`(bpc, rig_bpc)`: published T1 blueprints whose published T1 product
    is a module or a charge, split on the product being a rig."""
    bpc, rig = [], []
    for bp, product in sorted(products.items()):
        b, p = market.get(bp), market.get(product)
        if not b or not p or not b["published"] or not p["published"] or not is_t1(p):
            continue
        if p["category_id"] not in (CATEGORY_MODULE, CATEGORY_CHARGE):
            continue
        attrs = typedata.get(product, {}).get("attrs", {})
        (rig if p["category_id"] == CATEGORY_MODULE and ATTR_RIG_SIZE in attrs
         else bpc).append(bp)
    return bpc, rig


def members(market, prices, typedata, products):
    """`{(kind, class): [type_id]}`, each list ascending."""
    pub = {t: m for t, m in market.items() if m["published"]}

    def name(t):
        return typedata.get(t, {}).get("name", "")

    def group(g):
        return sorted(t for t, m in pub.items() if m["group_id"] == g)
    salvage = group(GROUP_SALVAGE)
    by_value = sorted(salvage, key=lambda t: (value_of(t, market, prices), t))
    bpc, rig = blueprint_classes(market, products, typedata)
    return {
        ("data", "datacore"): [t for t in group(GROUP_DATACORE)
                               if not any(x in name(t) for x in DATACORE_EXCLUDED)],
        ("data", "decryptor"): group(GROUP_DECRYPTOR),
        ("data", "data_interface"): group(GROUP_DATA_INTERFACE),
        ("data", "bpc"): bpc,
        ("data", "filler"): sorted(
            t for t, m in pub.items() if m["category_id"] == CATEGORY_SKILL
            and 0.0 < value_of(t, market, prices) <= FILLER_MAX_PRICE),
        ("relic", "salvage"): salvage,
        ("relic", "rig_bpc"): rig,
        ("relic", "relic_component"): sorted(
            by_value[:int(len(by_value) * COMPONENT_SHARE)]),
        ("ancient", "ancient_salvage"): group(GROUP_ANCIENT_SALVAGE),
        ("ancient", "t3_relic"): sorted(t for t, m in pub.items()
                                        if m["category_id"] == CATEGORY_ANCIENT_RELIC),
        ("ancient", "t3_datacore"): [t for t in group(GROUP_DATACORE)
                                     if any(x in name(t) for x in T3_DATACORE_NAMES)],
    }


def derive_loot(market, prices, typedata, products):
    """The rows of `explore/loot.tsv`: every class row, then every `can`
    row, each block in ascending `(kind, class, type_id)`."""
    got = members(market, prices, typedata, products)
    rows = []
    for kind, cls, weight, lo, hi, copy in CLASSES:
        for t in got[(kind, cls)]:
            rows.append((kind, cls, t, weight, lo, hi, copy,
                         float(value_of(t, market, prices))))
    cans = []
    for t, row in typedata.items():
        k = can_kind(row.get("attrs", {}), row.get("name", ""))
        if k is not None:
            cans.append((k, CAN, t, 0, 0, 0, False, 0.0))
    rows.sort(key=lambda r: (KINDS.index(r[0]), r[1], r[2]))
    cans.sort(key=lambda r: (KINDS.index(r[0]), r[2]))
    return rows + cans


# --- the bundle readers ---------------------------------------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _opt(v):
    return int(v) if v else None


def read_market(path):
    return {int(f[0]): {"group_id": int(f[1]), "category_id": _opt(f[2]),
                        "base_price": float(f[4]) if f[4] else 0.0,
                        "meta_group_id": _opt(f[5]), "tech_level": _opt(f[7]),
                        "published": f[13] == "1"}
            for f in _content().read_rows(path)}


def read_prices(path):
    return {int(f[0]): float(f[1]) for f in _content().read_rows(path)}


def read_products(path):
    """`{bp_type_id: product}`: the first manufacturing product."""
    out = {}
    for f in _content().read_rows(path):
        if int(f[1]) == MANUFACTURING and f[2] == "product":
            out.setdefault(int(f[0]), int(f[3]))
    return out


def read_typedata(path):
    """`{type_id: {"name", "attrs"}}` from the `type` lines (`schema.md` §3)."""
    c = _content()
    out = {}
    for f in c.read_rows(path):
        if f[0] == "type" and len(f) >= 14:
            out[int(f[1])] = {"name": f[13], "attrs": c.decode_map(f[12])}
    return out


def read_dungeons(path):
    return {int(f[0]): {"faction_id": _opt(f[2]), "difficulty": _opt(f[3])}
            for f in _content().read_rows(path)}


def read_dungeon_cans(path, hackable):
    """`{dungeon_id: sorted distinct hackable type ids}` from `objects.tsv`."""
    out = {}
    for f in _content().read_rows(path):
        t = int(f[3])
        if t in hackable:
            out.setdefault(int(f[0]), set()).add(t)
    return {d: sorted(v) for d, v in out.items()}


def read_faction_names(path):
    return {int(f[0]): f[18] for f in _content().read_rows(path)}


def gather_sites(ctx):
    typedata = gather(ctx)[2]
    hackable = {t for t, r in typedata.items()
                if r["attrs"].get(ATTR_HACKABLE, 0.0) >= 0.5}
    return (read_dungeons(ctx.path("dungeons/dungeons.tsv")),
            read_dungeon_cans(ctx.path("dungeons/objects.tsv"), hackable),
            typedata, read_faction_names(ctx.path("npc/factions.tsv")))


def gather(ctx):
    if "explore" in ctx._cache:  # noqa: SLF001
        return ctx._cache["explore"]  # noqa: SLF001
    got = (read_market(ctx.path("types/market.tsv")),
           read_prices(ctx.path("econ/prices.tsv")),
           read_typedata(ctx.path("types/typedata.tsv")),
           read_products(ctx.path("industry/blueprints.tsv")))
    ctx._cache["explore"] = got  # noqa: SLF001
    return got


def counts(rows):
    by = {}
    for r in rows:
        key = "%s/%s" % (r[0], r[1])
        by[key] = by.get(key, 0) + 1
    return by


def section(ctx):
    rows = derive_loot(*gather(ctx))
    ctx.note("explore/loot.tsv: %s" % ", ".join("%s %d" % kv for kv in sorted(counts(rows).items())))
    c = _content()
    site = derive_site_loot(*gather_sites(ctx))
    ctx.note("explore/site-loot.tsv: %d rows over %d dungeons"
             % (len(site), len({r[0] for r in site})))
    return [("explore/loot.tsv", LOOT_HEADER, rows,
             [c.fsdbin("types"), c.fsdbin("groups"), c.fsdbin("typedogma")]),
            ("explore/site-loot.tsv", SITE_LOOT_HEADER, site,
             [c.fsdbin("types"), c.fsdbin("dungeons")])]


# --- the bundle-backed check (counts live only here, plan §1.6) -------------------

def check(ctx):
    content = _content()
    market, prices, typedata, products = gather(ctx)
    rows = derive_loot(market, prices, typedata, products)
    bad = []
    got = list(content.read_rows(ctx.path("explore/loot.tsv")))
    if got != [content.encode_row(r).split("\t") for r in rows]:
        bad.append("explore/loot.tsv: differs from a fresh derive (%d vs %d rows)"
                   % (len(got), len(rows)))
    by = counts(rows)
    for kind, cls, *_ in CLASSES:
        if not by.get("%s/%s" % (kind, cls)):
            bad.append("explore/loot.tsv: class %s/%s is empty" % (kind, cls))
    for k in KINDS:
        if not by.get("%s/%s" % (k, CAN)):
            bad.append("explore/loot.tsv: no %s can" % k)
    loose = sorted({r[2] for r in rows if r[1] != CAN and r[2] not in market})
    if loose:
        bad.append("explore/loot.tsv: %d types not in types/market.tsv (first %d)"
                   % (len(loose), loose[0]))
    untyped = sorted({r[2] for r in rows if r[1] != CAN and r[2] not in typedata})
    ctx.note("explore/loot.tsv: %s; %d loot types have no typedata row (not mintable)"
             % (", ".join("%s %d" % kv for kv in sorted(by.items())), len(untyped)))
    site = derive_site_loot(*gather_sites(ctx))
    got = list(content.read_rows(ctx.path("explore/site-loot.tsv")))
    if got != [content.encode_row(r).split("\t") for r in site]:
        bad.append("explore/site-loot.tsv: differs from a fresh derive (%d vs %d rows)"
                   % (len(got), len(site)))
    ids = _first_id(typedata)
    for n in (MAINFRAME_ITEMS + (SIGNALLER, SIGNALLER_ENHANCED, DEPOSITION_GUIDE)
              + tuple(CONDENSER_EMPIRE)):
        if n not in ids:
            bad.append("explore/site-loot.tsv: no type named %r" % n)
        elif not any(r[2] == ids[n] for r in site):
            bad.append("explore/site-loot.tsv: nothing drops %r" % n)
    ctx.note("explore/site-loot.tsv: %d rows over %d dungeons" % (len(site), len({r[0] for r in site})))
    return bad
