"""Generate findings/push-events.tsv: every notify handler the client has, with its
signature and what it does to each argument (lane findings-gen, issue #18; the
table is explained in `docs/reference/push-events.md`, whose section 1 is the
algorithm this file implements).

    tools/analysis.sh python3 -I tools/scan_push.py \
        [--pyc out/pyc] [--out findings/push-events.tsv]
    ... --pyc out/pyc-new --old-pyc out/pyc-old --seed findings/push-events.tsv
                                    # re-anchor the committed table on a new build

One pass per stage over every .pyc, with `pyc27.load_pyc` / `walk` / `dis`:

1. **Index every class body** (the code object holding `LOAD_LOCALS`): its
   `__guid__`, the constant part of `__notifyevents__`, and every method
   (`LOAD_CONST <code>; MAKE_FUNCTION n; STORE_NAME name`) with `firstlineno`,
   `argcount`, `varnames`, flags and its number of defaults.
2. **Index base classes** at each `BUILD_CLASS`: the `BUILD_TUPLE n` of dotted
   names before the class code object. Resolved by name, a class in the same
   file first.
3. **Catch declarations mutated in the class body** (`.add`, `.extend`, `+=`).
4. **Index `RegisterForNotifyEvent(ob, 'OnX')` sites.**
5. **Index local emissions** (`ScatterEvent`, `SendEvent`, `ChainEvent`,
   `NotifySessionChange`, and the `...WithoutTheStars` forms) with a constant
   first argument: reported per row as `local_emitters=N`, never used to filter.
6. **Reconcile.** A method named like a declared event is kept if (a) its class
   declares the event, (b) an ancestor or a descendant does, (c) a
   `RegisterForNotifyEvent` in the same file names it, or (d) the class's
   declaration is computed and unresolvable. Otherwise it is a coincidence and
   is dropped (all 128 `Update` methods, say).
7. **Analyse the payload** of each surviving handler (`payload_shape`, `risk`,
   the evidence in `notes`) with a small abstract stack machine.
8. Sort by `(event, handler_file)`; declared events with no handler anywhere
   get one `(no handler)` row each.

Read-only; the output depends on the tree only, never on directory order.
"""
import argparse
import collections
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import pyc27
import findings_lib as L

COLUMNS = ['event', 'handler_file', 'service', 'declared_in', 'arity', 'params',
           'payload_shape', 'risk', 'notes']


# --------------------------------------------------------------------- stage 1-4

class ClassInfo(object):
    """One class body of the client."""
    __slots__ = ('rel', 'name', 'code', 'ins', 'guid', 'bases', 'methods', 'own', 'computed',
                 'registered', 'filename', 'uid', 'defs')

    def __init__(self):
        self.guid = None
        self.bases = []
        self.methods = {}
        self.own = None          # set of declared event names (constant part), or None
        self.computed = False    # the declaration has a part we could not resolve
        self.registered = set()


def _bases_by_class_code(parent, ins):
    """{id(class body code): (class name, [dotted base names])} for the BUILD_CLASS
    statements directly inside `parent`."""
    out = {}
    for j, (off, op, arg, rep) in enumerate(ins):
        if op != 'BUILD_CLASS' or j < 5:
            continue
        if not (ins[j - 1][1] == 'CALL_FUNCTION' and ins[j - 2][1] == 'MAKE_FUNCTION'
                and ins[j - 3][1] == 'LOAD_CONST' and ins[j - 4][1] == 'BUILD_TUPLE'):
            continue
        body = parent.consts[ins[j - 3][2]]
        if not isinstance(body, pyc27.Code):
            continue
        n = ins[j - 4][2]
        names = []
        k = j - 5
        want = n
        while want > 0 and k >= 0:
            attrs = []
            while k >= 0 and ins[k][1] == 'LOAD_ATTR':
                attrs.append(ins[k][3])
                k -= 1
            if k >= 0 and ins[k][1] in ('LOAD_NAME', 'LOAD_GLOBAL', 'LOAD_FAST', 'LOAD_DEREF'):
                names.append('.'.join([ins[k][3]] + list(reversed(attrs))))
            else:
                names.append(None)
            k -= 1
            want -= 1
        names.reverse()
        out[id(body)] = (L.dec(body.name), names)
    return out


def index_classes(root):
    """Stage 1-2: every class body in the tree. Returns (classes, files) where
    `files` maps rel -> list of ClassInfo."""
    classes = []
    for rel, top in L.load_tree(root):
        codes = pyc27.walk(top)
        bases_of = {}
        insn = {}
        for c in codes:
            insn[id(c)] = list(pyc27.dis(c))
            bases_of.update(_bases_by_class_code(c, insn[id(c)]))
        for c in codes:
            ins = insn[id(c)]
            if not any(op == 'LOAD_LOCALS' for _o, op, _a, _r in ins):
                continue
            ci = ClassInfo()
            ci.rel = rel
            ci.name = L.dec(c.name)
            ci.code = c
            ci.ins = ins
            ci.filename = L.rel_filename(c)
            ci.bases = bases_of.get(id(c), (ci.name, []))[1]
            ci.defs = L.method_defs(c, ins)
            ev = L.eval_assignments(c, ins, ('__guid__',))
            if '__guid__' in ev and ev['__guid__'][0].kind == 'const' and isinstance(ev['__guid__'][0].val, bytes):
                ci.guid = L.dec(ev['__guid__'][0].val)
            ci.methods = L.methods_of(c, ins)
            ci.uid = len(classes)
            classes.append(ci)
    return classes


