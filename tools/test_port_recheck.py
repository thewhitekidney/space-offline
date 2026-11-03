#!/usr/bin/env python3
"""Tests for tools/port-recheck.py (PORT-RECHECK, T4).

Every tree is invented: a tiny writer below makes Python 2.7 .pyc files from made-up
modules, names and ids, so nothing here needs a client.

    python3 -I tools/test_port_recheck.py
"""
import contextlib
import importlib.util
import io
import os
import shutil
import struct
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
_spec = importlib.util.spec_from_file_location('port_recheck', os.path.join(HERE, 'port-recheck.py'))
pr = importlib.util.module_from_spec(_spec)
sys.modules['port_recheck'] = pr
_spec.loader.exec_module(pr)

LOAD_CONST, STORE_NAME, LOAD_NAME, POP_TOP, RETURN_VALUE = 100, 90, 101, 1, 83


class W(object):
    """The marshal subset pyc27 reads."""

    def __init__(self):
        self.b = bytearray()

    def s(self, v):
        self.b += b's' + struct.pack('<i', len(v)) + v

    def obj(self, v):
        if v is None:
            self.b += b'N'
        elif isinstance(v, bool):
            self.b += b'T' if v else b'F'
        elif isinstance(v, int):
            self.b += b'i' + struct.pack('<i', v)
        elif isinstance(v, float):
            self.b += b'g' + struct.pack('<d', v)
        elif isinstance(v, bytes):
            self.s(v)
        elif isinstance(v, str):
            self.s(v.encode())
        elif isinstance(v, tuple):
            self.b += b'(' + struct.pack('<i', len(v))
            for x in v:
                self.obj(x)
        elif isinstance(v, C):
            self.code(v)
        else:
            raise TypeError(v)

    def code(self, c):
        self.b += b'c' + struct.pack('<iiii', c.argcount, 1, 4, c.flags)
        self.s(bytes(c.bytecode))
        for t in (c.consts, c.names, c.varnames, (), ()):
            self.obj(tuple(x.encode() if isinstance(x, str) and t is not c.consts else x for x in t))
        self.s(b'x.py')
        self.s(c.name.encode())
        self.b += struct.pack('<i', c.first)
        self.s(bytes(c.lnotab))


class C(object):
    """A code object from a list of (line, instructions)."""

    def __init__(self, name, lines, consts=(None,), names=(), first=None, flags=3, argcount=0):
        self.name, self.consts, self.names, self.flags, self.argcount = name, tuple(consts), tuple(names), flags, argcount
        self.varnames = ()
        self.first = first if first is not None else lines[0][0]
        self.bytecode, self.lnotab = bytearray(), bytearray()
        line, addr = self.first, 0
        for ln, ins in lines:
            if ln != line:
                self.lnotab += bytes((len(self.bytecode) - addr, ln - line))
                addr, line = len(self.bytecode), ln
            for i in ins:
                self.bytecode += bytes(i)


def op(code, arg=None):
    return (code,) if arg is None else (code, arg & 255, arg >> 8)


def fn(name, line, body=0, consts=None, flags=3):
    """A function whose meaning is the constant `body`."""
    return C(name, [(line, [op(LOAD_CONST, 0), op(RETURN_VALUE)])], consts=consts or (body,), flags=flags)


def klass(name, line, *methods):
    return C(name, [(line, [op(LOAD_CONST, 0), op(RETURN_VALUE)])], consts=(None,) + methods, flags=0)


def module(stores=(), nested=(), extra=(), tail=None):
    """stores: (line, name, value). A module's top level, flags 0 as the compiler sets."""
    consts, names, lines = [], [], []
    for ln, name, v in stores:
        consts.append(v)
        names.append(name)
        lines.append((ln, [op(LOAD_CONST, len(consts) - 1), op(STORE_NAME, len(names) - 1)]))
    consts.append(None)
    end = tail or (lines[-1][0] if lines else 1)
    i = len(consts) - 1
    if lines and end == lines[-1][0]:
        lines[-1][1].extend([op(LOAD_CONST, i), op(RETURN_VALUE)])
    else:
        lines.append((end, [op(LOAD_CONST, i), op(RETURN_VALUE)]))
    return C('<module>', lines, consts=tuple(consts) + tuple(nested), names=names, first=1, flags=0)


