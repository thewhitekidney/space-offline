#!/usr/bin/env python3
"""killfits.py: what real pilots fit, counted from killmails.

Reads the daily killmail archives (verbatim ESI killmails from zKillboard /
EVE Ref) and counts the victims' fitted modules per hull, so the bots can
seed their fits from what players actually fly. Standard library only; run
it with ``python3 -I tools/killfits.py``.

Inputs (all CCP game data; they stay under ``out/``, never in the repo):
  --raw DIR       out/killmails/raw: killmails-YYYY-MM-DD.tar.bz2, each
                  member killmails/<id>.json. Streamed (tarfile + bz2), never
                  extracted to disk.
  --typedata F    out/content/types/typedata.tsv, the client build's type
                  universe (``type`` rows: typeID groupID categoryID ...
                  name last). A fit with a hull, module or rig that is not
                  in it is dropped and counted.

Slot flags (inventory flags): lo 11-18, med 19-26, hi 27-34, rig 92-99,
subsystem 125-132, drone bay 87. In a slot flag a category-7 (module) or
category-32 (subsystem) item is the fitted module; a category-8 item is the
charge loaded in it. Cargo and other holds are ignored.

Outputs (--out DIR, default out/bots; tab-separated, ``#`` header line):
  fits.tsv            one row per distinct fit (hull + sorted slot multiset)
                      seen at least --min-count times, plus every hull's top
                      fit: hull_type_id fit_hash count share_of_hull
                      hi med lo rig sub (comma-separated type ids, sorted,
                      repeated per module) charges (weapon:charge pairs, the
                      most common charge per weapon type) drones (the most
                      common drone-bay set, type x qty) hull_name
  fits-summary.tsv    per hull: hull_type_id kills fits_kept distinct_fits
                      top1_share top5_share empty_fits hull_name
  fit-shapes.tsv      the same fits pooled by module GROUP (meta variants of
                      one module count as one): hull_type_id shape_hash count
                      share_of_hull hi/med/lo/rig/sub group lists, the most
                      common concrete fit of the shape (fit_hash, count)
  attacker-ships.tsv  per attacker (ship, weapon) pair: ship_type_id
                      weapon_type_id appearances final_blows damage_done
                      ship_name weapon_name (types not in the bundle are
                      kept, named "?")

No character, corporation or alliance id is kept: types only.
"""

import argparse
import bz2
import collections
import hashlib
import json
import multiprocessing
import os
import sys
import tarfile
import time

LO = range(11, 19)
MED = range(19, 27)
HI = range(27, 35)
RIG = range(92, 100)
SUB = range(125, 133)
DRONE_BAY = 87
CAT_SHIP, CAT_MODULE, CAT_CHARGE, CAT_SUB = 6, 7, 8, 32
GROUP_CAPSULE = 29


def slot_of(flag):
    if flag in HI:
        return 0
    if flag in MED:
        return 1
    if flag in LO:
        return 2
    if flag in RIG:
        return 3
    if flag in SUB:
        return 4
    return None


def load_types(path):
    """typeID -> (groupID, categoryID, name) from typedata.tsv."""
    types = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not line.startswith("type\t"):
                continue
            cols = line.rstrip("\n").split("\t")
            types[int(cols[1])] = (int(cols[2]), int(cols[3]), cols[-1])
    return types


def qty(item):
    return item.get("quantity_destroyed", 0) + item.get("quantity_dropped", 0)


def fit_of(victim, types):
    """(key, charges, drones) or (None, reason). key = (hull, hi, med, lo, rig, sub)."""
    hull = victim.get("ship_type_id")
    t = types.get(hull)
    if t is None:
        return None, "unknown_hull"
    if t[1] != CAT_SHIP or t[0] == GROUP_CAPSULE:
        return None, "not_ship"
    slots = ([], [], [], [], [])
    by_flag = collections.defaultdict(list)
    drones = collections.Counter()
    for it in victim.get("items", ()):
        flag = it.get("flag")
        tid = it.get("item_type_id")
        if flag == DRONE_BAY:
            if tid in types:
                drones[tid] += qty(it)
            continue
        s = slot_of(flag)
        if s is None:
            continue
        tt = types.get(tid)
        if tt is None:
            return None, "unknown_module"
        by_flag[flag].append((tid, tt[1]))
    charges = []
    for flag, items in by_flag.items():
        mods = [tid for tid, cat in items if cat in (CAT_MODULE, CAT_SUB)]
        chs = [tid for tid, cat in items if cat == CAT_CHARGE]
        if not mods:
            continue  # a charge with no module in the slot (should not happen)
        s = slot_of(flag)
        mod = mods[0]
        slots[s].append(mod)
        if chs:
            charges.append((mod, chs[0]))
    key = (hull,) + tuple(tuple(sorted(x)) for x in slots)
    return (key, charges, tuple(sorted(drones.items()))), None


