#!/usr/bin/env python3
"""Which of what we rely on did a client build move? Step H of the port checklist.

    tools/port-recheck.py OLD_PYC NEW_PYC [--table findings/port-recheck.tsv]
                          [--old-bundle DIR --new-bundle DIR] [--out DIR]
                          [--repo .] [--no-citations] [--no-consts]

Run it in the analysis container (tools/analysis.sh python3 -I ...; `tools/new-build.sh
--report` does). OLD_PYC and NEW_PYC are unpacked bytecode trees and are only read.
The checks:

  table      every row of findings/port-recheck.tsv. A lane that relied on client
             behaviour adds a row there; see "The table" below.
  citations  every `path.py:N` in server/crates and in the fci scenarios. The line
             is read against OLD. Path-qualified: Rust cites basenames, and basename
             joins fire on every `const` and `__init__`.
  constants  Rust consts against the client's own constants, and with bundles the
             effect, attribute and type rows behind them.

**Function level.** Two builds are compared code object by code object: co_code,
co_names, co_varnames, the signature, and constants with nested code replaced by
its name. Line numbers and co_filename are left out, so a moved function is not a
changed one. A nested code object is its own entry, and asking about a class or a
function covers what it contains. Line numbers here are always from co_firstlineno
and co_lnotab, never a decompiler's.

**Output**, in DIR (default <dir of NEW_PYC>/recheck when NEW_PYC ends in pyc, so
out/new-build/<build>/recheck, else out/port-recheck/<name of NEW_PYC>):

  recheck.md    what needs a reader, table rows first
  recheck.tsv   one row per finding: section, status, where, what, detail
  shifts.sh     one `sed` per moved citation in code that did NOT change. Comment
                edits only: apply it after reading recheck.md, from the repo root.

Exit 0: nothing needs a reader. Exit 3: something does. Exit 2: usage or a missing
tree. Exit 1: the tool failed, or a table row is malformed.

## The table

Tab-separated, `#` comments, one header row: kind, client_path, symbol, lane.
client_path has no extension and is relative to the tree's root; `-` means the
kind's default modules. A row holds paths and symbols only, never client data.

  module    the module; fires on any changed code object, and names them
  symbol    `Class.method`, `function`, a module name, or a name or string the code
            uses (then every code object using it is compared)
  const     a module-level name, `*` allowed; values are compared
  attr      a `dogma/const` name or id; with bundles, the attribute's row too
  effect    the same for effects; with bundles, its row and modifier rows
  group     an `inventorycommon/const` name or id (also category ids, by name)
  errorkey  a string constant in that module
  typelist  a typelists row by id; needs bundles, else skipped and said so

Statuses: `changed`, `gone` (was in OLD, not in NEW) and `ambiguous` need a reader.
`unresolved` means the row or citation matches neither tree: the row is stale, fix
it. `skipped` says a check had no input; it is never a pass.
"""
import argparse
import bisect
import collections
import fnmatch
import hashlib
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import pyc27  # noqa: E402

KINDS = ('module', 'symbol', 'const', 'attr', 'effect', 'group', 'errorkey', 'typelist')
#: Where the client keeps the constants Rust copies, in lookup order.
CONST_MODULES = ('dogma/const', 'inventorycommon/const', 'eve/common/lib/appConst')
#: Name prefix in those modules -> the id family it names.
FAMILIES = (('attribute', 'attr'), ('effect', 'effect'), ('group', 'group'),
            ('category', 'category'), ('type', 'type'))
READER = ('changed', 'gone', 'ambiguous')
#: Unset on a module or class body, whose statements are unrelated.
CO_OPTIMIZED = 1
SHOWN = 80


class Unknown(object):
    """A value the small evaluator below does not compute."""


UNKNOWN = Unknown()


# --- code objects -----------------------------------------------------------------

def text(b):
    return b.decode('utf8', 'replace') if isinstance(b, (bytes, bytearray)) else b


def norm_const(k):
    if isinstance(k, pyc27.Code):
        return ('<code>', text(k.name))
    if isinstance(k, (tuple, list)):
        return tuple(norm_const(x) for x in k)
    if isinstance(k, frozenset):
        return tuple(sorted(repr(norm_const(x)) for x in k))
    return k


def fingerprint(c):
    """A code object's meaning, with line numbers and the file name left out."""
    what = (c.argcount, c.flags, bytes(c.code), c.names, c.varnames, c.freevars,
            c.cellvars, norm_const(c.consts))
    return hashlib.sha1(repr(what).encode('utf8', 'surrogatepass')).hexdigest()


def line_rows(c):
    """(bytecode offset, line) rows from co_lnotab, offsets ascending."""
    tab, addr, line, rows = bytes(c.lnotab or b''), 0, c.firstlineno, []
    for i in range(0, len(tab) - 1, 2):
        if tab[i]:
            rows.append((addr, line))
            addr += tab[i]
        line += tab[i + 1]
    rows.append((addr, line))
    return rows


class Entry(object):
    __slots__ = ('key', 'first', 'last', 'fp', 'code', 'rows', 'depth')