def pyc(path, code):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    w = W()
    w.code(code)
    with open(path, 'wb') as f:
        f.write(b'\x03\xf3\r\n' + b'\0\0\0\0' + bytes(w.b))


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix='recheck-')
        self.addCleanup(shutil.rmtree, self.td)
        self.old, self.new = os.path.join(self.td, 'old'), os.path.join(self.td, 'pyc')
        self.repo = os.path.join(self.td, 'repo')
        for d in (self.old, self.new, os.path.join(self.repo, 'findings'),
                  os.path.join(self.repo, 'server', 'crates', 'x', 'src')):
            os.makedirs(d)
        self.table('')
        # The constant modules every run reads.
        self.both('dogma/const', module([(1, 'attributeFoo', 11), (2, 'effectBar', 22)]))
        self.both('inventorycommon/const', module([(1, 'groupBaz', 33), (2, 'typeQux', 44), (3, 'categoryOne', 5)]))

    def both(self, rel, code, code_new=None):
        pyc(os.path.join(self.old, rel + '.pyc'), code)
        pyc(os.path.join(self.new, rel + '.pyc'), code_new or code)

    def table(self, *rows):
        with open(os.path.join(self.repo, 'findings', 'port-recheck.tsv'), 'w') as f:
            f.write('# invented\nkind\tclient_path\tsymbol\tlane\n')
            f.write(''.join('\t'.join(r) + '\n' for r in rows if r))

    def rust(self, body, name='lib.rs'):
        with open(os.path.join(self.repo, 'server', 'crates', 'x', 'src', name), 'w') as f:
            f.write(body)

    def pair(self):
        return pr.Pair(pr.Tree(self.old), pr.Tree(self.new))

    def ids(self, p):
        return pr.Ids(p.old), pr.Ids(p.new)

    def row(self, kind, path, symbol, bundles=None):
        p = self.pair()
        return pr.check_row(p, self.ids(p), bundles, dict(kind=kind, path=path, symbol=symbol, lane='t'))

    def cite(self):
        return {c.token: c for c in pr.check_citations(self.pair(), self.repo)}

    def cli(self, *extra):
        out = os.path.join(self.td, 'out')
        with contextlib.redirect_stdout(io.StringIO()):
            rc = pr.main([self.old, self.new, '--repo', self.repo, '--out', out] + list(extra))
        return rc, out


