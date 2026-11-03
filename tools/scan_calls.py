# -*- coding: utf-8 -*-
"""Census of machoNet service call sites in the EVE V24.01 client bytecode.

A symbolic-stack machine over every Python 2.7 code object in out/pyc, plus a
structural pass for the guard around each call. Read-only; prints JSON.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import os, sys, json, collections
# `tools/` itself is already on the path -- the rotation above put it last,
# behind the stdlib, which is the whole point. See that comment.
from pyc27 import load_pyc, walk, dis, Code

# The tree, the start.ini that names the release line, the bound-object seed and
# the output come from the environment so tools/port-surface.sh can run this on
# two builds with one commit of the tool; the defaults are the pinned build.
ROOT = os.environ.get('SCAN_PYC', '/work/out/pyc')
SITES_OUT = os.environ.get('SCAN_SITES_OUT', '/scratch/sites.json')
BOUND_SEED = os.environ.get('SCAN_BOUND_SEED', '/work/findings/bound-objects.json')


def prefix_mark(ini=os.environ.get('SCAN_START_INI', '/client/start.ini'), default='release\\V24.01\\'):
    """The build-path prefix in code.ccp filenames, from start.ini's `branch`
    (`//eve/branches/release/V24.01` -> `release\\V24.01\\`), so a new release
    line does not silently empty the call_site column. Falls back to the
    V24.01 literal when there is no start.ini or no branch."""
    try:
        with open(ini, encoding='utf8', errors='replace') as fh:
            for line in fh:
                k, _, v = line.partition('=')
                if k.strip().lower() == 'branch' and '/branches/' in v:
                    tail = v.strip().split('/branches/', 1)[1].strip('/')
                    return tail.replace('/', '\\') + '\\'
    except OSError:
        pass
    return default


PREFIX_MARK = prefix_mark()
CMP = ('<','<=','==','!=','>','>=','in','not in','is','is not','exception match','BAD')
JUMP_ABS = ('JUMP_ABSOLUTE','POP_JUMP_IF_FALSE','POP_JUMP_IF_TRUE',
            'JUMP_IF_FALSE_OR_POP','JUMP_IF_TRUE_OR_POP','CONTINUE_LOOP')
JUMP_REL = ('JUMP_FORWARD','FOR_ITER','SETUP_LOOP','SETUP_EXCEPT','SETUP_FINALLY','SETUP_WITH')
COND_JUMP = ('POP_JUMP_IF_FALSE','POP_JUMP_IF_TRUE','JUMP_IF_FALSE_OR_POP','JUMP_IF_TRUE_OR_POP')

ROWSET_HINTS = ('Rowset','Recordset','RowLists','RowDict','DBRow')
# Attribute reads that only a real rowset answers (eve/common/script/sys/rowset.py).
ROWSET_ATTRS = set('''Index Filter InsertNew GetKeyColumn GetLine GetLines
GetIndexKeys UpdateLI ReIndex RenameField AddField RemoveField MakeImmutable
IsImmutable header lines columns RowClass __columns__ __header__'''.split())
NATIVE_ROOTS = ('blue','destiny','trinity','bloodyPlugin','bluepy','_blue','_trinity')
SVC_FACTORY = ('RemoteSvc','ProxySvc')
NATIVE_ATTR = set()
SEED_ATTR = {}
# Accessor names learned in *another* file. `scan_file` learns which functions
# and attributes hold a service handle from the file it is reading, which is why
# `GetSkillHandler()` resolves in `skillsvc.py` and nowhere else: 9 of the 29
# methods on the skillMgr2 skill handler -- `PurchaseSkills` among them -- are
# called from other modules, and had no row at all. These two tables are filled
# by one learning sweep over the whole tree before the sweep that collects
# sites. A name goes in only if `promotable()` accepts it.
GLOBAL_FUNC = {}
GLOBAL_ATTR = {}
GENERIC_NAMES = set('''GetMoniker GetService Get GetMgr GetManager GetSvc GetHandler
GetRemote GetObject GetLM GetProxy GetClient GetServer GetSession GetInstance
GetLocation GetAccess GetRegistry GetBroker GetSelf GetMe GetData GetState
get_service get_moniker get_handler get_mgr'''.split())

def promotable(name, where):
    """Is this accessor name safe to believe in *another* module?

    `func_svc` is keyed by bare name with no receiver, so promoting a name makes
    every call of that name in the client resolve to one service. That is the
    over-claim direction, so three brakes: the name must not be a generic one
    that many classes define, it must be long enough to be a real accessor, and
    it must have resolved to the *same* service everywhere it was seen.
    """
    if name in GENERIC_NAMES: return False
    if len(name.lstrip('_')) < 8: return False
    return len(where) <= 3
PROBE = set()
PROBE_OUT = []
# Methods of carbon moniker.Moniker / MonikerCallWrap: local, never on the wire.
MONIKER_LOCAL = set('''SetSessionCheck ToURLString FromURLString Bind MonikeredCall
QuickResolve Resolve Unresolve Unbind GetNodeID GetSelfLocal GetMoniker
__getstate__ __setstate__'''.split())
# Python container/str methods: a service handle that reaches one of these was
# mis-propagated, so the call is not on the wire.
PY_LOCAL = set('''append extend insert remove pop clear copy get keys values items
iteritems itervalues iterkeys has_key update setdefault sort sorted reverse add
discard count index join split strip lstrip rstrip lower upper replace startswith
endswith format encode decode find rfind title capitalize next close read write
write_text send throw iteritems_safe difference union intersection'''.split())
# Builtin type names: `x is tuple` is a type check, never a cache sentinel.
PY_TYPES = set('''tuple list dict set frozenset str unicode basestring bytes int
long float bool complex object type file buffer NotImplemented Ellipsis'''.split())
MEMO_DECOS = ('Memoize','MemoizeWithLock','Memoized','memoize','MemoizeAndTimeout')
# In 2.7 a *list* comprehension is compiled inline into the enclosing code
# object; a dict, set or generator comprehension gets its own code object,
# called with the iterator as `.0` and reaching everything else as a free
# variable. These are the three names those code objects carry.
COMP_NAMES = (b'<dictcomp>', b'<setcomp>', b'<genexpr>')
import re as _re
# A service method whose *return value* is itself a bound object the client then
# calls methods on. Tight on purpose: the suffix list is what makes the rule
# auditable, and every row it produces says `bound` in `kind`.
BIND_SHAPE = _re.compile(
    r'^(?:Get|Char ?Get|Macho)\w*'
    r'(?:Handler|Mgr|Manager|Access|Location|Object|Svc|Service|Broker|Editor'
    r'|Ballpark|Park|Inventory|InventoryFromId|LM|Registry|Moniker)$')

def linemap(c):
    tab = c.lnotab or b''
    line = c.firstlineno; off = 0
    m = [(0, line)]
    for i in range(0, len(tab) - 1, 2):
        off += tab[i]; line += tab[i+1]; m.append((off, line))
    return m

def line_at(m, off):
    cur = m[0][1]
    for o, l in m:
        if o <= off: cur = l
        else: break
    return cur

def srcpath(c):
    f = (c.filename or b'').decode('utf8','replace')
    i = f.find(PREFIX_MARK)
    if i >= 0: f = f[i+len(PREFIX_MARK):]
    return f.replace('\\','/')

def V(d, svc=None, bm=None, origin=None, indirect=None, site=None, deco=None,
      isfunc=False, natv=False, code=None, via=None):
    return {'d': d, 'svc': svc, 'bm': bm, 'origin': origin, 'indirect': indirect,
            'site': site, 'deco': deco, 'isfunc': isfunc, 'natv': natv, 'code': code,
            'via': via}

# A stack value can carry *several* pending sites, not one. The case that needs
# it is two branches storing to the same local:
#
#     if typeID == typePlex: orders = proxy.GetPlexOrders()
#     else:                  orders = proxy.GetOrders(typeID)
#     asks, bids = orders[0], orders[1]
#
# both stores tag the name `orders`, and the single `LOAD_FAST orders` that
# follows has to carry both sites or one of them never reaches `orders[0]`.
def sites_of(v):
    s = v.get('site')
    if s is None: return ()
    return tuple(s) if isinstance(s, list) else (s,)

def add_site(v, s):
    cur = v.get('site')
    if cur is None: v['site'] = s
    elif isinstance(cur, list):
        if s not in cur: cur.append(s)
    elif cur is not s: v['site'] = [cur, s]

class Site(object):
    __slots__ = ('service','method','path','line','kind','consumer','guard','notes',
                 'nargs','opidx','off','resolved','func','store_key','indirect',
                 'in_comp','comp_latch')
    def __init__(self, **kw):
        for k in self.__slots__: setattr(self, k, kw.get(k))
        self.notes = list(kw.get('notes') or [])
        self.resolved = False; self.store_key = None
        self.comp_latch = None

# ------------------------------------------------------------------ guards
def jump_target(off, nm, arg):
    if nm in JUMP_ABS: return arg
    if nm in JUMP_REL: return off + 3 + arg
    return None

def handler_excs(ops, idx):
    out = []; pend = []; j = idx; lim = min(len(ops), idx + 40)
    while j < lim:
        off, nm, arg, rep = ops[j]
        if nm in ('LOAD_GLOBAL','LOAD_NAME','LOAD_ATTR','LOAD_FAST','LOAD_DEREF'):
            pend.append(rep)
        elif nm == 'COMPARE_OP' and arg == 10:
            out.extend(pend); break
        elif nm == 'POP_TOP' and not pend:
            out.append('*'); break
        elif nm in ('STORE_FAST','STORE_NAME','CALL_FUNCTION','SETUP_EXCEPT',
                    'SETUP_FINALLY','RETURN_VALUE'):
            break
        j += 1
    return out or ['*']

def blocks_at(ops, target_idx):
    st = []
    for k in range(target_idx):
        off, nm, arg, rep = ops[k]
        if nm in ('SETUP_EXCEPT','SETUP_FINALLY','SETUP_LOOP','SETUP_WITH'):
            st.append((nm, jump_target(off, nm, arg), off))
        elif nm == 'POP_BLOCK':
            if st: st.pop()
    return st

def finally_returns(ops, tgt_off):
    i = None
    for k, op in enumerate(ops):
        if op[0] == tgt_off: i = k; break
    if i is None: return False
    depth = 0
    while i < len(ops):
        nm = ops[i][1]
        if nm in ('SETUP_EXCEPT','SETUP_FINALLY','SETUP_LOOP','SETUP_WITH'): depth += 1
        elif nm == 'POP_BLOCK' and depth: depth -= 1
        elif nm == 'END_FINALLY' and depth == 0: return False
        elif nm == 'RETURN_VALUE' and depth == 0: return True
        i += 1
    return False

def is_none_op(op):
    """`None`, however this build spells it.

    Most of the tree has `LOAD_CONST None`, but some modules -- including
    `createContractWnd.py`, which holds one of the four live locks in
    `docs/guides/debugging-the-client.md` §4 -- compile it as `LOAD_GLOBAL None`.
    Testing only for the const read `getattr(self, 'n', None) is None` as an
    identity test against a *sentinel*, which is nearly right and still the
    wrong label on the row a reader goes to that section for.
    """
    return ((op[1] == 'LOAD_CONST' and op[3] == 'None') or
            (op[1] in ('LOAD_GLOBAL','LOAD_NAME') and op[3] == 'None'))

def expr_parts(ops, a, b):
    parts = []
    for off, nm, arg, rep in ops[a:b]:
        if nm in ('LOAD_FAST','LOAD_DEREF','LOAD_NAME','LOAD_GLOBAL'):
            if rep == 'None': continue          # an operand, not a subject
            parts.append(rep)
        elif nm == 'LOAD_ATTR':
            if parts: parts[-1] += '.' + rep
            else: parts.append(rep)
        elif nm.startswith('CALL_FUNCTION'):
            # Name the *callee*, not its last argument: the arguments were
            # pushed after it, so drop them. Without this,
            # `if not self.cache.Get(key):` reads `guard on key()`, and the
            # `result stored back to the guarded name` note cannot match.
            a_ = (arg or 0) & 0xff; k_ = ((arg or 0) >> 8) & 0xff
            parts = parts[:max(1, len(parts) - a_ - 2 * k_)]
            if parts: parts[-1] += '()'
    return parts

def expr_text(ops, a, b):
    parts = expr_parts(ops, a, b)
    return parts[-1] if parts else ''


# Opcodes that end a statement. A clause of a boolean test contains none of
# them, which is what lets one clause be isolated from the next -- and what
# tells a real short-circuit chain from an `if` body that merely ended just
# before the call.
STMT_OPS = ('POP_TOP','STORE_FAST','STORE_NAME','STORE_ATTR','STORE_GLOBAL','STORE_DEREF',
            'STORE_SUBSCR','STORE_MAP','POP_BLOCK','JUMP_ABSOLUTE','JUMP_FORWARD',
            'SETUP_LOOP','SETUP_EXCEPT','SETUP_FINALLY','SETUP_WITH','FOR_ITER',
            'RETURN_VALUE','END_FINALLY','YIELD_VALUE','BREAK_LOOP','RAISE_VARARGS',
            'UNPACK_SEQUENCE','IMPORT_NAME','LIST_APPEND','SET_ADD','MAP_ADD',
            'MAKE_FUNCTION','MAKE_CLOSURE','PRINT_ITEM','PRINT_NEWLINE','EXEC_STMT',
            'DELETE_FAST','DELETE_NAME','DELETE_ATTR','WITH_CLEANUP')

def clause_span(ops, k):
    """`[start, k)` -- the expression the conditional jump at `k` tests."""
    start = k
    for q in range(k - 1, max(-1, k - 25), -1):
        if ops[q][1] in STMT_OPS or ops[q][1] in COND_JUMP:
            start = q + 1; break
        start = q
    return start

def clause_chain(ops, k0):
    """The conditional jumps of one compound boolean test, in source order.

    **This is the `guard` column's blind spot.** `if not self.cache:` is one
    conditional jump; `if not self.cache and foo:` and `if self.cache is None
    or stale:` are *two*, with nothing but expression opcodes between them --
    and the one nearest the call is the clause that is not the cache test. A
    detector that looks at that jump alone sees `foo` or `stale`, classifies it
    `cmp`/`truthy`, and reports `none`. So walk the whole chain: a conditional
    jump immediately preceded by another conditional jump is the next clause of
    the same test.
    """
    out = []; k = k0
    while k is not None and len(out) < 8:
        st = clause_span(ops, k)
        out.append((st, k))
        p = st - 1
        k = p if (p >= 0 and ops[p][1] in COND_JUMP) else None
    out.reverse()
    return out

def clause_role(ops, offmap, k, si, so):
    """How the jump at `k` relates to the call: 'skip', 'early', 'enter', None.

    - `skip`  the jump leaves the guarded region (target past the call).
    - `early` the jump goes to a `return` before the call -- the `if x: return`
      shape, which guards the call by falling through instead.
    - `enter` a short-circuit jump *into* the guarded region, which is how the
      first clause of `A or B` reaches the body.
    """
    off, nm, arg, rep = ops[k]
    tgt = jump_target(off, nm, arg)
    if tgt is None: return None
    if tgt > so: return 'skip'
    ti = offmap.get(tgt)
    if ti is None or ti <= k or ti > si: return None
    seg = [o[1] for o in ops[k+1:ti]]
    # `if x: return` guards the call by falling through -- but only if the
    # jumped-over block cannot *reach* the target by falling through itself.
    # The old test was "contains a RETURN_VALUE and no call or store", which
    # missed `if x: log(); return` and accepted
    # `if x:` / `  if y: return` / `  bar()`, where the call after is not
    # guarded at all. The sound test is that the block ends in a transfer that
    # does not simply go to this target -- because 2.7 ends *every* `if` body
    # with a jump to the join point, often `JUMP_FORWARD 0`. Reading that as an
    # exit put `if_x_is_none` on `beyonce.GetFormations`
    # (`michelle.py:452`), whose real shape is
    # `if self.__bp is not None: self.RemoveBallpark()` and then the call on
    # both paths. A wrong guard on that row is worse than no guard: it is the
    # one row in this file that decides a black screen.
    if not seg: return None
    # Skip the trailing jump 2.7 writes at the end of every `if` body, which
    # goes to this very target and is dead code when the body already
    # returned: `return member` compiles to RETURN_VALUE then JUMP_FORWARD 0.
    j = ti - 1
    while (j > k and ops[j][1] in ('JUMP_FORWARD','JUMP_ABSOLUTE') and
           jump_target(ops[j][0], ops[j][1], ops[j][2]) == tgt):
        j -= 1
    last = ops[j]
    if last[1] in ('RETURN_VALUE','RAISE_VARARGS','BREAK_LOOP'): return 'early'
    if last[1] in ('JUMP_ABSOLUTE','JUMP_FORWARD','CONTINUE_LOOP'):
        if jump_target(last[0], last[1], last[2]) != tgt: return 'early'
        return None
    # Everything between the jump and its target must be the *rest of the
    # condition*, ending in another conditional jump. Without that test,
    # `if self.cache: foo()` followed by an unguarded call reads as `if_not_x`
    # -- a latch the client does not have, which is worse than a gap.
    if seg and seg[-1] in COND_JUMP and not any(s in STMT_OPS for s in seg):
        return 'enter'
    return None

def clause_shape(ops, start, k):
    """(shape, expression text) for one clause of a test.

    Shapes: `is_none`, `is_not_none`, `is_sentinel`, `is_not_sentinel`, `in`,
    `not_in`, `cmp`, `not_truthy`, `truthy`.
    """
    seg = ops[start:k]
    cmps = [o[2] for o in seg if o[1] == 'COMPARE_OP']
    has_none = any(is_none_op(o) for o in seg)
    parts = expr_parts(ops, start, k)
    txt = parts[-1] if parts else ''
    if has_none and 8 in cmps: return 'is_none', txt
    if has_none and 9 in cmps: return 'is_not_none', txt
    # `if self._school_hq_id is UNCACHED:` (homestation/client/service.py)
    # latches exactly like `is None`, with a module-level sentinel instead.
    # The old detector called it `if_x_is_none`, which is the right conclusion
    # from the wrong evidence -- it never checked for a None at all -- so
    # requiring the None const here would have *lost* the latch. The identity
    # test is the thing that matters; name the operand honestly instead.
    if 8 in cmps or 9 in cmps:
        lhs = parts[0] if parts else ''
        rhs = parts[-1].split('.')[-1] if len(parts) > 1 else ''
        # `if type(itemID) is tuple:` is an identity test too, and it is a type
        # check, not a cache. Both scanners used to call it a cache guard. A
        # sentinel cache tests a *stored* value against a named sentinel, so
        # require a left side that is not a call and a right side that is not a
        # builtin type.
        if lhs and '(' not in lhs and rhs not in PY_TYPES:
            return ('is_sentinel' if 8 in cmps else 'is_not_sentinel'), lhs
        return 'cmp', txt
    # A *keyed* cache: `if itemName not in self.data:` ... `self.data[k] = v`.
    # `accountsvc.GetStaticData` is both this and a dynamic dispatch, and the
    # pair (`if_x_not_in_y`, consumer `iterate`, store after the loop) is the
    # live-lock shape with a `for` where the LP case has a comprehension.
    if 6 in cmps: return 'in', txt
    if 7 in cmps: return 'not_in', txt
    if cmps: return 'cmp', txt
    if any(o[1] == 'UNARY_NOT' for o in seg): return 'not_truthy', txt
    return 'truthy', txt

CACHE_TOKENS = ('if_x_is_none', 'if_not_x', 'if_x_is_sentinel', 'if_x_not_in_y')

def cache_token(shape, runs_when_true):
    """The guard token for a clause, or None if the clause cannot latch.

    `runs_when_true` is whether the call is reached when this clause is true.
    """
    if shape == 'is_none': return 'if_x_is_none' if runs_when_true else None
    if shape == 'is_not_none': return None if runs_when_true else 'if_x_is_none'
    if shape == 'truthy': return None if runs_when_true else 'if_not_x'
    if shape == 'not_truthy': return 'if_not_x' if runs_when_true else None
    if shape == 'is_sentinel': return 'if_x_is_sentinel' if runs_when_true else None
    if shape == 'is_not_sentinel': return None if runs_when_true else 'if_x_is_sentinel'
    if shape == 'in': return None if runs_when_true else 'if_x_not_in_y'
    if shape == 'not_in': return 'if_x_not_in_y' if runs_when_true else None
    return None

def detect_guard(ops, site, with_descs, decos, offmap=None):
    g = []; notes = []
    si, so = site.opidx, site.off
    if offmap is None:
        offmap = {}
        for k, op in enumerate(ops): offmap.setdefault(op[0], k)
    for nm, tgt, soff in blocks_at(ops, si):
        if nm == 'SETUP_EXCEPT':
            hi = None
            for k, op in enumerate(ops):
                if op[0] == tgt: hi = k; break
            g.append('try_except:' + '/'.join(handler_excs(ops, hi) if hi is not None else ['*']))
        elif nm == 'SETUP_FINALLY':
            g.append('finally_return' if finally_returns(ops, tgt) else 'try_finally')
        elif nm == 'SETUP_WITH':
            d = with_descs.get(soff, '')
            if 'Lock' in d or 'lock' in d: g.append('singleflight')
    for d in decos:
        if any(m in d for m in MEMO_DECOS):
            g.append('memoize'); notes.append('@%s' % d)
            if 'Lock' in d: g.append('singleflight')
    cache = None; shapes = None
    k = si - 1
    low = max(-1, si - 80)
    while k > low:
        if ops[k][1] not in COND_JUMP: k -= 1; continue
        chain = clause_chain(ops, k)
        cl = []
        for st, kk in chain:
            role = clause_role(ops, offmap, kk, si, so)
            if role is None: continue
            shape, txt = clause_shape(ops, st, kk)
            runs = ops[kk][1] in ('POP_JUMP_IF_TRUE','JUMP_IF_TRUE_OR_POP')
            if role == 'skip': runs = not runs
            cl.append((shape, txt, runs))
        if cl:
            for shape, txt, runs in cl:
                tok = cache_token(shape, runs)
                if tok and cache is None: cache = (tok, txt)
            if shapes is None and len(cl) > 1:
                shapes = ' and '.join('%s(%s)' % (s, t or '?') for s, t, _ in cl)
            # A chain with no cache clause does not end the search: the call can
            # sit inside `if typeID == plex:` inside `if not self.cache:`, and
            # the outer one is the one that latches. Only a clause whose target
            # really skips or guards this call is ever considered, so looking
            # outward cannot invent a guard.
            if cache: break
        k = min(chain[0][0], k) - 1
    if cache:
        g.insert(0, cache[0])
        if shapes: notes.append('compound test: %s' % shapes)
        if cache[1]:
            notes.append('guard on %s' % cache[1])
            if site.store_key and cache[1].split('.')[-1] == site.store_key.split('.')[-1]:
                notes.append('result stored back to the guarded name')
            if (site.comp_latch and
                    cache[1].split('.')[-1] == site.comp_latch.split('.')[-1]):
                # The LP live lock: the guarded name is filled *from a
                # comprehension over this answer*, so a bad answer raises
                # inside the comprehension, the assignment never runs, and the
                # next read calls again -- forever, at the caller's cadence.
                g.append('comp_latch')
                notes.append('the guard is filled by a comprehension over this '
                             'answer, so a bad answer leaves it unset: live lock')
    elif shapes:
        notes.append('compound test: %s' % shapes)
    return ('+'.join(g) if g else 'none'), notes

# ------------------------------------------------------------------ machine
class Machine(object):
    def __init__(self, c, path, helper_svc, attr_svc, func_svc, const_globals,
                 native_attr=None, outer_svc=None):
        self.c = c; self.path = path
        self.ops = list(dis(c)); self.lm = linemap(c)
        self.offmap = {}
        for k, op in enumerate(self.ops): self.offmap.setdefault(op[0], k)
        self.helper_svc = helper_svc; self.attr_svc = attr_svc
        self.func_svc = func_svc; self.cg = const_globals
        self.stack = []; self.sites = []
        self.returns_svc = None; self.returns_origin = None
        # `outer_svc` is the enclosing code object's service bindings, handed to
        # a comprehension's own code object: `[lm.Get(x) for x in ids]` reads
        # `lm` as a *free* variable, so without it the handle is unknown there.
        self.local_svc = dict(outer_svc or {}); self.learned_attr = {}
        self.native_attr = NATIVE_ATTR if native_attr is None else native_attr
        self.learned_decos = {}
        self.with_descs = {}
        self.local_tag = collections.defaultdict(list)
        self.attr_tag = collections.defaultdict(list)
        self.merges = {}          # join offset -> stack snapshot of the arm that jumped
        self.comp_sites = []      # sites found inside a comprehension code object
        self.merge_drop = 0       # stack depths that did not reconcile at a join

    # --- stack
    def popn(self, k):
        out = []
        for _ in range(k):
            out.append(self.stack.pop() if self.stack else V('?'))
        out.reverse(); return out
    def push(self, v): self.stack.append(v)
    def top(self, i=0): return self.stack[-1-i] if len(self.stack) > i else V('?')

    # --- branch merge
    def snapshot(self, tgt):
        """Remember this arm's stack and bindings for the join point ahead.

        The bindings matter as much as the stack. `_GetSvcForContactType` in
        `addressbookService.py` is the case that proves it:

            svc = ''
            if   t == 'contact':        svc = sm.RemoteSvc('charMgr')
            elif t == 'corpcontact':    svc = sm.GetService('corp')
            elif t == 'alliancecontact': svc = sm.GetService('alliance')
            return svc

        Walking only the first arm made this function "returns a charMgr
        moniker" and gave the census 8 `charMgr.*` rows. Walking all three and
        keeping only the last arm's bindings makes it "returns nothing" and
        drops all 8 -- and `sm.RemoteSvc('charMgr').GetLabels()` *is* a call the
        client makes. The census is a union over paths, so the bindings are
        unioned: a handle bound on any arm stays bound at the join.
        """
        cur = [dict(v) for v in self.stack]
        old = self.merges.get(tgt)
        loc = dict(self.local_svc)
        if old is None:
            self.merges[tgt] = (cur, loc)
        else:
            self.merges[tgt] = (self.merge_stacks(old[0], cur),
                                self.merge_locals(old[1], loc))

    def merge_locals(self, a, b):
        out = dict(a)
        for k, v in b.items():
            if k not in out: out[k] = v
            elif out[k] != v and out[k][0] != v[0]: out[k] = ('?', v[1])
        return out

    def merge_vals(self, a, b):
        """One stack slot, reached by two paths.

        Sites are unioned -- both arms' calls must stay traceable. A field the
        two arms disagree on is *dropped* rather than guessed: an ambiguous
        service handle becomes `?`, which the TSV reports as `unknown`, because
        a row naming the wrong service is worse than a row naming none.
        """
        if a is b: return a
        out = dict(a)
        for s in sites_of(b): add_site(out, s)
        for k in ('svc', 'bm', 'origin', 'indirect', 'code'):
            if not a.get(k): out[k] = b.get(k)
            elif b.get(k) and a[k] != b[k]:
                out[k] = '?' if k == 'svc' else None
        out['isfunc'] = a.get('isfunc') or b.get('isfunc')
        # `natv` decides the `native` consumer, the one that costs a black
        # screen, so an arm that reaches native code wins and says so.
        if bool(a.get('natv')) != bool(b.get('natv')):
            out['natv'] = True
            out['d'] = a['d'] if a.get('natv') else b['d']
        if a['d'] != b['d'] and not out.get('natv'):
            out['d'] = '%s|%s' % (a['d'], b['d'])
        return out

    def merge_stacks(self, a, b):
        n = min(len(a), len(b))
        out = [self.merge_vals(a[k], b[k]) for k in range(n)]
        if len(a) == len(b): return out
        # Both arms of a real `if`/`else` reach the join at the same depth; a
        # disagreement means this machine mis-modelled one of them. Keep the
        # arm that jumped (its depth was taken at a structured exit) and let
        # the dropped slots' sites fall through to `unknown` at the end of the
        # run, rather than attaching them to a slot they do not belong to.
        self.merge_drop += 1
        return out + a[n:] if len(a) > len(b) else out

    # --- comprehensions
    def latch(self, v, name):
        """Record that a comprehension over this value is what fills `name`.

        The LP live lock in `docs/guides/debugging-the-client.md` §4 is exactly this:
        `self._cache = {r[0]: r[1] for r in remote()}` under `if self._cache is
        None:`. Our answer is iterated, which the census already says, but the
        *store* is inside the comprehension's expression -- so one bad element
        raises, the assignment never runs, and the guard is never satisfied.
        """
        for s in (v.get('via') or ()):
            if s.comp_latch is None: s.comp_latch = name

    def run_comprehension(self, cco, args, opidx):
        """Walk a comprehension's own code object in the enclosing scope.

        In 2.7 a *list* comprehension is inlined, but a dict, set or generator
        comprehension is compiled to its own code object, taking the iterator
        as `.0` and reaching everything else as a free variable. The scanner
        does visit those code objects -- `walk()` returns them -- but it visits
        them *standalone*, so `[lm.Get(x) for x in ids]` loses `lm`: the
        binding lives in the enclosing function's locals. Hand them down.
        """
        kind = (cco.name or b'').decode('utf8','replace')
        sub = Machine(cco, srcpath(cco), self.helper_svc, self.attr_svc,
                      self.func_svc, self.cg, self.native_attr,
                      outer_svc=self.local_svc)
        pbase = len(PROBE_OUT)
        got = sub.run()
        for ps in PROBE_OUT[pbase:]:
            ps.opidx = opidx; ps.off = self.ops[opidx][0]
        for s in got:
            # The guard that decides whether this call can latch is the one
            # around the comprehension, and it is in *this* code object, so the
            # site's op index moves to this call. `line` stays the client's own
            # line for the call, which is what a traceback prints.
            s.opidx = opidx; s.off = self.ops[opidx][0]
            s.in_comp = kind
            s.func = (self.c.name or b'').decode('utf8','replace')
            if s.consumer in ('store', 'pass') and any(
                    n in ('cached as collected', '-> yield') for n in s.notes):
                # `collected into the list` is not a consumer: nothing has
                # looked at the value yet. What is true of every element is
                # that the container is built in one expression.
                s.consumer = 'comprehension'
            s.notes.append('called inside a %s, once per element' % kind)
            self.sites.append(s)
            self.comp_sites.append(s)
        return V('%s()' % kind, via=[s for a in args for s in sites_of(a)] +
                 [s for a in args for s in (a.get('via') or ())])

    # --- taint
    def tc(self, vals, how, extra=None):
        for v in vals:
            for s in sites_of(v):
                if s.consumer is not None: continue
                self.set_consumer(s, how, extra, v)

    def set_consumer(self, s, how, extra, v):
        if how == 'call':
            cv, na, nk = extra
            callee = cv['d'] if isinstance(cv, dict) else cv
            natv = bool(isinstance(cv, dict) and cv.get('natv'))
            root = callee.split('.')[0]; short = callee.split('.')[-1].rstrip('()')
            if natv:
                s.consumer = 'native'; s.notes.append('-> %s() [native object]' % callee)
            elif short == 'len': s.consumer = 'len'
            elif short in ('list','tuple','dict','set','sorted','iter','sum','max','min','any','all'):
                s.consumer = 'iterate'; s.notes.append('-> %s()' % short)
            elif any(h in callee for h in ROWSET_HINTS):
                s.consumer = 'rowset'; s.notes.append('-> %s()' % callee)
            elif root in NATIVE_ROOTS:
                s.consumer = 'native'; s.notes.append('-> %s()' % callee)
            else:
                s.consumer = 'pass'; s.notes.append('-> %s()' % callee)
        elif how == 'store':
            if s.resolved:
                s.consumer = 'store'
            else:
                s.resolved = True; s.store_key = extra
                s.notes.append('cached as %s' % extra)
                (self.attr_tag if extra.startswith('@') else self.local_tag)[
                    extra.lstrip('@').split('.')[-1]].append(s)
        else:
            s.consumer = how
            if extra: s.notes.append(extra)

    # --- run
    def run(self):
        ops = self.ops; i = 0; n = len(ops)
        while i < n:
            off, nm, arg, rep = ops[i]
            if self.merges:
                snap = self.merges.pop(off, None)
                if snap is not None:
                    self.stack = self.merge_stacks(snap[0], self.stack)
                    self.local_svc = self.merge_locals(snap[1], self.local_svc)
            if nm == 'LOAD_CONST':
                k = self.c.consts[arg]
                if isinstance(k, Code): self.push(V('<code>', code=k))
                elif isinstance(k, bytes): self.push(V(repr(k.decode('utf8','replace'))))
                else: self.push(V(repr(k)))
            elif nm in ('LOAD_NAME','LOAD_GLOBAL','LOAD_FAST','LOAD_DEREF'):
                if nm in ('LOAD_NAME','LOAD_GLOBAL') and rep in self.cg:
                    self.push(V(repr(self.cg[rep])))
                else:
                    sv = self.local_svc.get(rep)
                    self.push(V(rep, svc=sv[0] if sv else None,
                                origin=sv[1] if sv else None,
                                indirect='local %s' % rep if sv else None))
                for s in self.local_tag.pop(rep, []):
                    if s.consumer is None: add_site(self.stack[-1], s)
            elif nm == 'LOAD_CLOSURE': self.push(V(rep))
            elif nm == 'LOAD_ATTR':
                base = self.popn(1)[0]
                if base.get('svc') and rep not in MONIKER_LOCAL and rep not in PY_LOCAL:
                    self.push(V('%s::%s' % (base['svc'], rep),
                                bm=(base['svc'], rep, base.get('origin') or 'remotesvc',
                                    base.get('indirect'))))
                else:
                    if rep in ROWSET_ATTRS:
                        self.tc([base], 'rowset', 'reads .%s -- a real rowset' % rep)
                    else:
                        self.tc([base], 'attr', 'reads .%s' % rep)
                    a = self.attr_svc.get(rep)
                    ga = GLOBAL_ATTR.get(rep)
                    if ga and os.path.basename(ga[2]).rsplit('.', 1)[0] == \
                            os.path.basename(self.path).rsplit('.', 1)[0]: ga = None
                    self.push(V('%s.%s' % (base['d'], rep),
                                svc=a[0] if a else None, origin=a[1] if a else None,
                                indirect=('attr .%s [handle learned in %s]'
                                          % (rep, ga[2]) if a and ga and ga[0] == a[0]
                                          else 'attr .%s' % rep) if a else None,
                                natv=bool(base.get('natv')) or
                                     (rep in self.native_attr) or
                                     (base['d'].split('.')[0] in NATIVE_ROOTS)))
                for s in self.attr_tag.pop(rep, []):
                    if s.consumer is None: add_site(self.stack[-1], s)
            elif nm.startswith('CALL_FUNCTION'):
                na = arg & 0xff; nk = (arg >> 8) & 0xff
                extra = {'CALL_FUNCTION':0,'CALL_FUNCTION_VAR':1,
                         'CALL_FUNCTION_KW':1,'CALL_FUNCTION_VAR_KW':2}[nm]
                vals = self.popn(1 + na + 2*nk + extra)
                callee = vals[0]; args = vals[1:1+na]
                self.tc(vals[1:], 'call', (callee, na, nk))
                self.tc([callee], 'pass', '-> called as a function')
                self.push(self.call_result(callee, args, vals[1:], na, nk, i))
            elif nm in ('STORE_FAST','STORE_NAME','STORE_DEREF','STORE_GLOBAL'):
                v = self.popn(1)[0]
                if v.get('svc'): self.local_svc[rep] = (v['svc'], v.get('origin') or 'remotesvc')
                elif rep in self.local_svc: del self.local_svc[rep]
                self.tc([v], 'store', rep)
                self.latch(v, rep)
            elif nm == 'STORE_ATTR':
                v, base = self.popn(2)
                if v.get('natv'): self.native_attr.add(rep)
                if v.get('svc'):
                    self.learned_attr[rep] = (v['svc'], v.get('origin') or 'remotesvc')
                self.tc([v], 'store', '@%s.%s' % (base['d'], rep))
                self.latch(v, '%s.%s' % (base['d'], rep))
            elif nm == 'STORE_SUBSCR':
                v = self.popn(3); self.tc(v[:1], 'store', 'into a dict/list slot')
            elif nm == 'DELETE_SUBSCR': self.popn(2)
            elif nm == 'POP_TOP': self.tc(self.popn(1), 'discard')
            elif nm == 'DUP_TOP': self.push(dict(self.top()))
            elif nm == 'DUP_TOPX':
                seg = self.stack[-arg:] if arg and len(self.stack) >= arg else []
                self.stack.extend([dict(s) for s in seg])
            elif nm == 'ROT_TWO':
                if len(self.stack) >= 2: self.stack[-1], self.stack[-2] = self.stack[-2], self.stack[-1]
            elif nm == 'ROT_THREE':
                if len(self.stack) >= 3: self.stack.insert(-2, self.stack.pop())
            elif nm == 'ROT_FOUR':
                if len(self.stack) >= 4: self.stack.insert(-3, self.stack.pop())
            elif nm == 'UNPACK_SEQUENCE':
                self.tc(self.popn(1), 'unpack_%d' % arg)
                for _ in range(arg): self.push(V('unpacked'))
            elif nm == 'COMPARE_OP':
                v = self.popn(2)
                self.tc(v, 'compare', 'cmp %s' % (CMP[arg] if arg < len(CMP) else '?'))
                self.push(V('cmp'))
            elif nm.startswith('BINARY_') or nm.startswith('INPLACE_'):
                v = self.popn(2)
                self.tc(v, 'subscript' if nm == 'BINARY_SUBSCR' else 'arith')
                self.push(V('binop'))
            elif nm.startswith('UNARY_'):
                self.tc(self.popn(1), 'truth' if nm == 'UNARY_NOT' else 'arith')
                self.push(V('unop'))
            elif nm == 'GET_ITER':
                v = self.popn(1)
                self.tc(v, 'iterate')
                # keep the sites reachable: a comprehension that *stores* its
                # result decides whether this value's answer can latch at all.
                self.push(V('iter', via=[s for x in v for s in sites_of(x)]))
            elif nm == 'FOR_ITER': self.push(V('item'))
            elif nm in ('BUILD_TUPLE','BUILD_LIST','BUILD_SET'):
                self.tc(self.popn(arg), 'pack'); self.push(V('container'))
            elif nm == 'BUILD_MAP': self.push(V('dict'))
            elif nm == 'STORE_MAP': self.tc(self.popn(2), 'pack')
            elif nm == 'BUILD_SLICE': self.popn(arg); self.push(V('slice'))
            elif nm in ('LIST_APPEND','SET_ADD'):
                self.tc(self.popn(1), 'store', 'collected')
            elif nm == 'MAP_ADD': self.tc(self.popn(2), 'store', 'collected')
            elif nm.startswith('STORE_SLICE+'): self.popn(2 + int(nm[-1]))
            elif nm.startswith('DELETE_SLICE+'): self.popn(1 + int(nm[-1]))
            elif nm.startswith('SLICE+'):
                self.tc(self.popn(1 + int(nm[-1]))[:1], 'subscript'); self.push(V('slice'))
            elif nm == 'RETURN_VALUE':
                v = self.popn(1)[0]
                if v.get('svc'):
                    if self.returns_svc in (None, v['svc']):
                        self.returns_svc = v['svc']
                        self.returns_origin = v.get('origin') or 'remotesvc'
                    else: self.returns_svc = '#amb'
                self.tc([v], 'pass', '-> return')
            elif nm == 'YIELD_VALUE': self.tc([self.top()], 'pass', '-> yield')
            elif nm == 'IMPORT_NAME': self.popn(2); self.push(V(rep))
            elif nm == 'IMPORT_FROM': self.push(V(rep))
            elif nm == 'IMPORT_STAR': self.popn(1)
            elif nm == 'MAKE_FUNCTION':
                v = self.popn(arg + 1)
                self.push(V('func', isfunc=True, code=v[-1].get('code') if v else None))
            elif nm == 'MAKE_CLOSURE':
                v = self.popn(arg + 2)
                self.push(V('func', isfunc=True, code=v[-1].get('code') if v else None))
            elif nm == 'RAISE_VARARGS': self.tc(self.popn(arg), 'pass', '-> raise')
            elif nm in COND_JUMP: self.tc(self.popn(1), 'truth')
            elif nm == 'JUMP_FORWARD':
                # **Do not teleport.** This used to be `i = j; continue`, which
                # skipped the whole `else` arm of every `if`/`else` -- the arm
                # the `if` jumps over. 278 candidate call sites in this client
                # sit in skipped bytecode, `marketProxy.GetOrders` among them,
                # and it had no row in the census at all. Walk on, and leave
                # this arm's stack at the join point so the two arms reconcile
                # there instead of one of them piling junk on the other.
                j = self.offmap.get(off + 3 + arg)
                if j is not None and j > i: self.snapshot(off + 3 + arg)
            elif nm == 'SETUP_WITH':
                v = self.popn(1)[0]; self.with_descs[off] = v['d']
                self.push(V('mgr')); self.push(V('ctx'))
            elif nm == 'PRINT_ITEM': self.popn(1)
            elif nm == 'PRINT_ITEM_TO': self.popn(2)
            elif nm == 'PRINT_NEWLINE_TO': self.popn(1)
            elif nm == 'PRINT_EXPR': self.popn(1)
            elif nm == 'EXEC_STMT': self.popn(3)
            elif nm == 'BUILD_CLASS': self.popn(3); self.push(V('class'))
            elif nm == 'LOAD_LOCALS': self.push(V('locals'))
            elif nm == 'DELETE_ATTR': self.popn(1)
            i += 1
        for lst in list(self.local_tag.values()) + list(self.attr_tag.values()):
            for s in lst:
                if s.consumer is None: s.consumer = 'store'
        for s in self.sites:
            if s.consumer is None:
                s.consumer = 'unknown'; s.notes.append('value not traced to a consumer')
        return self.sites

    def call_result(self, callee, args, allargs, na, nk, opidx):
        d = callee['d']; short = d.split('.')[-1]
        def cs(v):
            s = v['d']
            return s[1:-1] if len(s) >= 2 and s[0] == "'" and s[-1] == "'" else None
        cco = callee.get('code')
        if (cco is not None and na == 1 and nk == 0 and
                (cco.name or b'') in COMP_NAMES):
            return self.run_comprehension(cco, args, opidx)
        # Dynamic dispatch: `getattr(account, itemName)(*args)`.
        #
        # `accountsvc.py:100` (`GetStaticData`, def at :97) is the case the
        # wallet lane hit: the method name is a runtime string, so there is no
        # `LOAD_ATTR` to see and `account.GetEntryTypes` -- a call the client
        # makes on every wallet open -- has no row at all. That is worse than
        # the branch and comprehension blind spots: those lose *a* row for a
        # method that has other call sites, this loses the method entirely, so
        # nothing in the file looks suspicious. **A missing row in
        # findings/client-calls.tsv does not mean "not called".**
        if short == 'getattr' and na >= 2 and args and args[0].get('svc'):
            svc = args[0]['svc']; origin = args[0].get('origin') or 'remotesvc'
            ind = args[0].get('indirect') or 'getattr()'
            meth = cs(args[1])
            if meth:
                return V('%s::%s' % (svc, meth), bm=(svc, meth, origin,
                                                     '%s, named by a constant' % ind))
            return V('%s::<dynamic>' % svc,
                     bm=(svc, 'unknown', origin,
                         '%s, callee is getattr(%s): dynamic dispatch, method '
                         'name not a constant' % (ind, args[1]['d'])))
        # decorator application: f = deco(f)
        if na == 1 and nk == 0 and args and args[0].get('isfunc'):
            dd = list(callee.get('deco') or [])
            dd.append(d)
            return V('func', isfunc=True, deco=dd)
        if PROBE and short in PROBE:
            ps = Site(service='#probe', method=short, path=self.path, line=0,
                      kind=None, nargs=na, opidx=opidx, off=self.ops[opidx][0],
                      func=(self.c.name or b'').decode('utf8','replace'))
            PROBE_OUT.append(ps)
            return V('call:%s' % d, site=ps)
        if short in SVC_FACTORY and na == 1 and args:
            s = cs(args[0]); return V('SVC(%s)' % (s or '?'), svc=s or '?', origin='remotesvc')
        if short == 'ConnectToRemoteService' and na >= 1 and args:
            s = cs(args[0]); return V('SVC(%s)' % (s or '?'), svc=s or '?', origin='remotesvc')
        if short == 'Moniker' and na >= 1 and args:
            s = cs(args[0]); return V('MON(%s)' % (s or '?'), svc=s or '?', origin='moniker')
        if short == 'GetService' and na == 1 and args:
            return V('clientsvc(%s)' % (cs(args[0]) or '?'))
        if short == 'GetMoniker' and na == 0:
            b = (d.rsplit('.', 1)[0] + ' ' + self.path).lower()
            if 'alliance' in b or 'all_' in b:
                return V('MON(allianceRegistry)', svc='allianceRegistry', origin='moniker',
                         indirect='GetMoniker()')
            if 'war' in b:
                return V('MON(warRegistry)', svc='warRegistry', origin='moniker',
                         indirect='GetMoniker()')
            return V('MON(?)', svc='?', origin='moniker', indirect='GetMoniker()')
        if short in self.func_svc:
            s, o = self.func_svc[short]
            g = GLOBAL_FUNC.get(short)
            ind = '%s()' % short
            here = os.path.basename(self.path).rsplit('.', 1)[0]
            if g and g[0] == s and os.path.basename(g[2]).rsplit('.', 1)[0] != here:
                # Say where the claim comes from: this row exists because an
                # accessor of this name returns a handle for `s` in that file,
                # and nothing here proves the receiver is that class.
                ind = '%s() [accessor learned in %s]' % (short, g[2])
            return V('SVC(%s)' % s, svc=s, origin=o, indirect=ind)
        if short in self.helper_svc and (d == short or 'oniker' in d or
                                         d.startswith('self.') or
                                         d.startswith('clientsvc(')):
            s, o = self.helper_svc[short]
            return V('MON(%s)' % s, svc=s, origin=o, indirect='%s()' % short)
        if short in ('GetInventoryFromId','GetInventory','GetInventoryMgr',
                     'GetStationInventoryMgr','GetSolarSystemInventoryMgr'):
            lo = d.lower()
            if any(t in lo for t in ('invcache','inv_cache','inventorymgr','invmgr')):
                return V('BOUND(invbroker)', svc='invbroker', origin='bound',
                         indirect=short + '()')
        if d.split('.')[0] in NATIVE_ROOTS or callee.get('natv'):
            return V('call:%s' % d, natv=True)
        if callee.get('bm'):
            svc, meth, origin, indirect = callee['bm']
            site = Site(service=svc, method=meth, path=self.path,
                        line=line_at(self.lm, self.ops[opidx][0]), kind=origin,
                        nargs=na, opidx=opidx, off=self.ops[opidx][0], indirect=indirect,
                        func=(self.c.name or b'').decode('utf8','replace'))
            self.sites.append(site)
            if BIND_SHAPE.match(meth):
                return V('BOUND(%s)' % svc, svc=svc, origin='bound',
                         indirect='%s.%s()' % (svc, meth), site=site)
            return V('RET(%s.%s)' % (svc, meth), site=site)
        return V('call:%s' % d)

# ------------------------------------------------------------------ per file
def const_globals_of(codes):
    cg = {}
    for c in codes:
        if (c.name or b'') not in (b'<module>',) and not (c.flags & 0x40 == 0 and False):
            pass
    for c in codes[:1] + [x for x in codes if (x.name or b'').startswith(b'<')]:
        ops = list(dis(c))
        for i in range(len(ops) - 1):
            if ops[i][1] == 'LOAD_CONST' and ops[i+1][1] in ('STORE_NAME','STORE_GLOBAL'):
                k = c.consts[ops[i][2]]
                if isinstance(k, bytes):
                    cg[ops[i+1][3]] = k.decode('utf8','replace')
    return cg

def scan_file(p, helper_svc, want_sites=True):
    # Three values on every path, including this one. It returned `[], {}`
    # here for a long time, and the arity is load-bearing: every caller
    # unpacks three. `sweep` and `learn_sweep` both wrap the call in
    # `except Exception`, so an unreadable `.pyc` reported itself as
    # `ERR <file> ValueError('not enough values to unpack')` -- a scan error
    # that never happened -- or was swallowed outright, while `build_helpers`
    # has no `try` and died on it. Keep the tuple the same width.
    try: co = load_pyc(p)
    except Exception: return [], {}, {}
    codes = walk(co)
    cg = const_globals_of(codes)
    attr_svc = dict(SEED_ATTR)
    attr_svc.update({k: (v[0], v[1]) for k, v in GLOBAL_ATTR.items()})
    func_svc = {k: (v[0], v[1]) for k, v in GLOBAL_FUNC.items()}
    decos = {}; nattr = set()
    for _ in range(2):
        for c in codes:
            m = Machine(c, srcpath(c), helper_svc, attr_svc, func_svc, cg, nattr)
            m.run()
            for k, v in m.learned_attr.items():
                if k in attr_svc and attr_svc[k][0] != v[0]: attr_svc[k] = ('#amb', None)
                else: attr_svc.setdefault(k, v)
            if m.returns_svc and m.returns_svc != '#amb':
                nmx = (c.name or b'').decode('utf8','replace')
                if nmx in func_svc and func_svc[nmx][0] != m.returns_svc:
                    func_svc[nmx] = ('#amb', None)
                else: func_svc[nmx] = (m.returns_svc, m.returns_origin)
        attr_svc = {k: v for k, v in attr_svc.items() if v[0] != '#amb'}
        func_svc = {k: v for k, v in func_svc.items() if v[0] != '#amb'}
    # decorators: class bodies and module level
    for c in codes:
        ops = list(dis(c))
        mm = Machine(c, srcpath(c), helper_svc, attr_svc, func_svc, cg, nattr)
        mm.run()
        # re-walk for name -> decorator
        st = []
        m2 = None
        for i, (off, nm, arg, rep) in enumerate(ops):
            if nm in ('STORE_NAME','STORE_GLOBAL') and i >= 1:
                pass
        # simple pattern: MAKE_FUNCTION/MAKE_CLOSURE ... CALL_FUNCTION 1 ... STORE_NAME
        for i, (off, nm, arg, rep) in enumerate(ops):
            if nm not in ('MAKE_FUNCTION','MAKE_CLOSURE'): continue
            fname = None
            if ops[i-1][1] == 'LOAD_CONST':
                k = c.consts[ops[i-1][2]]
                if isinstance(k, Code): fname = (k.name or b'').decode('utf8','replace')
            if fname is None: continue
            dl = []; j = i + 1
            while j < len(ops) and ops[j][1] == 'CALL_FUNCTION':
                # find the decorator expression before the function was pushed
                dl.append('?'); j += 1
            if j < len(ops) and ops[j][1] in ('STORE_NAME','STORE_GLOBAL') and dl:
                # recover the decorator names by scanning back from the MAKE_FUNCTION
                names = []
                q = i - 2
                depth = 0
                while q >= 0 and len(names) < len(dl) + 2:
                    if ops[q][1] in ('LOAD_NAME','LOAD_GLOBAL','LOAD_ATTR'):
                        names.append(ops[q][3])
                    if ops[q][1] in ('STORE_NAME','POP_TOP','MAKE_FUNCTION','MAKE_CLOSURE'):
                        break
                    q -= 1
                if names: decos.setdefault(fname, []).extend(names)
    if not want_sites: return [], func_svc, attr_svc
    sites = []
    for c in codes:
        # A comprehension's code object is reached through the code object that
        # builds it (`Machine.run_comprehension`), which is the only place its
        # free variables are known. Visiting it here as well would double-count
        # whatever that scope does resolve.
        if (c.name or b'') in COMP_NAMES: continue
        m = Machine(c, srcpath(c), helper_svc, attr_svc, func_svc, cg, nattr)
        pbase = len(PROBE_OUT)
        got = m.run()
        for ps in PROBE_OUT[pbase:]:
            ps.guard, _ = detect_guard(m.ops, ps, m.with_descs, decos.get(ps.func, []),
                                       m.offmap)
        for s in got:
            s.guard, gn = detect_guard(m.ops, s, m.with_descs, decos.get(s.func, []),
                                       m.offmap)
            s.notes = gn + s.notes
            sites.append(s)
    return sites, func_svc, attr_svc

def sweep(helper_svc):
    out = []; nf = 0
    for dp, dn, fn in os.walk(ROOT):
        for f in sorted(fn):
            if not f.endswith('.pyc'): continue
            nf += 1
            try: sites, _, _ = scan_file(os.path.join(dp, f), helper_svc)
            except Exception as e:
                sys.stderr.write('ERR %s %r\n' % (f, e)); continue
            out.extend(sites)
    return out, nf

def learn_sweep(helper_svc):
    """One pass over the tree to fill `GLOBAL_FUNC` / `GLOBAL_ATTR`.

    This is the two-pass shape: the accessor's *definition* is a plain
    `return sm.RemoteSvc('x').GetThing()` that the per-file learner already
    reads correctly; all that was missing was carrying what it learned to the
    other 12,773 files. One pass, not a fixpoint -- an accessor that can only
    be resolved *through* another promoted accessor is still missed, and
    `findings/bound-objects.json` (which does run a fixpoint) is the place to
    look when that matters.
    """
    fseen = collections.defaultdict(dict)   # name -> {service: [files]}
    aseen = collections.defaultdict(dict)
    nf = 0
    for dp, dn, fn in os.walk(ROOT):
        for f in sorted(fn):
            if not f.endswith('.pyc'): continue
            nf += 1
            try: _, fs, ats = scan_file(os.path.join(dp, f), helper_svc,
                                        want_sites=False)
            except Exception: continue
            for tbl, seen in ((fs, fseen), (ats, aseen)):
                for k, v in tbl.items():
                    if not v or not v[0] or v[0] in ('?', '#amb'): continue
                    seen[k].setdefault(v[0], []).append(f)
    for seen, out, kind in ((fseen, GLOBAL_FUNC, 'func'), (aseen, GLOBAL_ATTR, 'attr')):
        for name, bysvc in seen.items():
            if len(bysvc) != 1: continue          # two services, no claim
            svc, where = list(bysvc.items())[0]
            if not promotable(name, where): continue
            out[name] = (svc, 'moniker', where[0])
    sys.stderr.write('learn sweep: %d files, promoted %d accessor names and %d '
                     'attributes across modules\n'
                     % (nf, len(GLOBAL_FUNC), len(GLOBAL_ATTR)))

def seed_promoted():
    import json as _j
    try: b = _j.load(open(BOUND_SEED))
    except Exception: return {}, {}
    pr = b.get('promoted_names', {}).get('promoted', {})
    acc = {k: (v['service'], 'moniker')
           for k, v in pr.get('accessor_unique_name', {}).items()}
    att = {k: (v['service'], 'moniker')
           for k, v in pr.get('attribute', {}).items()}
    return acc, att

def build_helpers():
    """The service handles the scanner starts from, before any sweep.

    Split out of `main()` so one file can be scanned on its own:

        import scan_calls as sc
        sites, _, _ = sc.scan_file(path, sc.build_helpers())

    Three values, not two. This docstring said two, and that is where the
    arity bug in `scan_file`'s error path came from.
    """
    helper_svc = {}
    acc, att = seed_promoted()
    helper_svc.update(acc)
    SEED_ATTR.update(att)
    sys.stderr.write('seeded %d accessors, %d attributes from findings/bound-objects.json\n'
                     % (len(acc), len(att)))
    for seed in (os.path.join(ROOT, 'eve/common/script/net/eveMoniker.pyc'),):
        _, fs, _ = scan_file(seed, helper_svc, want_sites=False)
        for k, v in fs.items(): helper_svc.setdefault(k, v)
    sys.stderr.write('helpers: %d\n' % len(helper_svc))
    return helper_svc

def main():
    global PROBE
    # `ROOT` and the `build_helpers` seed are both container paths, because
    # that is where this tool is meant to run. Say so instead of printing
    # `seeded 0 accessors` and an empty census, which is what a host run used
    # to do once it got past the arity crash.
    if not os.path.isdir(ROOT):
        sys.exit('scan_calls: %s does not exist. Run it in the analysis\n'
                 'container, which mounts the repo at /work:\n'
                 '    tools/analysis.sh python3 /work/tools/scan_calls.py' % ROOT)
    helper_svc = build_helpers()
    learn_sweep(helper_svc)
    allsites, nfiles = sweep(helper_svc)
    sys.stderr.write('pass1 files=%d sites=%d\n' % (nfiles, len(allsites)))
    # pass 2: a site whose only immediate consumer is `return` hands the value
    # to this function's own callers. Chase one hop by probing every call to a
    # function of that name anywhere in the tree.
    ret = [s for s in allsites if s.consumer == 'pass' and '-> return' in s.notes]
    PROBE = set(s.func for s in ret if s.func and not s.func.startswith('<'))
    sys.stderr.write('probing %d function names for %d return-sites\n' % (len(PROBE), len(ret)))
    del PROBE_OUT[:]
    sweep(helper_svc)
    agg = {}
    gagg = {}
    for ps in PROBE_OUT:
        if ps.guard and ps.guard != 'none':
            gagg.setdefault(ps.method, collections.Counter())[ps.guard] += 1
        if ps.consumer is None: continue
        agg.setdefault(ps.method, collections.Counter())[
            (ps.consumer, '; '.join(n for n in ps.notes if n.startswith('->') or
                                    n.startswith('reads')))] += 1
    hit = 0
    for s in ret:
        gc = gagg.get(s.func)
        if gc:
            for g, k in gc.most_common(2):
                s.notes.append("caller guard %s x%d" % (g, k))
            if s.guard == 'none':
                # Prefer a caller guard that can *latch* over a more common one
                # that cannot: `contractProxy.NumOutstandingContracts` is a live
                # lock behind `getattr(self, 'n', None) is None`, and reporting
                # its callers' `finally_return` instead -- because more callers
                # had one -- hides the only column that would show it.
                latch = [g for g, _ in gc.most_common() if any(t in g for t in CACHE_TOKENS)]
                s.guard = 'caller:' + (latch[0] if latch else gc.most_common(1)[0][0])
        c = agg.get(s.func)
        if not c: s.notes.append('no caller of %s() found' % s.func); continue
        kinds = set(k[0] for k in c)
        tot = sum(c.values())
        if len(kinds) == 1 and 'pass' not in kinds and 'unknown' not in kinds:
            k, extra = c.most_common(1)[0][0]
            s.consumer = k
            s.notes.append('via return from %s(); all %d callers: %s %s'
                           % (s.func, tot, k, extra))
            hit += 1
        else:
            s.notes.append('callers of %s(): %s' % (s.func, ', '.join(
                '%s x%d' % (k[0], v) for k, v in c.most_common(4))))
    sys.stderr.write('return-sites resolved one hop: %d of %d\n' % (hit, len(ret)))
    json.dump([{'service': s.service, 'method': s.method, 'path': s.path, 'line': s.line,
                'kind': s.kind, 'consumer': s.consumer, 'guard': s.guard, 'notes': s.notes,
                'nargs': s.nargs, 'func': s.func, 'indirect': s.indirect}
               for s in allsites], open(SITES_OUT, 'w'))

if __name__ == '__main__':
    main()