def work(path, types):
    fits = collections.Counter()
    charges = collections.defaultdict(collections.Counter)
    drones = collections.defaultdict(collections.Counter)
    kills = collections.Counter()
    dropped = collections.Counter()
    attackers = collections.Counter()  # (ship, weapon) -> [appear, final, dmg]
    finals = collections.Counter()
    damage = collections.Counter()
    n = 0
    with tarfile.open(path, "r:bz2") as tar:
        for m in tar:
            if not m.isfile() or not m.name.endswith(".json"):
                continue
            km = json.loads(tar.extractfile(m).read())
            n += 1
            for a in km.get("attackers", ()):
                k = (a.get("ship_type_id", 0), a.get("weapon_type_id", 0))
                attackers[k] += 1
                if a.get("final_blow"):
                    finals[k] += 1
                damage[k] += a.get("damage_done", 0)
            v = km.get("victim", {})
            res, why = fit_of(v, types)
            if res is None:
                dropped[why] += 1
                continue
            key, chs, dr = res
            kills[key[0]] += 1
            fits[key] += 1
            for wc in chs:
                charges[key][wc] += 1
            if dr:
                drones[key][dr] += 1
    return dict(n=n, fits=fits, charges=charges, drones=drones, kills=kills,
                dropped=dropped, attackers=attackers, finals=finals, damage=damage)


_TYPES = None


def _init(typedata):
    global _TYPES
    _TYPES = load_types(typedata)


def _work(path):
    return work(path, _TYPES)


def ids(xs):
    return ",".join(str(x) for x in xs)