class FunctionLevel(Base):
    def test_a_moved_function_is_not_a_changed_one(self):
        self.both('m', module(nested=[fn('F', 10)]), module(nested=[fn('F', 90)]))
        self.assertEqual(self.row('symbol', 'm', 'F')[0], 'same')
        self.assertEqual(self.row('module', 'm', '')[0], 'same')

    def test_a_changed_constant_fires_the_function_and_names_it(self):
        self.both('m', module(nested=[fn('F', 10, 1), fn('G', 20, 1)]),
                  module(nested=[fn('F', 10, 2), fn('G', 20, 1)]))
        self.assertEqual(self.row('symbol', 'm', 'F')[0], 'changed')
        self.assertEqual(self.row('symbol', 'm', 'G')[0], 'same')
        st, detail = self.row('module', 'm', '')
        self.assertEqual(st, 'changed')
        self.assertIn('F', detail)
        self.assertNotIn('G', detail)

    def test_a_class_row_covers_its_methods_and_a_bare_name_finds_a_method(self):
        self.both('m', module(nested=[klass('K', 5, fn('M', 6, 1))]),
                  module(nested=[klass('K', 5, fn('M', 6, 2))]))
        for sym in ('K', 'K.M', 'M'):
            self.assertEqual(self.row('symbol', 'm', sym)[0], 'changed', sym)

    def test_gone_and_unresolved_are_different_answers(self):
        self.both('m', module(nested=[fn('F', 10), fn('G', 20)]), module(nested=[fn('G', 20)]))
        self.assertEqual(self.row('symbol', 'm', 'F')[0], 'gone')
        self.assertEqual(self.row('symbol', 'm', 'Nope')[0], 'unresolved')
        self.assertEqual(self.row('symbol', 'absent', 'F')[0], 'unresolved')
        self.assertEqual(self.row('module', 'absent', '')[0], 'unresolved')

    def test_a_module_that_left_is_gone(self):
        pyc(os.path.join(self.old, 'only_old.pyc'), module(nested=[fn('F', 10)]))
        self.assertEqual(self.row('module', 'only_old', '')[0], 'gone')
        self.assertEqual(self.row('symbol', 'only_old', 'F')[0], 'gone')

    def test_a_used_name_compares_the_code_that_uses_it(self):
        user = lambda n: C('U', [(3, [op(LOAD_NAME, 0), op(POP_TOP), op(LOAD_CONST, 0), op(RETURN_VALUE)])],
                           consts=(n,), names=('someEvent',))
        self.both('m', module(nested=[user(1), fn('Other', 9, 1)]),
                  module(nested=[user(2), fn('Other', 9, 2)]))
        st, detail = self.row('symbol', 'm', 'someEvent')
        self.assertEqual(st, 'changed')
        self.assertIn('U', detail)
        self.assertNotIn('Other', detail)

    def test_a_dotted_name_needs_every_part_in_one_code_object(self):
        a = C('A', [(3, [op(LOAD_NAME, 0), op(POP_TOP), op(LOAD_CONST, 0), op(RETURN_VALUE)])],
              consts=(1,), names=('Start',))
        self.both('m', module(nested=[a]))
        self.assertEqual(self.row('symbol', 'm', 'Thing.Start')[0], 'unresolved')


class Constants(Base):
    def test_const_compares_values_and_takes_a_pattern(self):
        self.both('c', module([(1, 'waitOne', 5), (2, 'waitTwo', 6), (3, 'other', 7)]),
                  module([(1, 'waitOne', 5), (2, 'waitTwo', 9), (3, 'other', 8)]))
        self.assertEqual(self.row('const', 'c', 'waitOne')[0], 'same')
        st, detail = self.row('const', 'c', 'wait*')
        self.assertEqual(st, 'changed')
        self.assertIn('waitTwo 6 -> 9', detail)
        self.assertNotIn('other', detail)

    def test_a_const_row_follows_import_star(self):
        star = C('<module>', [(1, [op(LOAD_CONST, 0), op(LOAD_CONST, 1), op(108, 0), op(84),
                                   op(LOAD_CONST, 2), op(RETURN_VALUE)])],
                 consts=(-1, ('*',), None), names=('base_mod',), first=1, flags=0)
        self.both('star', star)
        self.both('base_mod', module([(1, 'value', 1)]), module([(1, 'value', 2)]))
        self.assertEqual(self.row('const', 'star', 'value')[0], 'changed')

    def test_arithmetic_is_evaluated(self):
        c = C('<module>', [(1, [op(LOAD_CONST, 0), op(STORE_NAME, 0)]),
                           (2, [op(LOAD_NAME, 0), op(LOAD_CONST, 1), op(20), op(STORE_NAME, 1),
                                op(LOAD_CONST, 2), op(RETURN_VALUE)])],
              consts=(3, 7, None), names=('a', 'b'), first=1, flags=0)
        self.assertEqual(pr.eval_module(pr.pyc27.Unmarshal(serial(c)).load())['b'], 21)

    def test_attr_and_effect_rows_read_name_and_id(self):
        self.both('dogma/const', module([(1, 'attributeFoo', 11), (2, 'effectBar', 22)]),
                  module([(1, 'attributeRenamed', 11), (2, 'effectBar', 23)]))
        self.assertEqual(self.row('attr', '-', '11')[0], 'changed')
        self.assertEqual(self.row('effect', '-', 'effectBar')[0], 'changed')
        self.assertEqual(self.row('effect', '-', '22')[0], 'gone')
        self.assertEqual(self.row('effect', '-', '999')[0], 'unresolved')

    def test_group_row(self):
        self.assertEqual(self.row('group', '-', '33')[0], 'same')
        self.assertEqual(self.row('group', '-', 'categoryOne')[0], 'same')

    def test_errorkey(self):
        keyed = lambda k: C('F', [(2, [op(LOAD_CONST, 0), op(RETURN_VALUE)])], consts=(k,))
        self.both('e', module(nested=[keyed('SomeKey')]), module(nested=[keyed('OtherKey')]))
        self.assertEqual(self.row('errorkey', 'e', 'SomeKey')[0], 'gone')
        self.assertEqual(self.row('errorkey', 'e', 'Never')[0], 'unresolved')


