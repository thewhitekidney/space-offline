"""The `lp` content section: the LP store offers, one row per offer.

Packet CP37a (`docs/content/plan.md` §6.2, W14). Spec: C4 §5.2 (the offer
tables and the rule per table class, since the client ships no offer
contents), C5 §5.6 (the pricing identity); the headers are
`docs/content/schema.md`'s. Writes one bundle file:

    agents/lp-offers.tsv   one row per (table, product), with
                           `content/agents/lp-overrides.tsv` applied

It reads three earlier sections' files from the run's own bundle:
`npc/corporations.tsv` and `npc/factions.tsv` (CP11a), `types/market.tsv`
(CP12), `industry/blueprints.tsv` and `econ/prices.tsv` (CP24), plus the type
names from the client's `types` (never written anywhere: they only select).

**Classes** (C4 §5.2, by the member corporations of each table):

- `fw`: every member is a faction's militia corporation. Deferred with FW.
- `universal`: no faction holds `DOMINANT_SHARE` of the members, and there
  are at least `UNIVERSAL_MIN_CORPS`. Contents: the meta-0 implants whose
  name starts with an NPC corporation's (the brand families).
- `special`: mixed, but smaller. Deferred [A].
- `bulk`: one faction, at least `BULK_MIN_CORPS` members, under
  `MILITARY_SHARE` military. Contents: blueprint copies of the T1 ships and
  modules of the faction's one member race.
- `faction`: one faction, otherwise (navy, minor, pirate). Contents: the
  faction-meta items whose name starts with one of the faction's stems; each
  requires one of its T1 parent.

**Prices** (C5 §5.6): `P_ref × qty = lp × ISK_PER_LP + isk + Σ P_ref(req)`,
with `isk` the `ISK_SHARE` of the left side, `lp` to two significant figures
and `isk` absorbing the rounding; `LP_FLOOR` only makes an offer dearer.

The derivation is pure (plain dicts in, rows out), so the unit tests feed it
invented rows; `gather(ctx)` is the only client reader.
"""

import math
import os
import sys

SECTION = "lp"
DEPENDS = ("npc", "types2", "econ")
CFSD = ("types",)

_HERE = os.path.dirname(os.path.abspath(__file__))
OVERRIDES = os.path.join(os.path.dirname(_HERE), "content", "agents", "lp-overrides.tsv")

# The header lines, copied from `schema.md` §2 and §4.
OFFERS_HEADER = tuple("table_id offer_id type_id qty lp_cost isk_cost req_items".split())
OVERRIDES_HEADER = tuple("table_id type_id action lp_cost isk_cost why".split())
ACTIONS = ("add", "remove", "price")

#: [A] C5 §5.6: ISK per LP, the conventional exchange rate. Falsifier: players
#: converting LP at well over this; then cut it.
ISK_PER_LP = 1000
#: [A] C5 §5.6: the ISK part of an offer's value, which makes the store a
#: sink. Falsifier: LP piling up unspent because the ISK part is unaffordable.
ISK_SHARE = 0.4
#: [A] C4 §5.2: no offer costs less LP. Falsifier: cheap items nobody buys.
LP_FLOOR = 100
#: C4 §5.2: `offer_id = table_id × OFFER_STRIDE + n`.
OFFER_STRIDE = 10000
#: [A] a charge offer's `qty` is the smallest power of ten whose value clears
#: `LP_FLOOR`, up to this. Falsifier: TQ batch sizes that differ wildly.
CHARGE_BATCH_MAX = 10000

#: [A] C4 §5.2's classes, by membership. Falsifier: a table the census puts
#: in another class (C4 §5.2 lists every table id by class).
DOMINANT_SHARE = 0.9
UNIVERSAL_MIN_CORPS = 20
BULK_MIN_CORPS = 7
MILITARY_SHARE = 0.5
#: `mainActivityID` military [V C4 §5.2].
ACTIVITY_MILITARY = 5
#: `metaGroupID` Faction [V C5 §2.1].
META_FACTION = 4
#: Category and group ids [V `inventorycommon/const.pyc`].
CATEGORY_SHIP, CATEGORY_MODULE, CATEGORY_CHARGE, CATEGORY_IMPLANT = 6, 7, 8, 20
GROUP_BOOSTER = 303
#: `industry/blueprints.tsv` activity id of manufacturing [V CP24].
MANUFACTURING = 1
DEFERRED = ("fw", "special")

