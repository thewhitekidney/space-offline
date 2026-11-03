"""Shared helpers for the findings generators (lane findings-gen, issue #18).

`tools/scan_push.py`, `scan_client_callable.py`, `scan_session_keys.py` and
`scan_user_errors.py` all read the unpacked client bytecode (`out/pyc`, from
`tools/extract_code.py`) with `tools/pyc27.py` and write one TSV under
`findings/`. What they share lives here: walking a tree, the client's own line
numbers from `co_lnotab`, the class-body view of a module, and a small
abstract stack machine for `__exportedcalls__ = {...}` style declarations.

Nothing here knows a build number. A build's identity comes from the caller
(`--label`), and the branch part of a code object's embedded file name is
found by shape (`...\\release\\<branch>\\...`), not by a constant, so a new
release line needs no edit.
"""
# The stdlib has to win over `tools/` (`tools/dis.py` shadows the standard
# library's `dis`; see its header). Append, never insert at 0.
import os as _os
import sys as _sys

_HERE = _os.path.dirname(_os.path.abspath(__file__))
if _HERE not in _sys.path:
    _sys.path.append(_HERE)

import re
import pyc27

DEFAULT_PYC = '/work/out/pyc'
_BRANCH_RE = re.compile(rb'\\release\\[^\\]+\\')


def dec(b):
    """bytes -> str, leave str alone."""
    return b.decode('utf8', 'replace') if isinstance(b, bytes) else b


def rel_filename(code):
    """The code object's embedded file name with the build agent's prefix and the
    `release\\<branch>\\` part removed, with `/` separators. Packaged code keeps
    its `packages/...` prefix exactly as the client sees it."""
    fn = code.filename
    if isinstance(fn, str):
        fn = fn.encode('utf8', 'replace')
    m = _BRANCH_RE.search(fn)
    if m:
        fn = fn[m.end():]
    return fn.decode('utf8', 'replace').replace('\\', '/')


def iter_pyc(root):
    """Yield (relative path without .pyc, absolute path) for every .pyc, sorted
    so output never depends on directory order."""
    out = []
    for dp, dns, fns in _os.walk(root):
        dns.sort()
        for f in fns:
            if f.endswith('.pyc'):
                p = _os.path.join(dp, f)
                out.append((_os.path.relpath(p, root)[:-4].replace(_os.sep, '/'), p))
    out.sort()
    return out


def load_tree(root):
    """Yield (relpath, toplevel code object); files that do not parse are
    skipped and counted in `load_tree.failed`."""
    load_tree.failed = []
    for rel, p in iter_pyc(root):
        try:
            yield rel, pyc27.load_pyc(p)
        except Exception:
            load_tree.failed.append(rel)


load_tree.failed = []


def line_table(code):
    """[(addr, line)] from co_lnotab, ascending by addr."""
    out = []
    addr = 0
    line = code.firstlineno
    lt = code.lnotab or b''
    out.append((0, line))
    for i in range(0, len(lt) - 1, 2):
        addr += lt[i]
        inc = lt[i + 1]
        if inc >= 128:
            inc -= 256
        line += inc
        out.append((addr, line))
    return out


def line_at(code, offset, table=None):
    """The client's own source line for a bytecode offset (last entry whose
    address is <= offset), the way CPython's PyCode_Addr2Line does it."""
    table = table or line_table(code)
    best = table[0][1]
    for a, ln in table:
        if a > offset:
            break
        best = ln
    return best


def is_class_body(code):
    """A Python 2.7 class body is the code object that contains LOAD_LOCALS."""
    return any(op == 'LOAD_LOCALS' for _o, op, _a, _r in pyc27.dis(code))


def class_bodies(top):
    """Every class body in a module as (code, instructions)."""
    out = []
    for c in pyc27.walk(top):
        ins = list(pyc27.dis(c))
        if any(op == 'LOAD_LOCALS' for _o, op, _a, _r in ins):
            out.append((c, ins))
    return out


def method_defs(code, ins):
    """Every `LOAD_CONST <code>; MAKE_FUNCTION n; STORE_NAME name` of a class
    body, in source order, as [(name, function code, n_defaults, offset)]. A
    method defined twice appears twice: both definitions are real code the
    client runs at some point, and callers that want the live one use
    `methods_of`. A decorator application (`CALL_FUNCTION` between the
    `MAKE_FUNCTION` and the store) is looked through."""
    out = []
    for i, (off, op, arg, rep) in enumerate(ins):
        if op != 'STORE_NAME' or i < 2:
            continue
        j = i - 1
        while j > 1 and ins[j][1] == 'CALL_FUNCTION':   # @decorator(func)
            j -= 1
        if ins[j][1] != 'MAKE_FUNCTION' or ins[j - 1][1] != 'LOAD_CONST':
            continue
        k = code.consts[ins[j - 1][2]]
        if isinstance(k, pyc27.Code):
            out.append((rep, k, ins[j][2], off))
    return out