def serial(c):
    w = W()
    w.code(c)
    return bytes(w.b)


class Bundles(Base):
    def bundle(self, name, types='', lists='', manifest='x'):
        root = os.path.join(self.td, name)
        os.makedirs(os.path.join(root, 'types'))
        with open(os.path.join(root, 'types', 'typedata.tsv'), 'w') as f:
            f.write('# invented\n' + types)
        with open(os.path.join(root, 'types', 'typelists.tsv'), 'w') as f:
            f.write('#list_id\tinc\n' + lists)
        with open(os.path.join(root, 'MANIFEST.tsv'), 'w') as f:
            f.write('extractor\t%s\n' % manifest)
        return root

    def test_typelist_needs_bundles_and_says_so(self):
        self.assertEqual(self.row('typelist', '-', '7')[0], 'skipped')
        b = (pr.Bundle(self.bundle('a', lists='7\t1\n')), pr.Bundle(self.bundle('b', lists='7\t2\n')))
        self.assertEqual(self.row('typelist', '-', '7', b)[0], 'changed')
        self.assertEqual(self.row('typelist', '-', '8', b)[0], 'unresolved')

    def test_an_effect_row_with_a_new_last_column_is_equal(self):
        a = pr.Bundle(self.bundle('a', 'effect\t22\t1\t0\tname\n'))
        b = pr.Bundle(self.bundle('b', 'effect\t22\t1\t0\tname\t0\n'))
        self.assertEqual(pr.bundle_effect(a, b, 22)[0], 'same')

    def test_a_modifier_change_fires_the_effect(self):
        a = pr.Bundle(self.bundle('a', 'effect\t22\t1\nmodifier\t22\tshipID\tF\t1\t2\t3\t\t\n'))
        b = pr.Bundle(self.bundle('b', 'effect\t22\t1\nmodifier\t22\tshipID\tF\t1\t9\t3\t\t\n'))
        self.assertEqual(pr.bundle_effect(a, b, 22)[0], 'changed')

    def test_a_named_constant_reaches_its_type_row(self):
        old = self.bundle('a', 'type\t44\t33\t5\t1\ttail\n', manifest='one')
        new = self.bundle('b', 'type\t44\t33\t5\t0\ttail\n', manifest='two')
        self.rust('pub const THING_TYPE: i64 = 44;\n')
        rc, out = self.cli('--old-bundle', old, '--new-bundle', new)
        self.assertEqual(rc, 3)
        md = open(os.path.join(out, 'recheck.md')).read()
        self.assertIn('THING_TYPE', md)
        self.assertIn('different extractors', md)

    def test_one_bundle_is_a_usage_error(self):
        self.assertEqual(self.cli('--old-bundle', self.bundle('a'))[0], 2)


