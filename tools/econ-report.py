#!/usr/bin/env python3
"""Turn a soak's economy TSV into tables (X-ECONREPORT).

The server appends one block of rows per rollover to the file named by the
environment variable EVE_ECON_TSV (`server/crates/eve-server/src/econ_report.rs`),
and `/econ tsv` prints the same rows.  Columns:
day scope kind key n hours isk isk2 per_h.

    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv [--scope all|<regionID>]

Prints, for the scope (default `all`): net ISK/h by career per day, net ISK/h
by ship tier per day (does income grow as bots fly better ships and train?),
and ISK sources and sinks by class per day.  `career` and `tier` rows: n is
bots, hours their active game hours, isk the wallet delta, isk2 the earnings,
per_h = isk / hours.  Only the standard library.

FE-MEASURE adds the tape (R-FLUIDECON section 4); `--tape` prints it and
`--shock DAY` reads one basket across a price shock:

    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv --tape [--basket armour]
    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv --shock 5 --basket armour

Tape kinds (same nine columns; "x" is price over P_ref times 1000):
  price   key basket:type  n fills, isk window VWAP, isk2 P_ref, per_h index x
  spot    key basket:type  isk best ask, isk2 best bid, per_h spot x
  pbasket key basket       n types, isk history x, isk2 spot x, per_h units
  pindex  key basket       isk trading-index x
  book    key basket       n bid units, isk ask units, per_h pressure x 1000
  stock   key basket       n held by bots, isk on sale, per_h days of volume x 100
  build   key basket       n jobs, hours job-hours, per_h share x 1000
  level   key all          isk price level x, isk2 bot ISK, per_h bots / humans x 1000
  churn   key class        n destroyed, isk minted, per_h destroyed / minted x 100
  audit   key detector     n findings (outlier, pairs, round_trips, pinned, washes)
"""
import argparse
import sys


def read(path):
    f = sys.stdin if path == "-" else open(path)
    rows = []
    with f:
        header = f.readline().rstrip("\n").split("\t")
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == len(header) and parts[0] != header[0]:
                rows.append(dict(zip(header, parts)))
    return rows


def table(title, days, keys, cell):
    print(title)
    if not keys:
        print("  (no rows)\n")
        return
    width = max(10, *(len(k) for k in keys)) + 1
    print("  " + "day".rjust(5) + "".join(k.rjust(width) for k in keys))
    for d in days:
        print("  " + str(d).rjust(5) + "".join(cell(d, k).rjust(width) for k in keys))
    print()


def series(rows, kind, key, col):
    """{day: float(col)} of one kind/key; a day with no row is missing."""
    return {int(r["day"]): float(r[col]) for r in rows if r["kind"] == kind and r["key"] == key}


def shock(rows, basket, day):
    """Pre/post ratios of a basket's history index across `day`.

    Returns a dict: pre (mean of the days before), post (the day after the
    shock .. the last), peak, overshoot (peak / settled - 1), days_to_half
    (days until the series covers half the way from pre to its settled
    level), half_life (days for the overshoot to halve after the peak), or
    None entries when the series is too short."""
    ser = series(rows, "pbasket", basket, "isk")
    ser = {d: v for d, v in ser.items() if v > 0}
    before = [v for d, v in sorted(ser.items()) if d < day]
    after = [(d, v) for d, v in sorted(ser.items()) if d >= day]
    out = {"pre": None, "settled": None, "peak": None, "overshoot": None,
           "days_to_half": None, "half_life": None}
    if not before or not after:
        return out
    pre = sum(before) / len(before)
    settled = sum(v for _, v in after[-3:]) / len(after[-3:])
    out["pre"], out["settled"] = pre, settled
    sign = 1.0 if settled >= pre else -1.0
    peak_day, peak = max(after, key=lambda dv: sign * dv[1])
    out["peak"] = peak
    if settled != pre:
        out["overshoot"] = sign * (peak - settled) / abs(settled - pre)
        half = pre + (settled - pre) / 2.0
        for d, v in after:
            if sign * (v - half) >= 0:
                out["days_to_half"] = d - day
                break
    gap = abs(peak - settled)
    if gap > 0:
        for d, v in after:
            if d > peak_day and abs(v - settled) <= gap / 2.0:
                out["half_life"] = d - peak_day
                break
    return out