#: [I] CAP-LOOT, R-CAPLOOT (source: ESI (CCP)): each faction warfare
#: militia's store sells one capital condenser, by NAME (no client id is
#: written here): (militia corporation, condenser). Falsifier: a retail
#: store that sells another variant, or none.
FW_CONDENSER_MILITIA = (
    ("24th Imperial Crusade", "LM-Composite Molecular Condenser"),
    ("State Protectorate", "AG-Composite Molecular Condenser"),
    ("Federal Defense Union", "CV-Composite Molecular Condenser"),
    ("Tribal Liberation Force", "AV-Composite Molecular Condenser"),
)
#: [I] the owner's 600 LP plus 0.5 M ISK per condenser, qty 1, nothing
#: required (source: ESI (CCP)). Falsifier: the ESI snapshot at a later date.
FW_CONDENSER_LP = 600
FW_CONDENSER_ISK = 500_000


class LpError(Exception):
    """A refusal: the override file's header, or a table out of offer ids."""


# --- the authored file (`schema.md` §4) ------------------------------------------

def _opt_int(v):
    return int(v) if v != "" else None


def parse_overrides(lines):
    """`{(table_id, type_id): (action, lp_cost, isk_cost)}` and the problems.
    `#` lines anywhere; the first other line is the header (a mismatch is a
    refusal); a bad row is skipped and named."""
    out, problems, seen = {}, [], False
    for no, line in enumerate(lines, 1):
        line = line.rstrip("\n")
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t")
        if not seen:
            if tuple(f) != OVERRIDES_HEADER:
                raise LpError("lp-overrides.tsv:%d: header %r, want %r"
                              % (no, line, "\t".join(OVERRIDES_HEADER)))
            seen = True
            continue
        try:
            if len(f) != len(OVERRIDES_HEADER):
                raise ValueError("want %d fields, got %d" % (len(OVERRIDES_HEADER), len(f)))
            table, typ, action = int(f[0]), int(f[1]), f[2]
            lp, isk = _opt_int(f[3]), _opt_int(f[4])
            if action not in ACTIONS:
                raise ValueError("action %r" % action)
            if action != "remove" and (lp is None or isk is None or lp < 0 or isk < 0):
                raise ValueError("%s wants lp_cost and isk_cost, 0 or more" % action)
            out[(table, typ)] = (action, lp, isk)
        except ValueError as e:
            problems.append("lp-overrides.tsv:%d: skipped (%s)" % (no, e))
    return out, problems


# --- classification ---------------------------------------------------------------

def stem(name):
    """A name's first word, lower-cased, apostrophes dropped, a leading `the`
    skipped: the key a faction's names and its items share."""
    words = (name or "").replace("'", "").replace("’", "").lower().split()
    if words and words[0] == "the":
        words = words[1:]
    return words[0] if words else ""


def tables_of(corps):
    """`{table_id: [corp_id]}` from `{corp_id: corp}`."""
    out = {}
    for cid in sorted(corps):
        for t in corps[cid]["lp_offer_tables"]:
            out.setdefault(t, []).append(cid)
    return out


def classify(members, corps, militia):
    """`(class, faction_id)` of one table from its member corporations."""
    if all(c in militia for c in members):
        return "fw", None
    count = {}
    for c in members:
        f = corps[c]["faction_id"]
        count[f] = count.get(f, 0) + 1
    faction, top = max(sorted(count.items(), key=lambda kv: (kv[0] is None, kv[0] or 0)),
                       key=lambda kv: kv[1])
    if faction is None or top < DOMINANT_SHARE * len(members):
        if len(members) >= UNIVERSAL_MIN_CORPS:
            return "universal", None
        return "special", None
    military = sum(1 for c in members if corps[c]["main_activity_id"] == ACTIVITY_MILITARY)
    if len(members) >= BULK_MIN_CORPS and military < MILITARY_SHARE * len(members):
        return "bulk", faction
    return "faction", faction