def declared_events(ci):
    """Stage 3: the constant part of a class body's `__notifyevents__`, with the
    mutations the body applies to it. Returns (set or None, computed).

    `None` means the class never assigns the name. `computed` is True when some
    part of an assignment was neither a constant nor a reference to a base
    class's list, i.e. the declaration cannot be read off the bytecode.
    A reference to `Base.__notifyevents__` contributes nothing: the base class
    declares those events itself, and the descendant direction of the
    reconcile pass finds them from there."""
    code = ci.code
    own = None
    computed = False
    alias = False
    stack = []          # entries: ('c', set, bad) | ('base',) | ('self',) | ('n', name) | ('m', name) | ('x',)
    for off, op, arg, rep in ci.ins:
        if op == 'LOAD_CONST':
            k = code.consts[arg]
            if isinstance(k, bytes):
                stack.append(('s', k.decode('utf8', 'replace')))
            elif isinstance(k, tuple) and all(isinstance(x, bytes) for x in k):
                stack.append(('c', {x.decode('utf8', 'replace') for x in k}, False))
            else:
                stack.append(('x',))
        elif op in ('LOAD_NAME', 'LOAD_GLOBAL'):
            stack.append(('self',) if rep == '__notifyevents__' else ('n', rep))
        elif op == 'LOAD_ATTR':
            top = stack.pop() if stack else ('x',)
            if rep == '__notifyevents__' and top[0] in ('n', 'x', 'dotted'):
                stack.append(('base',))
            elif top[0] == 'self' and rep in ('extend', 'add', 'append', 'update'):
                stack.append(('m', rep))
            elif top[0] in ('n', 'dotted'):
                stack.append(('dotted', (top[1] + '.' + rep) if len(top) > 1 else rep))
            else:
                stack.append(('x',))
        elif op in ('BUILD_LIST', 'BUILD_SET', 'BUILD_TUPLE'):
            if arg > len(stack):
                stack = [('x',)]
                continue
            items = stack[len(stack) - arg:] if arg else []
            del stack[len(stack) - arg:]
            s, bad = set(), False
            for it in items:
                if it[0] == 'c':
                    s |= it[1]
                    bad = bad or it[2]
                elif it[0] == 's':
                    s.add(it[1])
                elif it[0] == 'base':
                    pass
                else:
                    bad = True
            stack.append(('c', s, bad))
        elif op in ('BINARY_ADD', 'INPLACE_ADD', 'BINARY_OR', 'INPLACE_OR'):
            if len(stack) < 2:
                stack = [('x',)]
                continue
            b = stack.pop()
            a = stack.pop()
            s, bad = set(), False
            for it in (a, b):
                if it[0] == 'c':
                    s |= it[1]
                    bad = bad or it[2]
                elif it[0] == 'base':
                    pass
                elif it[0] == 'self':
                    s |= own or set()
                else:
                    bad = True
            stack.append(('c', s, bad))
        elif op.startswith('SLICE+'):
            n = int(op[-1])
            drop = {0: 0, 1: 1, 2: 1, 3: 2}[n]
            if len(stack) < 1 + drop:
                stack = [('x',)]
                continue
            for _ in range(drop):
                stack.pop()
            # the slice of a list keeps its kind (`Base.__notifyevents__[:]`)
        elif op == 'CALL_FUNCTION':
            if arg > len(stack) - 1 or arg > 255:
                stack = [('x',)]
                continue
            args = stack[len(stack) - arg:] if arg else []
            del stack[len(stack) - arg:]
            fn = stack.pop() if stack else ('x',)
            if fn[0] == 'm':
                s, bad = set(), False
                for it in args:
                    if it[0] == 'c':
                        s |= it[1]
                        bad = bad or it[2]
                    elif it[0] == 's':
                        s.add(it[1])
                    elif it[0] != 'base':
                        bad = True
                own = (own or set()) | s
                alias = False
                computed = computed or bad
                stack.append(('x',))
            else:
                stack.append(('x',))
        elif op == 'STORE_NAME':
            if rep == '__notifyevents__':
                v = stack.pop() if stack else ('x',)
                if v[0] == 'c':
                    own = v[1]
                    computed = computed or v[2]
                elif v[0] == 's':
                    # `__notifyevents__ = 'OnX'` (a bare string): the client iterates it,
                    # so what it declares is the single characters. One class does this.
                    own = set(v[1])
                elif v[0] == 'base':
                    # `__notifyevents__ = Base.__notifyevents__` (or a slice of it): the
                    # list is the base's, so nothing can be read here -- unless the body
                    # then mutates it, which resolves it again (see CALL_FUNCTION below).
                    own = set()
                    alias = True
                else:
                    own = set()
                    computed = True
            elif stack:
                stack.pop()
        elif op == 'POP_TOP':
            if stack:
                stack.pop()
        else:
            stack = []
    return own, computed or alias


EMIT_NAMES = ('ScatterEvent', 'SendEvent', 'ChainEvent', 'NotifySessionChange',
              'ScatterEventWithoutTheStars', 'SendEventWithoutTheStars',
              'ChainEventWithoutTheStars', 'NotifySessionChangeWithoutTheStars')