class Citations(Base):
    def test_a_path_qualified_citation_ignores_other_files_of_that_name(self):
        self.both('a/const', module(nested=[fn('F', 10, 1)]), module(nested=[fn('F', 10, 2)]))
        self.both('b/const', module(nested=[fn('F', 10, 1)]))
        self.rust('// see `b/const.py:10`\n')
        self.assertEqual(self.cite()['b/const.py:10'].status, 'same')

    def test_a_changed_function_fires_its_citation(self):
        self.both('thing', module(nested=[fn('F', 10, 1)]), module(nested=[fn('F', 10, 2)]))
        self.rust('// `thing.py:10`\n')
        c = self.cite()['thing.py:10']
        self.assertEqual((c.status, c.entry, c.via), ('changed', 'F', 'basename'))

    def test_same_basename_with_only_one_changed_is_ambiguous_without_evidence(self):
        self.both('a/util', module(nested=[fn('F', 10, 1)]), module(nested=[fn('F', 10, 2)]))
        self.both('b/util', module(nested=[fn('F', 10, 1), fn('G', 11, 1)]),
                  module(nested=[fn('F', 10, 3), fn('G', 11, 1)]))
        self.rust('// `util.py:10`\n')
        self.assertEqual(self.cite()['util.py:10'].status, 'ambiguous')

    def test_all_candidates_equal_needs_no_answer(self):
        self.both('a/util', module(nested=[fn('F', 10, 1)]))
        self.both('b/util', module(nested=[fn('F', 10, 2)]))
        self.rust('// `util.py:10`\n')
        self.assertEqual(self.cite()['util.py:10'].status, 'same')

    def test_a_line_past_the_end_rules_a_candidate_out(self):
        self.both('a/util', module(nested=[fn('F', 10, 1)]), module(nested=[fn('F', 10, 2)]))
        self.both('b/util', module(nested=[fn('F', 90, 1)]), module(nested=[fn('F', 90, 2)]))
        self.rust('// `util.py:90`\n')
        c = self.cite()['util.py:90']
        self.assertEqual((c.status, c.module, c.via), ('changed', 'b/util', 'line'))

    def test_a_symbol_nearby_picks_the_file(self):
        self.both('a/util', module(nested=[fn('Alpha', 10, 1)]), module(nested=[fn('Alpha', 10, 2)]))
        self.both('b/util', module(nested=[fn('Beta', 10, 1)]), module(nested=[fn('Beta', 10, 2)]))
        self.rust('// the `Beta` call (`util.py:10`)\n')
        c = self.cite()['util.py:10']
        self.assertEqual((c.module, c.via), ('b/util', 'symbol nearby'))

    def test_a_full_path_in_the_same_file_picks_the_short_one(self):
        self.both('a/util', module(nested=[fn('F', 10, 1), fn('H', 30, 1)]), module(nested=[fn('F', 10, 2), fn('H', 30, 1)]))
        self.both('b/util', module(nested=[fn('F', 10, 1), fn('H', 30, 1)]), module(nested=[fn('F', 10, 2), fn('H', 30, 1)]))
        self.rust('// `a/util.py:30`\n\n\n\n\n\n// `util.py:10`\n')
        self.assertEqual(self.cite()['util.py:10'].via, 'same file cites it')

    def test_our_own_tools_are_not_client_citations(self):
        self.rust('// `tools/thing.py:10` and `server/x.py:3`\n')
        self.assertEqual(self.cite(), {})

    def test_a_name_we_do_not_know_is_unknown_not_same(self):
        self.rust('// `nowhere.py:10`\n')
        self.assertEqual(self.cite()['nowhere.py:10'].status, 'unknown')

    def test_scenarios_are_read(self):
        self.both('thing', module(nested=[fn('F', 10, 1)]), module(nested=[fn('F', 10, 2)]))
        d = os.path.join(self.repo, 'tools', 'client-ci', 'scenarios')
        os.makedirs(d)
        with open(os.path.join(d, 's.py'), 'w') as f:
            f.write('# thing.pyc:10\n')
        self.assertEqual(self.cite()['thing.pyc:10'].status, 'changed')

    def test_a_shifted_line_in_unchanged_code_makes_a_sed(self):
        self.both('thing', module(nested=[fn('F', 10)]), module(nested=[fn('F', 25)]))
        self.rust('// one\n// `thing.py:10`\n')
        rc, out = self.cli()
        self.assertEqual(rc, 0)
        self.assertIn("sed -i '2s/thing\\.py:10/thing.py:25/' server/crates/x/src/lib.rs",
                      open(os.path.join(out, 'shifts.sh')).read())

    def test_a_range_shifts_at_both_ends(self):
        c = C('F', [(10, [op(LOAD_CONST, 0), op(POP_TOP)]), (12, [op(LOAD_CONST, 0), op(RETURN_VALUE)])], consts=(1,))
        d = C('F', [(20, [op(LOAD_CONST, 0), op(POP_TOP)]), (22, [op(LOAD_CONST, 0), op(RETURN_VALUE)])], consts=(1,))
        self.both('thing', module(nested=[c]), module(nested=[d]))
        self.rust('// `thing.py:10-12`\n')
        got = self.cite()['thing.py:10-12']
        self.assertEqual((got.new_line, got.new_end), (20, 22))


