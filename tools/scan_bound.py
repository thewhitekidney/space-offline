#!/usr/bin/env python3
"""Enumerate the client's *bound-object* services and their methods from bytecode.

What this establishes
---------------------
`findings/remote-services.txt` (from `tools/scan_remote.py`) only sees the
client's first call style, `sm.RemoteSvc('name').Method()`.  The second style --
**bound objects** -- never appears as such a call site, and that is where
inventory, dogma and ship/space control live.  `docs/reference/bound-objects.md` describes
that protocol and quotes a hand-produced figure of 29 moniker services /
387 methods.  This script is the missing scanner behind that figure: it
re-derives the inventory from the client's own bytecode so the number is
reproducible against any build, and so every entry carries provenance a reader
can check by hand.

The mechanism it keys on (see `docs/reference/bound-objects.md` S1) is
`util.Moniker(serviceName, bindParams)`.  `Moniker.__getattr__` turns any
attribute whose **first character is uppercase** into a remote call and serves
everything else locally, so "the methods of a bound service" are exactly the
uppercase attributes the client takes on a value it obtained from a moniker.

How it works
------------
A moniker-valued expression is "tainted"; every uppercase attribute taken on a
tainted value is a remote method.  Taint reaches a call site one of these ways,
and the way is recorded per call site as `how`:

  moniker       a chain ending in `Moniker` called with a string literal first
                argument -- the ground truth.  Also yields `bind_params`.
  local         a local variable or module global holding a tainted value.
  attr-module   an attribute (`self.remoteDogmaLM`) assigned a tainted value
                somewhere in the *same* module.  Names that `__getattr__`
                dispatches on (`invCache.inventorymgr`) count as attributes.
  attr-global   the same, promoted tree-wide -- but only when that attribute
                name is written in exactly one module of the tree.  This is
                what finds methods in modules that never build the moniker.
  container-index
                a moniker parked at a fixed index of a list or tuple attribute
                and read back by that index (`gameui.shipAccess[0]`).
  accessor      a call to a function whose body was seen to *return* a tainted
                value, resolved inside its own module.
  accessor-module
                the same, where the receiver names a module exactly: an import
                alias followed back to its file, or `sm.GetService('godma')`
                resolved through that service's `__guid__`/`__servicename__`,
                or -- because `CoreService.__getattr__` turns an unknown
                attribute that names a service into `sm.GetService(name)` --
                `self.godma` on a service object.
  accessor-unique-name
                a bare `Foo()` whose name is defined in exactly one module of
                the 12,774, so it cannot be confused with another `Foo`.
  accessor-imported
                `self.Foo()` where exactly one module this module imports
                defines `Foo` as a moniker accessor: inherited methods.

The uniqueness rules are the load-bearing heuristics.  `promoted_names` in the
JSON lists every name they let cross a module boundary and `ambiguities_dropped`
lists what they refused, so both can be audited instead of trusted.

The maps are iterated to a fixpoint (an accessor can return an attribute which
was itself assigned by another accessor's result).  Arities come from a forward
stack-depth walk that finds the `CALL_FUNCTION` consuming the loaded method;
where the method is handed off as a callable instead of being called -- to
`sessionMgr.PerformSessionChange` and friends -- the arity is reported as `?`,
not guessed.

Second-level bound objects (`invbroker.GetInventory(...)` returns another bound
object) are reported in a separate `derived_second_level` section and are *not*
counted in the headline totals, because a `.Method()` on a call result is not
by itself evidence that the result was a bound object.

Deliberate non-goal: nothing here is transcribed from `docs/reference/bound-objects.md`.
`--compare docs/reference/bound-objects.md` parses that document and diffs it against what
was scanned, so a discrepancy is visible rather than papered over.

Usage
-----
    python3 tools/scan_bound.py                       # scan, print summary
    python3 tools/scan_bound.py -o findings/bound-objects.json
    python3 tools/scan_bound.py --compare docs/reference/bound-objects.md
    python3 tools/scan_bound.py --service dogmaIM     # provenance for one service

Run it in the analysis container, like every other tool here:

    tools/analysis.sh python3 tools/scan_bound.py -o findings/bound-objects.json \
        --compare docs/reference/bound-objects.md

No host-side dependencies: `tools/pyc27.py` plus the standard library.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import collections
import json
import os
import re
import sys

# `tools/` itself is already on the path -- the rotation above put it last,
# behind the stdlib, which is the whole point. See that comment.
import pyc27  # noqa: E402

# --------------------------------------------------------------------------
# 2.7 stack effects, enough to find the CALL_FUNCTION that consumes a callable.
# Anything not listed -- control flow above all -- aborts the walk, which is
# how an arity becomes '?' instead of a guess.
# --------------------------------------------------------------------------
_EFFECT = {
    'POP_TOP': -1, 'ROT_TWO': 0, 'ROT_THREE': 0, 'DUP_TOP': 1, 'ROT_FOUR': 0,
    'NOP': 0,
    'UNARY_POSITIVE': 0, 'UNARY_NEGATIVE': 0, 'UNARY_NOT': 0,
    'UNARY_CONVERT': 0, 'UNARY_INVERT': 0,
    'BINARY_POWER': -1, 'BINARY_MULTIPLY': -1, 'BINARY_DIVIDE': -1,
    'BINARY_MODULO': -1, 'BINARY_ADD': -1, 'BINARY_SUBTRACT': -1,
    'BINARY_SUBSCR': -1, 'BINARY_FLOOR_DIVIDE': -1, 'BINARY_TRUE_DIVIDE': -1,
    'BINARY_LSHIFT': -1, 'BINARY_RSHIFT': -1, 'BINARY_AND': -1,
    'BINARY_XOR': -1, 'BINARY_OR': -1,
    'INPLACE_POWER': -1, 'INPLACE_MULTIPLY': -1, 'INPLACE_DIVIDE': -1,
    'INPLACE_MODULO': -1, 'INPLACE_ADD': -1, 'INPLACE_SUBTRACT': -1,
    'INPLACE_FLOOR_DIVIDE': -1, 'INPLACE_TRUE_DIVIDE': -1,
    'INPLACE_LSHIFT': -1, 'INPLACE_RSHIFT': -1, 'INPLACE_AND': -1,
    'INPLACE_XOR': -1, 'INPLACE_OR': -1,
    'SLICE+0': 0, 'SLICE+1': -1, 'SLICE+2': -1, 'SLICE+3': -2,
    'STORE_SLICE+0': -2, 'STORE_SLICE+1': -3, 'STORE_SLICE+2': -3,
    'STORE_SLICE+3': -4,
    'DELETE_SLICE+0': -1, 'DELETE_SLICE+1': -2, 'DELETE_SLICE+2': -2,
    'DELETE_SLICE+3': -3,
    'STORE_MAP': -2, 'STORE_SUBSCR': -3, 'DELETE_SUBSCR': -2,
    'GET_ITER': 0,
    'PRINT_EXPR': -1, 'PRINT_ITEM': -1, 'PRINT_NEWLINE': 0,
    'PRINT_ITEM_TO': -2, 'PRINT_NEWLINE_TO': -1,
    'LOAD_CONST': 1, 'LOAD_NAME': 1, 'LOAD_GLOBAL': 1, 'LOAD_FAST': 1,
    'LOAD_DEREF': 1, 'LOAD_CLOSURE': 1,
    'LOAD_ATTR': 0,
    'STORE_NAME': -1, 'STORE_GLOBAL': -1, 'STORE_FAST': -1, 'STORE_DEREF': -1,
    'DELETE_NAME': 0, 'DELETE_GLOBAL': 0, 'DELETE_FAST': 0, 'DELETE_ATTR': -1,
    'STORE_ATTR': -2,
    'COMPARE_OP': -1,
    'IMPORT_NAME': -1, 'IMPORT_FROM': 1,
    'LIST_APPEND': -1, 'SET_ADD': -1, 'MAP_ADD': -2,
    # conditional jumps: the fall-through path, which is the one _walk takes.
    # `a or b` / `a if c else b` keep the net depth right when walked linearly.
    'POP_JUMP_IF_FALSE': -1, 'POP_JUMP_IF_TRUE': -1,
    'JUMP_IF_FALSE_OR_POP': -1, 'JUMP_IF_TRUE_OR_POP': -1,
}
_EFFECT_ARG = {
    'BUILD_TUPLE': lambda a: 1 - a,
    'BUILD_LIST': lambda a: 1 - a,
    'BUILD_SET': lambda a: 1 - a,
    'BUILD_MAP': lambda a: 1,
    'BUILD_SLICE': lambda a: 1 - a,
    'DUP_TOPX': lambda a: a,
    'UNPACK_SEQUENCE': lambda a: a - 1,
    'MAKE_FUNCTION': lambda a: -a,
    'MAKE_CLOSURE': lambda a: -a - 1,
}
_CALL_EXTRA = {
    'CALL_FUNCTION': 0, 'CALL_FUNCTION_VAR': 1, 'CALL_FUNCTION_KW': 1,
    'CALL_FUNCTION_VAR_KW': 2,
}
_BASE_LOADS = ('LOAD_GLOBAL', 'LOAD_NAME', 'LOAD_FAST', 'LOAD_DEREF')
_GETSERVICE = ('GetService', 'StartService', 'GetServiceIfRunning',
               'StartServiceAndWaitForRunningState', 'services')
_STORES = {'STORE_FAST': 'local', 'STORE_DEREF': 'local', 'STORE_NAME': 'name',
           'STORE_GLOBAL': 'global', 'STORE_ATTR': 'attr'}
_CALL_WINDOW = 600


def _walk(ins, start, want, index=None):
    """Walk one forward path from `start` with a value already on the stack.

    `want(j, op, arg, depth)` is consulted at each instruction and may end the
    walk by returning a result.  The walk follows the fall-through of a
    conditional jump and the target of a forward unconditional one -- both
    preserve stack depth for the expressions this scanner cares about -- and
    stops dead at anything else, which is how an arity honestly becomes '?'.
    """
    if index is None:
        index = {ins[j][0]: j for j in range(len(ins))}
    depth, j, steps = 1, start, 0
    while j < len(ins) and steps < _CALL_WINDOW:
        steps += 1
        off, op, arg = ins[j][0], ins[j][1], ins[j][2]
        r = want(j, op, arg, depth)
        if r is not None:
            return r if r is not True else None
        if op in _CALL_EXTRA:
            npos, nkw = arg & 0xFF, (arg >> 8) & 0xFF
            consumed = 1 + npos + 2 * nkw + _CALL_EXTRA[op]
            if depth < consumed:
                return None
            depth -= consumed - 1
        elif op in ('JUMP_FORWARD', 'JUMP_ABSOLUTE'):
            tgt = off + 3 + arg if op == 'JUMP_FORWARD' else arg
            if tgt <= off or tgt not in index:
                return None          # a loop: do not go round it
            j = index[tgt]
            continue
        elif op in _EFFECT:
            depth += _EFFECT[op]
        elif op in _EFFECT_ARG:
            depth += _EFFECT_ARG[op](arg)
        else:
            return None
        if depth <= 0:
            return None
        j += 1
    return None


def match_call(ins, start, index=None):
    """Find the CALL_FUNCTION* consuming a callable pushed just before `start`.

    Returns (index, opname, npos, nkw) or None.  `None` means "the client did
    not call it right here" -- a callable handed to something else, as with
    `sessionMgr.PerformSessionChange(..., svc.Undock, ...)`.
    """
    def want(j, op, arg, depth):
        if op in _CALL_EXTRA:
            npos, nkw = arg & 0xFF, (arg >> 8) & 0xFF
            consumed = 1 + npos + 2 * nkw + _CALL_EXTRA[op]
            if depth == consumed:
                return j, op, npos, nkw
            if depth < consumed:
                return True
        return None
    return _walk(ins, start, want, index)


def match_container(ins, start, index=None):
    """Find the BUILD_LIST/BUILD_TUPLE that swallows the value before `start`.

    Returns (index_of_build, position_within_the_container) or None.  This is
    what lets the scanner follow `self.shipAccess = [moniker, a, b, c]`, the
    pattern `docs/reference/bound-objects.md` gap 9 says had to be seeded by hand.
    """
    def want(j, op, arg, depth):
        if op in ('BUILD_LIST', 'BUILD_TUPLE'):
            return (j, arg - depth) if depth <= arg else True
        if op in _CALL_EXTRA:
            npos, nkw = arg & 0xFF, (arg >> 8) & 0xFF
            if depth <= 1 + npos + 2 * nkw + _CALL_EXTRA[op]:
                return True
        return None
    return _walk(ins, start, want, index)


def arity_str(call):
    """Render a matched call the way docs/reference/bound-objects.md writes arities."""
    if call is None:
        return '?'
    _, op, npos, nkw = call
    s = str(npos)
    if nkw:
        s += '+%dkw' % nkw
    if op in ('CALL_FUNCTION_VAR', 'CALL_FUNCTION_VAR_KW'):
        s += '+*a'
    if op in ('CALL_FUNCTION_KW', 'CALL_FUNCTION_VAR_KW'):
        s += '+**kw'
    return s


# --------------------------------------------------------------------------
# bindParams rendering: a tiny source reconstructor for the Moniker arguments.
# --------------------------------------------------------------------------
def render_args(c, ins, lo, hi):
    """Best-effort source text for the expressions pushed in ins[lo:hi]."""
    stack = []
    for j in range(lo, hi):
        op, arg, rep = ins[j][1], ins[j][2], ins[j][3]
        try:
            if op == 'LOAD_CONST':
                k = c.consts[arg]
                stack.append(repr(k.decode('utf8', 'replace'))
                             if isinstance(k, bytes) else repr(k))
            elif op in _BASE_LOADS:
                stack.append(rep)
            elif op == 'LOAD_ATTR':
                stack.append('%s.%s' % (stack.pop(), rep))
            elif op in ('BUILD_TUPLE', 'BUILD_LIST'):
                items = [stack.pop() for _ in range(arg)][::-1]
                stack.append(('[%s]' if op == 'BUILD_LIST' else '(%s)')
                             % ', '.join(items))
            elif op in _CALL_EXTRA:
                npos, nkw = arg & 0xFF, (arg >> 8) & 0xFF
                n = npos + 2 * nkw + _CALL_EXTRA[op]
                items = [stack.pop() for _ in range(n)][::-1]
                fn = stack.pop() if stack else '?'
                stack.append('%s(%s)' % (fn, ', '.join(items[:npos])))
            elif op == 'BINARY_SUBSCR':
                k = stack.pop()
                stack.append('%s[%s]' % (stack.pop(), k))
            else:
                return None
        except IndexError:
            return None
    return ', '.join(stack) if stack else ''


# --------------------------------------------------------------------------
# Module iteration
# --------------------------------------------------------------------------
def iter_modules(root):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for f in sorted(filenames):
            if f.endswith('.pyc'):
                yield os.path.join(dirpath, f)


def flatten(c, path, out):
    """Flatten a code tree into (qualname, code) pairs, parents before children."""
    out.append((path, c))
    for k in (c.consts or ()):
        if isinstance(k, pyc27.Code):
            flatten(k, '%s.%s' % (path, k.name.decode('utf8', 'replace')), out)
    return out


def load_tree(root):
    """Parse every module once: [(relpath, [(qualname, code), ...]), ...]."""
    tree, failed = [], []
    for p in iter_modules(root):
        rel = os.path.relpath(p, root)
        try:
            top = pyc27.load_pyc(p)
        except Exception as e:                                  # noqa: BLE001
            failed.append((rel, '%s: %s' % (type(e).__name__, e)))
            continue
        tree.append((rel, flatten(top, '<module>', [])))
    return tree, failed


# --------------------------------------------------------------------------
# The Moniker / MachoObjectConnection local API, derived not hardcoded.
# Uppercase attributes defined on these classes are served locally and are
# therefore NOT remote methods.
# --------------------------------------------------------------------------
_LOCAL_API_CLASSES = {
    'carbon/common/script/net/moniker.pyc':
        ('Moniker', 'MonikerCallWrap'),
    'carbon/common/script/net/ObjectCallGPCS.pyc':
        ('MachoObjectConnection', 'MachoBoobyTrap', 'MachoObjectCallWrapper'),
}


def derive_local_api(tree):
    """Uppercase attributes the moniker/bound-object wrappers handle locally."""
    want = {}
    for rel, classes in _LOCAL_API_CLASSES.items():
        want[rel.replace('/', os.sep)] = set(classes)
    names, where = set(), {}
    for rel, codes in tree:
        if rel not in want:
            continue
        for qual, c in codes:
            cname = c.name.decode('utf8', 'replace')
            if cname not in want[rel]:
                continue
            for off, op, arg, rep in pyc27.dis(c):
                if op == 'STORE_NAME' and rep[:1].isupper():
                    names.add(rep)
                    where.setdefault(rep, []).append('%s::%s' % (rel, cname))
            # __remobsuppmeth__ = names forwarded to the bound object, not remoted
            prev = None
            for off, op, arg, rep in pyc27.dis(c):
                if op == 'LOAD_CONST':
                    prev = c.consts[arg]
                elif op == 'STORE_NAME' and rep == '__remobsuppmeth__':
                    for k in (prev or ()):
                        if isinstance(k, bytes):
                            names.add(k.decode())
                            where.setdefault(k.decode(), []).append(
                                '%s::%s.__remobsuppmeth__' % (rel, cname))
    return names, where


# --------------------------------------------------------------------------
# The scanner proper
# --------------------------------------------------------------------------
def getattr_guards(c, ins):
    """`def __getattr__(self, key)` dispatchers: which names guard which region.

    Yields (lo, hi, [names]) for every `if key == 'x':` / `if key in (...):`
    branch.  `invCache.inventorymgr` is served this way -- there is no
    STORE_ATTR for it anywhere -- and the docs call this exact shape a
    taint-scanner blind spot.  Modelling only this literal-compare form keeps
    it precise: no name is invented that the client does not compare against.
    """
    if c.name != b'__getattr__' or c.argcount < 2 or not c.varnames:
        return
    key = c.varnames[1]
    index = {ins[j][0]: j for j in range(len(ins))}
    for i in range(len(ins) - 3):
        if ins[i][1] != 'LOAD_FAST' or c.varnames[ins[i][2]] != key:
            continue
        if ins[i + 1][1] != 'LOAD_CONST' or ins[i + 2][1] != 'COMPARE_OP':
            continue
        if ins[i + 3][1] != 'POP_JUMP_IF_FALSE':
            continue
        k = c.consts[ins[i + 1][2]]
        if isinstance(k, bytes):
            names = [k.decode('utf8', 'replace')]
        elif isinstance(k, (tuple, frozenset)):
            names = [x.decode('utf8', 'replace') for x in k
                     if isinstance(x, bytes)]
        else:
            continue
        hi = index.get(ins[i + 3][2])
        if names and hi is not None and hi > i:
            yield i + 4, hi, names


def name_census(tree):
    """Tree-wide: where each STORE_ATTR target and each def'd name lives.

    This is what the two uniqueness rules are measured against.  Counting *all*
    stores and *all* definitions -- not just tainted ones -- is the point: a
    name like `object` or `GetMoniker` must be seen to be ambiguous even though
    only one of its many definitions touches a moniker.
    """
    attr_mods = collections.defaultdict(set)
    def_mods = collections.defaultdict(set)
    base_mods = collections.defaultdict(set)
    for rel, codes in tree:
        base_mods[os.path.basename(rel)[:-4]].add(rel)
        for qual, c in codes:
            ins = list(pyc27.dis(c))
            for off, op, arg, rep in ins:
                if op == 'STORE_ATTR':
                    attr_mods[rep].add(rel)
            # a name only `__getattr__` ever produces is an attribute too, and
            # must count towards the uniqueness rule like a real one
            for lo, hi, names in getattr_guards(c, ins):
                for nm in names:
                    attr_mods[nm].add(rel)
            for k in (c.consts or ()):
                if isinstance(k, pyc27.Code):
                    def_mods[k.name.decode('utf8', 'replace')].add(rel)
    return attr_mods, def_mods, base_mods


def module_index(tree, base_mods):
    """Two exact ways to say which module a receiver expression names.

    * `aliases[module][localname] -> module` from the module-level import
      bytecode, so `import ... eveMoniker as moniker` is followed correctly
      rather than guessed from the alias spelling.
    * `svc_module[servicename] -> module` from `__servicename__` in a client
      service class, so `sm.GetService('gameui')` names gameui.pyc.
    """
    by_dotted = {}
    for rel, _ in tree:
        by_dotted[rel[:-4].replace(os.sep, '.')] = rel

    def resolve(dotted):
        if dotted in by_dotted:
            return by_dotted[dotted]
        tail = dotted.rsplit('.', 1)[-1]
        mods = base_mods.get(tail, ())
        return next(iter(mods)) if len(mods) == 1 else None

    aliases = {}
    svc_module = collections.defaultdict(set)
    for rel, codes in tree:
        amap = {}
        for qual, c in codes:
            ins = list(pyc27.dis(c))
            for i, (off, op, arg, rep) in enumerate(ins):
                if op != 'IMPORT_NAME':
                    continue
                dotted, j, attrs = rep, i + 1, 0
                while j < len(ins) and ins[j][1] == 'LOAD_ATTR':
                    attrs += 1
                    j += 1
                # `import a.b.c as alias` -> LOAD_ATTRs then a store
                if attrs and j < len(ins) and ins[j][1] in _STORES:
                    m = resolve(dotted)
                    if m:
                        amap[ins[j][3]] = m
                # `from a.b import c [as alias]`
                while j + 1 < len(ins) and ins[j][1] == 'IMPORT_FROM':
                    if ins[j + 1][1] in _STORES:
                        m = resolve(dotted + '.' + ins[j][3])
                        if m:
                            amap[ins[j + 1][3]] = m
                    j += 2
            prev = None
            for off, op, arg, rep in ins:
                if op == 'LOAD_CONST':
                    prev = c.consts[arg]
                elif op == 'STORE_NAME' and isinstance(prev, bytes):
                    # server-side services name themselves with
                    # __servicename__, client-side ones with __guid__='svc.x'
                    if rep == '__servicename__':
                        svc_module[prev.decode('utf8', 'replace')].add(rel)
                    elif rep == '__guid__' and prev.startswith(b'svc.'):
                        svc_module[prev[4:].decode('utf8', 'replace')].add(rel)
        aliases[rel] = {k: v for k, v in amap.items() if v}
    return aliases, {k: next(iter(v)) for k, v in svc_module.items()
                     if len(v) == 1}


class Env(object):
    """The tree-wide maps that make taint cross function and module borders."""

    def __init__(self, census=None):
        c = census or {}
        self.attr_census = c.get('attr', {})
        self.def_census = c.get('def', {})
        self.base_census = c.get('base', {})
        self.aliases = c.get('aliases', {})
        self.svc_module = c.get('svc_module', {})
        self.census = c
        self.accessor = collections.defaultdict(dict)    # module -> fn -> {svc}
        self.accessor_src = collections.defaultdict(set)
        self.attr_mod = collections.defaultdict(dict)    # module -> attr -> {svc}
        self.glob = collections.defaultdict(dict)        # module -> name -> {svc}
        # module -> attr -> module: `self.godma = sm.GetService('godma')`,
        # i.e. an attribute that names another module exactly.
        self.svc_attr = collections.defaultdict(dict)
        # module -> attr -> index -> {svc}: a moniker stashed inside a list or
        # tuple, e.g. gameui's `self.shipAccess = [GetShipAccess(), ...]`.
        self.cont = collections.defaultdict(lambda: collections.defaultdict(dict))

    def signature(self):
        def norm(d):
            return tuple(sorted((k, tuple(sorted(v))) for k, v in d.items()))
        def norm2(d):
            return tuple(sorted((m, norm(x)) for m, x in d.items()))
        return (norm2(self.accessor), norm2(self.attr_mod), norm2(self.glob),
                tuple(sorted((m, tuple(sorted(d.items())))
                             for m, d in self.svc_attr.items())),
                tuple(sorted((m, tuple(sorted(
                    (a, tuple(sorted((i, tuple(sorted(s)))
                                     for i, s in idx.items())))
                    for a, idx in d.items())))
                    for m, d in self.cont.items())))

    def container_service(self, module, name, index):
        return self._one(self.cont.get(module, {}).get(name, {}).get(index))

    @staticmethod
    def _one(svcs):
        return next(iter(svcs)) if svcs and len(svcs) == 1 else None

    def attr_service(self, module, name):
        """(service, how) for an attribute read, or None."""
        svc = self._one(self.attr_mod.get(module, {}).get(name))
        if svc:
            return svc, 'attr-module'
        if len(self.attr_census.get(name, ())) == 1:
            for mod, d in self.attr_mod.items():
                svc = self._one(d.get(name))
                if svc:
                    return svc, 'attr-global'
        return None

    def accessor_service(self, module, name, recv_module=None):
        # 1. the receiver is a module we identified exactly (import alias or
        #    sm.GetService('x')): the only fully sound cross-module rule.
        if recv_module:
            svc = self._one(self.accessor.get(recv_module, {}).get(name))
            if svc:
                return svc, 'accessor-module'
        # 2. defined in this very module
        svc = self._one(self.accessor.get(module, {}).get(name))
        if svc:
            return svc, 'accessor'
        # 3. bare name, only if that function name exists in exactly one module
        if recv_module is None and len(self.def_census.get(name, ())) == 1:
            for mod, d in self.accessor.items():
                svc = self._one(d.get(name))
                if svc:
                    return svc, 'accessor-unique-name'
        # 4. inherited: `self.GetCorpRegistry()` in a bco_*.pyc subclass whose
        #    base class lives in base_corporation.pyc.  Restricted to the
        #    modules this module actually imports, and only when exactly one
        #    of them defines that name as a moniker accessor.
        if recv_module is None:
            found = {self._one(self.accessor.get(m, {}).get(name))
                     for m in set(self.aliases.get(module, {}).values())}
            found.discard(None)
            if len(found) == 1:
                return next(iter(found)), 'accessor-imported'
        return None

    def promoted(self):
        """The names the uniqueness rules let cross a module boundary."""
        out = {'accessor_unique_name': {}, 'accessor_by_module': {},
               'attribute': {}, 'container': {}}
        for mod, d in sorted(self.accessor.items()):
            for nm, svcs in sorted(d.items()):
                if len(svcs) != 1:
                    continue
                entry = {'service': next(iter(svcs)),
                         'defined_in': mod.replace(os.sep, '/')}
                if len(self.def_census.get(nm, ())) == 1:
                    out['accessor_unique_name'][nm] = entry
                else:
                    out['accessor_by_module'].setdefault(
                        mod.replace(os.sep, '/'), {})[nm] = entry['service']
        for mod, d in sorted(self.attr_mod.items()):
            for nm, svcs in sorted(d.items()):
                if len(self.attr_census.get(nm, ())) == 1 and len(svcs) == 1:
                    out['attribute'][nm] = {
                        'service': next(iter(svcs)),
                        'stored_in': mod.replace(os.sep, '/')}
        for mod, d in sorted(self.cont.items()):
            for nm, idx in sorted(d.items()):
                for i, svcs in sorted(idx.items()):
                    if len(svcs) == 1:
                        out['container']['%s::%s[%d]'
                                         % (mod.replace(os.sep, '/'), nm, i)] \
                            = next(iter(svcs))
        return out


class Scan(object):
    def __init__(self, tree, local_api):
        self.tree = tree
        self.local_api = local_api
        self.methods = {}     # service -> method -> {'arities': set, 'sites': []}
        self.second = {}      # "svc->Method" -> same shape
        self.bind_sites = collections.defaultdict(list)
        self.ambiguous = []
        self._idx = {}
        self._guards = []
        self._tstores = set()
        self._blocked = set()

    # -- recording -----------------------------------------------------
    def _rec(self, table, svc, method, site):
        e = table.setdefault(svc, {}).setdefault(
            method, {'arities': set(), 'sites': []})
        e['arities'].add(site['args'])
        e['sites'].append(site)

    def record(self, svc, method, site):
        self._rec(self.methods, svc, method, site)

    def record_second(self, svc, method, site):
        self._rec(self.second, svc, method, site)

    def is_method(self, name):
        return bool(name) and name[0].isupper() and name not in self.local_api

    def conflicting_locals(self, c, ins):
        """Locals that hold something other than a moniker on some path.

        A store of a literal `None` does not count -- `x = None` followed by a
        conditional `x = Moniker(...)` is an idiom all over this tree -- but
        any other unexplained store does.

        Such a variable is still trusted for method calls *inside* this
        function, where the surrounding code shows which object is meant; it
        is refused as an accessor's return value, because that would export
        the confusion to every other module.  `invCache.GetInventory` is the
        case that matters: `inv` holds a charMgr moniker on one branch and a
        client-local `invCacheContainer` wrapper on another, and exporting the
        latter as a moniker hands the wrapper's own methods to charMgr.
        """
        bad = set()
        for j in range(len(ins)):
            op, rep = ins[j][1], ins[j][3]
            if op not in ('STORE_FAST', 'STORE_NAME', 'STORE_DEREF'):
                continue
            if j in self._tstores:
                continue
            if j and ins[j - 1][1] == 'LOAD_CONST' \
                    and c.consts[ins[j - 1][2]] is None:
                continue
            bad.add(rep)
        return bad

    # -- one full pass -------------------------------------------------
    def run(self, env):
        new = Env(env.census)
        for mod, d in env.accessor.items():
            new.accessor[mod] = {k: set(v) for k, v in d.items()}
        for mod, d in env.svc_attr.items():
            new.svc_attr[mod] = dict(d)
        new.accessor_src = collections.defaultdict(
            set, {k: set(v) for k, v in env.accessor_src.items()})
        self.methods, self.second = {}, {}
        self.bind_sites = collections.defaultdict(list)
        self.ambiguous = []
        for rel, codes in self.tree:
            for qual, c in codes:
                ins = list(pyc27.dis(c))
                if not ins:
                    continue
                self._idx = {ins[j][0]: j for j in range(len(ins))}
                self._guards = list(getattr_guards(c, ins))
                # Three sweeps.  0 and 2 exist because a local may be stored
                # after its first use (loops) and a self.x attribute after the
                # method that reads it.  Sweep 0 also tells us which locals
                # are *not always* a moniker; those are refused as accessor
                # return values (see conflicting_locals).
                local, local_mod = {}, {}
                self.scan_service_handles(rel, c, ins, env, new, local_mod)
                self._tstores = set()
                self._blocked = set()
                # sweep 0 only measures which stores are tainted; its findings
                # go to a throwaway Env so nothing is recorded before the
                # blocked set below is known.
                self.scan_code(rel, qual, c, ins, {}, env, Env(env.census),
                               emit=False, local_mod=local_mod)
                self._blocked = self.conflicting_locals(c, ins)
                for sweep in (1, 2):
                    self.scan_code(rel, qual, c, ins, local, env, new,
                                   emit=(sweep == 2), local_mod=local_mod)
        seen = set()
        for mod, d in sorted(new.accessor.items()):
            for name, svcs in sorted(d.items()):
                if len(svcs) > 1 and name not in seen:
                    seen.add(name)
                    self.ambiguous.append(
                        {'kind': 'accessor', 'name': name,
                         'services': sorted(svcs),
                         'reason': 'returns more than one service',
                         'defined_in': sorted(new.accessor_src[name])})
                elif len(new.def_census.get(name, ())) > 1 and name not in seen:
                    seen.add(name)
                    self.ambiguous.append(
                        {'kind': 'accessor', 'name': name,
                         'services': sorted(svcs),
                         'reason': 'name defined in %d modules, not promoted '
                                   'tree-wide'
                                   % len(new.def_census.get(name, ())),
                         'defined_in': sorted(new.accessor_src[name])})
        seen = set()
        for mod, d in sorted(new.attr_mod.items()):
            for name, svcs in sorted(d.items()):
                if name in seen:
                    continue
                nmods = len(new.attr_census.get(name, ()))
                if len(svcs) > 1 or nmods > 1:
                    seen.add(name)
                    self.ambiguous.append(
                        {'kind': 'attribute', 'name': name,
                         'services': sorted(svcs),
                         'reason': ('resolves to %d services' % len(svcs))
                                   if len(svcs) > 1 else
                                   ('stored in %d modules, not promoted '
                                    'tree-wide' % nmods),
                         'stored_in': sorted(
                             m.replace(os.sep, '/')
                             for m, dd in new.attr_mod.items() if name in dd)})
        return new

    # -- resolve one chain --------------------------------------------
    def resolve(self, rel, c, ins, i, local, env, local_mod=None):
        """Resolve a tainted expression starting at ins[i].

        Returns (service, how, end_index, extra) or None.  `end_index` is the
        last instruction of the tainted expression.
        """
        if ins[i][1] not in _BASE_LOADS:
            return None
        base_kind, base = ins[i][1], ins[i][3]
        names, pos, called = [], i, False
        # The module the current receiver denotes, when that is knowable
        # exactly: an import alias at the base, or an sm.GetService('x') step.
        local_mod = local_mod or {}
        if base_kind in ('LOAD_GLOBAL', 'LOAD_NAME'):
            recv = env.aliases.get(rel, {}).get(base)
        else:
            recv = local_mod.get(base)
        prev_recv = recv

        while True:
            full = ([base] + names)[-1]
            if not called:
                if not names and base_kind in ('LOAD_FAST', 'LOAD_DEREF') \
                        and base in local:
                    return local[base], 'local', pos, None
                if not names and base_kind in ('LOAD_GLOBAL', 'LOAD_NAME'):
                    g = env.glob.get(rel, {}).get(base)
                    if g and len(g) == 1:
                        return next(iter(g)), 'module-global', pos, None
                if names:
                    hit = env.attr_service(rel, names[-1])
                    if hit:
                        return hit[0], hit[1], pos, None
                    # `self.shipAccess[0]` -- a moniker kept inside a list
                    if pos + 2 < len(ins) and ins[pos + 1][1] == 'LOAD_CONST' \
                            and ins[pos + 2][1] == 'BINARY_SUBSCR':
                        k = c.consts[ins[pos + 1][2]]
                        if isinstance(k, int):
                            svc = env.container_service(rel, names[-1], k)
                            if svc:
                                return svc, 'container-index', pos + 2, None
                # sm.GetService('gameui') / sm.services['gameui'] names a module
                if names and names[-1] in _GETSERVICE and env.svc_module:
                    step = self._service_step(c, ins, pos, env, self._idx)
                    if step:
                        recv, pos = step
                        prev_recv, names, called = recv, [], False
                        base_kind, base = 'LOAD_GLOBAL', '<service>'
                        continue
                if full == 'Moniker':
                    # Moniker(<const serviceName>, <bindParams>)
                    if pos + 1 < len(ins) and ins[pos + 1][1] == 'LOAD_CONST':
                        k = c.consts[ins[pos + 1][2]]
                        if isinstance(k, bytes):
                            call = match_call(ins, pos + 1, self._idx)
                            if call:
                                return (k.decode('utf8', 'replace'), 'moniker',
                                        call[0],
                                        {'bind_params': render_args(
                                            c, ins, pos + 2, call[0])})
                    return None
            else:
                hit = env.accessor_service(rel, full, prev_recv)
                if hit:
                    return hit[0], hit[1], pos, None

            nxt = ins[pos + 1] if pos + 1 < len(ins) else None
            if nxt is not None and nxt[1] == 'LOAD_ATTR':
                pos += 1
                prev_recv = recv
                recv = env.svc_attr.get(rel, {}).get(nxt[3])
                if recv is None and not names and base_kind == 'LOAD_FAST' \
                        and nxt[3] in env.svc_module \
                        and rel not in env.attr_census.get(nxt[3], ()):
                    # CoreService.__getattr__ resolves an unknown attribute
                    # that names a service to sm.GetService(name), so on a
                    # service `self.godma` *is* the godma service.  Only when
                    # this module never stores an attribute of that name.
                    recv = env.svc_module[nxt[3]]
                names.append(nxt[3])
                called = False
                continue
            call = match_call(ins, pos + 1, self._idx)
            if call is None:
                return None
            pos = call[0]
            called = True

    @staticmethod
    def _service_step(c, ins, pos, env, idx=None):
        """`sm.GetService('x')` / `sm.services['x']` -> (module, end_index)."""
        if pos + 1 >= len(ins) or ins[pos + 1][1] != 'LOAD_CONST':
            return None
        k = c.consts[ins[pos + 1][2]]
        if not isinstance(k, bytes):
            return None
        mod = env.svc_module.get(k.decode('utf8', 'replace'))
        if not mod:
            return None
        if ins[pos][3] == 'services':
            if pos + 2 < len(ins) and ins[pos + 2][1] == 'BINARY_SUBSCR':
                return mod, pos + 2
            return None
        call = match_call(ins, pos + 1, idx)
        return (mod, call[0]) if call else None

    # -- the driver ----------------------------------------------------
    def scan_service_handles(self, rel, c, ins, env, new, local_mod):
        """Record `x = sm.GetService('y')` so `x.Foo()` names y's module."""
        for i in range(len(ins)):
            if ins[i][1] != 'LOAD_ATTR' or ins[i][3] not in _GETSERVICE:
                continue
            step = self._service_step(c, ins, i, env, self._idx)
            if not step:
                continue
            mod, end = step
            nxt = ins[end + 1] if end + 1 < len(ins) else None
            if nxt is None:
                continue
            if nxt[1] in ('STORE_FAST', 'STORE_NAME', 'STORE_DEREF'):
                local_mod[nxt[3]] = mod
            elif nxt[1] in _BASE_LOADS:
                j = end + 2
                while j < len(ins) and ins[j][1] == 'LOAD_ATTR':
                    j += 1
                if j < len(ins) and ins[j][1] == 'STORE_ATTR':
                    new.svc_attr[rel][ins[j][3]] = mod

    def scan_code(self, rel, qual, c, ins, local, env, new, emit,
                  local_mod=None):
        for i in range(len(ins)):
            hit = self.resolve(rel, c, ins, i, local, env, local_mod)
            if hit is None:
                continue
            svc, how, end, extra = hit
            if how == 'moniker' and emit:
                self.bind_sites[svc].append({
                    'module': rel.replace(os.sep, '/'),
                    'function': qual,
                    'offset': ins[i][0],
                    'bind_params': (extra or {}).get('bind_params'),
                })
            nxt = ins[end + 1] if end + 1 < len(ins) else None
            if nxt is None:
                continue
            op, rep = nxt[1], nxt[3]

            # `self.x = <tainted>` compiles to <value...>, LOAD_FAST self,
            # [LOAD_ATTR ...], STORE_ATTR x -- the object is pushed *after*
            # the value, so look past the receiver chain.
            if op in _BASE_LOADS:
                j = end + 2
                while j < len(ins) and ins[j][1] == 'LOAD_ATTR':
                    j += 1
                if j < len(ins) and ins[j][1] == 'STORE_ATTR':
                    new.attr_mod[rel].setdefault(ins[j][3], set()).add(svc)
                    continue

            # `self.x = [<tainted>, ...]` / unpacking: remember the slot.
            built = match_container(ins, end + 1, self._idx)
            if built:
                bidx, slot = built
                j = bidx + 1
                if j < len(ins) and ins[j][1] in _BASE_LOADS:
                    j += 1
                    while j < len(ins) and ins[j][1] == 'LOAD_ATTR':
                        j += 1
                    if j < len(ins) and ins[j][1] == 'STORE_ATTR':
                        new.cont[rel][ins[j][3]].setdefault(
                            slot, set()).add(svc)
                        continue

            if op == 'LOAD_ATTR':
                if not self.is_method(rep):
                    continue
                call = match_call(ins, end + 2, self._idx)
                site = {'module': rel.replace(os.sep, '/'), 'function': qual,
                        'offset': nxt[0], 'how': how, 'args': arity_str(call)}
                if emit:
                    self.record(svc, rep, site)
                # the result of a bound call may itself be a bound object
                if call is not None:
                    nx2 = ins[call[0] + 1] if call[0] + 1 < len(ins) else None
                    if nx2 is not None and nx2[1] == 'LOAD_ATTR' \
                            and self.is_method(nx2[3]):
                        c2 = match_call(ins, call[0] + 2, self._idx)
                        if emit:
                            self.record_second(
                                '%s->%s' % (svc, rep), nx2[3],
                                {'module': rel.replace(os.sep, '/'),
                                 'function': qual, 'offset': nx2[0],
                                 'how': 'second-level',
                                 'args': arity_str(c2)})
                continue

            if op == 'RETURN_VALUE':
                # `__getattr__` returning a moniker makes the guarded name an
                # attribute of this module, however it was spelled.
                hit_guard = False
                for lo, hi, gnames in self._guards:
                    if lo <= end < hi:
                        hit_guard = True
                        for nm in gnames:
                            new.attr_mod[rel].setdefault(nm, set()).add(svc)
                base_local = (ins[end][1] in ('LOAD_FAST', 'LOAD_NAME',
                                              'LOAD_DEREF')
                              and ins[end][3] in self._blocked)
                if not hit_guard and not base_local:
                    fn = qual.rsplit('.', 1)[-1]
                    new.accessor[rel].setdefault(fn, set()).add(svc)
                    new.accessor_src[fn].add(
                        '%s::%s' % (rel.replace(os.sep, '/'), qual))
                continue

            kind = _STORES.get(op)
            if kind in ('local', 'name', 'global'):
                self._tstores.add(end + 1)
            if kind == 'local':
                local[rep] = svc
            elif kind == 'name':
                if qual == '<module>':
                    new.glob[rel].setdefault(rep, set()).add(svc)
                else:
                    local[rep] = svc
            elif kind == 'global':
                new.glob[rel].setdefault(rep, set()).add(svc)
            elif kind == 'attr':
                new.attr_mod[rel].setdefault(rep, set()).add(svc)