# --- contents -----------------------------------------------------------------------

def _sellable(t, market, prices):
    m = market.get(t)
    return bool(m and m["published"] and m["market_group_id"] is not None and t in prices)


def faction_stems(faction_id, factions, corps):
    """The faction's own name, its R1 NPC prefixes and its corporations'
    names, each as a `stem`."""
    f = factions.get(faction_id) or {}
    names = [f.get("name")] + list(f.get("prefixes") or ())
    names += [c["name"] for c in corps.values() if c["faction_id"] == faction_id]
    return {s for s in map(stem, names) if s}


def faction_items(faction_id, market, prices, names, factions, corps):
    stems = faction_stems(faction_id, factions, corps)
    return sorted(t for t, m in market.items()
                  if m["meta_group_id"] == META_FACTION and _sellable(t, market, prices)
                  and stem(names.get(t)) in stems)


def implants(market, prices, names, corps):
    stems = {stem(c["name"]) for c in corps.values()} - {""}
    return sorted(t for t, m in market.items()
                  if m["category_id"] == CATEGORY_IMPLANT and m["group_id"] != GROUP_BOOSTER
                  and m["meta_group_id"] is None and _sellable(t, market, prices)
                  and stem(names.get(t)) in stems)


def bulk_items(faction_id, market, prices, factions, products):
    """Blueprints of the T1 ships and modules of the faction's one race."""
    races = (factions.get(faction_id) or {}).get("member_races") or []
    if len(races) != 1:
        return []
    out = []
    for bp, prod in products.items():
        m = market.get(prod)
        if (m and _sellable(bp, market, prices) and m["race_id"] == races[0]
                and m["category_id"] in (CATEGORY_SHIP, CATEGORY_MODULE)
                and m["tech_level"] == 1 and m["meta_group_id"] in (None, 1)):
            out.append(bp)
    return sorted(out)


def requirements(t, cls, market, prices):
    """C4 §5.2 [A]: a faction item requires one of its T1 parent."""
    parent = market[t]["parent"]
    if cls == "faction" and parent is not None and parent in prices:
        return {parent: 1}
    return {}


# --- prices ---------------------------------------------------------------------

def round_sig2(x):
    if x <= 0:
        return 0
    e = int(math.floor(math.log10(x))) - 1
    return int(round(x / 10 ** e) * 10 ** e) if e >= 0 else int(round(x))


def batch(p_ref, category_id):
    """`qty`: 1, or for a charge the smallest power of ten clearing the floor."""
    if category_id != CATEGORY_CHARGE or p_ref <= 0:
        return 1
    qty = 1
    while (qty < CHARGE_BATCH_MAX
           and p_ref * qty * (1 - ISK_SHARE) < LP_FLOOR * ISK_PER_LP):
        qty *= 10
    return qty


def price(value, req_value):
    """`(lp_cost, isk_cost)` for an offer worth `value` that takes items worth
    `req_value` (C5 §5.6): `lp × ISK_PER_LP + isk + req_value = value` within
    rounding, `isk` about `ISK_SHARE` of `value`; `LP_FLOOR` only adds."""
    lp = max(LP_FLOOR, round_sig2((value * (1 - ISK_SHARE) - req_value) / ISK_PER_LP))
    isk = max(0, int(round(value - lp * ISK_PER_LP - req_value)))
    return lp, isk


# --- offers ---------------------------------------------------------------------

