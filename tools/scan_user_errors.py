"""Generate findings/user-errors.tsv: the client's `UserError` vocabulary (lane
findings-gen, issue #18; the table's meaning is `docs/re/errors.md` §2).

    tools/analysis.sh python3 -I tools/scan_user_errors.py \
        [--pyc out/pyc] [--seed findings/user-errors.tsv] [--out findings/user-errors.tsv] [--client /client]

One row per message key, sorted case-insensitively. Three readers feed a row
and `source` says which (`fsd`, `loc`, `pyc`, joined with `+` in that order):

- **fsd**: `res:/staticdata/dialogs.static` through `tools/fsd.py`. It supplies
  `dialog_id`, `dialog_type`, `suppressable`, `closable`, `title_id`,
  `body_id`; `presentation` is `dialog_type` mapped as in errors.md §2.2
  (modal / logline / silent, `unknown` where there is no row).
- **loc**: the `UI/Messages/<dialogID>/<Key>[Title|Body]` labels of
  `localization_fsd_main.pickle`. A label-only key takes its ids from the
  labels; `loc_args` is the sorted set of `{token}` names in the *body* text
  of `localization_fsd_en-us.pickle` (typed tokens keep their `[type]` prefix).
- **pyc**: the bytecode. A *raise site* is any load of the name `UserError`
  whose next instruction is `LOAD_CONST <bytes>`; `raise_sites` counts them,
  `raised_at` is the first in tree order (client file:line from `co_lnotab`),
  `args_at_raise` the dict keys stored before the first store or jump after it
  (approximate, as errors.md says). A *catch site* compares `.msg` or
  `.args[0]` with a constant (or a tuple of constants) using
  `==`/`!=`/`in`/`not in`; `caught_by_key_at` lists the first three sites
  sorted, then `+N`; `caught_in` the first three function names sorted;
  `caught_via` the forms seen. `args_read_by_handler` is the set of
  `.dict.get('k')` / `.dict['k']` constants in the functions that catch it by
  `.msg` (an `args[0]`-only catch is not read: the committed table never did),
  and `handler_fn_refs` the names from HANDLER_REFS those functions call.

`ui_behaviour` is prose written by a human who read the handler; it cannot be
generated. It comes from `--seed` (the previous table), keyed by key, and is
`-` for a key the seed does not know. A seeded key whose catch site moved or
vanished is reported on stderr as drift, because its prose may no longer be true.

**Known gap, kept on purpose.** Only `.msg == K` (attribute on the left) is
recognised, as in the committed table. `K == e.msg` is not; `--reversed` also
reads that form. On the pinned build it adds the catch of `WarpDisrupted`
(autopilot.py) and a bogus row `too large a packet` (a socket error, not a
UserError), which is why it is off by default.

Needs the analysis container (`/client`, `/client-root/ResFiles`) for the two
resource readers; `--no-resources` skips them and leaves their columns empty, so only the
bytecode columns mean anything (tools/port_surface.py knows to compare those
only).
"""
import argparse
import collections
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import pyc27
import findings_lib as L

COLUMNS = ['key', 'source', 'dialog_id', 'dialog_type', 'presentation', 'suppressable', 'closable',
           'title_id', 'body_id', 'loc_args', 'raise_sites', 'raised_at', 'args_at_raise',
           'args_read_by_handler', 'caught_by_key_at', 'caught_in', 'caught_via',
           'handler_fn_refs', 'ui_behaviour']
MODAL = ('info', 'infomodal', 'warning', 'question', 'error', 'fatal', 'windowhelp')
LOGLINE = ('notify', 'hint', 'event')
CMP = ('<', '<=', '==', '!=', '>', '>=', 'in', 'not in', 'is', 'is not', 'exception match', 'BAD')
LOAD_OPS = ('LOAD_GLOBAL', 'LOAD_ATTR', 'LOAD_NAME', 'LOAD_FAST', 'LOAD_DEREF')
STOPS = ('JUMP_FORWARD', 'JUMP_ABSOLUTE', 'POP_JUMP_IF_FALSE', 'POP_JUMP_IF_TRUE',
         'JUMP_IF_FALSE_OR_POP', 'JUMP_IF_TRUE_OR_POP', 'STORE_FAST', 'STORE_NAME', 'STORE_GLOBAL',
         'STORE_ATTR', 'STORE_SUBSCR', 'STORE_DEREF', 'RETURN_VALUE', 'RAISE_VARARGS', 'POP_TOP',
         'FOR_ITER', 'SETUP_EXCEPT', 'POP_BLOCK')