def methods_of(code, ins):
    """{name: (function code, n_defaults, offset)}; the last definition wins, as
    it does at run time."""
    return {n: (k, d, o) for n, k, d, o in method_defs(code, ins)}


class Sym(object):
    """A symbolic stack value for the tiny evaluator below."""
    __slots__ = ('kind', 'val')

    def __init__(self, kind, val=None):
        self.kind = kind
        self.val = val

    def __repr__(self):
        return 'Sym(%s,%r)' % (self.kind, self.val)


def fmt_sym(s):
    """Render a Sym the way findings/client-callable.tsv writes metadata."""
    k, v = s.kind, s.val
    if k == 'name':
        return v
    if k == 'const':
        if isinstance(v, bytes):
            return v.decode('utf8', 'replace')
        return repr(v) if not isinstance(v, (int, float)) else str(v)
    if k == 'attr':
        return fmt_sym(v[0]) + '.' + v[1]
    if k == 'or':
        return '|'.join(fmt_sym(x) for x in v)
    if k == 'list':
        return '[' + ', '.join(fmt_sym(x) for x in v) + ']'
    if k == 'dict':
        return '{' + ', '.join('%s: %s' % (fmt_sym(a), fmt_sym(b)) for a, b in sorted(v, key=lambda ab: fmt_sym(ab[0]))) + '}'
    return '?'


def eval_assignments(code, ins, wanted):
    """Symbolically evaluate the statements of a class body that assign one of
    the `wanted` names with a list/dict/const display. Returns
    {name: (Sym, offset of the first instruction of the value, offset of the
    STORE_NAME)}. Anything the machine does not understand becomes Sym('?')
    and the stack is rebuilt from there; a jump clears it, so an assignment
    behind a branch is simply not seen."""
    stack = []
    start = []          # offset where each stack slot began
    out = {}
    for off, op, arg, rep in ins:
        if op == 'LOAD_NAME' or op == 'LOAD_GLOBAL':
            stack.append(Sym('name', rep)); start.append(off)
        elif op == 'LOAD_CONST':
            k = None
            try:
                k = _const_of(code, arg)
            except IndexError:
                pass
            stack.append(Sym('const', k)); start.append(off)
        elif op == 'BUILD_LIST' or op == 'BUILD_TUPLE':
            n = arg
            if n > len(stack):
                stack = [Sym('?')]; start = [off]
                continue
            items = stack[len(stack) - n:] if n else []
            s0 = start[len(start) - n] if n else off
            del stack[len(stack) - n:]; del start[len(start) - n:]
            stack.append(Sym('list', list(items))); start.append(s0)
        elif op == 'BUILD_MAP':
            stack.append(Sym('dict', [])); start.append(off)
        elif op == 'STORE_MAP':
            if len(stack) >= 3:
                key = stack.pop(); val = stack.pop(); start.pop(); start.pop()
                if stack[-1].kind == 'dict':
                    stack[-1].val.append((key, val))
        elif op == 'BINARY_OR':
            if len(stack) >= 2:
                b = stack.pop(); a = stack.pop(); start.pop()
                items = (a.val if a.kind == 'or' else [a]) + [b]
                stack.append(Sym('or', items))
        elif op == 'LOAD_ATTR':
            if stack:
                a = stack.pop()
                stack.append(Sym('attr', (a, rep)))
        elif op == 'STORE_NAME':
            if stack and rep in wanted:
                out[rep] = (stack[-1], start[-1], off)
            if stack:
                stack.pop(); start.pop()
        elif op in ('RETURN_VALUE', 'POP_TOP'):
            if stack:
                stack.pop(); start.pop()
        elif op.startswith('JUMP') or op.startswith('POP_JUMP') or op in ('FOR_ITER', 'SETUP_LOOP', 'SETUP_EXCEPT'):
            stack = []; start = []
        else:
            # unknown effect on the stack: forget it all rather than guess
            stack = []; start = []
    return out


def _const_of(code, idx):
    k = code.consts[idx]
    return k


def tsv_line(cols):
    """One TSV row. Tabs and newlines inside a cell are flattened to a space,
    because the findings tables are read with a plain split on tab."""
    return '\t'.join(str(c).replace('\t', ' ').replace('\n', ' ').replace('\r', ' ') for c in cols)


def write_tsv(path, lines):
    """Write lines with a single trailing newline. `-` is stdout."""
    text = '\n'.join(lines) + '\n'
    if path == '-':
        _sys.stdout.write(text)
        return
    d = _os.path.dirname(path)
    if d:
        _os.makedirs(d, exist_ok=True)
    with open(path, 'w', encoding='utf8', newline='\n') as f:
        f.write(text)