def condenser_offers(corps, classes, tables, condensers):
    """`{(table, type): ("add", lp, isk)}`: each militia's condenser in the
    lowest-numbered `fw` table that only its corporation uses. `condensers`
    is `{type name: type id}`; a militia or a type the bundle lacks is
    skipped."""
    by_name = {c["name"]: cid for cid, c in corps.items()}
    out = {}
    for corp_name, type_name in FW_CONDENSER_MILITIA:
        corp, t = by_name.get(corp_name), condensers.get(type_name)
        own = sorted(tb for tb, members in tables.items()
                     if members == [corp] and classes.get(tb) == "fw")
        if corp is not None and t is not None and own:
            out[(own[0], t)] = ("add", FW_CONDENSER_LP, FW_CONDENSER_ISK)
    return out


def derive_offers(corps, factions, market, prices, names, products, overrides,
                  condensers=None):
    """`(rows, stats)`. `rows` are `OFFERS_HEADER` tuples, by table then
    offer id; `stats` is `{table_id: class}` for the notes and the check.
    `condensers` (`{type name: id}`) adds the militias' condenser offers
    under the authored overrides, which win."""
    militia = {f["militia_corporation_id"] for f in factions.values()} - {None}
    tables = tables_of(corps)
    classes, offers = {}, {}
    cache = {}
    for table, members in sorted(tables.items()):
        cls, faction = classify(members, corps, militia)
        classes[table] = cls
        key = (cls, faction)
        if key not in cache:
            if cls == "universal":
                cache[key] = implants(market, prices, names, corps)
            elif cls == "bulk":
                cache[key] = bulk_items(faction, market, prices, factions, products)
            elif cls == "faction":
                cache[key] = faction_items(faction, market, prices, names, factions, corps)
            else:
                cache[key] = []
        got = offers.setdefault(table, {})
        for t in cache[key]:
            qty = batch(prices[t], market[t]["category_id"])
            req = requirements(t, cls, market, prices)
            lp, isk = price(prices[t] * qty, sum(prices[r] * n for r, n in req.items()))
            got[t] = (qty, lp, isk, req)
    overrides = {**condenser_offers(corps, classes, tables, condensers or {}), **overrides}
    for (table, t), (action, lp, isk) in sorted(overrides.items()):
        got = offers.setdefault(table, {})
        if action == "remove":
            got.pop(t, None)
        elif action == "price" and t in got:
            got[t] = (got[t][0], lp, isk, got[t][3])
        else:
            got[t] = (1, lp, isk, {})
    rows = []
    for table in sorted(offers):
        types = sorted(offers[table])
        if len(types) > OFFER_STRIDE:
            raise LpError("table %d: %d offers, over OFFER_STRIDE" % (table, len(types)))
        for n, t in enumerate(types):
            qty, lp, isk, req = offers[table][t]
            rows.append((table, table * OFFER_STRIDE + n, t, qty, lp, isk,
                         dict(sorted(req.items()))))
    return rows, classes


# --- bundle readers (the files of `npc`, `types2` and `econ`) ------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _opt(v):
    return int(v) if v else None


def read_corps(path):
    c = _content()
    return {int(f[0]): {"faction_id": _opt(f[1]), "main_activity_id": _opt(f[8]),
                        "lp_offer_tables": c.decode_list(f[11]), "name": f[13]}
            for f in c.read_rows(path)}


def read_factions(path):
    c = _content()
    return {int(f[0]): {"militia_corporation_id": _opt(f[2]),
                        "member_races": c.decode_list(f[4]),
                        "prefixes": [p for p in f[12:15] if p], "name": f[18]}
            for f in c.read_rows(path)}


