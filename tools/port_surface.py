#!/usr/bin/env python3
"""Compare the client-surface tables of two builds and say what we serve that moved.

    python3 -I tools/port_surface.py OLD_DIR NEW_DIR --out DIR [--repo /work]

OLD_DIR and NEW_DIR are what `tools/port-surface.sh` wrote for each build: the
tables `tools/scan_*.py` and `gw_dump.py` generate (client-calls.tsv,
bound-objects.json, remote-services.txt, client-services.json,
client-callable.tsv, push-events.tsv, session-keys.tsv, user-errors.tsv,
proto-messages.txt, gw-protos.txt). This file only reads those tables and our
own crates; it never reads the client or a bytecode tree.

Writes DIR/surface.md (one section per step G1-G6 of the port-day checklist,
stage G of r-port-1109 section 1; **served and changed first**) and DIR/surface-diff.tsv (one row per
change, machine-readable). Exit 0 when nothing we serve moved, 3 when
something did. An impact is:

- a call (service.method) we answer that is gone from the client or whose
  argument count changed (G1);
- a push event we send whose handler's event, declaring list, arity or
  parameters changed or whose handler is gone, or whose payload shape changed (G3);
- a gateway message our crates name (by full protobuf name, or by the package
  they build names from) that was removed or changed (G4);
- a remote service or exported client call our crates name that is gone (G5);
- a UserError key our crates name that is gone or changed its `loc_args` (G6);
- a session key that stopped being read at all (G2).

"Our crates" is a text search of `server/crates` for the quoted name (the
dispatcher's served set comes from `served.json`, which port-surface.sh makes
with `tools/scan_calls_served.py`). A text search can over-claim a name that
only appears in a comment, and that is the right way to be wrong here: it asks
for a human look, it never hides a change.

User errors: the resource columns (dialog and label ids, `loc_args`) are only
comparable when both sides were read against an install. A side that was not is
marked `resources=no` in its `meta.txt`, and then only the bytecode columns are
compared.
"""
import argparse
import collections
import json
import os
import re
import sys

LIST_MAX = 40          # rows per list in surface.md; the tsv has all of them

IMPACT_EXIT = 3


# ---------------------------------------------------------------- readers

def read_tsv(path):
    """(header, rows) of a findings TSV; `#` lines are comments. None if the file is absent."""
    if not os.path.isfile(path):
        return None
    with open(path, encoding='utf8') as fh:
        lines = [ln.rstrip('\n') for ln in fh if not ln.startswith('#')]
    if not lines:
        return [], []
    head = lines[0].split('\t')
    return head, [ln.split('\t') for ln in lines[1:] if ln]


def read_text(path):
    if not os.path.isfile(path):
        return None
    with open(path, encoding='utf8') as fh:
        return fh.read()


def read_json(path):
    t = read_text(path)
    return None if t is None else json.loads(t)


def read_meta(side):
    out = {}
    t = read_text(os.path.join(side, 'meta.txt')) or ''
    for ln in t.splitlines():
        k, _, v = ln.partition('=')
        out[k.strip()] = v.strip()
    return out


class Crates:
    """A text search of our server source for quoted names."""

    def __init__(self, repo):
        self.files = {}
        root = os.path.join(repo, 'server', 'crates')
        for dp, dn, fn in os.walk(root):
            dn[:] = sorted(d for d in dn if d not in ('target', '.git'))
            for f in sorted(fn):
                if f.endswith('.rs'):
                    p = os.path.join(dp, f)
                    try:
                        with open(p, encoding='utf8', errors='replace') as fh:
                            self.files[os.path.relpath(p, repo)] = fh.read()
                    except OSError:
                        pass
        self.blob = '\n'.join(self.files.values())
        self._cache = {}

    def quoted(self, name):
        return ('"%s"' % name) in self.blob

    def where(self, name, quoted=True):
        """Up to three crate files that name it (for the report)."""
        if len(name) < 3:
            return []
        needle = '"%s"' % name if quoted else name
        if needle not in self.blob:
            return []
        if needle not in self._cache:
            self._cache[needle] = [p for p, t in self.files.items() if needle in t]
        return self._cache[needle]

    def package_prefixes(self):
        """String literals like "eve_public.plex.vault." that code builds a message name from."""
        if 'pp' not in self._cache:
            rx = re.compile(r'"(eve_public(?:\.[A-Za-z0-9_]+)+\.)"')
            self._cache['pp'] = sorted(set(m for m in rx.findall(self.blob) if m.count('.') >= 3))
        return self._cache['pp']