class ModuleBodies(Base):
    """One new constant in a module must not fire every citation into it."""

    def test_an_added_constant_leaves_cited_lines_alone_and_shifts_them(self):
        self.both('settings', module([(5, 'one', 1), (9, 'two', 2)]),
                  module([(3, 'new', 0), (6, 'one', 1), (10, 'two', 2)]))
        self.rust('// `settings.py:9`\n')
        c = self.cite()['settings.py:9']
        self.assertEqual((c.status, c.new_line), ('same', 10))

    def test_the_last_line_is_not_changed_by_a_constant_after_it(self):
        self.both('settings', module([(5, 'one', 1)]), module([(5, 'one', 1), (8, 'two', 2)]))
        self.rust('// `settings.py:5`\n')
        self.assertEqual(self.cite()['settings.py:5'].status, 'same')

    def test_a_changed_cited_value_fires(self):
        self.both('settings', module([(5, 'one', 1), (9, 'two', 2)]),
                  module([(3, 'new', 0), (5, 'one', 7), (9, 'two', 2)]))
        self.rust('// `settings.py:5`\n')
        self.assertEqual(self.cite()['settings.py:5'].status, 'changed')


class RustSide(Base):
    def test_a_bound_constant_that_moved_fires(self):
        self.both('dogma/const', module([(1, 'attributeFoo', 11)]), module([(1, 'attributeFoo', 12)]))
        self.rust('/// `attributeFoo` [V].\npub const FOO_RANGE: f64 = 11.0;\npub const OTHER: i64 = 1;\n')
        rc, out = self.cli()
        self.assertEqual(rc, 3)
        self.assertIn('FOO_RANGE', open(os.path.join(out, 'recheck.tsv')).read())

    def test_a_comment_with_several_names_binds_the_one_that_matches(self):
        self.both('dogma/const', module([(1, 'effectA', 21), (2, 'effectB', 22)]))
        self.rust('/// `effectA` / `effectB`.\npub const A: i64 = 21;\npub const B: i64 = 22;\n')
        self.assertEqual(self.cli()[0], 0)

    def test_a_value_old_does_not_confirm_is_listed_not_fired(self):
        self.rust('/// `attributeFoo`, the default.\nconst SCALE: f64 = 0.5;\n')
        rc, out = self.cli()
        self.assertEqual(rc, 0)
        self.assertIn('disagrees', open(os.path.join(out, 'recheck.tsv')).read())

    def test_an_id_in_a_mod_attr_block_keeps_its_name(self):
        self.both('dogma/const', module([(1, 'attributeFoo', 11)]), module([(1, 'attributeBaz', 11)]))
        self.rust('pub mod attr {\n    pub const THING: i64 = 11;\n}\npub const UNRELATED: i64 = 11;\n')
        rc, out = self.cli()
        self.assertEqual(rc, 3)
        self.assertIn('attr 11 was attributeFoo, now attributeBaz', open(os.path.join(out, 'recheck.tsv')).read())

    def test_an_id_list_is_read_entry_by_entry(self):
        self.both('inventorycommon/const', module([(1, 'groupOne', 31), (2, 'groupTwo', 32)]),
                  module([(1, 'groupOne', 31)]))
        self.rust('pub const THING_GROUPS: &[i64] = &[\n    31,\n    32,\n];\n')
        rc, out = self.cli()
        self.assertEqual(rc, 3)
        self.assertIn('group 32', open(os.path.join(out, 'recheck.tsv')).read())

    def test_test_code_is_not_read(self):
        self.both('dogma/const', module([(1, 'attributeFoo', 11)]), module([(1, 'attributeFoo', 12)]))
        self.rust('#[cfg(test)]\nmod tests {\n    /// `attributeFoo`.\n    const A_ATTR: i64 = 11;\n}\n')
        self.assertEqual(self.cli()[0], 0)

    def test_a_type_word_needs_to_be_a_whole_word(self):
        self.assertIsNone(pr.rust_family('PROTOTYPE_LIMIT', None))
        self.assertEqual(pr.rust_family('SHIP_TYPE', None), 'type')
        self.assertEqual(pr.rust_family('RANGE', 'attr'), 'attr')