def scan_sites(root):
    """Stage 4-5: ({rel: {event: [register-for-self?]}}, Counter of local emissions)."""
    registered = collections.defaultdict(lambda: collections.defaultdict(list))
    emitters = collections.Counter()
    for rel, top in L.load_tree(root):
        for c in pyc27.walk(top):
            ins = list(pyc27.dis(c))
            for i, (off, op, arg, rep) in enumerate(ins):
                if op != 'LOAD_ATTR':
                    continue
                if rep == 'RegisterForNotifyEvent' and i + 3 < len(ins):
                    ob, ev = ins[i + 1], ins[i + 2]
                    if ev[1] == 'LOAD_CONST' and isinstance(c.consts[ev[2]], bytes) and ins[i + 3][1] == 'CALL_FUNCTION':
                        registered[rel][L.dec(c.consts[ev[2]])].append(ob[1] == 'LOAD_FAST' and ob[3] == 'self')
                elif rep in EMIT_NAMES and i + 1 < len(ins):
                    ev = ins[i + 1]
                    if ev[1] == 'LOAD_CONST' and isinstance(c.consts[ev[2]], bytes):
                        emitters[L.dec(c.consts[ev[2]])] += 1
    return registered, emitters


# --------------------------------------------------------------------- stage 6

def resolve_bases(classes):
    """Link each class to the classes its base names resolve to (by final name
    component, same file preferred). Returns {uid: [parent uids]}."""
    by_name = collections.defaultdict(list)
    for c in classes:
        by_name[c.name].append(c)
    parents = {}
    for c in classes:
        ps = []
        for b in c.bases:
            if not b:
                continue
            cand = by_name.get(b.split('.')[-1], [])
            same = [x for x in cand if x.rel == c.rel and x.uid != c.uid]
            pick = same or [x for x in cand if x.uid != c.uid]
            ps.extend(x.uid for x in pick[-1:])     # POLICY: last in tree order
        parents[c.uid] = ps
    return parents


def ancestors_of(uid, parents):
    """Ancestors in breadth-first order (nearest first), without repeats."""
    seen, order, queue = {uid}, [], list(parents.get(uid, []))
    while queue:
        u = queue.pop(0)
        if u in seen:
            continue
        seen.add(u)
        order.append(u)
        queue.extend(parents.get(u, []))
    return order


def arity_text(fn, ndef):
    """`3`, `2..3` (one default), with `+*args` / `+**kwargs` as present; `self`
    is not counted."""
    n = max(fn.argcount - 1, 0)
    lo = max(n - ndef, 0)
    text = str(n) if lo == n else '%d..%d' % (lo, n)
    if fn.flags & 0x04:
        text += '+*args'
    if fn.flags & 0x08:
        text += '+**kwargs'
    return text


def nearest_descendant(uid, name, children, declarers):
    """The first declaring class found going down from `uid` level by level."""
    seen, level = {uid}, [uid]
    while level:
        nxt = []
        for u in level:
            for ch in children.get(u, ()):
                if ch not in seen:
                    seen.add(ch)
                    nxt.append(ch)
        hit = [u for u in nxt if u in declarers[name]]
        if hit:
            return hit[0]
        level = nxt
    return None


def reconcile(classes, own_by_uid, computed_by_uid, registered, parents):
    """Stage 6. Returns (rows, declarers) where rows is a list of dicts with
    `cls`, `name`, `fn`, `ndef`, `how` and `related`."""
    declarers = collections.defaultdict(set)
    for c in classes:
        for e in own_by_uid.get(c.uid) or ():
            declarers[e].add(c.uid)
    by_uid = {c.uid: c for c in classes}
    children = collections.defaultdict(list)
    for u, ps in parents.items():
        for p in ps:
            children[p].append(u)
    anc = {c.uid: ancestors_of(c.uid, parents) for c in classes}
    desc = collections.defaultdict(set)
    for u, al in anc.items():
        for a in al:
            desc[a].add(u)
    out = []
    for c in classes:
        for name, fn, ndef, _off in c.defs:
            if name not in declarers:
                continue
            related = subclass = None
            if name in (own_by_uid.get(c.uid) or ()):
                how = 'notifyevents'
            elif any(a in declarers[name] for a in anc[c.uid]):
                how = 'notifyevents'
                related = next(a for a in anc[c.uid] if a in declarers[name])
            elif any(d in declarers[name] for d in desc[c.uid]):
                how = 'notifyevents'
                subclass = nearest_descendant(c.uid, name, children, declarers)
            elif name in registered.get(c.rel, {}):
                how = 'other'
            elif computed_by_uid.get(c.uid) or any(computed_by_uid.get(a) for a in anc[c.uid]):
                how = 'scatter_only'
            else:
                continue
            svc = any((by_uid[u].guid or '').startswith('svc.') for u in [c.uid] + anc[c.uid])
            out.append({'cls': c, 'name': name, 'fn': fn, 'ndef': ndef, 'how': how, 'related': related,
                        'subclass': subclass, 'svc': svc})
    return out, declarers


# --------------------------------------------------------------------- stage 7