class Report:
    """Collects rows; renders them. A row is one change in one section."""

    def __init__(self):
        self.rows = []          # (section, change, key, detail, served, impact)
        self.stats = collections.OrderedDict()
        self.notes = collections.defaultdict(list)

    def add(self, section, change, key, detail='', served='', impact=False):
        self.rows.append((section, change, key, detail, served, bool(impact)))

    def stat(self, section, old, new):
        self.stats[section] = (old, new)

    def note(self, section, text):
        self.notes[section].append(text)

    def impacts(self):
        return [r for r in self.rows if r[5]]


# ---------------------------------------------------------------- G1 client calls

NARGS = re.compile(r'(\d+) args?')


def calls_by_pair(table):
    """{(service, method): {'sites': n, 'nargs': set, 'kind': set, 'consumer': set, 'served': str}}"""
    if table is None:
        return None
    head, rows = table
    ix = {c: i for i, c in enumerate(head)}
    out = {}
    for r in rows:
        if len(r) < len(head):
            continue
        d = out.setdefault((r[ix['service']], r[ix['method']]),
                           {'sites': 0, 'nargs': set(), 'kind': set(), 'consumer': set(), 'served': ''})
        d['sites'] += 1
        m = NARGS.search(r[ix['notes']])
        if m:
            d['nargs'].add(int(m.group(1)))
        d['kind'].add(r[ix['kind']])
        d['consumer'].add(r[ix['consumer']])
        d['served'] = r[ix['served']] or d['served']
    return out


def served_state(served, svc, meth):
    """yes / partial / no / unknown from served.json, or '' when we have no served set."""
    if served is None:
        return ''
    if svc in ('?', 'unknown') or meth in ('?', 'unknown'):
        return 'unknown'
    if svc not in served['base']:
        return 'no'
    return 'yes' if meth in served['by_base'].get(svc, ()) else 'partial'


def load_served(path):
    d = read_json(path)
    if d is None:
        return None
    pairs = set(tuple(p.split('\t')) for p in d.get('pairs', []))
    by_base = collections.defaultdict(set)
    for s, m in pairs:
        by_base[s.split('::')[0]].add(m)
    return {'services': set(d.get('services', [])), 'base': set(s.split('::')[0] for s in d.get('services', [])),
            'by_base': by_base}


def compare_calls(rep, old, new, served):
    a = calls_by_pair(read_tsv(os.path.join(old, 'client-calls.tsv')))
    b = calls_by_pair(read_tsv(os.path.join(new, 'client-calls.tsv')))
    if a is None or b is None:
        rep.note('G1', 'client-calls.tsv missing on a side: not compared')
        return
    rep.stat('G1 client calls (sites)', sum(d['sites'] for d in a.values()), sum(d['sites'] for d in b.values()))
    for k in sorted(set(a) | set(b)):
        svc, meth = k
        name = '%s.%s' % k
        sv = served_state(served, svc, meth) or (a.get(k) or b.get(k))['served']
        hit = sv in ('yes', 'partial')
        if k not in b:
            rep.add('G1', 'removed', name, 'was %d site(s)' % a[k]['sites'], sv, hit)
        elif k not in a:
            rep.add('G1', 'added', name, '%d site(s), consumer %s' % (b[k]['sites'], '/'.join(sorted(b[k]['consumer']))), sv)
        else:
            x, y = a[k], b[k]
            if x['nargs'] != y['nargs']:
                rep.add('G1', 'arity', name, 'args %s -> %s' % (sorted(x['nargs']), sorted(y['nargs'])), sv, hit)
            elif x['consumer'] != y['consumer'] or x['kind'] != y['kind']:
                rep.add('G1', 'consumer', name, 'kind %s -> %s, consumer %s -> %s' % (
                    '/'.join(sorted(x['kind'])), '/'.join(sorted(y['kind'])),
                    '/'.join(sorted(x['consumer'])), '/'.join(sorted(y['consumer']))), sv)