# Calls in a handling function that the vocabulary lane judged worth naming:
# they say what the client does after it catches the key (prompt, scatter an
# event, re-enter the session machinery, reset the autopilot ...).
HANDLER_REFS = ('AbortUndock', 'Close', 'EnableScreen', 'MakeShipActive', 'Maximize',
                'PerformSessionChange', 'RefreshUIOnUpdate', 'ResetSessionChangeTimer', 'ScatterEvent',
                'SetOff', 'SleepWallclock', 'clear', 'message_player', 'prompt_player_fsd_dialog')
TOKEN_RE = re.compile(r'\{([^{}]*)\}')


def presentation(dialog_type):
    if dialog_type in MODAL:
        return 'modal'
    if dialog_type in LOGLINE:
        return 'logline'
    if dialog_type == 'audio':
        return 'audio-only'
    return 'unknown'


def _s(v):
    return '-' if v is None or v == '' else str(v)


# ----------------------------------------------------------------- resources

def read_resources(client_root):
    """({key: fsd row dict}, {key: {'num', 'title', 'body'}}, {messageID: text})."""
    import fsd
    c = fsd.Client(client_root)
    dlg = c.load('res:/staticdata/dialogs.static')
    main = c.load('res:/localizationfsd/localization_fsd_main.pickle')
    _lang, msgs = c.load('res:/localizationfsd/localization_fsd_en-us.pickle')
    fsd_rows = {}
    for k in dlg.keys():
        r = dlg[k]
        fsd_rows[k] = {'dialogID': r.dialogID, 'dialogType': r.dialogType,
                       'suppressable': r.suppressable, 'closable': r.closable,
                       'titleID': r.titleID, 'bodyID': r.bodyID}
    labels = labels_of(main['labels'].values())
    texts = {mid: v[0] for mid, v in msgs.items()}
    return fsd_rows, labels, texts


def labels_of(label_rows):
    """`UI/Messages/<num>/<Key>[Title|Body]` rows -> {key: {'num','title','body'}}.
    A leaf with neither suffix is the body, as `loctable.py errors` reads it."""
    out = {}
    for row in label_rows:
        path = row['FullPath'] + '/' + row['label'] if row.get('FullPath') else row['label']
        parts = path.split('/')
        if len(parts) != 4 or parts[0] != 'UI' or parts[1] != 'Messages':
            continue
        num, leaf = parts[2], parts[3]
        if leaf.endswith('Title'):
            key, slot = leaf[:-5], 'title'
        elif leaf.endswith('Body'):
            key, slot = leaf[:-4], 'body'
        else:
            key, slot = leaf, 'body'
        d = out.setdefault(key, {'num': num})
        d[slot] = row['messageID']
    return out


def loc_args(body_id, texts):
    if body_id is None or body_id not in texts:
        return '-'
    return ','.join(sorted(set(TOKEN_RE.findall(texts[body_id])))) or '-'


# ------------------------------------------------------------------ bytecode

def _bytes_consts(code, idx):
    k = code.consts[idx]
    if isinstance(k, bytes):
        return [k.decode('utf8', 'replace')]
    if isinstance(k, tuple) and k and all(isinstance(x, bytes) for x in k):
        return [x.decode('utf8', 'replace') for x in k]
    return None