# --------------------------------------------------------------------------
# docs/reference/bound-objects.md comparison (parsed, never transcribed)
# --------------------------------------------------------------------------
_H4 = re.compile(r'^####\s+(?P<svc>[A-Za-z_0-9]+)'
                 r'(?P<arrow>\s*(?:->|→)\s*[^—-]+?)?'
                 r'\s*—\s*(?P<n>\d+)')
_ROW = re.compile(r'^\|\s*`(?P<svc>[A-Za-z_0-9]+)`\s*\|\s*(?P<n>\d+)\s*\|'
                  r'(?P<rest>.*)\|\s*$')
_TOTALS = re.compile(r'\*\*(?P<m>\d+)\s+distinct method names across\s+'
                     r'(?P<s>\d+)\s+bound services')
# A documented method name is one written as a call -- `Foo`(2) or `Foo(bar)` --
# or standing alone in a table cell.  Bare prose mentions are deliberately not
# collected: the document also names client-local methods in running text.
_TOK = (re.compile(r'`([A-Z][A-Za-z_0-9]*)`?\s*\('),
        re.compile(r'`([A-Z][A-Za-z_0-9]*)`\s*\|'))
_REMAINING = re.compile(r'^####\s+Remaining bound services')


def _tokens(s):
    out = set()
    for pat in _TOK:
        out |= set(pat.findall(s))
    return out