def read_market(path):
    return {int(f[0]): {"group_id": int(f[1]), "category_id": _opt(f[2]),
                        "market_group_id": _opt(f[3]), "meta_group_id": _opt(f[5]),
                        "tech_level": _opt(f[7]), "parent": _opt(f[8]),
                        "race_id": _opt(f[10]), "published": f[13] == "1"}
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


def gather(ctx):
    """Everything `derive_offers` reads, as plain dicts."""
    if "lp" in ctx._cache:  # noqa: SLF001
        return ctx._cache["lp"]  # noqa: SLF001
    content = _content()
    market = read_market(ctx.path("types/market.tsv"))
    wanted = {t for t, m in market.items()
              if m["meta_group_id"] == META_FACTION or m["category_id"] == CATEGORY_IMPLANT}
    fsd = content._imp("fsd")  # noqa: SLF001
    names = {int(t): ctx.text(r.get("typeNameID"))
             for t, r in fsd.rows(ctx.client, "types").items() if int(t) in wanted}
    overrides = {}
    if os.path.isfile(OVERRIDES):
        with open(ctx.input(OVERRIDES), encoding="utf-8") as fh:
            overrides, problems = parse_overrides(fh)
        for p in problems:
            ctx.note(p)
    else:
        ctx.note("lp-overrides.tsv: absent, using no rows")
    want = {n for _c, n in FW_CONDENSER_MILITIA}
    condensers = {}
    for f in content.read_rows(ctx.path("types/typedata.tsv")):
        if f[0] == "type" and len(f) >= 14 and f[13] in want:
            condensers.setdefault(f[13], int(f[1]))
    got = (read_corps(ctx.path("npc/corporations.tsv")),
           read_factions(ctx.path("npc/factions.tsv")), market,
           read_prices(ctx.path("econ/prices.tsv")), names,
           read_products(ctx.path("industry/blueprints.tsv")), overrides, condensers)
    ctx._cache["lp"] = got  # noqa: SLF001
    return got


def section(ctx):
    rows, classes = derive_offers(*gather(ctx))
    by = {}
    for cls in classes.values():
        by[cls] = by.get(cls, 0) + 1
    ctx.note("agents/lp-offers.tsv: tables %s"
             % ", ".join("%s %d" % kv for kv in sorted(by.items())))
    return [("agents/lp-offers.tsv", OFFERS_HEADER, rows,
             [_content().fsdbin("npccorporations"), _content().fsdbin("types")])]


# --- the client-backed check (counts live only here, plan §1.6) --------------------

def check(ctx):
    content = _content()
    corps, _f, market, prices, _n, _p, _o, _c = data = gather(ctx)
    rows, classes = derive_offers(*data)
    bad = []
    got = list(content.read_rows(ctx.path("agents/lp-offers.tsv")))
    if got != [content.encode_row(r).split("\t") for r in rows]:
        bad.append("agents/lp-offers.tsv: differs from a fresh derive (%d vs %d rows)"
                   % (len(got), len(rows)))
    loose = sorted({t for r in rows for t in [r[2]] + list(r[6]) if t not in market})
    if loose:
        bad.append("agents/lp-offers.tsv: %d types not in types/market.tsv (first %d)"
                   % (len(loose), loose[0]))
    per = {}
    for r in rows:
        per.setdefault(classes.get(r[0], "authored"), [0, set()])
        per[classes.get(r[0], "authored")][0] += 1
        per[classes.get(r[0], "authored")][1].add(r[0])
    tables = tables_of(corps)
    empty = sorted(t for t, cls in classes.items()
                   if cls not in DEFERRED and not any(r[0] == t for r in rows))
    floored = sum(1 for r in rows if r[4] == LP_FLOOR)
    worst = max((abs(r[4] * ISK_PER_LP + r[5] + sum(prices.get(k, 0) * v for k, v in r[6].items())
                     - prices.get(r[2], 0) * r[3]) / max(prices.get(r[2], 0) * r[3], 1)
                 for r in rows if r[4] > LP_FLOOR), default=0.0)
    ctx.note("agents/lp-offers.tsv: %d rows over %d tables (%d with corporations); %s"
             % (len(rows), len({r[0] for r in rows}), len(tables),
                "; ".join("%s %d offers in %d tables" % (k, v[0], len(v[1]))
                          for k, v in sorted(per.items()))))
    ctx.note("agents/lp-offers.tsv: deferred tables %s; classified but empty %s; "
             "%d at LP_FLOOR; worst identity residual above the floor %.4f"
             % (sorted(t for t, c in classes.items() if c in DEFERRED), empty, floored, worst))
    return bad
