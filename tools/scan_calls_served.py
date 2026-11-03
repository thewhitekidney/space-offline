# -*- coding: utf-8 -*-
"""Extract (service, method) pairs our dispatcher actually answers.

Parses server/crates/eve-server/src/service.rs and service/*.rs: the SERVICES
lists give the routable service names, and the nested `match req.service` /
`match req.method` arms give the methods with a real arm.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import os, re, json, sys

SRC = os.environ.get('SCAN_SRC', '/work/server/crates/eve-server/src')
FILES = [os.path.join(SRC, 'service.rs')] + sorted(
    os.path.join(SRC, 'service', f) for f in os.listdir(os.path.join(SRC, 'service'))
    if f.endswith('.rs'))

def strip_comment(line):
    out = []; q = False; i = 0
    while i < len(line):
        ch = line[i]
        if ch == '"' and (i == 0 or line[i-1] != '\\'): q = not q
        if not q and ch == '/' and i + 1 < len(line) and line[i+1] == '/':
            break
        out.append(ch); i += 1
    return ''.join(out)

def logical_lines(lines):
    """Join a match arm's pattern list back onto one line.

    **rustfmt wraps a long `"a" | "b" | "c" => ...` across lines**, and this
    scanner matches per line with a regex anchored on a quote -- so every arm
    rustfmt had wrapped was invisible, and its methods read `partial` in the
    census while being answered correctly. That is the `GetHomeStationRow`
    error pointing the other way: the column under-claimed instead of
    over-claiming, which is the direction nobody checks. Found by a lane whose
    own five-way arm came back `partial`.

    Two shapes to rejoin, because rustfmt uses both: a line that is only
    patterns and pipes with no `=>` yet, and a continuation line that *starts*
    with `|`. Joining keeps brace counting correct, because the joined text
    carries the same braces as the lines it replaces.
    """
    out = []
    i = 0
    while i < len(lines):
        cur = lines[i]
        while i + 1 < len(lines):
            here = strip_comment(cur)
            nxt = strip_comment(lines[i + 1]).strip()
            only_patterns = re.match(r'\s*"[^"]*"(\s*\|\s*"[^"]*")*\s*\|?\s*$', here)
            if nxt.startswith('|') or (only_patterns and '=>' not in here):
                cur = here.rstrip() + ' ' + lines[i + 1].lstrip()
                i += 1
                continue
            break
        out.append(cur)
        i += 1
    return out


services = set()
pairs = set()
# delegating arms:  "name" => reg.some_fn(req)
delegates = {}

for f in FILES:
    lines = logical_lines(open(f).read().split('\n'))
    # SERVICES lists
    in_list = False
    for ln in lines:
        s = strip_comment(ln)
        if 'SERVICES: &[&str]' in s: in_list = True
        if in_list:
            if '];' in s: in_list = False
            for m in re.findall(r'"([^"]+)"', s): services.add(m)
    # delegating arms
    for ln in lines:
        s = strip_comment(ln)
        m = re.match(r'\s*((?:"[^"]+"\s*\|\s*)*"[^"]+")\s*=>\s*reg\.([a-z_0-9]+)\(req\)', s)
        if m:
            for nm in re.findall(r'"([^"]+)"', m.group(1)):
                delegates[m.group(2)] = delegates.get(m.group(2), []) + [nm]

# `Registry::OBJECT_RETURNING` lists the bind accessors answered by the bind
# path rather than by a method arm. Without it those rows read `partial`.
txt = open(os.path.join(SRC, 'service.rs')).read()
mo = re.search(r'OBJECT_RETURNING[^=]*=\s*&\[(.*?)\n    \];', txt, re.S)
if mo:
    for a, b, c in re.findall(r'\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)', mo.group(1)):
        pairs.add((a, b)); services.add(a); services.add(c)

# now walk each file with brace depth, tracking the service context
for f in FILES:
    lines = logical_lines(open(f).read().split('\n'))
    depth = 0
    ctx = []          # stack of (depth_at_open, [service names] or None)
    fnctx = None      # service names implied by a delegating fn
    for ln in lines:
        s = strip_comment(ln)
        # a `fn x(...)` that a delegating arm names
        mf = re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+([a-z_0-9]+)\s*\(', s)
        if mf and mf.group(1) in delegates:
            fnctx = (depth, delegates[mf.group(1)])
        m = re.match(r'\s*((?:"[^"]+"\s*\|\s*)*"[^"]+")\s*=>\s*match\s+req\.method\s*\{', s)
        if m:
            ctx.append((depth, re.findall(r'"([^"]+)"', m.group(1))))
        else:
            cur = ctx[-1][1] if ctx else (fnctx[1] if fnctx else None)
            if cur:
                mm = re.match(r'\s*((?:"[^"]+"\s*\|\s*)*"[^"]+")\s*(?:\|\s*)?=>', s)
                if mm and 'req.method' not in s and 'req.service' not in s:
                    for meth in re.findall(r'"([^"]+)"', mm.group(1)):
                        for svc in cur:
                            pairs.add((svc, meth))
        depth += s.count('{') - s.count('}')
        while ctx and depth <= ctx[-1][0]: ctx.pop()
        if fnctx and depth <= fnctx[0]: fnctx = None

json.dump({'services': sorted(services),
           'pairs': sorted('%s\t%s' % p for p in pairs)},
          open(os.environ.get('SCAN_SERVED', '/scratch/served.json'), 'w'), indent=0)
sys.stderr.write('services=%d pairs=%d\n' % (len(services), len(pairs)))