# ---------------------------------------------------------------- G2 session keys

READS = re.compile(r'^(\d+) reads? in (\d+) modules?')


def compare_session(rep, old, new):
    a = read_tsv(os.path.join(old, 'session-keys.tsv'))
    b = read_tsv(os.path.join(new, 'session-keys.tsv'))
    if a is None or b is None:
        rep.note('G2', 'session-keys.tsv missing on a side: not compared')
        return

    def reads(t):
        head, rows = t
        ix = {c: i for i, c in enumerate(head)}
        out = {}
        for r in rows:
            if len(r) <= ix['readers']:
                continue
            m = READS.match(r[ix['readers']])
            out[r[ix['key']]] = (int(m.group(1)), int(m.group(2))) if m else None
        return out

    x, y = reads(a), reads(b)
    rep.stat('G2 session keys', len(x), len(y))
    for k in sorted(set(x) | set(y)):
        if k not in y:
            rep.add('G2', 'removed', k, 'key left the table', '', True)
        elif k not in x:
            rep.add('G2', 'added', k, '', '')
        elif x[k] != y[k]:
            ox, ny = x[k], y[k]
            gone = ox and ox[0] > 0 and (ny is None or ny[0] == 0)
            d = '%s -> %s' % (('%d reads in %d modules' % ox) if ox else 'prose', ('%d reads in %d modules' % ny) if ny else 'prose')
            rep.add('G2', 'readers', k, d, '', gone)
    # session attributes read but not in the table: the new ones stand out
    def unlisted(side):
        t = read_text(os.path.join(side, 'session-keys.log')) or ''
        m = re.search(r'not in the table: (.*)', t)
        return set(re.sub(r'\(\d+\)$', '', s.strip()) for s in m.group(1).split(',')) if m and m.group(1).strip() else set()
    for k in sorted(unlisted(new) - unlisted(old)):
        rep.add('G2', 'unlisted', k, 'a most-read session attribute that is not in the table and was not before', '')


# ---------------------------------------------------------------- G3 push events

def push_groups(table):
    if table is None:
        return None
    head, rows = table
    ix = {c: i for i, c in enumerate(head)}
    out = {}
    for r in rows:
        if len(r) < len(head):
            continue
        path = r[ix['handler_file']].rsplit(':', 1)[0]
        g = out.setdefault((r[ix['event']], path, r[ix['service']]), {'exact': [], 'shape': [], 'line': []})
        g['exact'].append((r[ix['declared_in']], r[ix['arity']], r[ix['params']]))
        g['shape'].append(r[ix['payload_shape']])
        g['line'].append(r[ix['handler_file']])
    for g in out.values():
        g['exact'].sort()
        g['shape'].sort()
    return out


def compare_push(rep, old, new, crates):
    a = push_groups(read_tsv(os.path.join(old, 'push-events.tsv')))
    b = push_groups(read_tsv(os.path.join(new, 'push-events.tsv')))
    if a is None or b is None:
        rep.note('G3', 'push-events.tsv missing on a side: not compared')
        return
    rep.stat('G3 push handlers', sum(len(g['exact']) for g in a.values()), sum(len(g['exact']) for g in b.values()))
    for k in sorted(set(a) | set(b)):
        ev, path, svc = k
        name = '%s @ %s' % (ev, path)
        where = crates.where(ev)
        sv = 'yes' if where else 'no'
        hit = bool(where)
        if k not in b:
            rep.add('G3', 'removed', name, 'handler gone (%s)' % svc, sv, hit)
        elif k not in a:
            rep.add('G3', 'added', name, '%s, params %s' % (svc, '; '.join(e[2] for e in b[k]['exact'])), sv)
        else:
            x, y = a[k], b[k]
            if x['exact'] != y['exact']:
                rep.add('G3', 'signature', name, '%s -> %s' % (x['exact'], y['exact']), sv, hit)
            elif x['shape'] != y['shape']:
                rep.add('G3', 'shape', name, '%s -> %s' % (x['shape'], y['shape']), sv, hit)
    # events, not handlers: an event with no handler left anywhere
    ea = set(k[0] for k in a)
    eb = set(k[0] for k in b)
    for ev in sorted(ea - eb):
        if crates.where(ev):
            rep.add('G3', 'event-gone', ev, 'no handler in the new build for an event our crates send', 'yes', True)