CMP = ('<', '<=', '==', '!=', '>', '>=', 'in', 'not in', 'is', 'is not', 'exception match', 'BAD')
# Callees that reach native code without a `blue.` prefix: the ballpark handle.
NATIVE_MARKS = ('GetBallpark', '__bp')
NATIVE_ROOTS = ('blue', 'destiny', 'trinity', 'bluepy', '_blue', '_destiny', '_trinity')
# Order in which several uses of one parameter are written, most constraining first.
TAG_ORDER = ['native', 'unpack', 'iterate', 'contains', 'subscript', 'slice', 'attr', 'as-key', 'len',
             'elem.unpack', 'elem[]', 'elem.attr', 'arith', 'format', 'truth', 'pass', 'store']
STRONG = ('native', 'unpack_', 'iterate', 'contains', 'subscript', 'slice', 'attr', 'as-key', 'elem.')
RISK_ORDER = ['lock', 'reentrant-call', 'native-payload', 'native-touch', 'dbrow', 'strict-arity',
              'raises-on-shape', 'raise-stmt', 'do-sync-blocktrap', 'session-change-path']
REMOTE_ATTRS = ('RemoteSvc', 'ProxySvc', 'MachoBindObject', 'MachoResolveObject', 'RemoteSvcBlocking')
LOCK_ATTRS = ('Lock', 'Semaphore', 'CriticalSection', 'RLock', 'LockedService')
JUMP_ABS = ('JUMP_ABSOLUTE', 'POP_JUMP_IF_FALSE', 'POP_JUMP_IF_TRUE', 'JUMP_IF_FALSE_OR_POP',
            'JUMP_IF_TRUE_OR_POP', 'CONTINUE_LOOP')
JUMP_REL = ('JUMP_FORWARD', 'FOR_ITER', 'SETUP_LOOP', 'SETUP_EXCEPT', 'SETUP_FINALLY', 'SETUP_WITH')
# (pops, pushes) for the opcodes the machine does not model itself.
EFFECT = {
    'POP_TOP': (1, 0), 'ROT_TWO': (2, 2), 'ROT_THREE': (3, 3), 'DUP_TOP': (1, 2), 'ROT_FOUR': (4, 4),
    'NOP': (0, 0), 'UNARY_POSITIVE': (1, 1), 'UNARY_NEGATIVE': (1, 1), 'UNARY_NOT': (1, 1),
    'UNARY_CONVERT': (1, 1), 'UNARY_INVERT': (1, 1), 'LOAD_CONST': (0, 1), 'LOAD_NAME': (0, 1),
    'LOAD_GLOBAL': (0, 1), 'LOAD_FAST': (0, 1), 'LOAD_DEREF': (0, 1), 'LOAD_CLOSURE': (0, 1),
    'STORE_FAST': (1, 0), 'STORE_NAME': (1, 0), 'STORE_GLOBAL': (1, 0), 'STORE_DEREF': (1, 0),
    'DELETE_FAST': (0, 0), 'DELETE_NAME': (0, 0), 'DELETE_GLOBAL': (0, 0), 'STORE_ATTR': (2, 0),
    'DELETE_ATTR': (1, 0), 'STORE_SUBSCR': (3, 0), 'DELETE_SUBSCR': (2, 0), 'GET_ITER': (1, 1),
    'PRINT_ITEM': (1, 0), 'PRINT_NEWLINE': (0, 0), 'PRINT_EXPR': (1, 0), 'RETURN_VALUE': (1, 0),
    'YIELD_VALUE': (1, 1), 'POP_BLOCK': (0, 0), 'END_FINALLY': (0, 0), 'WITH_CLEANUP': (0, 0),   # exception plumbing: not modelled, never a desync
    'BUILD_SLICE': (2, 1), 'IMPORT_NAME': (2, 1), 'IMPORT_FROM': (0, 1), 'IMPORT_STAR': (1, 0),
    'LIST_APPEND': (2, 1), 'SET_ADD': (2, 1), 'MAP_ADD': (3, 1), 'STORE_MAP': (2, 0),
    'BUILD_MAP': (0, 1), 'LOAD_LOCALS': (0, 1), 'EXEC_STMT': (3, 0), 'BREAK_LOOP': (0, 0),
    'SETUP_LOOP': (0, 0), 'SETUP_EXCEPT': (0, 0), 'SETUP_FINALLY': (0, 0), 'SETUP_WITH': (1, 2),
    'JUMP_FORWARD': (0, 0), 'JUMP_ABSOLUTE': (0, 0), 'POP_JUMP_IF_FALSE': (1, 0),
    'POP_JUMP_IF_TRUE': (1, 0), 'CONTINUE_LOOP': (0, 0), 'FOR_ITER': (0, 1),
    'SLICE+0': (1, 1), 'SLICE+1': (2, 1), 'SLICE+2': (2, 1), 'SLICE+3': (3, 1),
    'STORE_SLICE+0': (2, 0), 'STORE_SLICE+1': (3, 0), 'STORE_SLICE+2': (3, 0), 'STORE_SLICE+3': (4, 0),
    'DELETE_SLICE+0': (1, 0), 'DELETE_SLICE+1': (2, 0), 'DELETE_SLICE+2': (2, 0), 'DELETE_SLICE+3': (3, 0),
}
BINARY = ('BINARY_POWER', 'BINARY_MULTIPLY', 'BINARY_DIVIDE', 'BINARY_MODULO', 'BINARY_ADD',
          'BINARY_SUBTRACT', 'BINARY_FLOOR_DIVIDE', 'BINARY_TRUE_DIVIDE', 'BINARY_LSHIFT',
          'BINARY_RSHIFT', 'BINARY_AND', 'BINARY_XOR', 'BINARY_OR')