def fit_hash(key):
    return hashlib.sha1(repr(key).encode()).hexdigest()[:12]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--raw", default="out/killmails/raw")
    ap.add_argument("--typedata", default="out/content/types/typedata.tsv")
    ap.add_argument("--out", default="out/bots")
    ap.add_argument("--min-count", type=int, default=2)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--days", type=int, default=0, help="only the first N archives (0 = all)")
    a = ap.parse_args(argv)

    t0 = time.time()
    paths = sorted(os.path.join(a.raw, f) for f in os.listdir(a.raw)
                   if f.startswith("killmails-") and f.endswith(".tar.bz2"))
    if a.days:
        paths = paths[:a.days]
    types = load_types(a.typedata)

    total = dict(n=0, fits=collections.Counter(),
                 charges=collections.defaultdict(collections.Counter),
                 drones=collections.defaultdict(collections.Counter),
                 kills=collections.Counter(), dropped=collections.Counter(),
                 attackers=collections.Counter(), finals=collections.Counter(),
                 damage=collections.Counter())
    with multiprocessing.Pool(a.jobs, _init, (a.typedata,)) as pool:
        for i, r in enumerate(pool.imap_unordered(_work, paths), 1):
            total["n"] += r["n"]
            for k in ("fits", "kills", "dropped", "attackers", "finals", "damage"):
                total[k].update(r[k])
            for k in ("charges", "drones"):
                for fk, c in r[k].items():
                    total[k][fk].update(c)
            print(f"[{i}/{len(paths)}] {total['n']} killmails, {time.time()-t0:.0f}s",
                  file=sys.stderr)

    name = lambda t: types[t][2] if t in types else "?"
    os.makedirs(a.out, exist_ok=True)
    per_hull = collections.defaultdict(list)
    for key, c in total["fits"].items():
        per_hull[key[0]].append((c, key))
    rows = 0
    with open(os.path.join(a.out, "fits.tsv"), "w", encoding="utf-8") as f, \
         open(os.path.join(a.out, "fits-summary.tsv"), "w", encoding="utf-8") as s:
        f.write("#hull_type_id\tfit_hash\tcount\tshare_of_hull\thi\tmed\tlo\trig\tsub"
                "\tcharges\tdrones\thull_name\n")
        s.write("#hull_type_id\tkills\tfits_kept\tdistinct_fits\ttop1_share\ttop5_share"
                "\tempty_fits\thull_name\n")
        for hull in sorted(per_hull, key=lambda h: -total["kills"][h]):
            fl = sorted(per_hull[hull], key=lambda x: (-x[0], x[1]))
            kills = total["kills"][hull]
            empty = sum(c for c, k in fl if not any(k[1:]))
            kept = 0
            for i, (c, key) in enumerate(fl):
                if c < a.min_count and i > 0:
                    break
                kept += 1
                best = {}
                for (w, ch), n in sorted(total["charges"][key].items(),
                                         key=lambda x: (-x[1], x[0])):
                    best.setdefault(w, ch)
                dr = total["drones"][key].most_common(1)
                drs = ",".join(f"{t}x{q}" for t, q in dr[0][0]) if dr else ""
                f.write(f"{hull}\t{fit_hash(key)}\t{c}\t{c/kills:.4f}\t"
                        + "\t".join(ids(x) for x in key[1:])
                        + f"\t{','.join(f'{w}:{ch}' for w, ch in sorted(best.items()))}"
                        + f"\t{drs}\t{name(hull)}\n")
                rows += 1
            top1 = fl[0][0] / kills
            top5 = sum(c for c, _ in fl[:5]) / kills
            s.write(f"{hull}\t{kills}\t{kept}\t{len(fl)}\t{top1:.4f}\t{top5:.4f}"
                    f"\t{empty}\t{name(hull)}\n")
    # Shapes: the fit with every module replaced by its group (a Compact and
    # an Enduring of one module count as one), so meta variants pool.
    shapes = collections.Counter()
    shape_top = {}
    for key, c in total["fits"].items():
        sk = (key[0],) + tuple(tuple(sorted(types[t][0] for t in x)) for x in key[1:])
        shapes[sk] += c
        if c > shape_top.get(sk, (0, None))[0]:
            shape_top[sk] = (c, key)
    srows = 0
    with open(os.path.join(a.out, "fit-shapes.tsv"), "w", encoding="utf-8") as f:
        f.write("#hull_type_id\tshape_hash\tcount\tshare_of_hull\thi_groups\tmed_groups"
                "\tlo_groups\trig_groups\tsub_groups\ttop_fit_hash\ttop_fit_count\thull_name\n")
        for sk, c in sorted(shapes.items(),
                            key=lambda x: (-total["kills"][x[0][0]], x[0][0], -x[1], x[0])):
            if c < a.min_count:
                continue
            tc, tk = shape_top[sk]
            f.write(f"{sk[0]}\t{fit_hash(sk)}\t{c}\t{c/total['kills'][sk[0]]:.4f}\t"
                    + "\t".join(ids(x) for x in sk[1:])
                    + f"\t{fit_hash(tk)}\t{tc}\t{name(sk[0])}\n")
            srows += 1
    print(f"fit-shapes.tsv rows {srows} (distinct shapes {len(shapes)})")
    with open(os.path.join(a.out, "attacker-ships.tsv"), "w", encoding="utf-8") as f:
        f.write("#ship_type_id\tweapon_type_id\tappearances\tfinal_blows\tdamage_done"
                "\tship_name\tweapon_name\n")
        for (sh, w), c in total["attackers"].most_common():
            f.write(f"{sh}\t{w}\t{c}\t{total['finals'][(sh, w)]}"
                    f"\t{total['damage'][(sh, w)]}\t{name(sh)}\t{name(w)}\n")
    fitted = sum(total["kills"].values())
    print(f"killmails {total['n']}  fits counted {fitted}  dropped "
          f"{dict(total['dropped'])}  hulls {len(per_hull)}  distinct fits "
          f"{len(total['fits'])}  fits.tsv rows {rows}  attacker pairs "
          f"{len(total['attackers'])}  {time.time()-t0:.0f}s")


if __name__ == "__main__":
    main()