class Module(object):
    """One .pyc: its code objects by qualified name, in tree order."""

    def __init__(self, path):
        self.entries = collections.OrderedDict()
        self.top = pyc27.load_pyc(path)
        self._add(self.top, '', 0)
        self._env = None

    def _add(self, c, prefix, depth):
        name = text(c.name)
        q = name if depth == 0 else (prefix + '.' + name if prefix else name)
        key, n = q, 1
        while key in self.entries:
            n += 1
            key = '%s#%d' % (q, n)
        e = Entry()
        e.key, e.first, e.fp, e.code, e.rows, e.depth = key, c.firstlineno, fingerprint(c), c, line_rows(c), depth
        e.last = max(r[1] for r in e.rows)
        self.entries[key] = e
        for k in c.consts or ():
            if isinstance(k, pyc27.Code):
                e.last = max(e.last, self._add(k, '' if depth == 0 else key, depth + 1))
        return e.last

    def at_line(self, n):
        """The innermost code object holding line n; the module when none does."""
        best = None
        for e in self.entries.values():
            if e.first <= n <= e.last and (best is None or e.depth > best.depth
                                           or (e.depth == best.depth and e.first > best.first)):
                best = e
        return best or next(iter(self.entries.values()))

    def last_line(self):
        return max(e.last for e in self.entries.values())

    def env(self):
        if self._env is None:
            self._env = eval_module(self.top)
        return self._env

    def uses(self, word):
        """Entries whose names or string constants hold word."""
        b, out = word.encode('utf8'), []
        for e in self.entries.values():
            if b in e.code.names or holds_string(e.code.consts, word, b):
                out.append(e.key)
        return out


def using(mod, sym):
    """Entries that use sym: as a whole, or a dotted name's parts in one code object."""
    if mod is None:
        return []
    got = mod.uses(sym)
    if got or '.' not in sym:
        return got
    keys = None
    for part in sym.split('.'):
        hits = set(mod.uses(part))
        keys = hits if keys is None else keys & hits
    return [k for k in mod.entries if k in keys]


def holds_string(k, s, b):
    if isinstance(k, (tuple, list, frozenset)):
        return any(holds_string(x, s, b) for x in k)
    return k == s or k == b


# --- module-level constants -------------------------------------------------------