def scan_code(root, reversed_form=False):
    """Walk the tree once. Returns (raises, catches) where
    raises[key]   = [(file, line, [dict keys])] in tree order and
    catches[key]  = [(file, line, function, via, handler)] with `handler` =
    {'dict_keys': set, 'refs': set} for the catching function."""
    raises = collections.defaultdict(list)
    catches = collections.defaultdict(list)
    for rel, top in L.load_tree(root):
        for c in pyc27.walk(top):
            ins = list(pyc27.dis(c))
            tab = None
            n = len(ins)
            handler = None
            for i, (off, op, arg, rep) in enumerate(ins):
                if op in LOAD_OPS and rep == 'UserError' and i + 1 < n and ins[i + 1][1] == 'LOAD_CONST':
                    k = c.consts[ins[i + 1][2]]
                    if isinstance(k, bytes):
                        tab = tab or L.line_table(c)
                        ks = []
                        j = i + 2
                        while j < n and ins[j][1] not in STOPS:
                            if ins[j][1] == 'STORE_MAP' and ins[j - 1][1] == 'LOAD_CONST':
                                kk = c.consts[ins[j - 1][2]]
                                if isinstance(kk, bytes):
                                    ks.append(kk.decode('utf8', 'replace'))
                            j += 1
                        raises[k.decode('utf8', 'replace')].append(
                            (rel + '.py', L.line_at(c, off, tab), ks))
                if op != 'COMPARE_OP' or CMP[arg] not in ('==', '!=', 'in', 'not in'):
                    continue
                via = kidx = attr_at = None
                if i >= 2 and ins[i - 1][1] == 'LOAD_CONST' and ins[i - 2][1] == 'LOAD_ATTR' and ins[i - 2][3] == 'msg':
                    via, kidx, attr_at = 'msg', ins[i - 1][2], ins[i - 2][0]
                elif (i >= 4 and ins[i - 1][1] == 'LOAD_CONST' and ins[i - 2][1] == 'BINARY_SUBSCR'
                      and ins[i - 3][1] == 'LOAD_CONST' and ins[i - 4][1] == 'LOAD_ATTR'
                      and ins[i - 4][3] == 'args' and c.consts[ins[i - 3][2]] == 0):
                    via, kidx, attr_at = 'args[0]', ins[i - 1][2], ins[i - 4][0]
                elif reversed_form and (i >= 3 and ins[i - 1][1] == 'LOAD_ATTR' and ins[i - 1][3] == 'msg'
                                        and ins[i - 3][1] == 'LOAD_CONST'):
                    via, kidx, attr_at = 'msg', ins[i - 3][2], ins[i - 1][0]
                elif reversed_form and (i >= 5 and ins[i - 1][1] == 'BINARY_SUBSCR'
                                        and ins[i - 2][1] == 'LOAD_CONST' and ins[i - 3][1] == 'LOAD_ATTR'
                                        and ins[i - 3][3] == 'args' and ins[i - 5][1] == 'LOAD_CONST'):
                    via, kidx, attr_at = 'args[0]', ins[i - 5][2], ins[i - 3][0]
                if via is None:
                    continue
                keys = _bytes_consts(c, kidx)
                if not keys:
                    continue
                if handler is None:
                    handler = handler_facts(c, ins)
                tab = tab or L.line_table(c)
                for key in keys:
                    catches[key].append((rel + '.py', L.line_at(c, attr_at, tab),
                                         L.dec(c.name), via, handler))
    return raises, catches


def handler_facts(code, ins):
    """Dict keys read off an exception's `.dict` and the HANDLER_REFS called in
    one function (function scope, not branch scope: a hint, as errors.md says)."""
    keys, refs = set(), set()
    for i, (off, op, arg, rep) in enumerate(ins):
        if op in ('LOAD_ATTR', 'LOAD_GLOBAL', 'LOAD_NAME') and rep in HANDLER_REFS:
            refs.add(rep)
        if op == 'LOAD_ATTR' and rep == 'dict':
            j = i + 1
            if j < len(ins) and ins[j][1] == 'LOAD_ATTR' and ins[j][3] == 'get':
                j += 1
            if j + 1 < len(ins) and ins[j][1] == 'LOAD_CONST':
                k = code.consts[ins[j][2]]
                if isinstance(k, bytes) and (ins[j - 1][3] == 'get' or ins[j + 1][1] == 'BINARY_SUBSCR'):
                    keys.add(k.decode('utf8', 'replace'))
    return {'dict_keys': keys, 'refs': refs}


# ---------------------------------------------------------------------- rows