class Cli(Base):
    def test_clean_run_is_zero_and_writes_three_files(self):
        self.table(('module', 'dogma/const', '', 't'))
        rc, out = self.cli()
        self.assertEqual(rc, 0)
        self.assertEqual(sorted(os.listdir(out)), ['recheck.md', 'recheck.tsv', 'shifts.sh'])

    def test_a_row_that_needs_a_reader_is_three(self):
        self.both('m', module(nested=[fn('F', 1, 1)]), module(nested=[fn('F', 1, 2)]))
        self.table(('symbol', 'm', 'F', 't'))
        self.assertEqual(self.cli()[0], 3)

    def test_a_stale_row_is_shown_and_is_not_an_impact(self):
        self.table(('symbol', 'dogma/const', 'NoSuchThing', 't'))
        rc, out = self.cli()
        self.assertEqual(rc, 0)
        self.assertIn('Stale table rows', open(os.path.join(out, 'recheck.md')).read())

    def test_a_missing_tree_is_two(self):
        self.assertEqual(pr.main([self.old, os.path.join(self.td, 'absent'), '--repo', self.repo]), 2)

    def test_a_malformed_row_is_one(self):
        self.table(('nonsense', 'm', 'F', 't'))
        self.assertEqual(self.cli()[0], 1)
        self.table(('symbol', 'm', 't'))
        self.assertEqual(self.cli()[0], 1)

    def test_default_out_follows_new_build(self):
        self.assertEqual(pr.default_out('out/new-build/7/pyc'), 'out/new-build/7/recheck')
        self.assertEqual(pr.default_out('out/pyc-1'), os.path.join('out', 'port-recheck', 'pyc-1'))


class CommittedTable(unittest.TestCase):
    def test_every_row_parses_and_is_a_path_not_a_file(self):
        path = os.path.join(os.path.dirname(HERE), 'findings', 'port-recheck.tsv')
        rows, bad = pr.read_table(path)
        self.assertEqual(bad, [])
        self.assertGreater(len(rows), 100)
        seen = set()
        for r in rows:
            self.assertFalse(r['path'].endswith(('.py', '.pyc')), r)
            key = (r['kind'], r['path'], r['symbol'])
            self.assertNotIn(key, seen)
            seen.add(key)


if __name__ == '__main__':
    unittest.main()