_BINARY = {'BINARY_ADD': lambda a, b: a + b, 'BINARY_SUBTRACT': lambda a, b: a - b,
           'BINARY_MULTIPLY': lambda a, b: a * b, 'BINARY_LSHIFT': lambda a, b: a << b,
           'BINARY_OR': lambda a, b: a | b, 'BINARY_POWER': lambda a, b: a ** b,
           'BINARY_FLOOR_DIVIDE': lambda a, b: a // b, 'BINARY_MODULO': lambda a, b: a % b}


def constant(k):
    if isinstance(k, bytes):
        return text(k)
    if isinstance(k, (tuple, list)):
        items = [constant(x) for x in k]
        return UNKNOWN if any(x is UNKNOWN for x in items) else tuple(items)
    if isinstance(k, (int, float, str, frozenset)) or k is None:
        return k
    return UNKNOWN


def eval_module(c):
    """name -> value for a module's top level.

    Literals, names, simple arithmetic, tuples and lists. Anything else stores
    ('expr', its instructions), so two builds still compare; ('import', module,
    name) records `from module import name`. '*stars*' lists `import *` modules."""
    env, stack, since, stars = {}, [], [], []
    def pop():
        return stack.pop() if stack else UNKNOWN
    for _, op, arg, rep in pyc27.dis(c):
        since.append((op, rep))
        if op == 'LOAD_CONST':
            stack.append(constant(c.consts[arg]))
        elif op == 'LOAD_NAME':
            stack.append(env.get(rep, UNKNOWN))
        elif op in _BINARY:
            b, a = pop(), pop()
            try:
                stack.append(_BINARY[op](a, b) if a is not UNKNOWN and b is not UNKNOWN else UNKNOWN)
            except (TypeError, ValueError, ZeroDivisionError, OverflowError):
                stack.append(UNKNOWN)
        elif op == 'UNARY_NEGATIVE':
            a = pop()
            stack.append(-a if isinstance(a, (int, float)) else UNKNOWN)
        elif op in ('BUILD_TUPLE', 'BUILD_LIST'):
            items = [pop() for _ in range(arg)][::-1]
            stack.append(UNKNOWN if any(x is UNKNOWN for x in items) else tuple(items))
        elif op == 'DUP_TOP':
            stack.append(stack[-1] if stack else UNKNOWN)
        elif op == 'IMPORT_NAME':
            del stack[:]
            stack.append(('module', rep))
        elif op == 'IMPORT_FROM':
            top = stack[-1] if stack else UNKNOWN
            stack.append(('import', top[1], rep) if isinstance(top, tuple) and top[:1] == ('module',) else UNKNOWN)
        elif op == 'IMPORT_STAR':
            top = pop()
            if isinstance(top, tuple) and top[:1] == ('module',):
                stars.append(top[1])
            since = []
        elif op == 'POP_TOP':
            pop()
            since = []
        elif op == 'STORE_NAME':
            v = pop()
            env[rep] = v if v is not UNKNOWN else ('expr', tuple(since[:-1]))
            since = [] if not stack else since
        else:
            stack.append(UNKNOWN)
    env['*stars*'] = stars
    return env


# --- trees ------------------------------------------------------------------------

class Tree(object):
    def __init__(self, root):
        self.root = root
        self.paths = {}  # 'a/b/c' -> absolute .pyc
        self.by_base = collections.defaultdict(list)
        self.mods = {}
        for d, subdirs, files in os.walk(root):
            subdirs.sort()
            for f in sorted(files):
                if f.endswith('.pyc'):
                    full = os.path.join(d, f)
                    rel = os.path.relpath(full, root)[:-4].replace(os.sep, '/')
                    self.paths[rel] = full
                    self.by_base[rel.rsplit('/', 1)[-1]].append(rel)

    def module(self, rel):
        if rel not in self.mods:
            full = self.paths.get(rel)
            self.mods[rel] = Module(full) if full else None
        return self.mods[rel]

    def body_hash(self, rel):
        """The file past the pyc header; equal bytes need no parsing."""
        full = self.paths.get(rel)
        if not full:
            return None
        with open(full, 'rb') as f:
            return hashlib.sha1(f.read()[8:]).hexdigest()

    def resolve_const(self, rel, name, depth=0):
        """A module-level name's value, through imports."""
        m = self.module(rel)
        if m is None or depth > 6:
            return None
        env = m.env()
        v = env.get(name)
        if isinstance(v, tuple) and v[:1] == ('import',):
            got = self.resolve_const(dotted(v[1]), v[2], depth + 1)
            return got if got is not None else v
        if v is None and name != '*stars*':
            for s in env.get('*stars*', ()):
                got = self.resolve_const(dotted(s), name, depth + 1)
                if got is not None:
                    return got
        return v

    def names(self, rel, pattern):
        m = self.module(rel)
        if m is None:
            return []
        return sorted(k for k in m.env() if k != '*stars*' and fnmatch.fnmatchcase(k, pattern))


def dotted(name):
    return name.replace('.', '/')


class Pair(object):
    """OLD and NEW, with the id families read from their constant modules."""

    def __init__(self, old, new):
        self.old, self.new = old, new
        self.same = {}

    def identical(self, rel):
        if rel not in self.same:
            a, b = self.old.body_hash(rel), self.new.body_hash(rel)
            self.same[rel] = a is not None and a == b
        return self.same[rel]

    def differing(self, rel, keys):
        """Keys whose fingerprints differ between the trees."""
        a, b = self.old.module(rel), self.new.module(rel)
        out = []
        for k in keys:
            ea, eb = a.entries.get(k) if a else None, b.entries.get(k) if b else None
            if ea is None or eb is None or ea.fp != eb.fp:
                out.append(k)
        return out


def base_key(k):
    return k.split('#', 1)[0]


def covered(mod, sym):
    """Entries sym names: itself, what it contains, or `Class.sym` for a bare name."""
    if mod is None:
        return []
    out = []
    for k in mod.entries:
        b = base_key(k)
        if b == sym or b.startswith(sym + '.') or b.endswith('.' + sym) or ('.' + sym + '.') in b:
            out.append(k)
    return out


# --- the table --------------------------------------------------------------------

def read_table(path):
    rows, bad = [], []
    with open(path, encoding='utf8') as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip('\n')
            if not line.strip() or line.startswith('#'):
                continue
            cells = line.split('\t')
            if cells[:2] == ['kind', 'client_path']:
                continue
            if len(cells) != 4 or cells[0] not in KINDS or not cells[2] and cells[0] != 'module':
                bad.append('%s:%d: %r' % (path, n, line))
                continue
            rows.append(dict(zip(('kind', 'path', 'symbol', 'lane'), cells), line=n))
    return rows, bad


def check_module(pair, rel):
    a, b = pair.old.module(rel), pair.new.module(rel)
    if a is None and b is None:
        return 'unresolved', 'no such module in either tree'
    if b is None:
        return 'gone', 'module not in NEW'
    if a is None:
        return 'same', 'module is new'
    if pair.identical(rel):
        return 'same', ''
    keys = list(a.entries) + [k for k in b.entries if k not in a.entries]
    moved = pair.differing(rel, keys)
    return ('changed', ', '.join(base_key(k) for k in moved)) if moved else ('same', 'moved only')


def check_symbol(pair, rel, sym):
    a, b = pair.old.module(rel), pair.new.module(rel)
    if a is None and b is None:
        return 'unresolved', 'no such module in either tree'
    if b is None:
        return 'gone', 'module not in NEW'
    keys_a, keys_b = covered(a, sym), covered(b, sym)
    how = 'code'
    if not keys_a and not keys_b:
        vals = [m.env()[sym] for m in (a, b) if m is not None and sym in m.env()]
        if vals:
            if a is None or sym not in a.env():
                return 'same', 'new in NEW'
            if sym not in b.env():
                return 'gone', 'module name left'
            return ('same', 'value') if a.env()[sym] == b.env()[sym] else ('changed', 'module value')
        how = 'uses'
        keys_a, keys_b = using(a, sym), using(b, sym)
    if not keys_a and not keys_b:
        return 'unresolved', 'not found in the module on either side'
    if not keys_b:
        return 'gone', ', '.join(base_key(k) for k in keys_a)
    if not keys_a:
        return 'same', 'new in NEW'
    moved = pair.differing(rel, list(dict.fromkeys(keys_a + keys_b)))
    if moved:
        return 'changed', ('%s: ' % how if how == 'uses' else '') + ', '.join(base_key(k) for k in moved)
    return 'same', how


def fmt_value(v):
    if isinstance(v, tuple) and v[:1] == ('expr',):
        return '<expression>'
    return repr(v)


def check_const(pair, rel, pattern):
    names = sorted(set(pair.old.names(rel, pattern)) | set(pair.new.names(rel, pattern)))
    if not names:
        # `from x import *` modules: a name is only reachable through resolve_const.
        va, vb = pair.old.resolve_const(rel, pattern), pair.new.resolve_const(rel, pattern)
        if va is None and vb is None:
            return 'unresolved', 'no such module-level name'
        names = [pattern]
    moved, gone = [], []
    for n in names:
        va, vb = pair.old.resolve_const(rel, n), pair.new.resolve_const(rel, n)
        if va is not None and vb is None:
            gone.append(n)
        elif va is not None and va != vb:
            moved.append('%s %s -> %s' % (n, fmt_value(va), fmt_value(vb)))
    if gone:
        return 'gone', ', '.join(gone + moved)
    return ('changed', ', '.join(moved)) if moved else ('same', '')


def family_prefix(name):
    for pre, fam in FAMILIES:
        if name.startswith(pre) and name[len(pre):len(pre) + 1].isupper():
            return fam
    return None


class Ids(object):
    """name <-> id for the id families, from one tree's constant modules."""

    def __init__(self, tree):
        self.by_name = {}   # name -> value, every module-level number
        self.by_id = collections.defaultdict(set)  # (family, id) -> names
        for rel in CONST_MODULES:
            m = tree.module(rel)
            if m is None:
                continue
            for name, v in m.env().items():
                if isinstance(v, tuple) and v[:1] == ('import',):
                    v = tree.resolve_const(rel, name)
                if isinstance(v, bool) or not isinstance(v, (int, float)):
                    continue
                self.by_name.setdefault(name, v)
                fam = family_prefix(name)
                if fam and isinstance(v, int):
                    self.by_id[(fam, v)].add(name)


def check_id(ids_old, ids_new, fam, sym):
    if sym.lstrip('-').isdigit():
        i = int(sym)
        a, b = ids_old.by_id.get((fam, i), set()), ids_new.by_id.get((fam, i), set())
        if not a and not b:
            return 'unresolved', '%s id has no constant in either tree' % fam
        if a and not b:
            return 'gone', 'was ' + ', '.join(sorted(a))
        if a - b:
            return 'changed', '%s -> %s' % (', '.join(sorted(a)), ', '.join(sorted(b)))
        return 'same', ''
    a, b = ids_old.by_name.get(sym), ids_new.by_name.get(sym)
    if a is None and b is None:
        return 'unresolved', 'no such constant in either tree'
    if b is None:
        return 'gone', 'was %s' % a
    if a is not None and a != b:
        return 'changed', '%s -> %s' % (a, b)
    return 'same', ''


# --- bundles ----------------------------------------------------------------------

class Bundle(object):
    """types/typedata.tsv and types/typelists.tsv of one content bundle."""

    def __init__(self, root):
        self.types, self.effects, self.attrs, self.lists = {}, {}, {}, {}
        self.extractor = None
        manifest = os.path.join(root, 'MANIFEST.tsv')
        if os.path.exists(manifest):
            with open(manifest, encoding='utf8') as f:
                for line in f:
                    if line.startswith('extractor\t'):
                        self.extractor = line.split('\t')[1].strip()
        self.mods = collections.defaultdict(list)
        self.groups, self.cats = set(), set()
        data = os.path.join(root, 'types', 'typedata.tsv')
        with open(data, encoding='utf8', errors='replace') as f:
            for line in f:
                c = line.rstrip('\n').split('\t')
                if c[0] == 'type':
                    # id, then group, category, published.
                    self.types[int(c[1])] = (c[2], c[3], c[4])
                    self.groups.add(int(c[2]))
                    self.cats.add(int(c[3]))
                elif c[0] == 'effect':
                    self.effects[int(c[1])] = tuple(c[2:])
                elif c[0] == 'modifier':
                    self.mods[int(c[1])].append(tuple(c[2:]))
                elif c[0] in ('attribute', 'attrmeta'):
                    self.attrs.setdefault(int(c[1]), []).append(tuple(c[2:]))
        lists = os.path.join(root, 'types', 'typelists.tsv')
        if os.path.exists(lists):
            with open(lists, encoding='utf8') as f:
                for line in f:
                    if line.strip() and not line.startswith('#'):
                        c = line.rstrip('\n').split('\t')
                        self.lists[c[0]] = tuple(c[1:])


def shared(a, b):
    """Rows of two extractors: a later one may add columns at the end."""
    n = min(len(a), len(b))
    return a[:n] == b[:n]


def bundle_effect(old, new, i):
    if i not in old.effects:
        return None
    if i not in new.effects:
        return 'gone', 'effect row left the bundle'
    if not shared(old.effects[i], new.effects[i]) or sorted(old.mods.get(i, ())) != sorted(new.mods.get(i, ())):
        return 'changed', 'effect or modifier row differs'
    return 'same', ''


def bundle_attr(old, new, i):
    if i not in old.attrs:
        return None
    if i not in new.attrs:
        return 'gone', 'attribute row left the bundle'
    return ('changed', 'attribute row differs') if old.attrs[i] != new.attrs[i] else ('same', '')


def bundle_type(old, new, i):
    if i not in old.types:
        return None
    if i not in new.types:
        return 'gone', 'type row left the bundle'
    a, b = old.types[i], new.types[i]
    if a != b:
        return 'changed', 'group, category, published %s -> %s' % ('/'.join(a), '/'.join(b))
    return 'same', ''


def worst(*results):
    got = [r for r in results if r]
    for s in ('gone', 'changed'):
        for r in got:
            if r[0] == s:
                return r
    return got[0] if got else None


def check_row(pair, ids, bundles, row):
    kind, rel, sym = row['kind'], row['path'], row['symbol']
    if kind == 'module':
        return check_module(pair, rel)
    if kind == 'symbol':
        return check_symbol(pair, rel, sym)
    if kind == 'const':
        return check_const(pair, rel, sym)
    if kind == 'errorkey':
        a, b = pair.old.module(rel), pair.new.module(rel)
        if a is None and b is None:
            return 'unresolved', 'no such module in either tree'
        ha, hb = bool(a and a.uses(sym)), bool(b and b.uses(sym))
        if ha and not hb:
            return 'gone', 'string left the module'
        return ('same', '') if ha or hb else ('unresolved', 'string in neither tree')
    if kind == 'typelist':
        if not bundles:
            return 'skipped', 'needs --old-bundle and --new-bundle'
        a, b = bundles[0].lists.get(sym), bundles[1].lists.get(sym)
        if a is None and b is None:
            return 'unresolved', 'no such list'
        if b is None:
            return 'gone', 'list left the bundle'
        return ('changed', 'list row differs') if a != b else ('same', '')
    got = check_id(ids[0], ids[1], kind if kind != 'group' else id_family(sym), sym)
    if kind == 'group' and bundles and sym.isdigit() and got[0] == 'unresolved':
        # A group with no constant in the client still has types in the bundle.
        if int(sym) in bundles[0].groups:
            return ('same', 'no constant; types in both bundles') if int(sym) in bundles[1].groups \
                else ('gone', 'no type has it now')
    if bundles and sym.isdigit():
        extra = None
        if kind == 'effect':
            extra = bundle_effect(bundles[0], bundles[1], int(sym))
        elif kind == 'attr':
            extra = bundle_attr(bundles[0], bundles[1], int(sym))
        got = worst(got, extra) if got[0] != 'unresolved' or extra is None else extra
    elif kind in ('attr', 'effect') and bundles:
        v = ids[0].by_name.get(sym)
        if isinstance(v, int):
            extra = (bundle_effect if kind == 'effect' else bundle_attr)(bundles[0], bundles[1], v)
            got = worst(got, extra)
    return got


def id_family(sym):
    return family_prefix(sym) or 'group'


# --- citations --------------------------------------------------------------------

CITE = re.compile(r'(?<![\w/.-])((?:\.\.\./|\.\./)?[A-Za-z_][\w./-]*?)\.pyc?:(\d+)(?:-(\d+))?(?![\w])')
#: Our own trees: a citation of these is not a client citation.
OURS = ('tools/', 'server/', 'docs/', 'crates/', 'out/', 'findings/', 'content/', 'containers/')
WORD = re.compile(r'`[^`\n]*`')
NAME = re.compile(r'[A-Za-z_][A-Za-z0-9_]{3,}')


class Cite(object):
    __slots__ = ('file', 'lineno', 'token', 'path', 'line', 'end', 'near', 'status',
                 'module', 'entry', 'via', 'new_line', 'new_end')


def source_files(repo):
    roots = [(os.path.join(repo, 'server', 'crates'), '.rs'),
             (os.path.join(repo, 'tools', 'client-ci', 'scenarios'), '.py')]
    for root, ext in roots:
        for d, subdirs, files in os.walk(root):
            subdirs[:] = sorted(s for s in subdirs if s != 'target')
            for f in sorted(files):
                if f.endswith(ext):
                    yield os.path.join(d, f)


def find_citations(repo):
    out = []
    for full in source_files(repo):
        with open(full, encoding='utf8', errors='replace') as f:
            lines = f.read().split('\n')
        rel = os.path.relpath(full, repo).replace(os.sep, '/')
        for i, line in enumerate(lines):
            if '.py' not in line:
                continue
            for m in CITE.finditer(line):
                path = m.group(1)
                while path.startswith(('../', './', '.../', '/')):
                    path = path.split('/', 1)[1]
                if path.startswith(OURS):
                    continue
                c = Cite()
                c.file, c.lineno, c.token, c.path = rel, i + 1, m.group(0), path
                c.line, c.end = int(m.group(2)), int(m.group(3)) if m.group(3) else None
                near = lines[max(0, i - 2):i + 3]
                c.near = sorted({n for s in near for w in WORD.findall(s) for n in NAME.findall(w)})
                out.append(c)
    return out


def candidates(tree, path):
    base = path.rsplit('/', 1)[-1]
    got = tree.by_base.get(base, ())
    if '/' not in path:
        return list(got)
    return [p for p in got if p == path or p.endswith('/' + path)]


def called_sites(repo):
    """{path: lines} from findings/client-calls.tsv call_site cells."""
    out = collections.defaultdict(set)
    path = os.path.join(repo, 'findings', 'client-calls.tsv')
    if os.path.exists(path):
        with open(path, encoding='utf8') as f:
            for line in f:
                c = line.split('\t')
                if len(c) > 2 and '.py:' in c[2]:
                    p, n = c[2].rsplit('.py:', 1)
                    if n.isdigit():
                        out[p].add(int(n))
    return out


def narrow(tree, cite, cands, sites, cited_here):
    """One candidate for a citation, or None. Each step is evidence, not a tiebreak."""
    if len(cands) == 1:
        return cands[0], 'path' if '/' in cite.path else 'basename'
    fits = []
    for p in cands:
        m = tree.module(p)
        if m is not None and cite.line <= m.last_line():
            fits.append(p)
    if len(fits) == 1:
        return fits[0], 'line'
    exact = [p for p in fits if cite.line in sites.get(p, ())]
    if len(exact) == 1:
        return exact[0], 'client-calls line'
    here = [p for p in fits if p in cited_here]
    if len(here) == 1:
        return here[0], 'same file cites it'
    if cite.near:
        scored = []
        for p in fits:
            m = tree.module(p)
            have = set(k.rsplit('.', 1)[-1] for k in map(base_key, m.entries)) | set(m.env())
            for e in m.entries.values():
                have.update(text(x) for x in e.code.names)
            scored.append((len(have.intersection(cite.near)), p))
        top = max((s for s, _ in scored), default=0)
        best = [p for s, p in scored if s == top]
        if top and len(best) == 1:
            return best[0], 'symbol nearby'
    called = [p for p in fits if p in sites]
    if len(called) == 1:
        return called[0], 'client-calls path'
    return None, None


def map_line(old, new, n):
    """Where line n of an unchanged code object is in NEW."""
    rows = [r for r in old.rows if r[1] <= n]
    if not rows:
        return n + new.first - old.first
    off, line = max(rows, key=lambda r: (r[1], r[0]))
    at = [r for r in new.rows if r[0] <= off]
    return at[-1][1] + n - line if at else n + new.first - old.first


def line_slices(c, rows):
    """line -> its instructions, for a code object. Offsets are left out."""
    starts = [r[0] for r in rows]
    out = collections.defaultdict(list)
    for off, op, _, rep in pyc27.dis(c):
        i = bisect.bisect_right(starts, off) - 1
        out[rows[i][1]].append((op, rep))
    # The body's implicit `return None` sits on its last line.
    tail = rows[-1][1]
    if out[tail][-2:] == [('LOAD_CONST', 'None'), ('RETURN_VALUE', '')]:
        del out[tail][-2:]
    return {k: tuple(v) for k, v in out.items() if v}


def body_lines(old, new, first, last):
    """A module or class body changed around the cited lines: did those lines?

    A body holds many unrelated statements, so one new constant must not fire every
    citation into the file. Returns (new first, new last), or None if a cited line
    has no equal line in NEW."""
    a, b = line_slices(old.code, old.rows), line_slices(new.code, new.rows)
    where = collections.defaultdict(list)
    for n, sl in b.items():
        where[sl].append(n)
    lines = [n for n in sorted(a) if first <= n <= last]
    if not lines:
        lines = [max([n for n in a if n <= first] or [min(a)])]
    moved, shift = [], 0
    for n in lines:
        hits = where.get(a[n])
        if not hits:
            return None
        to = min(hits, key=lambda h: abs(h - (n + shift)))
        shift = to - n
        moved.append((n, to))
    return first + moved[0][1] - moved[0][0], (last + moved[-1][1] - moved[-1][0])


def judge(pair, cite, rel):
    """Sets a resolved citation's status."""
    cite.module = rel
    a, b = pair.old.module(rel), pair.new.module(rel)
    if a is None:
        cite.status = 'unresolved'
        return
    if b is None:
        cite.status, cite.entry = 'gone', '<module>'
        return
    e = a.at_line(cite.line)
    cite.entry = base_key(e.key)
    if pair.identical(rel):
        cite.status = 'same'
        return
    n = b.entries.get(e.key)
    if n is None:
        cite.status = 'gone'
    elif n.fp != e.fp:
        cite.status = 'changed'
        if not e.code.flags & CO_OPTIMIZED:
            got = body_lines(e, n, cite.line, cite.end or cite.line)
            if got:
                cite.status, cite.new_line, cite.new_end = 'same', got[0], got[1] if cite.end else None
    else:
        cite.status = 'same'
        cite.new_line = map_line(e, n, cite.line)
        if cite.end:
            cite.new_end = map_line(e, n, cite.end)


def check_citations(pair, repo):
    cites = find_citations(repo)
    sites = called_sites(repo)
    tree = pair.old
    here = collections.defaultdict(set)
    for c in cites:
        c.status, c.module, c.entry, c.via, c.new_line, c.new_end = None, None, None, None, None, None
        if '/' in c.path:
            cands = candidates(tree, c.path)
            if len(cands) == 1:
                here[c.file].add(cands[0])
    for c in cites:
        cands = candidates(tree, c.path)
        if not cands:
            c.status = 'unknown'
            continue
        # Every candidate byte-identical: whichever is meant has not moved.
        if all(pair.identical(p) for p in cands):
            c.status = 'same'
            continue
        rel, via = narrow(tree, c, cands, sites, here[c.file])
        if rel:
            c.via = via
            judge(pair, c, rel)
            continue
        # No single file. If the line's code is equal in all of them, no answer is
        # needed; otherwise a reader picks.
        open_ = []
        for p in cands:
            if pair.identical(p):
                continue
            m = pair.old.module(p)
            if m is None or c.line > m.last_line():
                continue
            t = Cite()
            t.line, t.end = c.line, None
            t.new_line, t.new_end, t.entry, t.status, t.module = None, None, None, None, None
            judge(pair, t, p)
            if t.status != 'same':
                open_.append('%s (%s)' % (p, t.entry))
        c.status = 'ambiguous' if open_ else 'same'
        c.entry = '; '.join(open_[:6])
    return cites


def shift_lines(cites):
    out = []
    for c in cites:
        if c.status != 'same' or c.new_line is None:
            continue
        if c.new_line == c.line and (c.end is None or c.new_end == c.end):
            continue
        new = c.token.replace(':%d' % c.line, ':%d' % c.new_line, 1)
        if c.end:
            new = new.replace('-%d' % c.end, '-%d' % c.new_end, 1)
        old = re.sub(r'([/.&\[\]*])', r'\\\1', c.token)
        out.append("sed -i '%ds/%s/%s/' %s" % (c.lineno, old, new.replace('/', r'\/'), c.file))
    return sorted(set(out))


# --- Rust constants ---------------------------------------------------------------

RUST_CONST = re.compile(
    r'^\s*(?:pub(?:\([a-z ]+\))?\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*'
    r'(?:(i8|i16|i32|i64|u8|u16|u32|u64|usize|isize|f32|f64)\s*=\s*(-?[0-9][0-9_.eE+-]*)'
    r'(?:[iuf](?:8|16|32|64|size))?\s*;'
    r'|&?(?:\'static\s+)?\[\s*(?:i32|i64|u32|u64)\s*(?:;\s*\w+\s*)?\]\s*=\s*&?\[([0-9_,\s]*)\]\s*;)',
    re.M)
MOD_OPEN = re.compile(r'^\s*(?:pub(?:\([a-z ]+\))?\s+)?mod\s+([a-z_]+)\s*\{', re.M)


class RustConst(object):
    __slots__ = ('file', 'lineno', 'name', 'value', 'family', 'bound', 'array')


def rust_family(name, mod):
    words = name.split('_')
    if mod == 'attr' or 'ATTR' in words or 'ATTRIBUTE' in words:
        return 'attr'
    if mod == 'effect' or 'EFFECT' in words:
        return 'effect'
    for w, fam in (('GROUP', 'group'), ('GROUPS', 'group'), ('CATEGORY', 'category'),
                   ('CATEGORIES', 'category'), ('TYPE', 'type'), ('TYPES', 'type')):
        if w in words:
            return fam
    return None


def number(s):
    s = s.replace('_', '')
    try:
        return int(s)
    except ValueError:
        try:
            return float(s)
        except ValueError:
            return None


def comment_above(lines, i):
    """The contiguous comment above line i (0-based), with the line's own tail."""
    got = [lines[i].split('//', 1)[1]] if '//' in lines[i] else []
    j = i - 1
    while j >= 0 and lines[j].lstrip().startswith(('//', '#[')):
        got.append(lines[j])
        j -= 1
    return '\n'.join(got)


def rust_consts(repo):
    out = []
    root = os.path.join(repo, 'server', 'crates')
    for d, subdirs, files in os.walk(root):
        subdirs[:] = sorted(s for s in subdirs if s not in ('target', 'tests', 'benches'))
        for f in sorted(files):
            if not f.endswith('.rs'):
                continue
            full = os.path.join(d, f)
            with open(full, encoding='utf8', errors='replace') as fh:
                body = fh.read()
            cut = re.search(r'^\s*#\[cfg\(test\)\]\s*\n\s*(?:pub\s+)?mod\s', body, re.M)
            if cut:
                body = body[:cut.start()]
            lines = body.split('\n')
            opens = [(m.start(), m.group(1)) for m in MOD_OPEN.finditer(body)]
            rel = os.path.relpath(full, repo).replace(os.sep, '/')
            for m in RUST_CONST.finditer(body):
                lineno = body.count('\n', 0, m.start(1)) + 1
                mod = None
                # A `mod attr {` block ends at its first `}` at column 0.
                for pos, name in opens:
                    if pos < m.start() and '\n}' not in body[pos:m.start()]:
                        mod = name
                family = rust_family(m.group(1), mod)
                vals = [number(m.group(3))] if m.group(3) else \
                    [number(x) for x in re.split(r'[,\s]+', m.group(4)) if x]
                words = set()
                for s in WORD.findall(comment_above(lines, lineno - 1)):
                    words.update(re.findall(r'[A-Za-z_][A-Za-z0-9_]*', s))
                for v in vals:
                    if v is None:
                        continue
                    c = RustConst()
                    c.file, c.lineno, c.name, c.value, c.family, c.bound = rel, lineno, m.group(1), v, family, words
                    c.array = not m.group(3)
                    out.append(c)
    return out


def check_constants(pair, ids, bundles, repo, emit):
    """Per const: a client name the comment names and OLD confirms must hold in NEW,
    and an id's constant name must survive. A value OLD does not confirm is not bound
    to that name, and is only listed."""
    old, new = ids
    counts = collections.Counter()
    seen = set()
    for c in rust_consts(repo):
        where = '%s:%d' % (c.file, c.lineno)
        counts['consts'] += 1
        known = [w for w in sorted(c.bound) if w in old.by_name]
        hit = [w for w in known if old.by_name[w] == c.value]
        if hit:
            counts['bound'] += 1
            for w in hit:
                if w not in new.by_name:
                    emit('constant', 'gone', where, c.name, '%s left the client' % w)
                elif new.by_name[w] != c.value:
                    emit('constant', 'changed', where, c.name, '%s %s -> %s' % (w, c.value, new.by_name[w]))
        elif known and len(c.bound) == 1 and not c.array:
            counts['disagrees'] += 1
            emit('constant', 'disagrees', where, c.name, '%s is %s in OLD, Rust says %s' % (known[0], old.by_name[known[0]], c.value))
        if c.family is None or not isinstance(c.value, int):
            continue
        key = (c.family, c.value)
        if key in seen:
            continue
        seen.add(key)
        a, b = old.by_id.get(key, set()), new.by_id.get(key, set())
        if a:
            counts['ids'] += 1
            if a - b:
                emit('constant', 'changed' if b else 'gone', where, c.name,
                     '%s %d was %s, now %s' % (c.family, c.value, ', '.join(sorted(a)), ', '.join(sorted(b)) or 'nothing'))
        if not bundles:
            continue
        check = {'effect': bundle_effect, 'attr': bundle_attr, 'type': bundle_type}.get(c.family)
        got = check(bundles[0], bundles[1], c.value) if check else None
        if c.family in ('group', 'category'):
            pool = 'groups' if c.family == 'group' else 'cats'
            if c.value in getattr(bundles[0], pool):
                got = ('same', '') if c.value in getattr(bundles[1], pool) else ('gone', 'no type has it now')
        if got:
            counts['rows'] += 1
            if got[0] != 'same':
                emit('constant', got[0], where, c.name, '%s %d: %s' % (c.family, c.value, got[1]))
    return counts


# --- the run ----------------------------------------------------------------------

def default_out(new):
    new = new.rstrip('/')
    if os.path.basename(new) == 'pyc' and os.path.dirname(new) not in ('', 'out'):
        return os.path.join(os.path.dirname(new), 'recheck')
    return os.path.join('out', 'port-recheck', os.path.basename(new))


def render(args, found, stats):
    by = collections.defaultdict(list)
    for r in found:
        by[(r[0], r[1])].append(r)
    out = ['# Port recheck', '', '`%s` -> `%s`' % (args.old, args.new), '']
    for s in stats:
        out.append('- ' + s)
    out.append('')
    titles = (('table', 'Table rows'), ('citation', 'Citations in changed code'),
              ('constant', 'Constants'))
    for sec, title in titles:
        rows = [r for st in READER for r in by.get((sec, st), ())]
        out.append('## %s: %d need a reader' % (title, len(rows)))
        out.append('')
        for r in rows[:SHOWN]:
            out.append('- %s `%s` %s: %s' % (r[1], r[2], r[3], r[4]))
        if len(rows) > SHOWN:
            out.append('- ... and %d more (recheck.tsv has all of them)' % (len(rows) - SHOWN))
        out.append('')
    for sec, st, title in (('table', 'unresolved', 'Stale table rows: fix them'),
                           ('table', 'skipped', 'Table rows not checked'),
                           ('constant', 'disagrees', 'Rust and OLD disagree on a name (not a port change)'),
                           ('citation', 'unresolved', 'Citations that resolve to no module')):
        rows = by.get((sec, st), ())
        if rows:
            out.append('## %s: %d' % (title, len(rows)))
            out.append('')
            out.extend('- `%s` %s: %s' % (r[2], r[3], r[4]) for r in rows[:SHOWN])
            out.append('')
    return '\n'.join(out)


def run(args):
    for t in (args.old, args.new):
        if not os.path.isdir(t):
            print('port-recheck: no tree %s' % t, file=sys.stderr)
            return 2
    if bool(args.old_bundle) != bool(args.new_bundle):
        print('port-recheck: --old-bundle and --new-bundle go together', file=sys.stderr)
        return 2
    table = args.table or os.path.join(args.repo, 'findings', 'port-recheck.tsv')
    rows, bad = read_table(table)
    if bad:
        print('port-recheck: malformed rows:\n  ' + '\n  '.join(bad), file=sys.stderr)
        return 1
    pair = Pair(Tree(args.old), Tree(args.new))
    ids = (Ids(pair.old), Ids(pair.new))
    bundles = None
    if args.old_bundle:
        have = [os.path.exists(os.path.join(b, 'types', 'typedata.tsv'))
                for b in (args.old_bundle, args.new_bundle)]
        if all(have):
            bundles = (Bundle(args.old_bundle), Bundle(args.new_bundle))
    found, stats = [], []

    def emit(section, status, where, what, detail):
        found.append((section, status, where, what, detail))

    tally = collections.Counter()
    for r in rows:
        st, detail = check_row(pair, ids, bundles, r)
        tally[st] += 1
        emit('table', st, '%s %s' % (r['kind'], r['path']), r['symbol'], '%s [%s]' % (detail, r['lane']))
    stats.append('table: %d rows; %s' % (len(rows), ', '.join('%s %d' % kv for kv in sorted(tally.items()))))

    shifts = []
    if args.no_citations:
        stats.append('citations: skipped')
    else:
        cites = check_citations(pair, args.repo)
        c_tally = collections.Counter(c.status for c in cites)
        stats.append('citations: %d; %s' % (len(cites), ', '.join('%s %d' % kv for kv in sorted(c_tally.items()))))
        via = collections.Counter(c.via for c in cites if c.via)
        stats.append('resolved by: ' + (', '.join('%s %d' % kv for kv in sorted(via.items())) or 'none'))
        for c in cites:
            if c.status in READER or c.status == 'unresolved':
                emit('citation', c.status, '%s:%d' % (c.file, c.lineno), c.token,
                     '%s %s' % (c.module or '', c.entry or ''))
        shifts = shift_lines(cites)
        stats.append('line shifts in unchanged code: %d (shifts.sh)' % len(shifts))

    if args.no_consts:
        stats.append('constants: skipped')
    else:
        counts = check_constants(pair, ids, bundles, args.repo, emit)
        stats.append('constants: %d Rust values, %d bound to a client name, %d disagree; %d ids named, %d bundle rows%s'
                     % (counts['consts'], counts['bound'], counts['disagrees'], counts['ids'], counts['rows'],
                        '' if bundles else ' (no bundles: rows not checked)'))

    if bundles and bundles[0].extractor != bundles[1].extractor:
        stats.append('**bundles are from different extractors**: a row difference may be the extractor, not the client')
    need = [r for r in found if r[1] in READER]
    out = args.out or default_out(args.new)
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, 'recheck.tsv'), 'w', encoding='utf8') as f:
        f.write('section\tstatus\twhere\twhat\tdetail\n')
        for r in found:
            if r[1] != 'same':
                f.write('\t'.join(str(x).replace('\t', ' ') for x in r) + '\n')
    with open(os.path.join(out, 'recheck.md'), 'w', encoding='utf8') as f:
        f.write(render(args, found, stats) + '\n')
    with open(os.path.join(out, 'shifts.sh'), 'w', encoding='utf8') as f:
        f.write('#!/bin/sh\n# Comment-only line shifts. Run from the repo root.\n' + ''.join(s + '\n' for s in shifts))
    for s in stats:
        print('port-recheck: ' + s)
    print('port-recheck: %d need a reader; see %s' % (len(need), os.path.join(out, 'recheck.md')))
    return 3 if need else 0


def main(argv=None):
    p = argparse.ArgumentParser(description='Recheck what we rely on across two client builds.')
    p.add_argument('old')
    p.add_argument('new')
    p.add_argument('--table')
    p.add_argument('--old-bundle')
    p.add_argument('--new-bundle')
    p.add_argument('--out')
    p.add_argument('--repo', default='.')
    p.add_argument('--no-citations', action='store_true')
    p.add_argument('--no-consts', action='store_true')
    return run(p.parse_args(argv))


if __name__ == '__main__':
    sys.exit(main())