def parse_doc(path):
    """Extract per-service documented counts and explicitly named methods."""
    text = open(path, encoding='utf8').read()
    doc, cur, remaining = {}, None, False
    for ln in text.splitlines():
        if _REMAINING.match(ln):
            cur, remaining = None, True
            continue
        m = _H4.match(ln)
        if m:
            key = m.group('svc')
            if m.group('arrow'):
                key += '->' + m.group('arrow').strip(' -→').strip()
            cur = doc.setdefault(key, {'count': int(m.group('n')),
                                       'named': set()})
            remaining = False
            continue
        if ln.startswith('####'):
            cur, remaining = None, False
            continue
        if remaining:
            m = _ROW.match(ln)
            if m:
                e = doc.setdefault(m.group('svc'),
                                   {'count': int(m.group('n')), 'named': set()})
                e['count'] = int(m.group('n'))
                e['named'] |= _tokens(m.group('rest'))
            continue
        if cur is not None and ln.strip():
            cur['named'] |= _tokens(ln)
    tm = _TOTALS.search(text)
    totals = ({'methods': int(tm.group('m')), 'services': int(tm.group('s'))}
              if tm else None)
    # S2.1: the moniker service-name list
    names = set()
    m = re.search(r'constructor sites only\), (\d+) service names:\n+(.*?)\n\n',
                  text, re.S)
    if m:
        names = set(re.findall(r'`([A-Za-z_0-9]+)`', m.group(2)))
    return doc, totals, names