# ---------------------------------------------------------------- G4 gateway

def gw_blocks(text):
    """({(file, 'message X' | 'rpc ...'): text}, {file: package}) from gw_dump output.
    A top-level message is the lines from `message X` to the next unindented line."""
    out = collections.OrderedDict()
    pkgs = {}
    cur_file = None
    cur = None
    for ln in text.split('\n'):
        if ln.startswith('FILE '):
            parts = ln.split()
            cur_file = parts[1]
            pkgs[cur_file] = parts[3] if len(parts) > 3 else ''
            cur = None
        elif ln.startswith('message '):
            cur = (cur_file, ln)
            out[cur] = [ln]
        elif ln.startswith('rpc '):
            out[(cur_file, ln)] = [ln]
            cur = None
        elif ln.startswith(' ') and cur:
            out[cur].append(ln)
    return {k: '\n'.join(v) for k, v in out.items()}, pkgs


def gw_served(crates, item, pkg):
    """Does our code name this gateway item? Our crates write a message as its full
    protobuf name (`eve_public.plex.vault.api.BalanceChangedNotice`), or build it from a
    package constant (`"eve_public.character.skill.plan."`; at least three segments). A bare message name is
    not a claim: `Price` and `GetResponse` are everywhere. -> '' | 'name' | 'package'."""
    kind, _, rest = item.partition(' ')
    if kind == 'message':
        full = '%s.%s' % (pkg, rest.strip())
    else:
        m = re.match(r'[^(]*\((\.?)([^)]*)\)', rest)
        full = m.group(2) if m else ''
    if full and full in crates.blob:
        return 'name'
    if pkg and any((pkg + '.').startswith(p) for p in crates.package_prefixes()):
        return 'package'
    return ''


def compare_gateway(rep, old, new, crates):
    ta = read_text(os.path.join(old, 'gw-protos.txt'))
    tb = read_text(os.path.join(new, 'gw-protos.txt'))
    if ta is None or tb is None:
        rep.note('G4', 'gw-protos.txt missing on a side: not compared')
        return
    (a, pa_), (b, pb_) = gw_blocks(ta), gw_blocks(tb)
    pkgs = dict(pa_)
    pkgs.update(pb_)
    rep.stat('G4 gateway items (eve_public)', len(a), len(b))
    for k in sorted(set(a) | set(b), key=lambda k: (k[0] or '', k[1])):
        f, item = k
        how = gw_served(crates, item, pkgs.get(f, ''))
        sv = how or 'no'
        hit = bool(how)
        if k not in b:
            rep.add('G4', 'removed', '%s %s' % (f, item), '', sv, hit)
        elif k not in a:
            rep.add('G4', 'added', '%s %s' % (f, item), '%d line(s)' % (b[k].count('\n') + 1), sv)
        elif a[k] != b[k]:
            la, lb = a[k].split('\n'), b[k].split('\n')
            gone = [x.strip() for x in la if x not in lb][:3]
            came = [x.strip() for x in lb if x not in la][:3]
            rep.add('G4', 'changed', '%s %s' % (f, item), '- %s | + %s' % (' ; '.join(gone), ' ; '.join(came)), sv, hit)
    # the message list is the same data seen from the other side; keep its numbers
    pa = read_text(os.path.join(old, 'proto-messages.txt'))
    pb = read_text(os.path.join(new, 'proto-messages.txt'))
    if pa is not None and pb is not None:
        sa, sb = set(pa.splitlines()), set(pb.splitlines())
        rep.stat('G4 protobuf messages (all packages)', len(sa), len(sb))
        for ln in sorted(sa - sb):
            f, _, m = ln.partition('\t')
            if not f.startswith('eve_public'):
                hit = m in crates.blob
                rep.add('G4', 'msg-removed', m, f, 'name' if hit else 'no', hit)
        for ln in sorted(sb - sa):
            f, _, m = ln.partition('\t')
            if not f.startswith('eve_public'):
                rep.add('G4', 'msg-added', m, f, 'name' if m in crates.blob else 'no')