def tape(rows, scope, only):
    days = sorted({int(r["day"]) for r in rows})
    baskets = sorted({r["key"] for r in rows if r["kind"] == "pbasket"})
    if only:
        baskets = [b for b in baskets if b == only]
    for kind, col, title, fmt in (
        ("pbasket", "isk", "basket price index, history (1000 = P_ref)", "%.0f"),
        ("pbasket", "isk2", "basket price index, spot", "%.0f"),
        ("pindex", "isk", "basket trading index (mktidx)", "%.0f"),
        ("book", "per_h", "book pressure (bids - asks) / (bids + asks) x 1000", "%.0f"),
        ("stock", "per_h", "bot stock, days of volume x 100", "%.0f"),
        ("build", "per_h", "builder share of job-hours x 1000", "%.0f"),
        ("churn", "per_h", "destroyed / minted x 100", "%.0f"),
    ):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel} if kind in ("churn",) else
                      {k for _, k in sel if not only or k == only})
        table("%s, scope %s" % (title, scope), days, keys,
              lambda d, k, sel=sel, col=col, fmt=fmt: fmt % float(sel[(d, k)][col]) if (d, k) in sel else "-")
    lv = {(int(r["day"])): r for r in rows if r["kind"] == "level"}
    if lv:
        print("price level x, bot ISK, bots/humans x 1000")
        for d in days:
            if d in lv:
                r = lv[d]
                print("  day %s level %s bots %s ratio %s" % (d, r["isk"], r["isk2"], r["per_h"]))
        print()
    au = [r for r in rows if r["kind"] == "audit" and float(r["n"]) > 0]
    print("audit findings" + ("" if au else ": none"))
    for r in au:
        print("  day %s %s %s" % (r["day"], r["key"], r["n"]))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("tsv", help="the TSV, or - for stdin")
    ap.add_argument("--scope", default="all")
    ap.add_argument("--tape", action="store_true", help="print the price tape tables")
    ap.add_argument("--basket", help="only this basket in --tape / --shock")
    ap.add_argument("--shock", type=int, metavar="DAY", help="pre/post ratios across a shock on DAY")
    a = ap.parse_args()
    rows = [r for r in read(a.tsv) if r["scope"] == a.scope]
    days = sorted({int(r["day"]) for r in rows})
    if a.shock is not None:
        baskets = [a.basket] if a.basket else sorted({r["key"] for r in rows if r["kind"] == "pbasket"})
        for b in baskets:
            s = shock(rows, b, a.shock)
            f = lambda x, p="%.0f": "-" if x is None else p % x
            print("basket %s, shock on day %d: pre %s settled %s peak %s overshoot %s days_to_half %s half_life %s" % (
                b, a.shock, f(s["pre"]), f(s["settled"]), f(s["peak"]),
                f(s["overshoot"], "%.2f"), f(s["days_to_half"]), f(s["half_life"])))
        return
    if a.tape:
        tape(rows, a.scope, a.basket)
        return
    for kind, title in (("career", "net ISK/h by career"), ("tier", "net ISK/h by ship tier")):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel})

        def cell(d, k, sel=sel):
            r = sel.get((d, k))
            return "-" if r is None or float(r["hours"]) == 0 else "%.0f (%s)" % (float(r["per_h"]), r["n"])

        table("%s, scope %s (bots in brackets)" % (title, a.scope), days, keys, cell)
    for kind, title in (("isk_src", "ISK sources"), ("isk_sink", "ISK sinks")):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel})
        table("%s by class, scope %s" % (title, a.scope), days, keys,
              lambda d, k, sel=sel: "%.0f" % float(sel[(d, k)]["isk"]) if (d, k) in sel else "-")
    goods = [r for r in rows if r["kind"] == "goods"]
    if goods:
        print("goods minted/destroyed per class, per day")
        for r in goods:
            print("  day %s %-10s minted %s destroyed %s" % (r["day"], r["key"], r["isk"], r["isk2"]))


if __name__ == "__main__":
    main()