def compare(doc_path, services, bind_sites, second):
    doc, totals, doc_names = parse_doc(doc_path)
    out = {'source': doc_path, 'documented_totals': totals,
           'documented_service_names': sorted(doc_names)}
    found_names = sorted(bind_sites)
    out['bind_site_services'] = {
        'documented_only': sorted(doc_names - set(found_names)),
        'scanned_only': sorted(set(found_names) - doc_names),
    }
    rows = []
    for key in sorted(doc):
        svc = key.split('->')[0]
        if '->' in key:
            got = {m for k, ms in second.items()
                   if k.split('->')[0] == svc for m in ms}
        else:
            got = set(services.get(key) or services.get(svc) or {})
        named = doc[key]['named'] - {'UnBind'}
        rows.append({
            'service': key,
            'second_level': '->' in key,
            'documented_count': doc[key]['count'],
            'scanned_count': len(got),
            'scanned_count_direct_service': len(services.get(svc) or {}),
            'delta': len(got) - doc[key]['count'],
            'documented_named_not_scanned': sorted(named - got),
            'scanned_not_documented_by_name': sorted(got - named)
                if len(named) >= doc[key]['count'] else [],
            'doc_enumerates_all': len(named) >= doc[key]['count'],
        })
    out['per_service'] = rows
    out['scanned_services_not_in_doc'] = sorted(
        s for s in services if s not in doc and s.split('->')[0] not in doc)
    out['scanned_totals'] = {
        'services': len(services),
        'methods': len({(s, m) for s, ms in services.items() for m in ms}),
        'distinct_method_names': len({m for ms in services.values() for m in ms}),
    }
    return out