def build_rows(fsd_rows, labels, texts, raises, catches, seed_ui):
    keys = set(fsd_rows) | set(labels) | set(raises) | set(catches)
    rows = []
    for key in sorted(keys, key=lambda k: (k.lower(), k)):
        f, lb = fsd_rows.get(key), labels.get(key)
        in_py = key in raises or key in catches
        source = '+'.join(s for s, on in (('fsd', f is not None), ('loc', lb is not None), ('pyc', in_py)) if on)
        if f is not None:
            did, dtype = f['dialogID'], f['dialogType']
            sup, clo = f['suppressable'], f['closable']
            tid, bid = f['titleID'], f['bodyID']
        elif lb is not None:
            did, dtype, sup, clo = lb['num'], None, None, None
            tid, bid = lb.get('title'), lb.get('body')
        else:
            did = dtype = sup = clo = tid = bid = None
        largs = loc_args(lb.get('body') if lb else None, texts) if lb else '-'
        rs = raises.get(key, [])
        cs = sorted(set((s[0], s[1], s[2], s[3]) for s in catches.get(key, [])))
        sites = sorted(set('%s:%d' % (s[0], s[1]) for s in cs))
        at = ';'.join(sites[:3]) + (';+%d' % (len(sites) - 3) if len(sites) > 3 else '')
        funcs = sorted(set(s[2] for s in cs))[:3]
        via = sorted(set(s[3] for s in cs))
        dk, refs = set(), set()
        for s in catches.get(key, []):
            if s[3] != 'msg':       # the committed reading looked inside `.msg` handlers only
                continue
            dk |= s[4]['dict_keys']
            refs |= s[4]['refs']
        rows.append([
            key, source, _s(did), _s(dtype), presentation(dtype), _s(sup), _s(clo), _s(tid), _s(bid),
            largs, len(rs), ('%s:%d' % rs[0][:2]) if rs else '-',
            ','.join(sorted(set(x for r in rs for x in r[2]))) or '-',
            ','.join(sorted(dk)) or '-', at if cs else '-', ';'.join(funcs) or '-',
            ','.join(via) or '-', ','.join(sorted(refs)) or '-', seed_ui.get(key, '-')])
    return rows


def read_seed(path):
    if not path or not os.path.exists(path):
        return {}, []
    with open(path, encoding='utf8') as fh:
        lines = fh.read().split('\n')
    if lines and lines[-1] == '':
        lines.pop()
    if lines[0].split('\t') != COLUMNS:
        sys.exit('scan_user_errors: %s: unexpected header' % path)
    rows = [ln.split('\t') for ln in lines[1:]]
    return {r[0]: r[-1] for r in rows if len(r) == len(COLUMNS)}, rows


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--pyc', default=L.DEFAULT_PYC)
    ap.add_argument('--client', default='/client')
    ap.add_argument('--seed', default='findings/user-errors.tsv')
    ap.add_argument('--out', default='findings/user-errors.tsv', help="'-' for stdout")
    ap.add_argument('--no-resources', action='store_true',
                    help='no install to read: leave the resource columns empty (compare only the bytecode ones)')
    ap.add_argument('--reversed', action='store_true', help="also read `K == e.msg` (see the module note)")
    a = ap.parse_args(argv)
    if not os.path.isdir(a.pyc):
        sys.exit('scan_user_errors: no such tree: %s' % a.pyc)
    seed_ui, seed_rows = read_seed(a.seed)
    fsd_rows, labels, texts = ({}, {}, {}) if a.no_resources else read_resources(a.client)
    raises, catches = scan_code(a.pyc, a.reversed)
    rows = build_rows(fsd_rows, labels, texts, raises, catches, seed_ui)
    new = {r[0] for r in rows}
    for r in seed_rows:
        if r[0] not in new and r[-1] != '-':
            print('drift: key %r has ui_behaviour in the seed but is gone from the build' % r[0], file=sys.stderr)
    seed_at = {r[0]: r[14] for r in seed_rows if len(r) == len(COLUMNS)}
    for r in rows:
        if r[-1] != '-' and seed_at.get(r[0], r[14]) != r[14]:
            print('drift: %s: catch site moved %s -> %s; re-read its ui_behaviour' % (r[0], seed_at[r[0]], r[14]),
                  file=sys.stderr)
    L.write_tsv(a.out, [L.tsv_line(COLUMNS)] + [L.tsv_line(r) for r in rows])
    print('user-errors: %d rows (%d raise keys, %d caught keys)' % (len(rows), len(raises), len(catches)),
          file=sys.stderr)


if __name__ == '__main__':
    main()
