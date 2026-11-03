#!/usr/bin/env python3
"""Reader for the EVE client's localisation corpus (``res:/localizationFSD/*.pickle``).

Claims below are marked **[V]** (read from client code or from the data itself)
or **[I]** (inferred).  Nothing here executes client code.

The corpus
----------
Ten files, named ``res:/localizationFSD/localization_fsd_<lang>.pickle`` plus a
``_main`` one, listed in ``$EVE_CLIENT/resfileindex.txt`` and stored under
``$EVE_CLIENT/../ResFiles/<2hex>/<hash>_<md5>``. **[V]** The client reads them in
``localization/localizationBase.py``: ``Localization.FSD_CLIENT_PREFIX =
'res:/localizationFSD/localization_fsd_'``, ``PICKLE_EXT = '.pickle'``,
``_LoadPickleData`` does ``blue.ResFile().Read()`` then ``cPickle.loads``. There
is no container, no compression and no header of CCP's own: the resource *is* the
pickle. **[V]**

Both files are **Python 2 ``cPickle`` protocol 0** -- the printable-ASCII
opcode dialect. Verified by disassembling the opcode stream: ``(d`` ... ``S'..'``
... ``V..`` ... ``t`` ... ``s``. **[V]** Neither ``main`` nor ``en-us`` contains a
single ``GLOBAL`` (``c``) or ``REDUCE``/``INST`` opcode, so nothing in them can
import or construct a class; every object is a dict, list, tuple, int, unicode,
str or None. **[V]** This reader still refuses imports (see ``SafeUnpickler``)
because that is a property of the reader, not a favour the data does us.

``localization_fsd_main.pickle`` -- the index **[V]**
    A dict with exactly three keys, read by ``_ReadMainLocalizationData``:

    ``'labels'``
        ``{messageID: {'FullPath': str, 'label': str, 'messageID': int}}``.
        The client builds its label table as
        ``languageLabels['/'.join([FullPath, label])] = messageID``, or just
        ``label`` when ``FullPath`` is empty. The key constants are
        ``localization/const.py``: ``PICKLE_LABELS_FULLPATH = 'FullPath'``,
        ``PICKLE_LABELS_LABEL = 'label'``, ``PICKLE_LABELS_MESSAGEID =
        'messageID'``. **[V]**
    ``'languages'``
        list of language ids (``en-us de es fr it ja ko ru zh``). **[V]**
    ``'types'``
        ``{typeName: {languageID: [propertyName, ...]}}`` per
        ``IsValidTypeAndProperty`` -- which ``{thing.property}`` tokens are legal
        for a declension type. **Empty (``{}``) in the shipped V24.01 file**, so
        the client logs ``didn't find 'types'``-adjacent nothing and every
        ``IsValidTypeAndProperty`` call returns false; the real property
        vocabulary lives in ``localization/propertyHandlers/``. **[V]**
    ``'important'``
        a fourth key the client's ``_ReadMainLocalizationData`` never reads.
        Also empty (``{}``) in V24.01. **[V]** Presumably the "important name"
        (``GetImportantByMessageID``) override set. **[I]**

``localization_fsd_<lang>.pickle`` -- one language's messages **[V]**
    A 2-tuple ``(languageID, {messageID: (text, metaData, tokens)})``. The client
    splats it straight into native code:
    ``eveLocalization.LoadMessageData(*unPickledObject)``
    (``localizationBase.py:617``), and the triple shape is confirmed from the
    other direction by ``Localization.UpdateTextCache``
    (``localizationBase.py:727``), which rebuilds entries as
    ``newData[messageID] = (text, metaData, parser._Tokenize(text))`` and reads
    them back as ``text, metaData, tokens =
    eveLocalization.GetMessageDataByID(messageID, language)``. **[V]**

    ``text``
        the message, with ``{...}`` substitution tokens and the client's
        pseudo-HTML (``<br>``, ``<b>``, ``<font color=0x...>``,
        ``<a href=showinfo:34>``) left in. ``\r\n`` appears in older strings.
    ``metaData``
        ``None`` for all 308,657 en-us messages. **[V]** Read by
        ``Localization.GetMetaData`` via native ``GetMetaDataByID``; evidently
        only populated by the ``UpdateTextCache`` live-edit path, not shipped.
        **[I]**
    ``tokens``
        the *pre-computed* output of ``localization.parser._Tokenize(text)``:
        ``None`` for a message with no ``{...}`` token, and otherwise a dict
        ``{tokenSourceText: record}``. 10,782 of 308,657 en-us messages carry
        one, which is within 4 of the 10,786 whose text contains a ``{``. **[V]**
        Each record is::

            {'variableName': 'player',       # the kwarg the server must supply
             'variableType': 0,              # see table below
             'propertyName': 'name',         # attribute after the dot, or None
             'args': 0,                      # bitmask of TOKEN_FLAGs
             'kwargs': {'decimalPlaces': 0}, # ``name=value`` modifiers
             'conditionalValues': []}        # the ``-> "a", "b"`` arm, if any

        ``variableType`` is ``eveLocalization.VARIABLE_TYPE``, chosen by the
        ``[prefix]`` in the token. The numbering below is read off the shipped
        data by grouping every token record by its own ``[prefix]`` **[V]**;
        ``4`` is ``characterlist`` by elimination against
        ``parser.VARIABLE_TYPE_CHOOSER`` **[I]**::

            0 character      5 messageid       9 numeric
            1 npcorganization 6 datetime      10 generic (no [prefix])
            2 item           7 formattedtime
            3 location       8 timeinterval
            4 characterlist (inferred)

        Observed ``propertyName`` values per type, again from the data **[V]**:
        ``character`` -> name, gender, nameWithPossessive; ``item`` -> name,
        quantityName, nameWithArticle, rawName; ``location`` -> name, rawName;
        ``npcorganization`` -> name, nameWithArticle, rawName; ``numeric`` ->
        isk, distance, quantity; ``timeinterval`` -> writtenForm,
        shortWrittenForm, writtenFormTwoPart, shortForm.

        519 token records carry ``conditionalValues`` -- plural and gendered
        selection, e.g. ``{[numeric]numFighters-> "fighter", "fighters"}`` and
        ``{[character]agent.gender -> "his", "her"}``. The *selection* is done
        in native code from the supplied argument; the two arms are data. **[V]**

Resolving a string, as the client does it **[V]**
    ``localization.Get(messageIDorLabel, languageID=None, **kwargs)``:

    1. If the argument is a string it is a *label path*, looked up in
       ``self.languageLabels`` (built from ``main``'s ``labels``) to get the
       integer messageID. A miss logs ``No label with name %r`` and returns
       ``None``, which ``GetByLabel`` turns into the literal ``[no label: X]``.
    2. ``kwargs['player']`` is defaulted to ``session.charid`` if absent -- so
       gendered and second-person forms key off the *session*, not the message.
    3. ``self._GetByMessageID(messageID, comfylang.cerberus_code(), **kwargs)``
       -- that is ``eveLocalization.GetMessageByID``, native. This is where the
       message text's ``{tokens}`` are substituted from ``kwargs``.
    4. A leading ``<localized hint="...">`` wrapper is stripped, then
       ``uiutil.PrepareLocalizationSafeString``.
    5. On ``KeyError`` (unknown messageID): ``None``, which ``GetByMessageID``
       renders as ``u'%s %s]' % (UNRESOLVED_LOC_MARKER, messageID)``. On any
       other exception it falls back to ``eveLocalization.GetRawByMessageID``,
       i.e. the untokenised raw text. **[V]**

    So for the server the contract is: **a message id plus the dict of token
    arguments its text names.** The argument *names* are in the text itself
    (``{[item]itemID.name}``-style tokens); ``tools/loctable.py id <N>`` prints
    them, and ``--tokens`` lists every distinct token in the corpus.

What joins this to the rest of the static corpus **[V]**
    Everything outside the UI reaches a string through an **integer
    messageID stored in an FSD table**, which is the join key:

    - ``evetypes/localizationUtils.py:21,39`` --
      ``GetLocalizedTypeName(messageID)`` / ``GetLocalizedTypeDescription
      (messageID)`` call straight into ``localization.GetByMessageID``. So
      ``types.fsdbinary``'s name/description columns hold messageIDs.
    - ``eve/common/script/sys/eveCfg.py:1755`` -- ``_LoadMessagesFromFSD()``
      loads ``res:/staticdata/dialogs.static`` (+ ``.schema``), keyed by a
      *message key string* (``ItemLocked``, ``JumpAttemptTooSoon``), each row
      carrying ``titleID``, ``bodyID``, ``dialogType``, ``urlAudio``,
      ``urlIcon``, ``suppressable``, ``closable``. ``cfg.GetMessage(key, dict)``
      then does ``localization.GetByMessageID(bodyID, **dict)``. **That is the
      whole ``UserError`` path.** This corpus also carries the same table
      denormalised as labels ``UI/Messages/<numericID>/<Key>Title`` and
      ``.../<Key>Body`` (4,380 labels, 2,974 distinct keys), so the ``errors``
      subcommand enumerates it without reading ``dialogs.static`` at all.
    - ``eve/client/script/ui/station/agents/agents.py:698``
      (``Agents.ProcessMessage``) -- an agent/mission message arrives from the
      server as ``(msgInfo, contentID)`` where ``msgInfo`` is either a raw
      string (used verbatim), an **int messageID** (resolved here), or a
      ``(label, argsDict)`` pair. The keys ``missionOfferText``,
      ``missionBriefingText``, ``missionCompletionText``,
      ``declineMessageText`` and ``locationString`` inside ``argsDict`` are
      themselves recursively resolved (``agents.py:723``). So **the server
      sends ids, the client renders CCP's prose.**

Not handled here
    The ``.pyc`` side of tokenisation (``localization/parser.py``,
    ``localization/propertyHandlers/``) and the FSD tables that hold the
    messageIDs are described in ``docs/re/localization.md``; decoding
    ``res:/staticdata/*.fsdbinary`` belongs to the ``fsd-reader`` lane and is
    deliberately not attempted here.

Usage
-----
Run it in the analysis container. ``ResFiles`` is a *sibling* of ``$EVE_CLIENT``,
so point ``EVE_CLIENT`` one directory up for the mount to include it::

    EVE_CLIENT=~/.local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE \
        tools/analysis.sh python3 tools/loctable.py info

The reader finds ``resfileindex.txt`` and ``ResFiles/`` under either the ``tq``
directory or its parent, so both spellings of ``EVE_CLIENT`` work for ``--root``.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import io
import json
import os
import pickle
import re
import sys

RES_PREFIX = 'res:/localizationfsd/localization_fsd_'
DEFAULT_LANG = 'en-us'

# ---------------------------------------------------------------- safe loading


class SafeUnpickler(pickle.Unpickler):
    """An unpickler that cannot import, instantiate or call anything.

    Protocol 0 can carry ``GLOBAL``/``REDUCE``/``INST``/``OBJ``, all of which
    reach arbitrary code. The localisation pickles contain none of them, but we
    refuse rather than trust.
    """

    def find_class(self, module, name):
        raise pickle.UnpicklingError(
            'refused to import %s.%s from client data' % (module, name))

    def persistent_load(self, pid):
        raise pickle.UnpicklingError('refused persistent id %r' % (pid,))


def opcode_audit(data):
    """Return the set of code-reaching protocol-0 opcodes present in ``data``.

    Protocol 0 is line oriented: every opcode that can reach code starts a
    token line. This is a cheap pre-flight, not a parser.
    """
    found = set()
    for op, name in ((b'c', 'GLOBAL'), (b'i', 'INST'), (b'o', 'OBJ'),
                     (b'R', 'REDUCE'), (b'b', 'BUILD'), (b'P', 'PERSID')):
        if re.search(b'(?m)^' + op, data):
            found.add(name)
    return found


def load_pickle(path, audit=True):
    with open(path, 'rb') as fh:
        data = fh.read()
    if data[:1] == b'\x80':
        raise SystemExit('%s: binary pickle protocol, expected protocol 0' % path)
    if audit:
        bad = opcode_audit(data)
        if bad:
            print('loctable: warning: %s carries %s opcodes; the unpickler will '
                  'refuse them' % (path, ','.join(sorted(bad))), file=sys.stderr)
    # Python 2 ``str`` -> ``str`` via latin-1 so no byte can fail to decode;
    # every key we care about is ASCII anyway.
    return SafeUnpickler(io.BytesIO(data), encoding='latin-1').load()


# ------------------------------------------------------------- res resolution


def find_root(explicit=None):
    cand = []
    if explicit:
        cand.append(explicit)
    if os.environ.get('EVE_CLIENT'):
        cand.append(os.environ['EVE_CLIENT'])
    cand += ['/client', os.path.expanduser(
        '~/.local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE/tq')]
    for root in cand:
        for base in (root, os.path.join(root, 'tq')):
            if os.path.isfile(os.path.join(base, 'resfileindex.txt')):
                return base
    raise SystemExit('loctable: no resfileindex.txt found; pass --root')


def find_resfiles(index_dir):
    for cand in (os.path.join(index_dir, '..', 'ResFiles'),
                 os.path.join(index_dir, 'ResFiles')):
        if os.path.isdir(cand):
            return os.path.normpath(cand)
    raise SystemExit('loctable: ResFiles/ not found next to %s. It is a sibling '
                     'of the tq directory, so point EVE_CLIENT at the parent.'
                     % index_dir)


def resolve(res_name, root=None):
    """Map a ``res:/...`` name to an absolute path under ``ResFiles``."""
    index_dir = find_root(root)
    want = res_name.lower()
    with open(os.path.join(index_dir, 'resfileindex.txt'), 'r',
              encoding='utf-8', errors='replace') as fh:
        for line in fh:
            parts = line.rstrip('\n').split(',')
            if parts and parts[0].lower() == want:
                return os.path.join(find_resfiles(index_dir), parts[1])
    raise SystemExit('loctable: %s not in resfileindex.txt' % res_name)


def load_lang(lang, root=None, path=None):
    """Return ``(languageID, {messageID: (text, metaData, tokens)})``."""
    obj = load_pickle(path or resolve(RES_PREFIX + lang + '.pickle', root))
    if not (isinstance(obj, tuple) and len(obj) == 2 and isinstance(obj[1], dict)):
        raise SystemExit('loctable: %s is not a (langID, messages) 2-tuple' % lang)
    return obj


def load_main(root=None, path=None):
    obj = load_pickle(path or resolve(RES_PREFIX + 'main.pickle', root))
    if not isinstance(obj, dict):
        raise SystemExit('loctable: main pickle is not a dict')
    return obj


def label_index(main):
    """``{'Path/Label': messageID}``, exactly as the client builds it."""
    out = {}
    for mid, row in main.get('labels', {}).items():
        full = row.get('FullPath')
        label = row.get('label')
        out['/'.join([full, label]) if full else label] = mid
    return out


# ------------------------------------------------------------------ token work

# ``localization/parser.py`` tokenises ``{...}`` groups; this is the shallow
# surface form, good enough to report which arguments a message needs. **[I]**
TOKEN_RE = re.compile(r'\{([^{}]*)\}')


def tokens_of(text):
    return TOKEN_RE.findall(text or '')


# ------------------------------------------------------------------- reporting


def _print_entry(mid, entry, labels_by_id=None):
    text, meta, toks = (entry + (None, None, None))[:3]
    print('messageID %s' % mid)
    if labels_by_id and mid in labels_by_id:
        print('  label     %s' % labels_by_id[mid])
    print('  text      %r' % (text,))
    if meta is not None:
        print('  metaData  %r' % (meta,))
    if toks is not None:
        print('  tokens    %r' % (toks,))
    args = tokens_of(text)
    if args:
        print('  arguments %s' % ', '.join(sorted(set(args))))


def cmd_info(a):
    main = load_main(a.root, a.main_file)
    labels = label_index(main)
    print('main pickle')
    print('  keys           %s' % sorted(main.keys()))
    print('  languages      %s' % sorted(main.get('languages', [])))
    print('  labels         %d' % len(main.get('labels', {})))
    print('  label paths    %d distinct FullPath values'
          % len({r.get('FullPath') for r in main.get('labels', {}).values()}))
    types = main.get('types', {})
    print('  types          %d' % len(types))
    if types:
        k = sorted(types)[0]
        print('  types[%r]  %r' % (k, types[k]))
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    ids = set(msgs)
    lab_ids = set(labels.values())
    meta_n = sum(1 for v in msgs.values() if v[1] is not None)
    tok_n = sum(1 for v in msgs.values() if v[2] is not None)
    print('%s pickle' % lang)
    print('  messages       %d' % len(msgs))
    print('  id range       %d .. %d' % (min(ids), max(ids)))
    print('  with metaData  %d' % meta_n)
    print('  with tokens    %d' % tok_n)
    print('  labelled       %d  (messageID appears in main labels)'
          % len(ids & lab_ids))
    print('  unlabelled     %d  (reachable only by numeric id)'
          % len(ids - lab_ids))
    print('  labels w/o msg %d' % len(lab_ids - ids))


def cmd_id(a):
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    labels_by_id = {}
    if not a.no_main:
        labels_by_id = {v: k for k, v in label_index(
            load_main(a.root, a.main_file)).items()}
    for mid in a.ids:
        mid = int(mid)
        if mid not in msgs:
            print('messageID %s: absent from %s (client would render '
                  '"[no messageID: %s]")' % (mid, lang, mid))
            continue
        _print_entry(mid, msgs[mid], labels_by_id)


def cmd_label(a):
    main = load_main(a.root, a.main_file)
    labels = label_index(main)
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    for name in a.labels:
        if name not in labels:
            hits = [k for k in labels if name.lower() in k.lower()][:10]
            print('label %r: not found.%s' % (
                name, (' Near: ' + ', '.join(hits)) if hits else ''))
            continue
        mid = labels[name]
        print('label     %s' % name)
        if mid in msgs:
            _print_entry(mid, msgs[mid])
        else:
            print('messageID %s (no text in %s)' % (mid, lang))


def cmd_grep(a):
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    labels_by_id = {}
    if not a.no_main:
        labels_by_id = {v: k for k, v in label_index(
            load_main(a.root, a.main_file)).items()}
    flags = 0 if a.case else re.IGNORECASE
    rx = re.compile(a.pattern, flags)
    n = 0
    for mid in sorted(msgs):
        text = msgs[mid][0]
        if text and rx.search(text):
            n += 1
            if n <= a.limit:
                lab = labels_by_id.get(mid)
                head = text if a.full else (text[:a.width].replace('\n', ' | ')
                                            + ('...' if len(text) > a.width else ''))
                print('%-9s %-44s %s' % (mid, lab or '-', head))
    print('-- %d match%s (%d shown)' % (n, '' if n == 1 else 'es', min(n, a.limit)))


def cmd_labels(a):
    labels = label_index(load_main(a.root, a.main_file))
    pref = (a.prefix or '').lower()
    hits = sorted(k for k in labels if k.lower().startswith(pref))
    for k in hits[:a.limit]:
        print('%-9s %s' % (labels[k], k))
    print('-- %d label%s under %r (%d shown)'
          % (len(hits), '' if len(hits) == 1 else 's', a.prefix, min(len(hits), a.limit)))


def cmd_census(a):
    """Counts that answer "what kind of text is in here".

    The split is by *length* and by whether the id carries a UI label, because
    those are the two properties the data itself gives us. Any claim stronger
    than that is inference and belongs in the doc, not here.
    """
    main = load_main(a.root, a.main_file)
    labels = label_index(main)
    lab_ids = set(labels.values())
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    buckets = [(0, 16), (16, 48), (48, 120), (120, 400), (400, 1200),
               (1200, 4000), (4000, 10 ** 9)]
    counts = {b: [0, 0] for b in buckets}       # [labelled, unlabelled]
    chars = 0
    multiline = 0
    tagged = 0
    tokened = 0
    for mid, entry in msgs.items():
        text = entry[0] or ''
        chars += len(text)
        if '\n' in text or '<br>' in text:
            multiline += 1
        if '<' in text and '>' in text:
            tagged += 1
        if '{' in text:
            tokened += 1
        for b in buckets:
            if b[0] <= len(text) < b[1]:
                counts[b][int(mid not in lab_ids)] += 1
                break
    print('language %s: %d messages, %d characters' % (lang, len(msgs), chars))
    print('%-16s %10s %12s' % ('length', 'labelled', 'unlabelled'))
    for b in buckets:
        print('%-16s %10d %12d' % ('%d-%d' % b if b[1] < 10 ** 9
                                   else '%d+' % b[0], counts[b][0], counts[b][1]))
    print('with {tokens}   %d' % tokened)
    print('with <markup>   %d' % tagged)
    print('multi-line      %d' % multiline)
    print('id histogram by 2^19 block (messageID >> 19):')
    hist = {}
    for mid in msgs:
        hist[mid >> 19] = hist.get(mid >> 19, 0) + 1
    for k in sorted(hist):
        print('  block %-4d %-24s %d' % (k, '[%d .. %d]' % (k << 19, ((k + 1) << 19) - 1),
                                         hist[k]))


def cmd_tokens(a):
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    hist = {}
    for entry in msgs.values():
        for t in tokens_of(entry[0]):
            key = re.sub(r'\d+', 'N', t)
            hist[key] = hist.get(key, 0) + 1
    for k, v in sorted(hist.items(), key=lambda kv: -kv[1])[:a.limit]:
        print('%8d  %s' % (v, k))
    print('-- %d distinct token forms' % len(hist))


def cmd_errors(a):
    """Enumerate the ``UI/Messages/<id>/<Key>{Title,Body}`` family.

    This is the ``UserError`` vocabulary: one row per message key with the
    numeric id the ``dialogs`` FSD table uses, the title and body messageIDs,
    and the arguments the body names.
    """
    labels = label_index(load_main(a.root, a.main_file))
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    rows = {}
    for path, mid in labels.items():
        parts = path.split('/')
        if len(parts) < 4 or parts[0] != 'UI' or parts[1] != 'Messages':
            continue
        num, leaf = parts[2], parts[3]
        if leaf.endswith('Title'):
            key, slot = leaf[:-5], 'title'
        elif leaf.endswith('Body'):
            key, slot = leaf[:-4], 'body'
        else:
            key, slot = leaf, 'body'
        rows.setdefault((num, key), {})[slot] = mid
    out = 0
    for (num, key), slots in sorted(rows.items(), key=lambda kv: int(kv[0][0])):
        if a.pattern and not re.search(a.pattern, key, re.IGNORECASE):
            continue
        out += 1
        if out > a.limit:
            continue
        body = slots.get('body')
        title = slots.get('title')
        btext = (msgs.get(body) or ('',))[0] if body else None
        ttext = (msgs.get(title) or ('',))[0] if title else None
        print('%-6s %-44s title=%-8s body=%-8s' % (num, key, title, body))
        if ttext:
            print('         title: %s' % ttext.replace('\n', ' '))
        if btext:
            print('         body : %s' % btext.replace('\n', ' '))
            args = sorted(set(tokens_of(btext)))
            if args:
                print('         args : %s' % ', '.join(args))
    print('-- %d message key%s (%d shown)'
          % (out, '' if out == 1 else 's', min(out, a.limit)))


def cmd_export(a):
    """Bulk dump, for scratch analysis only. Never commit the output."""
    lang, msgs = load_lang(a.lang, a.root, a.lang_file)
    labels_by_id = {v: k for k, v in label_index(
        load_main(a.root, a.main_file)).items()}
    with open(a.out, 'w', encoding='utf-8') as fh:
        for mid in sorted(msgs):
            fh.write(json.dumps({'id': mid, 'label': labels_by_id.get(mid),
                                 'text': msgs[mid][0]}, ensure_ascii=False) + '\n')
    print('wrote %d messages to %s' % (len(msgs), a.out))


def main():
    ap = argparse.ArgumentParser(
        description='read the EVE client localisation pickles',
        epilog='ResFiles/ is a sibling of the tq directory: point EVE_CLIENT at '
               'the parent (.../CCP/EVE) so the container mount includes it.')
    ap.add_argument('--root', help='client dir (tq, or its parent)')
    ap.add_argument('--lang', default=DEFAULT_LANG, help='language id (default en-us)')
    ap.add_argument('--lang-file', help='read this file instead of resolving --lang')
    ap.add_argument('--main-file', help='read this file as the main pickle')
    sub = ap.add_subparsers(dest='cmd', required=True)

    p = sub.add_parser('info', help='structure and counts of main + one language')
    p.set_defaults(fn=cmd_info)

    p = sub.add_parser('id', help='look up message ids')
    p.add_argument('ids', nargs='+')
    p.add_argument('--no-main', action='store_true', help='skip the label index')
    p.set_defaults(fn=cmd_id)

    p = sub.add_parser('label', help='look up label paths (Path/Label)')
    p.add_argument('labels', nargs='+')
    p.set_defaults(fn=cmd_label)

    p = sub.add_parser('grep', help='regex search message texts')
    p.add_argument('pattern')
    p.add_argument('--limit', type=int, default=40)
    p.add_argument('--width', type=int, default=110)
    p.add_argument('--full', action='store_true')
    p.add_argument('--case', action='store_true', help='case sensitive')
    p.add_argument('--no-main', action='store_true')
    p.set_defaults(fn=cmd_grep)

    p = sub.add_parser('labels', help='list label paths under a prefix')
    p.add_argument('prefix', nargs='?', default='')
    p.add_argument('--limit', type=int, default=200)
    p.set_defaults(fn=cmd_labels)

    p = sub.add_parser('census', help='counts by length / labelled / id block')
    p.set_defaults(fn=cmd_census)

    p = sub.add_parser('tokens', help='histogram of {token} forms')
    p.add_argument('--limit', type=int, default=60)
    p.set_defaults(fn=cmd_tokens)

    p = sub.add_parser('errors', help='the UI/Messages UserError vocabulary')
    p.add_argument('pattern', nargs='?', help='regex filter on the message key')
    p.add_argument('--limit', type=int, default=60)
    p.set_defaults(fn=cmd_errors)

    p = sub.add_parser('export', help='JSONL dump (scratch only, do not commit)')
    p.add_argument('out')
    p.set_defaults(fn=cmd_export)

    a = ap.parse_args()
    a.fn(a)


if __name__ == '__main__':
    main()