# ---------------------------------------------------------------- G5 services

REMOTE = re.compile(r'^\s+(\S+)\s+refs=\s*(\d+)\s+methods\((\d+)\)')


def remote_services(text):
    out = {}
    for ln in (text or '').splitlines():
        m = REMOTE.match(ln)
        if m:
            out[m.group(1)] = (int(m.group(2)), int(m.group(3)))
    return out


def compare_services(rep, old, new, crates, served):
    ra = read_text(os.path.join(old, 'remote-services.txt'))
    rb = read_text(os.path.join(new, 'remote-services.txt'))
    if ra is not None and rb is not None:
        x, y = remote_services(ra), remote_services(rb)
        rep.stat('G5 remote services', len(x), len(y))
        for k in sorted(set(x) | set(y)):
            ours = bool(served and k in served['base'])
            if k not in y:
                rep.add('G5', 'remote-removed', k, 'refs=%d' % x[k][0], 'yes' if ours else 'no', ours)
            elif k not in x:
                rep.add('G5', 'remote-added', k, 'refs=%d methods=%d' % y[k], 'yes' if ours else 'no')
            elif x[k][1] != y[k][1]:
                rep.add('G5', 'remote-methods', k, 'method count %d -> %d (refs %d -> %d)' % (x[k][1], y[k][1], x[k][0], y[k][0]),
                        'yes' if ours else 'no')
    else:
        rep.note('G5', 'remote-services.txt missing on a side: not compared')

    ja = read_json(os.path.join(old, 'bound-objects.json'))
    jb = read_json(os.path.join(new, 'bound-objects.json'))
    if ja is not None and jb is not None:
        x, y = ja.get('services', {}), jb.get('services', {})
        rep.stat('G5 bound-object services', len(x), len(y))
        for k in sorted(set(x) | set(y)):
            ours = bool(served and (k in served['base']))
            if k not in y:
                rep.add('G5', 'bound-removed', k, '', 'yes' if ours else 'no', ours)
            elif k not in x:
                rep.add('G5', 'bound-added', k, '%d method(s)' % len(y[k]), 'yes' if ours else 'no')
            else:
                gone = sorted(set(x[k]) - set(y[k]))
                came = sorted(set(y[k]) - set(x[k]))
                ar = sorted(m for m in set(x[k]) & set(y[k]) if x[k][m].get('arities') != y[k][m].get('arities'))
                if gone or came or ar:
                    served_m = served['by_base'].get(k, set()) if served else set()
                    hit = bool([m for m in gone + ar if m in served_m])
                    rep.add('G5', 'bound-methods', k, 'removed %s | added %s | arity changed %s' % (gone, came, ar),
                            'yes' if ours else 'no', hit)
    else:
        rep.note('G5', 'bound-objects.json missing on a side: not compared')

    ca = read_json(os.path.join(old, 'client-services.json'))
    cb = read_json(os.path.join(new, 'client-services.json'))
    if ca is not None and cb is not None:
        rep.stat('G5 client services (class table)', len(ca), len(cb))
        for k in sorted(set(ca) | set(cb)):
            if k not in cb:
                rep.add('G5', 'service-removed', k, ca[k].get('module', ''), '')
            elif k not in ca:
                rep.add('G5', 'service-added', k, cb[k].get('module', ''), '')
            else:
                bits = []
                for f in ('module', 'class', 'guid'):
                    if ca[k].get(f) != cb[k].get(f):
                        bits.append('%s %s -> %s' % (f, ca[k].get(f), cb[k].get(f)))
                gone = sorted(set(ca[k].get('methods', [])) - set(cb[k].get('methods', [])))
                came = sorted(set(cb[k].get('methods', [])) - set(ca[k].get('methods', [])))
                if gone:
                    bits.append('methods removed %s' % gone)
                if came:
                    bits.append('methods added %s' % came)
                if bits:
                    rep.add('G5', 'service-changed', k, '; '.join(bits), '')
    else:
        rep.note('G5', 'client-services.json missing on a side: not compared')

    ta = read_tsv(os.path.join(old, 'client-callable.tsv'))
    tb = read_tsv(os.path.join(new, 'client-callable.tsv'))
    if ta is not None and tb is not None:
        def callable_rows(t):
            head, rows = t
            ix = {c: i for i, c in enumerate(head)}
            return {(r[ix['service']], r[ix['class']], r[ix['method']]): (r[ix['args']], r[ix['returns']])
                    for r in rows if len(r) >= len(head)}
        x, y = callable_rows(ta), callable_rows(tb)
        rep.stat('G5 client-callable (server may call)', len(x), len(y))
        for k in sorted(set(x) | set(y)):
            name = '%s.%s' % (k[0], k[2])
            called = bool(crates.where(k[2]))
            sv = 'yes' if called else 'no'
            if k not in y:
                rep.add('G5', 'callable-removed', name, 'class %s' % k[1], sv, called)
            elif k not in x:
                rep.add('G5', 'callable-added', name, 'class %s' % k[1], sv)
            elif x[k] != y[k]:
                rep.add('G5', 'callable-changed', name, 'args/returns %s -> %s' % (x[k], y[k]), sv, called)
    else:
        rep.note('G5', 'client-callable.tsv missing on a side: not compared')


