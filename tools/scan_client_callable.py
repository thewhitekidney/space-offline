"""Generate findings/client-callable.tsv: every `__exportedcalls__` method of the
client, i.e. what a server may call ON the client (lane findings-gen, issue #18;
the table itself is `docs/re/client-callable.md`).

    tools/analysis.sh python3 -I tools/scan_client_callable.py \\
        [--pyc out/pyc] [--out findings/client-callable.tsv] [--label V24.01/3662168]

Method (the committed table's header says the same):
- every class body (the code object holding `LOAD_LOCALS`) is evaluated with a
  small stack machine for `__exportedcalls__ = {name: metadata, ...}`; the
  declaration line is the client's own `co_lnotab` line of the `BUILD_MAP`;
- a name defined in that class body gets its own `co_firstlineno` and argument
  names; a name defined elsewhere is `inherited`, with `?` and `unknown`;
- `service` is the attribute name `svc.pyc` imports the class under
  (`from <module> import <Class> as <name>`), NOT `__guid__.split('.')[1]`;
  a class `svc.pyc` does not import is `(not-in-svc)`;
- `module` is the tree path of the .pyc plus `.py` (so `packages/` code has no
  `packages/` prefix, as in the committed table);
- `returns` comes from what precedes each `RETURN_VALUE`: `LOAD_CONST None`
  is a None return, anything else a value; `YIELD_VALUE` makes it `generator`.

Read-only: it opens .pyc files and writes one TSV. Rows are sorted by
(service, method) so the file does not depend on directory order.
"""
import argparse
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import pyc27
import findings_lib as L

HEADER = [
    '# Every __exportedcalls__ method in the {label} client: what a server may call ON the client.',
    '# Built by walking class-body bytecode (BUILD_MAP/STORE_MAP) in out/pyc; line numbers are co_lnotab,',
    "# i.e. the client's own, not a decompiler's. `service` is the name the server puts in",
    '# destination.service -- the attribute name in the client\'s svc.py registry, NOT __guid__.split(".")[1].',
    '# `metadata` is the declared value: [] or [role, *preargs] or a dict. On the CLIENT the role is NOT',
    '# enforced -- ServiceCallWrapper.__precall__ omits PreCall_RoleCheck (service.py:576-577).',
    '# `returns` is derived from what precedes each RETURN_VALUE: None / value / None-or-value / generator.',
    '# It says whether a reply can carry anything, not what shape it is; see docs/re/client-callable.md §3.',
]
COLUMNS = ['service', 'class', 'guid', 'module', 'decl_line', 'method', 'method_line',
           'args', 'metadata', 'returns']
NOT_IN_SVC = '(not-in-svc)'


def registry(svc_top):
    """{(module dotted path, class name): [registry names]} from svc.pyc."""
    ins = list(pyc27.dis(svc_top))
    reg = {}
    for i, (off, op, arg, rep) in enumerate(ins):
        if op == 'IMPORT_FROM' and i + 1 < len(ins) and ins[i + 1][1] == 'STORE_NAME':
            # the module is the IMPORT_NAME that opened this group
            j = i - 1
            while j >= 0 and ins[j][1] != 'IMPORT_NAME':
                j -= 1
            if j >= 0:
                reg.setdefault((ins[j][3], rep), []).append(ins[i + 1][3])
    return reg


def arg_names(fn):
    """Argument names without `self`; `*name` as `*args`, `**name` as `**kw`."""
    n = fn.argcount
    names = [L.dec(v) for v in fn.varnames[:n]]
    if names and names[0] == 'self':
        names = names[1:]
    k = n
    if fn.flags & 0x04:
        names.append('*args'); k += 1
    if fn.flags & 0x08:
        names.append('**kw')   # the table normalises both star names
    return ', '.join(names)


def returns_of(fn):
    """None / value / None-or-value / generator / unknown for one function."""
    ins = list(pyc27.dis(fn))
    if any(op == 'YIELD_VALUE' for _o, op, _a, _r in ins):
        return 'generator'
    saw_none = saw_val = False
    for i, (off, op, arg, rep) in enumerate(ins):
        if op != 'RETURN_VALUE':
            continue
        prev = ins[i - 1] if i else None
        if prev and prev[1] == 'LOAD_CONST' and fn.consts[prev[2]] is None:
            saw_none = True
        else:
            saw_val = True
    if saw_none and saw_val:
        return 'None-or-value'
    if saw_val:
        return 'value'
    if saw_none:
        return 'None'
    return 'unknown'


def scan(root):
    """Return (rows, stats). Each row is the list of COLUMNS."""
    reg = {}
    mods = []
    nclass = nempty = 0
    for rel, top in L.load_tree(root):
        if rel == 'svc':
            reg = registry(top)
            continue
        mods.append((rel, top))
    rows = []
    for rel, top in mods:
        dotted = rel.replace('/', '.')
        for code, ins in L.class_bodies(top):
            ev = L.eval_assignments(code, ins, ('__exportedcalls__', '__guid__'))
            if '__exportedcalls__' not in ev:
                continue
            sym, _first, store_off = ev['__exportedcalls__']
            nclass += 1
            if sym.kind != 'dict' or not sym.val:
                nempty += 1
                continue
            guid = ''
            if '__guid__' in ev and ev['__guid__'][0].kind == 'const':
                guid = L.dec(ev['__guid__'][0].val)
            decl = L.line_at(code, store_off)
            meths = L.methods_of(code, ins)
            cls = L.dec(code.name)
            names = reg.get((dotted, cls)) or [NOT_IN_SVC]
            module = rel + '.py'
            for service in names:
                for k, v in sym.val:
                    if k.kind != 'const':
                        continue
                    m = L.dec(k.val)
                    if m in meths:
                        fn = meths[m][0]
                        row = [service, cls, guid, module, decl, m, fn.firstlineno,
                               arg_names(fn), L.fmt_sym(v), returns_of(fn)]
                    else:
                        row = [service, cls, guid, module, decl, m, 'inherited',
                               '?', L.fmt_sym(v), 'unknown']
                    rows.append(row)
    rows.sort(key=lambda r: (r[0], r[1], r[5]))
    return rows, {'classes': nclass, 'empty': nempty, 'failed': len(L.load_tree.failed)}


def render(rows, label):
    lines = [h.format(label=label) for h in HEADER]
    lines.append(L.tsv_line(COLUMNS))
    lines.extend(L.tsv_line(r) for r in rows)
    return lines


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--pyc', default=L.DEFAULT_PYC, help='unpacked bytecode tree (default %(default)s)')
    ap.add_argument('--out', default='findings/client-callable.tsv', help="output TSV, '-' for stdout")
    ap.add_argument('--label', default='V24.01/3662168', help='release line/build for the header')
    a = ap.parse_args(argv)
    if not os.path.isdir(a.pyc):
        sys.exit('scan_client_callable: no such tree: %s (run tools/extract_code.py first)' % a.pyc)
    rows, st = scan(a.pyc)
    L.write_tsv(a.out, render(rows, a.label))
    print('client-callable: %d rows, %d classes with __exportedcalls__ (%d empty), %d unparsed'
          % (len(rows), st['classes'], st['empty'], st['failed']), file=sys.stderr)


if __name__ == '__main__':
    main()