# --------------------------------------------------------------------------
def to_json(table):
    return {
        svc: {m: {'arities': sorted(e['arities']),
                  'call_sites': len(e['sites']),
                  'how': sorted({s['how'] for s in e['sites']}),
                  'sites': sorted(e['sites'],
                                  key=lambda s: (s['module'], s['function'],
                                                 s['offset']))}
              for m, e in sorted(ms.items())}
        for svc, ms in sorted(table.items())
    }


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog='scan_bound.py',
        description='Enumerate bound-object services and methods from the '
                    'client bytecode in out/pyc (see module docstring).',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog='Run inside the analysis container:\n'
               '  tools/analysis.sh python3 tools/scan_bound.py '
               '-o findings/bound-objects.json --compare docs/reference/bound-objects.md')
    ap.add_argument('--pyc-root', default='out/pyc',
                    help='tree of extracted 2.7 .pyc files (default: out/pyc)')
    ap.add_argument('-o', '--output', metavar='FILE',
                    help='write the structured result as JSON')
    ap.add_argument('--compare', metavar='MARKDOWN',
                    help='diff the result against a docs/reference/bound-objects.md')
    ap.add_argument('--service', metavar='NAME', action='append',
                    help='print full per-site provenance for this service')
    ap.add_argument('--max-passes', type=int, default=8,
                    help='fixpoint iteration cap (default: 8)')
    args = ap.parse_args(argv)

    if not os.path.isdir(args.pyc_root):
        ap.error('no such pyc tree: %s (run tools/extract_code.py first)'
                 % args.pyc_root)

    tree, failed = load_tree(args.pyc_root)
    local_api, local_api_src = derive_local_api(tree)
    if not local_api:
        ap.error('could not derive the Moniker local API from %s -- is this '
                 'the right pyc tree?' % args.pyc_root)

    attr_census, def_census, base_census = name_census(tree)
    aliases, svc_module = module_index(tree, base_census)
    census = {'attr': attr_census, 'def': def_census, 'base': base_census,
              'aliases': aliases, 'svc_module': svc_module}
    scan = Scan(tree, local_api)
    env = Env(census)
    passes, seen = 0, set()
    while passes < args.max_passes:
        passes += 1
        env = scan.run(env)
        sig = env.signature()
        if sig in seen:
            break
        seen.add(sig)
    else:
        print('warning: no fixpoint after %d passes' % passes, file=sys.stderr)

    services = scan.methods
    nsvc = len(services)
    npairs = len({(s, m) for s, ms in services.items() for m in ms})
    nnames = len({m for ms in services.values() for m in ms})

    result = {
        'generated_by': 'tools/scan_bound.py',
        'establishes': 'bound-object (util.Moniker) services and the methods '
                       'the client calls on them, derived from client bytecode',
        'pyc_root': args.pyc_root,
        'modules_parsed': len(tree),
        'modules_unparseable': [{'module': m.replace(os.sep, '/'),
                                 'error': e} for m, e in sorted(failed)],
        'fixpoint_passes': passes,
        'moniker_local_api': {
            'note': 'uppercase attributes handled locally by Moniker / '
                    'MachoObjectConnection, therefore not remote methods; '
                    'derived from the client, not hardcoded',
            'names': {n: sorted(set(local_api_src.get(n, [])))
                      for n in sorted(local_api)},
        },
        'summary': {
            'moniker_service_names': len(scan.bind_sites),
            'services_with_methods': nsvc,
            'service_method_pairs': npairs,
            'distinct_method_names': nnames,
        },
        'moniker_service_names': sorted(scan.bind_sites),
        'bind_sites': {s: sorted(v, key=lambda d: (d['module'], d['function'],
                                                   d['offset']))
                       for s, v in sorted(scan.bind_sites.items())},
        'services': to_json(services),
        'derived_second_level': {
            'note': 'uppercase attribute taken on the RESULT of a bound call. '
                    'Suggestive, not evidence: the result may be plain data. '
                    'Not included in the headline totals.',
            'objects': to_json(scan.second),
        },
        'promoted_names': {
            'note': 'the load-bearing heuristic: names allowed to carry taint '
                    'across module boundaries because the name occurs in '
                    'exactly one module of the whole tree. Audit this list.',
            'rules': {
                'accessor_unique_name': 'function name defined in exactly one '
                                        'module of the tree',
                'accessor_by_module': 'receiver identified exactly, by import '
                                      'alias or sm.GetService(name)',
                'attribute': 'STORE_ATTR target in exactly one module',
                'container': 'moniker stored at a fixed index of a list/tuple '
                             'attribute, read back by that same index',
            },
            'promoted': env.promoted(),
        },
        'ambiguities_dropped': sorted(
            scan.ambiguous, key=lambda d: (d['kind'], d['name'])),
        'services_with_bind_site_but_no_methods': sorted(
            set(scan.bind_sites) - set(services)),
    }

    if args.compare:
        result['comparison'] = compare(args.compare, services,
                                       scan.bind_sites, scan.second)

    # ---- stdout summary -------------------------------------------------
    print('modules parsed: %d (unparseable: %d), fixpoint in %d passes'
          % (len(tree), len(failed), passes))
    print('moniker local API excluded (%d): %s'
          % (len(local_api), ', '.join(sorted(local_api))))
    print()
    print('=== bound services: %d with methods, %d moniker names, '
          '%d service/method pairs, %d distinct method names ==='
          % (nsvc, len(scan.bind_sites), npairs, nnames))
    for svc in sorted(services, key=lambda s: (-len(services[s]), s)):
        ms = services[svc]
        hows = sorted({h for e in ms.values() for h in
                       {s['how'] for s in e['sites']}})
        print('  %-30s %3d methods  [%s]' % (svc, len(ms), ','.join(hows)))
    noneyet = sorted(set(scan.bind_sites) - set(services))
    if noneyet:
        print('\nmoniker constructed but no method call seen (%d): %s'
              % (len(noneyet), ', '.join(noneyet)))
    if scan.second:
        print('\nsecond-level candidates (not counted): %s'
              % ', '.join('%s=%d' % (k, len(v))
                          for k, v in sorted(scan.second.items())))
    if scan.ambiguous:
        print('\nambiguous names dropped: %s'
              % ', '.join('%s(%s)' % (a['name'], a['kind'])
                          for a in result['ambiguities_dropped']))

    for want in (args.service or []):
        print('\n=== %s ===' % want)
        for m, e in sorted(services.get(want, {}).items()):
            print('  %-40s args=%s' % (m, '|'.join(sorted(e['arities']))))
            for s in sorted(e['sites'], key=lambda s: (s['module'], s['offset'])):
                print('      %s::%s @%d  how=%s args=%s'
                      % (s['module'], s['function'], s['offset'], s['how'],
                         s['args']))

    if args.compare:
        cmp = result['comparison']
        print('\n=== vs %s ===' % args.compare)
        print('documented totals: %s' % (cmp['documented_totals'],))
        print('scanned totals:    %s' % (cmp['scanned_totals'],))
        bs = cmp['bind_site_services']
        print('moniker names documented but not scanned: %s'
              % (bs['documented_only'] or 'none'))
        print('moniker names scanned but not documented: %s'
              % (bs['scanned_only'] or 'none'))
        print('%-34s %5s %5s %6s' % ('service', 'doc', 'scan', 'delta'))
        for r in cmp['per_service']:
            note = '' if r['doc_enumerates_all'] else '  (doc partial)'
            if r['second_level']:
                note += '  (second-level; direct service scan=%d)' \
                    % r['scanned_count_direct_service']
            print('  %-32s %5d %5d %+6d%s'
                  % (r['service'], r['documented_count'], r['scanned_count'],
                     r['delta'], note))
            if r['documented_named_not_scanned']:
                print('        documented, NOT scanned: %s'
                      % ', '.join(r['documented_named_not_scanned']))
            if r['scanned_not_documented_by_name']:
                print('        scanned, NOT documented: %s'
                      % ', '.join(r['scanned_not_documented_by_name']))
        if cmp['scanned_services_not_in_doc']:
            print('services scanned with no section in the doc: %s'
                  % ', '.join(cmp['scanned_services_not_in_doc']))

    if args.output:
        with open(args.output, 'w', encoding='utf8') as f:
            json.dump(result, f, indent=1, sort_keys=False)
            f.write('\n')
        print('\nwrote %s' % args.output)
    return 0


if __name__ == '__main__':
    sys.exit(main())
