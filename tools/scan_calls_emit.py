# -*- coding: utf-8 -*-
"""Join the bytecode census with the dispatcher's served set and write the TSV."""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import json, collections, os, sys, re

# Paths from the environment (tools/port-surface.sh runs this once per build);
# the defaults are what tools/census-calls.sh has always used.
SITES = os.environ.get('SCAN_SITES', '/scratch/sites.json')
SERVED = os.environ.get('SCAN_SERVED', '/scratch/served.json')
CALLS_OUT = os.environ.get('SCAN_CALLS_OUT', '/work/findings/client-calls.tsv')
STATS_OUT = os.environ.get('SCAN_STATS_OUT', '/scratch/stats.json')

sites = json.load(open(SITES))
sv = json.load(open(SERVED))
SERVED_SVC = set(sv['services'])
SERVED = set(tuple(p.split('\t')) for p in sv['pairs'])
# our Rust names bound sub-objects `svc::sub`; the census names the service the
# moniker names, so a lookup must accept either spelling.
BY_BASE = collections.defaultdict(set)
for s, m in SERVED:
    BY_BASE[s.split('::')[0]].add(m)
SVC_BASE = set(s.split('::')[0] for s in SERVED_SVC)

def served_of(svc, meth):
    # `unknown` for either half means the census could not name the pair, so no
    # claim about our dispatcher is possible. A dynamic-dispatch row (method
    # `unknown`, from `getattr(svc, name)()`) must not read `partial`: that
    # would say we answer the service but not this method, and we do not know
    # what the method is.
    if svc in ('?', 'unknown') or meth in ('?', 'unknown'): return 'unknown'
    if svc not in SVC_BASE: return 'no'
    return 'yes' if meth in BY_BASE.get(svc, ()) else 'partial'

def clean(t):
    return re.sub(r'\s+', ' ', t.replace('\t', ' ')).strip()

rows = {}
for x in sites:
    svc = x['service'] if x['service'] != '?' else 'unknown'
    cs = '%s:%d' % (x['path'], x['line'])
    notes = []
    if x['indirect']: notes.append('handle via %s' % x['indirect'])
    notes.append('in %s(), %d args' % (x['func'] or '?', x['nargs']))
    notes.extend(x['notes'])
    key = (svc, x['method'], cs, x['kind'], x['consumer'], x['guard'])
    note = clean('; '.join(notes))
    if key in rows: rows[key][1] += 1
    else: rows[key] = [note, 1]

out = []
for (svc, meth, cs, kind, cons, guard), (note, n) in rows.items():
    if n > 1: note = 'x%d identical sites on this line; %s' % (n, note)
    out.append((svc, meth, cs, kind, cons, guard, served_of(svc, meth), note))

def sortkey(r):
    svc, meth, cs = r[0], r[1], r[2]
    p, _, ln = cs.rpartition(':')
    return (svc.lower(), svc, meth.lower(), meth, p, int(ln) if ln.isdigit() else 0)

out.sort(key=sortkey)
HDR = ('service','method','call_site','kind','consumer','guard','served','notes')
with open(CALLS_OUT, 'w') as f:
    f.write('\t'.join(HDR) + '\n')
    for r in out: f.write('\t'.join(r) + '\n')

# ---- numbers for the doc
st = {}
st['call_sites'] = len(out)
st['raw_sites'] = len(sites)
st['pairs'] = len(set((r[0], r[1]) for r in out))
st['services'] = len(set(r[0] for r in out))
st['files'] = len(set(r[2].rpartition(':')[0] for r in out))
for col, i in (('kind',3), ('consumer',4), ('guard',6), ('served',5)):
    pass
st['kind'] = collections.Counter(r[3] for r in out)
st['consumer'] = collections.Counter(r[4] for r in out)
st['guard'] = collections.Counter(r[5] for r in out)
st['served'] = collections.Counter(r[6] for r in out)
st['served_pairs'] = collections.Counter(
    r[6] for r in {(x[0], x[1]): x for x in out}.values())
json.dump({k: (dict(v) if isinstance(v, collections.Counter) else v)
           for k, v in st.items()}, open(STATS_OUT, 'w'), indent=1)
for k, v in st.items():
    if isinstance(v, collections.Counter):
        print(k); [print('   %-40s %d' % (a, b)) for a, b in v.most_common()]
    else: print('%-14s %s' % (k, v))