def _val(kind, pi=None, extra=None):
    return (kind, pi, extra)


def analyse_handler(fn, event):
    """Run the payload analysis on one handler. Returns a dict with
    `shapes` (one string per parameter), `evidence` (per parameter text),
    `risk` (list of tags), `desync` (bool)."""
    nparams = max(fn.argcount - 1, 0)
    pnames = [L.dec(v) for v in fn.varnames[1:fn.argcount]]
    pindex = {n: i for i, n in enumerate(pnames)}
    ins = list(pyc27.dis(fn))
    targets = set()
    for off, op, arg, rep in ins:
        if op in JUMP_ABS:
            targets.add(arg)
    nxt = {}
    for k in range(len(ins) - 1):
        nxt[ins[k][0]] = ins[k + 1][0]
    for off, op, arg, rep in ins:
        if op in JUMP_REL:
            targets.add(nxt.get(off, off + 3) + arg)
    tags = [[] for _ in range(nparams)]
    ev_attr = [[] for _ in range(nparams)]
    ev_eattr = [[] for _ in range(nparams)]
    touches = 0
    payload_native_calls = 0
    ev_sub = [[] for _ in range(nparams)]
    ev_cmp = [[] for _ in range(nparams)]
    ev_pass = [[] for _ in range(nparams)]
    loaded = [False] * nparams
    alias = {}
    risk = set()
    desync = False
    stack = []

    def tag(pi, t, k=None):
        if t not in tags[pi]:
            tags[pi].append(t)

    def note(lst, pi, text):
        if text not in lst[pi]:
            lst[pi].append(text)

    reasons = []

    def pop():
        nonlocal desync
        if stack:
            return stack.pop()
        desync = True
        reasons.append(('underflow', cur_op[0], cleared[0]))
        return _val(None)

    cur_op = ['']
    cleared = [False]
    for off, op, arg, rep in ins:
        cur_op[0] = op
        if off in targets and op != 'FOR_ITER':
            cleared[0] = True
            stack = []      # a loop head keeps its iterator: every path into it holds the same stack
        if op == 'LOAD_FAST':
            if rep in pindex:
                loaded[pindex[rep]] = True
                stack.append(_val('p', pindex[rep]))
            elif rep in alias:
                stack.append(alias[rep])
            else:
                stack.append(_val('l', None, rep))
        elif op == 'LOAD_CONST':
            k = fn.consts[arg]
            stack.append(_val('c', None, k))
        elif op in ('LOAD_GLOBAL', 'LOAD_NAME'):
            if rep in NATIVE_ROOTS:
                touches += 1
            stack.append(_val('g', None, rep))
        elif op == 'LOAD_ATTR':
            t = pop()
            if t[0] == 'p':
                tag(t[1], 'attr')
                note(ev_attr, t[1], '.' + rep)
                stack.append(_val('pa', t[1], rep))
            elif t[0] == 'e':
                tag(t[1], 'elem.attr')
                note(ev_eattr, t[1], '.' + rep)
                stack.append(_val(None))
            elif t[0] in ('g', 'a', 'l'):
                stack.append(_val('a', None, '%s.%s' % (t[2], rep)))
            else:
                # receiver unknown (a call result, say): the method name still says what is called
                stack.append(_val('a', None, '?.%s' % rep))
            if rep in REMOTE_ATTRS:
                risk.add('reentrant-call')
            if rep in LOCK_ATTRS:
                risk.add('lock')
            if any(m in rep for m in NATIVE_MARKS):
                touches += 1
        elif op == 'BINARY_SUBSCR':
            idx = pop()
            base = pop()
            if base[0] == 'p':
                tag(base[1], 'subscript')
                if idx[0] == 'c':
                    note(ev_sub, base[1], repr(idx[2].decode('utf8', 'replace')) if isinstance(idx[2], bytes)
                         else str(idx[2]))
            elif base[0] == 'e':
                tag(base[1], 'elem[]')
            if idx[0] == 'p':
                tag(idx[1], 'as-key')
            stack.append(_val(None))
        elif op == 'COMPARE_OP':
            b = pop()
            a = pop()
            o = CMP[arg] if arg < len(CMP) else '?'
            if o in ('in', 'not in'):
                if b[0] == 'p':
                    tag(b[1], 'contains')
                if a[0] == 'p':
                    tag(a[1], 'truth')
            else:
                for x, y in ((a, b), (b, a)):
                    if x[0] in ('p', 'e'):
                        tag(x[1], 'truth')
                        if y[0] == 'c':
                            note(ev_cmp, x[1], '%s %s' % (o, _cname(y)))
            stack.append(_val(None))
        elif op in ('POP_JUMP_IF_FALSE', 'POP_JUMP_IF_TRUE'):
            t = pop()
            if t[0] == 'p':
                tag(t[1], 'truth')
        elif op in ('JUMP_IF_FALSE_OR_POP', 'JUMP_IF_TRUE_OR_POP'):
            t = stack[-1] if stack else _val(None)
            if t[0] == 'p':
                tag(t[1], 'truth')
            if stack:
                stack.pop()
        elif op == 'UNARY_NOT':
            t = pop()
            if t[0] == 'p':
                tag(t[1], 'truth')
            stack.append(_val(None))
        elif op == 'GET_ITER':
            t = pop()
            if t[0] == 'p':
                tag(t[1], 'iterate')
                stack.append(_val('it', t[1]))
            else:
                stack.append(_val(None))
        elif op == 'FOR_ITER':
            it = stack[-1] if stack else _val(None)
            stack.append(_val('e', it[1]) if it[0] == 'it' else _val(None))
        elif op == 'UNPACK_SEQUENCE':
            t = pop()
            if t[0] == 'p':
                tag(t[1], 'unpack_%d' % arg)
                risk.add('strict-arity')
            elif t[0] == 'e':
                tag(t[1], 'elem.unpack_%d' % arg)
            for _ in range(arg):
                stack.append(_val(None))
        elif op == 'STORE_FAST':
            t = pop()
            if t[0] in ('e', 'p'):
                alias[rep] = t
            elif rep in alias:
                del alias[rep]
        elif op == 'STORE_ATTR':
            obj = pop()
            val = pop()
            if val[0] == 'p':
                tag(val[1], 'store')
        elif op == 'STORE_SUBSCR':
            idx = pop()
            pop()
            val = pop()
            if idx[0] == 'p':
                tag(idx[1], 'as-key')
            if val[0] == 'p':
                tag(val[1], 'store')   # stashed in a container, not inspected
        elif op == 'DELETE_SUBSCR':
            pop()
            pop()
        elif op in BINARY or op.startswith('INPLACE_'):
            b = pop()
            a = pop()
            for x, y in ((a, b), (b, a)):
                if x[0] == 'p':
                    tag(x[1], 'format' if (op == 'BINARY_MODULO' and y[0] == 'c') or
                        (op == 'BINARY_MODULO' and False) else 'arith')
            stack.append(_val(None))
        elif op.startswith('SLICE+'):
            n = int(op[-1])
            drop = {0: 0, 1: 1, 2: 1, 3: 2}[n]
            for _ in range(drop):
                pop()
            base = pop()
            if base[0] == 'p':
                tag(base[1], 'slice')
            stack.append(_val(None))
        elif op in ('CALL_FUNCTION', 'CALL_FUNCTION_VAR', 'CALL_FUNCTION_KW', 'CALL_FUNCTION_VAR_KW'):
            npos = arg & 0xFF
            nkw = (arg >> 8) & 0xFF
            extra = 1 if op.endswith('_VAR') or op.endswith('_KW') else 0
            if op == 'CALL_FUNCTION_VAR_KW':
                extra = 2
            args = [pop() for _ in range(npos + 2 * nkw + extra)]
            callee = pop()
            cname = callee[2] if callee[0] in ('g', 'a') else None
            native = cname is not None and (cname.split('.')[0] in NATIVE_ROOTS or
                                            any(m in cname for m in NATIVE_MARKS))
            if native and any(x[0] == 'p' for x in args):
                payload_native_calls += 1
            if cname and cname.endswith('DBRow') and any(x[0] == 'p' for x in args):
                risk.add('dbrow')
            for x in args:
                if x[0] == 'p':
                    if cname == 'len':
                        tag(x[1], 'len')
                    elif native:
                        tag(x[1], 'native')
                        risk.add('native-payload')
                        note(ev_pass, x[1], '->' + cname)
                    else:
                        tag(x[1], 'pass')
                        if cname:
                            note(ev_pass, x[1], '>' + cname.split('.')[-1])
            stack.append(_val(None))
        elif op in ('BUILD_TUPLE', 'BUILD_LIST', 'BUILD_SET'):
            items = [pop() for _ in range(arg)]
            for x in items:
                if x[0] == 'p':
                    tag(x[1], 'pass')
                    note(ev_pass, x[1], '>container')
            stack.append(_val(None))
        elif op == 'RAISE_VARARGS':
            risk.add('raise-stmt')
            for _ in range(arg):
                pop()
        elif op == 'STORE_MAP':
            key = pop()
            val = pop()
            for x in (key, val):
                if x[0] == 'p':
                    tag(x[1], 'pass')
                    note(ev_pass, x[1], '>container')
        elif op in EFFECT:
            n, m = EFFECT[op]
            for _ in range(n):
                pop()
            for _ in range(m):
                stack.append(_val(None))
        elif op == 'DUP_TOPX':
            top = stack[-arg:] if arg <= len(stack) else []
            stack.extend(top)
        elif op in ('MAKE_FUNCTION', 'MAKE_CLOSURE'):
            n = arg + 1 + (1 if op == 'MAKE_CLOSURE' else 0)
            for _ in range(n):
                pop()
            stack.append(_val(None))
        elif op == 'BUILD_CLASS':
            pop(); pop(); pop()
            stack.append(_val(None))
        else:
            stack = []
            desync = True
            reasons.append(('unknown-op', op, None))

    # Flags that describe what the handler touches, not what it does to the payload,
    # also look into nested defs and lambdas (their own code objects).
    for sub in pyc27.walk(fn)[1:]:
        for _off, op, _arg, rep in pyc27.dis(sub):
            if op in ('LOAD_GLOBAL', 'LOAD_NAME') and rep in NATIVE_ROOTS:
                risk.add('native-touch')
            elif op == 'LOAD_ATTR' and rep in REMOTE_ATTRS:
                risk.add('reentrant-call')
            elif op == 'LOAD_ATTR' and rep in LOCK_ATTRS:
                risk.add('lock')
            elif op == 'LOAD_ATTR' and any(m in rep for m in NATIVE_MARKS):
                risk.add('native-touch')
            elif op == 'RAISE_VARARGS':
                risk.add('raise-stmt')
    if touches > payload_native_calls:
        risk.add('native-touch')
    shapes = []
    for i in range(nparams):
        if not loaded[i]:
            shapes.append('discard')
            continue
        ts = tags[i]
        if not ts:
            shapes.append('unknown')
            continue
        # A use that pins the payload's shape makes the weak ones redundant: a
        # parameter that is subscripted is not also reported as merely passed.
        if any(t.startswith(STRONG) for t in ts):
            ts = [t for t in ts if t not in ('truth', 'pass', 'store')]
        def key(t):
            base = t.split('_')[0] if t.startswith(('unpack_', 'elem.unpack_')) else t
            base = 'unpack' if t.startswith('unpack_') else ('elem.unpack' if t.startswith('elem.unpack_') else t)
            return TAG_ORDER.index(base) if base in TAG_ORDER else 99
        shapes.append('+'.join(sorted(ts, key=key)))
    return {'shapes': shapes, 'tags': tags, 'risk': risk, 'desync': desync, 'pnames': pnames,
            'reasons': reasons, 'ev_attr': ev_attr, 'ev_eattr': ev_eattr, 'ev_sub': ev_sub, 'ev_cmp': ev_cmp, 'ev_pass': ev_pass}