# ---------------------------------------------------------------- G6 user errors

# columns that come from the install's resources, not from the bytecode
RESOURCE_COLS = ('dialog_id', 'dialog_type', 'presentation', 'suppressable', 'closable',
                 'title_id', 'body_id', 'loc_args')
CODE_COLS = ('raise_sites', 'caught_via', 'args_at_raise')      # not `raised_at`/`caught_*_at`: file:line


def compare_errors(rep, old, new, crates):
    a = read_tsv(os.path.join(old, 'user-errors.tsv'))
    b = read_tsv(os.path.join(new, 'user-errors.tsv'))
    if a is None or b is None:
        rep.note('G6', 'user-errors.tsv missing on a side: not compared')
        return
    res = read_meta(old).get('resources') != 'no' and read_meta(new).get('resources') != 'no'
    if not res:
        rep.note('G6', 'a side was read without an install: only the bytecode columns are compared')
    cols = list(CODE_COLS) + (list(RESOURCE_COLS) if res else [])

    def table(t):
        head, rows = t
        ix = {c: i for i, c in enumerate(head)}
        out = {}
        for r in rows:
            if len(r) < len(head):
                continue
            row = {c: r[ix[c]] for c in head}
            if not res and 'pyc' not in row['source']:
                continue
            out[row['key']] = row
        return out
    x, y = table(a), table(b)
    rep.stat('G6 user errors', len(x), len(y))
    for k in sorted(set(x) | set(y), key=lambda s: (s.lower(), s)):
        used = bool(crates.where(k))
        sv = 'yes' if used else 'no'
        if k not in y:
            rep.add('G6', 'removed', k, x[k]['source'], sv, used)
        elif k not in x:
            rep.add('G6', 'added', k, y[k]['source'], sv)
        else:
            diff = [c for c in cols if x[k][c] != y[k][c]]
            if diff:
                rep.add('G6', 'changed', k, '; '.join('%s %s -> %s' % (c, x[k][c], y[k][c]) for c in diff),
                        sv, used and 'loc_args' in diff)


# ---------------------------------------------------------------- output

SECTIONS = [
    ('G1', 'Client calls (client-calls.tsv)'),
    ('G2', 'Session keys (session-keys.tsv)'),
    ('G3', 'Push events (push-events.tsv)'),
    ('G4', 'Gateway (gw-protos.txt, proto-messages.txt)'),
    ('G5', 'Services (bound-objects.json, remote-services.txt, client-services.json, client-callable.tsv)'),
    ('G6', 'User errors (user-errors.tsv)'),
]