def _cname(v):
    if v[0] == 'c':
        k = v[2]
        if isinstance(k, bytes):
            return repr(k.decode('utf8', 'replace'))
        return repr(k)
    if v[0] in ('g', 'a'):
        return v[2]
    return '?'


def _sub_text(items):
    out = []
    for it in items:
        out.append(it if not it.startswith("'") else it)
    return '[' + ','.join(out) + ']'


def evidence_text(a):
    """The `notes` evidence for every parameter that has some, `; `-joined."""
    parts = []
    for i, name in enumerate(a['pnames']):
        bits = []
        if a['ev_attr'][i]:
            bits.append(','.join(sorted(a['ev_attr'][i])))
        if a['ev_eattr'][i]:
            bits.append('elem' + ','.join(sorted(a['ev_eattr'][i])))
        if a['ev_sub'][i]:
            bits.append(_sub_text(sorted(a['ev_sub'][i], key=lambda t: (t.startswith("'"), t))))
        for t in a['tags'][i]:
            if t.startswith('unpack_') or t.startswith('elem.unpack_'):
                if t.startswith('unpack_'):
                    bits.append(t)
        native = [x for x in a['ev_pass'][i] if x.startswith('->')]
        calls = sorted({x[1:] for x in a['ev_pass'][i] if x.startswith('>')})
        bits.extend(native)
        bits.extend(sorted(a['ev_cmp'][i]))
        if calls:
            bits.append('>' + ','.join(calls))
        if bits:
            parts.append(name + ' ' + ' '.join(bits))
    return '; '.join(parts)


def risk_tags(a, event):
    r = set(a['risk'])
    tags = [t for ts in a['tags'] for t in ts]
    if any(t == 'native' for t in tags):
        r.add('native-payload')
    if any(t in ('subscript', 'iterate', 'len', 'attr', 'contains', 'slice') or t.startswith('elem')
           for t in tags):
        r.add('raises-on-shape')
    if event.startswith('Do'):
        r.add('do-sync-blocktrap')
    if event in ('DoSessionChanging', 'ProcessSessionChange', 'OnSessionChanged'):
        r.add('session-change-path')
    return ';'.join(t for t in RISK_ORDER if t in r) or 'none'


def build_rows(classes, rows, declarers, emit_files, own_by_uid):
    """Stage 7-8: the TSV rows (as lists of the nine columns), sorted."""
    by_uid = {c.uid: c for c in classes}
    out = []
    for r in rows:
        c, name, fn = r['cls'], r['name'], r['fn']
        a = analyse_handler(fn, name)
        how = r['how']
        if how == 'notifyevents' and c.name == 'Michelle' and name in (own_by_uid.get(c.uid) or ()):
            how = 'michelle'
        note = []
        if how == 'other':
            note.append('registerfor:%s.pyc' % c.rel)
        elif r.get('related') is not None:
            note.append('base:%s.pyc::%s' % (by_uid[r['related']].rel, by_uid[r['related']].name))
        elif r.get('subclass') is not None:
            note.append('subclass:%s.pyc::%s' % (by_uid[r['subclass']].rel, by_uid[r['subclass']].name))
        if r['ndef']:
            note.append('%d default(s)' % r['ndef'])
        note.append('kind=%s' % ('svc' if r['svc'] else 'ui'))
        note.append('decl=%d' % len(declarers[name]))
        note.append('local_emitters=%d' % len(emit_files.get(name, ())))
        if a['desync']:
            note.append('[stack-desync: shapes partial]')
        ev = evidence_text(a)
        if ev:
            note.append(ev)
        svc = c.name + (' [%s]' % c.guid if c.guid else '')
        out.append([name, '%s:%d' % (c.filename, fn.firstlineno), svc, how, arity_text(fn, r['ndef']),
                    ','.join(a['pnames']) or '-', '|'.join(a['shapes']) or '-', risk_tags(a, name),
                    ' | '.join(note)])
    return out