def render(rep, old, new, meta_old, meta_new):
    out = ['# Client surface: %s -> %s' % (meta_old.get('label', old), meta_new.get('label', new)), '']
    imp = rep.impacts()
    out.append('Verdict: %s.' % (('%d change(s) touch what we serve (exit %d)' % (len(imp), IMPACT_EXIT)) if imp
                                 else 'no served impact (exit 0)'))
    out.append('')
    out.append('| table | old | new | changes | touching what we serve |')
    out.append('|---|---|---|---|---|')
    for sec, title in SECTIONS:
        rows = [r for r in rep.rows if r[0] == sec]
        keys = [k for k in rep.stats if k.startswith(sec)]
        o = ', '.join(str(rep.stats[k][0]) for k in keys) or '-'
        n = ', '.join(str(rep.stats[k][1]) for k in keys) or '-'
        out.append('| %s | %s | %s | %d | %d |' % (sec, o, n, len(rows), len([r for r in rows if r[5]])))
    out.append('')
    for sec, title in SECTIONS:
        rows = [r for r in rep.rows if r[0] == sec]
        out.append('## %s %s' % (sec, title))
        out.append('')
        for k in rep.stats:
            if k.startswith(sec):
                out.append('- %s: %s -> %s' % (k, rep.stats[k][0], rep.stats[k][1]))
        for n in rep.notes.get(sec, []):
            out.append('- NOTE: %s' % n)
        if not rows:
            out.append('- no changes')
            out.append('')
            continue
        # served and changed first, then the rest
        rows.sort(key=lambda r: (not r[5], r[4] not in ('yes', 'partial'), r[1], r[2]))
        shown = 0
        for r in rows:
            if shown >= LIST_MAX:
                break
            out.append('- %s%s `%s` %s%s' % ('**SERVED** ' if r[5] else '', r[1], r[2], r[3],
                                             (' [served: %s]' % r[4]) if r[4] else ''))
            shown += 1
        if len(rows) > shown:
            out.append('- ... and %d more (surface-diff.tsv has all of them)' % (len(rows) - shown))
        out.append('')
    out.append('## Not automated')
    out.append('')
    out.append('- G7: `performanceMetricsService` not reached on the pinned build: grep the client log for `no such service`.')
    out.append('- `payload_shape`, `risk` and `notes` of push-events are loose cells (62 of them are judgement); a `shape` row is a prompt to re-read the handler, not a verdict.')
    out.append('- call_site line numbers move with every build; the comparison is on the (service, method) pair, argument counts, kind and consumer.')
    out.append('- Run `tools/port-recheck.py` over `findings/port-recheck.tsv` for the citation and constant checks (step H of the checklist).')
    out.append('')
    return '\n'.join(out)


def run(old, new, outdir, repo):
    rep = Report()
    crates = Crates(repo)
    served = load_served(os.path.join(outdir, 'served.json'))
    compare_calls(rep, old, new, served)
    compare_session(rep, old, new)
    compare_push(rep, old, new, crates)
    compare_gateway(rep, old, new, crates)
    compare_services(rep, old, new, crates, served)
    compare_errors(rep, old, new, crates)
    os.makedirs(outdir, exist_ok=True)
    with open(os.path.join(outdir, 'surface.md'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write(render(rep, old, new, read_meta(old), read_meta(new)))
    with open(os.path.join(outdir, 'surface-diff.tsv'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write('section\tchange\tkey\tdetail\tserved\timpact\n')
        for r in rep.rows:
            fh.write('\t'.join([r[0], r[1], r[2], r[3].replace('\t', ' ').replace('\n', ' '), r[4], 'yes' if r[5] else 'no']) + '\n')
    return rep


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('old')
    ap.add_argument('new')
    ap.add_argument('--out', required=True, help='where surface.md and surface-diff.tsv go (served.json is read from here)')
    ap.add_argument('--repo', default='/work', help='repo root, for the search of server/crates')
    a = ap.parse_args(argv)
    for d in (a.old, a.new):
        if not os.path.isdir(d):
            sys.exit('port_surface: no such directory: %s' % d)
    rep = run(a.old, a.new, a.out, a.repo)
    imp = rep.impacts()
    print('port-surface: %d change(s), %d touching what we serve' % (len(rep.rows), len(imp)), file=sys.stderr)
    return IMPACT_EXIT if imp else 0


if __name__ == '__main__':
    sys.exit(main())