def emit_file_index(root):
    """{event: set of modules that emit it with a constant name} (stage 5)."""
    out = collections.defaultdict(set)
    for rel, top in L.load_tree(root):
        for c in pyc27.walk(top):
            ins = list(pyc27.dis(c))
            for i, (_off, op, _arg, rep) in enumerate(ins):
                if op == 'LOAD_ATTR' and rep in EMIT_NAMES and i + 1 < len(ins) and ins[i + 1][1] == 'LOAD_CONST':
                    k = c.consts[ins[i + 1][2]]
                    if isinstance(k, bytes):
                        out[L.dec(k)].add(rel)
    return out


def no_handler_rows(classes, rows, declarers, emit_files):
    handled = {r['name'] for r in rows}
    by_uid = {c.uid: c for c in classes}
    out = []
    for ev in sorted(declarers):
        if ev in handled:
            continue
        who = ', '.join('%s.py::%s' % (by_uid[u].rel, by_uid[u].name) for u in sorted(declarers[ev]))
        out.append([ev, '(none)', '(none)', 'notifyevents (no handler)', '-', '-', '-',
                    'none (getattr on the registrant raises AttributeError, caught per-handler)',
                    'declared by %d class(es): %s | local_emitters=%d | NO method of this name in any class '
                    'that registers it' % (len(declarers[ev]), who, len(emit_files.get(ev, ())))])
    return out


def generate(root):
    classes = index_classes(root)
    own, computed = {}, {}
    for c in classes:
        own[c.uid], computed[c.uid] = declared_events(c)
        c.own, c.computed = own[c.uid], computed[c.uid]
    registered, _emit = scan_sites(root)
    parents = resolve_bases(classes)
    rows, declarers = reconcile(classes, own, computed, registered, parents)
    emit_files = emit_file_index(root)
    table = build_rows(classes, rows, declarers, emit_files, own)
    table.extend(no_handler_rows(classes, rows, declarers, emit_files))
    table.sort(key=lambda r: (r[0], r[1]))
    stats = {'classes': len(classes), 'declaring': sum(1 for c in classes if c.own is not None),
             'events': len(declarers), 'handlers': len(rows), 'failed': len(L.load_tree.failed)}
    return table, stats


def _row_keys(table):
    """(event, handler path without its line, service, nth) per row: the key a
    row keeps when only its line number moves."""
    seen, keys = {}, []
    for r in table:
        k = (r[0], r[1].rsplit(':', 1)[0], r[2])
        seen[k] = seen.get(k, 0) + 1
        keys.append(k + (seen[k],))
    return keys


def carry(new_table, old_table, seed_rows):
    """Re-anchor a hand-curated table on a new build (re-pin, issue #19).

    The generator is "re-derive and diff, not a drop-in": some payload_shape,
    risk and notes cells of the committed table are judgement. So a cell is
    taken from the new build only where the generator's own output moved
    between the two builds; where it did not, the committed (seed) cell stays.
    A row with no counterpart in the old build or the seed is taken whole.
    Returns (rows, number of cells that came from the new build in a kept row).
    """
    old = dict(zip(_row_keys(old_table), old_table))
    seed = dict(zip(_row_keys(seed_rows), seed_rows))
    out, moved = [], 0
    for k, row in zip(_row_keys(new_table), new_table):
        o, sd = old.get(k), seed.get(k)
        if o is None or sd is None:
            out.append(row)
            continue
        cells = []
        for i, c in enumerate(row):
            if c == o[i]:
                cells.append(sd[i])
            else:
                cells.append(c)
                moved += c != sd[i]
        out.append(cells)
    return out, moved


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--pyc', default=L.DEFAULT_PYC)
    ap.add_argument('--out', default='findings/push-events.tsv', help="'-' for stdout")
    ap.add_argument('--old-pyc', help='the tree the --seed table was made from: carry its '
                    'hand-curated cells over to --pyc (see carry())')
    ap.add_argument('--seed', help='the committed table made from --old-pyc (required with it)')
    a = ap.parse_args(argv)
    if not os.path.isdir(a.pyc):
        sys.exit('scan_push: no such tree: %s (run tools/extract_code.py first)' % a.pyc)
    if bool(a.old_pyc) != bool(a.seed):
        sys.exit('scan_push: --old-pyc and --seed go together (the old tree and the table made from it)')
    table, st = generate(a.pyc)
    if a.old_pyc:
        if not os.path.isdir(a.old_pyc):
            sys.exit('scan_push: no such tree: %s' % a.old_pyc)
        old_table, _ = generate(a.old_pyc)
        with open(a.seed, encoding='utf-8') as fh:
            seed_rows = [ln.rstrip('\n').split('\t') for ln in fh][1:]
        table, moved = carry(table, old_table, seed_rows)
        print('push-events: carried the committed cells; %d cells came from the new build'
              % moved, file=sys.stderr)
    L.write_tsv(a.out, [L.tsv_line(COLUMNS)] + [L.tsv_line(r) for r in table])
    print('push-events: %d rows (%d handlers, %d declared events, %d classes, %d declaring, %d unparsed)'
          % (len(table), st['handlers'], st['events'], st['classes'], st['declaring'], st['failed']),
          file=sys.stderr)


if __name__ == '__main__':
    main()
