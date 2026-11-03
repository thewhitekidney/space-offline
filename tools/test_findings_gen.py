"""Tests for the findings generators (lane findings-gen, issue #18).

Stdlib unittest on invented code only: every Python 2.7 code object here is
assembled by hand from opcodes and written to a temp directory as a `.pyc`
by a tiny marshal writer, so the generators run end to end on a tree that
contains nothing from any client. Names, keys, lines and events are made up.

    tools/analysis.sh python3 -I tools/test_findings_gen.py
    python3 tools/test_findings_gen.py
"""
import importlib.util
import io
import os
import pathlib
import struct
import sys
import tempfile
import unittest
from unittest import mock

TOOLS = pathlib.Path(__file__).resolve().parent
if str(TOOLS) not in sys.path:
    sys.path.append(str(TOOLS))


def _load(name):
    spec = importlib.util.spec_from_file_location(name, TOOLS / f"{name}.py")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


import pyc27  # noqa: E402

L = _load("findings_lib")
CC = _load("scan_client_callable")
SK = _load("scan_session_keys")
UE = _load("scan_user_errors")
PU = _load("scan_push")
GW = _load("gw_dump")
PM = _load("scan_proto_messages")

OPCODE = {name: num for num, name in pyc27.OPNAME.items()}
FILE = b"C:\\Agent\\work\\abc\\eve\\release\\V99.00\\pkg\\mod.py"


# ----------------------------------------------------------- code construction

def asm(items):
    """[(opname, arg or None)] -> bytes."""
    out = bytearray()
    for name, arg in items:
        out.append(OPCODE[name])
        if OPCODE[name] >= 90:
            out += struct.pack("<H", arg or 0)
    return bytes(out)


def mk(name, items, consts=(), names=(), varnames=(), argcount=0, flags=0, firstlineno=1,
       lnotab=b"", filename=FILE):
    c = pyc27.Code()
    c.argcount = argcount
    c.nlocals = len(varnames)
    c.stacksize = 8
    c.flags = flags
    c.code = asm(items)
    c.consts = tuple(consts)
    c.names = tuple(n.encode() for n in names)
    c.varnames = tuple(v.encode() for v in varnames)
    c.freevars = ()
    c.cellvars = ()
    c.filename = filename
    c.name = name.encode()
    c.firstlineno = firstlineno
    c.lnotab = lnotab
    return c


def dump(v):
    """A marshal writer for just the types the invented code objects use."""
    if v is None:
        return b"N"
    if isinstance(v, bool):
        return b"T" if v else b"F"
    if isinstance(v, int):
        return b"i" + struct.pack("<i", v)
    if isinstance(v, bytes):
        return b"s" + struct.pack("<i", len(v)) + v
    if isinstance(v, tuple):
        return b"(" + struct.pack("<i", len(v)) + b"".join(dump(x) for x in v)
    if isinstance(v, pyc27.Code):
        return (b"c" + struct.pack("<iiii", v.argcount, v.nlocals, v.stacksize, v.flags) +
                dump(v.code) + dump(v.consts) + dump(v.names) + dump(v.varnames) +
                dump(v.freevars) + dump(v.cellvars) + dump(v.filename) + dump(v.name) +
                struct.pack("<i", v.firstlineno) + dump(v.lnotab))
    raise TypeError(v)


def write_pyc(root, rel, code):
    p = pathlib.Path(root) / (rel + ".pyc")
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(b"\x03\xf3\r\n\0\0\0\0" + dump(code))
    return p


def class_body(name, body_items, consts, names, firstlineno=1, lnotab=b""):
    return mk(name, [("LOAD_NAME", 0), ("STORE_NAME", 1)] + body_items + [("LOAD_LOCALS", None), ("RETURN_VALUE", None)],
              consts=consts, names=["__name__", "__module__"] + names, firstlineno=firstlineno, lnotab=lnotab)


def module_with_class(cls_code, bases=(), extra_names=()):
    """Module code: `class Name(bases): ...` assembled the way 2.7 does."""
    cname = cls_code.name
    items = [("LOAD_CONST", 0)]
    names = list(bases) + list(extra_names) + [cname.decode()]
    for i, _b in enumerate(bases):
        items.append(("LOAD_NAME", i))
    items += [("BUILD_TUPLE", len(bases)), ("LOAD_CONST", 1), ("MAKE_FUNCTION", 0), ("CALL_FUNCTION", 0),
              ("BUILD_CLASS", None), ("STORE_NAME", len(bases) + len(extra_names)), ("LOAD_CONST", 2), ("RETURN_VALUE", None)]
    return mk("<module>", items, consts=(cname, cls_code, None), names=names)


# ----------------------------------------------------------------- findings_lib

class LibTests(unittest.TestCase):
    def test_rel_filename_strips_agent_and_branch(self):
        c = mk("f", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,))
        self.assertEqual(L.rel_filename(c), "pkg/mod.py")
        c.filename = b"C:\\x\\release\\V77.7\\packages\\a\\b.py"
        self.assertEqual(L.rel_filename(c), "packages/a/b.py")
        c.filename = b"plain\\name.py"
        self.assertEqual(L.rel_filename(c), "plain/name.py")

    def test_line_table_and_line_at(self):
        c = mk("f", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), firstlineno=10,
               lnotab=bytes([3, 2, 6, 1]))   # addr 3 -> line 12, addr 9 -> line 13
        self.assertEqual(L.line_table(c), [(0, 10), (3, 12), (9, 13)])
        self.assertEqual(L.line_at(c, 0), 10)
        self.assertEqual(L.line_at(c, 4), 12)
        self.assertEqual(L.line_at(c, 99), 13)

    def test_method_defs_sees_duplicates_and_decorators(self):
        f1 = mk("run", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), firstlineno=5)
        f2 = mk("run", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), firstlineno=9)
        f3 = mk("deco", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), firstlineno=14)
        body = [("LOAD_CONST", 0), ("MAKE_FUNCTION", 0), ("STORE_NAME", 2),
                ("LOAD_CONST", 1), ("MAKE_FUNCTION", 0), ("STORE_NAME", 2),
                ("LOAD_NAME", 3), ("LOAD_CONST", 2), ("MAKE_FUNCTION", 0), ("CALL_FUNCTION", 1), ("STORE_NAME", 4)]
        cb = class_body("C", body, (f1, f2, f3), ["run", "decorate", "deco"])
        # names: 0 __name__, 1 __module__, 2 run, 3 decorate, 4 deco
        ins = list(pyc27.dis(cb))
        defs = L.method_defs(cb, ins)
        self.assertEqual([(n, k.firstlineno) for n, k, _d, _o in defs], [("run", 5), ("run", 9), ("deco", 14)])
        self.assertEqual(L.methods_of(cb, ins)["run"][0].firstlineno, 9)   # the live one

    def test_eval_assignments_dict_with_or_and_fmt(self):
        body = [("BUILD_MAP", 2),
                ("LOAD_NAME", 2), ("LOAD_NAME", 3), ("BINARY_OR", None), ("BUILD_LIST", 1), ("LOAD_CONST", 0), ("STORE_MAP", None),
                ("LOAD_CONST", 3), ("LOAD_CONST", 2), ("STORE_MAP", None), ("STORE_NAME", 4)]
        # second entry: {1: ...}? keep it a plain const value under key b'beta'
        cb = class_body("C", body, (b"alpha", None, b"beta", 1), ["ROLE_A", "ROLE_B", "__exportedcalls__"])
        ins = list(pyc27.dis(cb))
        got = L.eval_assignments(cb, ins, ("__exportedcalls__",))
        sym, _first, _store = got["__exportedcalls__"]
        self.assertEqual(sym.kind, "dict")
        self.assertEqual(L.fmt_sym(sym.val[0][1]), "[ROLE_A|ROLE_B]")

    def test_fmt_sym_sorts_dict_keys(self):
        d = L.Sym("dict", [(L.Sym("const", b"role"), L.Sym("name", "ROLE_ANY")),
                           (L.Sym("const", b"fastcall"), L.Sym("const", 1))])
        self.assertEqual(L.fmt_sym(d), "{fastcall: 1, role: ROLE_ANY}")

    def test_tsv_line_flattens_separators(self):
        self.assertEqual(L.tsv_line(["a\tb", "c\nd", 3]), "a b\tc d\t3")

    def test_write_tsv_single_trailing_newline(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "x", "t.tsv")
            L.write_tsv(p, ["h", "r"])
            with open(p) as fh:
                self.assertEqual(fh.read(), "h\nr\n")


# -------------------------------------------------------------- client-callable

class ClientCallableTests(unittest.TestCase):
    def test_arg_names_normalises_star_names(self):
        f = mk("m", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), argcount=2,
               varnames=("self", "a", "rest", "extra"), flags=0x04 | 0x08)
        self.assertEqual(CC.arg_names(f), "a, *args, **kw")

    def test_returns_of(self):
        none = mk("n", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,))
        val = mk("v", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(5,))
        mixed = mk("m", [("LOAD_CONST", 0), ("RETURN_VALUE", None), ("LOAD_CONST", 1), ("RETURN_VALUE", None)],
                   consts=(None, 5))
        gen = mk("g", [("LOAD_CONST", 0), ("YIELD_VALUE", None), ("LOAD_CONST", 1), ("RETURN_VALUE", None)],
                 consts=(5, None))
        self.assertEqual(CC.returns_of(none), "None")
        self.assertEqual(CC.returns_of(val), "value")
        self.assertEqual(CC.returns_of(mixed), "None-or-value")
        self.assertEqual(CC.returns_of(gen), "generator")

    def test_scan_end_to_end(self):
        got = mk("Get", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(7,), argcount=2,
                 varnames=("self", "thing"), firstlineno=30)
        body = [("BUILD_MAP", 2),
                ("BUILD_LIST", 0), ("LOAD_CONST", 0), ("STORE_MAP", None),
                ("BUILD_LIST", 0), ("LOAD_CONST", 1), ("STORE_MAP", None), ("STORE_NAME", 2),
                ("LOAD_CONST", 3), ("STORE_NAME", 3),
                ("LOAD_CONST", 2), ("MAKE_FUNCTION", 0), ("STORE_NAME", 4)]
        cb = class_body("Widget", body, (b"Get", b"FromBase", got, b"svc.widget"),
                        ["__exportedcalls__", "__guid__", "Get"],
                        firstlineno=20, lnotab=bytes([0, 0]))
        mod = module_with_class(cb)
        svc_items = [("LOAD_CONST", 0), ("LOAD_CONST", 1), ("IMPORT_NAME", 0), ("IMPORT_FROM", 1), ("STORE_NAME", 2),
                     ("POP_TOP", None), ("LOAD_CONST", 2), ("RETURN_VALUE", None)]
        svc = mk("<module>", svc_items, consts=(-1, (b"Widget",), None), names=["pkg.mod", "Widget", "widgets"])
        with tempfile.TemporaryDirectory() as d:
            write_pyc(d, "pkg/mod", mod)
            write_pyc(d, "svc", svc)
            rows, st = CC.scan(d)
        self.assertEqual(st["classes"], 1)
        self.assertEqual(len(rows), 2)
        by = {r[5]: r for r in rows}
        self.assertEqual(by["Get"][:5], ["widgets", "Widget", "svc.widget", "pkg/mod.py", 20])
        self.assertEqual(by["Get"][6:], [30, "thing", "[]", "value"])
        self.assertEqual(by["FromBase"][6:], ["inherited", "?", "[]", "unknown"])

    def test_render_has_header_and_label(self):
        lines = CC.render([["s", "C", "g", "m.py", 1, "M", 2, "", "[]", "None"]], "V1/2")
        self.assertIn("V1/2", lines[0])
        self.assertEqual(lines[len(CC.HEADER)], "\t".join(CC.COLUMNS))
        self.assertEqual(lines[-1].split("\t")[0], "s")


# ---------------------------------------------------------------- session keys

class SessionKeysTests(unittest.TestCase):
    ROW = ["charid", "int", "received", "change", "9 reads in 9 modules; gate key", "a", "b", "V",
           "pkg/mod.py:2; pkg/gone.py:5"]

    def _tree(self, d):
        # two modules read session.charid, one reads a local called session (not counted)
        for rel in ("pkg/mod", "pkg/other"):
            c = mk("f", [("LOAD_GLOBAL", 0), ("LOAD_ATTR", 1), ("POP_TOP", None), ("LOAD_CONST", 0), ("RETURN_VALUE", None)],
                   consts=(None,), names=["session", "charid"], firstlineno=2)
            write_pyc(d, rel, c)
        c = mk("g", [("LOAD_FAST", 0), ("LOAD_ATTR", 0), ("POP_TOP", None), ("LOAD_CONST", 0), ("RETURN_VALUE", None)],
               consts=(None,), names=["charid"], varnames=("session",))
        write_pyc(d, "pkg/local", c)

    def test_count_reads_only_global_session(self):
        with tempfile.TemporaryDirectory() as d:
            self._tree(d)
            got, other, lines = SK.count_reads(d, ["charid", "ghost"])
        self.assertEqual(got["charid"][0], 2)
        self.assertEqual(sorted(got["charid"][1]), ["pkg/mod", "pkg/other"])
        self.assertEqual(got["ghost"][0], 0)
        self.assertIn(2, lines["pkg/mod.py"])

    def test_phrase_singular_plural(self):
        self.assertEqual(SK.phrase(1, 1), "1 read in 1 module")
        self.assertEqual(SK.phrase(3, 2), "3 reads in 2 modules")

    def test_refresh_rewrites_count_keeps_prose_and_reports_drift(self):
        rows = [list(self.ROW)]
        lines_of = {"pkg/mod.py": {2, 3}}
        new, drift = SK.refresh(rows, {"charid": (2, {"a", "b"})}, lines_of)
        self.assertEqual(new[0][4], "2 reads in 2 modules; gate key")
        self.assertEqual(new[0][5:8], ["a", "b", "V"])
        self.assertTrue(any("readers" in d for d in drift))
        self.assertTrue(any("pkg/gone.py:5" in d and "no such module" in d for d in drift))
        self.assertFalse(any("pkg/mod.py:2" in d for d in drift))

    def test_refresh_flags_line_without_bytecode(self):
        rows = [list(self.ROW)]
        _new, drift = SK.refresh(rows, {"charid": (9, set(range(9)))}, {"pkg/mod.py": {7}, "pkg/gone.py": {5}})
        self.assertTrue(any("pkg/mod.py:2" in d and "no bytecode" in d for d in drift))

    def test_zero_direct_reads_row_kept_until_it_changes(self):
        row = ["hq", "int", "received", "change", "0 direct reads: feeds corprole", "", "", "V", "pkg/mod.py"]
        new, drift = SK.refresh([list(row)], {"hq": (0, set())}, {"pkg/mod.py": {1}})
        self.assertEqual(new[0][4], "0 direct reads: feeds corprole")
        self.assertEqual(drift, [])
        new, drift = SK.refresh([list(row)], {"hq": (2, {"x"})}, {"pkg/mod.py": {1}})
        self.assertTrue(new[0][4].startswith("2 reads in 1 module"))
        self.assertTrue(drift)

    def test_idempotent_on_its_own_output(self):
        with tempfile.TemporaryDirectory() as d:
            self._tree(d)
            seed = os.path.join(d, "seed.tsv")
            row = list(self.ROW)
            row[4] = "2 reads in 2 modules; gate key"
            row[8] = "pkg/mod.py:2"
            with open(seed, "w", encoding="utf8") as fh:
                fh.write("\n".join(["\t".join(SK.COLUMNS), "\t".join(row)]) + "\n")
            out = os.path.join(d, "out.tsv")
            SK.main(["--pyc", d, "--seed", seed, "--out", out])
            with open(out) as fo, open(seed) as fs:
                self.assertEqual(fo.read(), fs.read())


# ----------------------------------------------------------------- user errors

class UserErrorTests(unittest.TestCase):
    def test_presentation_mapping(self):
        self.assertEqual(UE.presentation("info"), "modal")
        self.assertEqual(UE.presentation("notify"), "logline")
        self.assertEqual(UE.presentation("audio"), "audio-only")
        self.assertEqual(UE.presentation(None), "unknown")

    def test_labels_of_suffixes(self):
        rows = [{"FullPath": "UI/Messages/12", "label": "KeyOneTitle", "messageID": 1},
                {"FullPath": "UI/Messages/12", "label": "KeyOneBody", "messageID": 2},
                {"FullPath": "UI/Messages/13", "label": "KeyTwo", "messageID": 3},
                {"FullPath": "UI/Other/13", "label": "Nope", "messageID": 4}]
        got = UE.labels_of(rows)
        self.assertEqual(got["KeyOne"], {"num": "12", "title": 1, "body": 2})
        self.assertEqual(got["KeyTwo"], {"num": "13", "body": 3})
        self.assertNotIn("Nope", got)

    def test_loc_args_sorted_set_keeps_types(self):
        texts = {2: "A {b} and {[numeric]a} and {b}"}
        self.assertEqual(UE.loc_args(2, texts), "[numeric]a,b")
        self.assertEqual(UE.loc_args(None, texts), "-")
        self.assertEqual(UE.loc_args(9, texts), "-")

    def _tree(self, d):
        # raise UserError('NoRoom', {'need': 1}) at line 4 and catch `e.msg == 'NoRoom'` at line 9
        raise_fn = mk("fail", [("LOAD_GLOBAL", 0), ("LOAD_CONST", 0), ("LOAD_CONST", 1), ("LOAD_CONST", 2),
                               ("BUILD_MAP", 1), ("STORE_MAP", None), ("CALL_FUNCTION", 2), ("RAISE_VARARGS", 1),
                               ("LOAD_CONST", 3), ("RETURN_VALUE", None)],
                      consts=(b"NoRoom", 1, b"need", None), names=["UserError"], firstlineno=4)
        # the dict order on the stack is value, key; build it faithfully
        raise_fn.code = asm([("LOAD_GLOBAL", 0), ("LOAD_CONST", 0), ("BUILD_MAP", 1), ("LOAD_CONST", 1),
                             ("LOAD_CONST", 2), ("STORE_MAP", None), ("CALL_FUNCTION", 2), ("RAISE_VARARGS", 1),
                             ("LOAD_CONST", 3), ("RETURN_VALUE", None)])
        catch_fn = mk("handle", [("LOAD_FAST", 0), ("LOAD_ATTR", 0), ("LOAD_CONST", 0), ("COMPARE_OP", 2),
                                 ("POP_JUMP_IF_FALSE", 13), ("LOAD_FAST", 0), ("LOAD_ATTR", 1), ("LOAD_ATTR", 2),
                                 ("POP_TOP", None), ("LOAD_CONST", 1), ("RETURN_VALUE", None)],
                      consts=(b"NoRoom", None), names=["msg", "dict", "get"], varnames=("e",), argcount=1,
                      firstlineno=9)
        top = mk("<module>", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(raise_fn,))
        top.consts = (raise_fn, catch_fn)
        write_pyc(d, "pkg/mod", top)

    def test_scan_code_finds_raise_and_catch(self):
        with tempfile.TemporaryDirectory() as d:
            self._tree(d)
            raises, catches = UE.scan_code(d)
        self.assertEqual([(f, ks) for f, _l, ks in raises["NoRoom"]], [("pkg/mod.py", ["need"])])
        self.assertEqual(raises["NoRoom"][0][1], 4)
        c = catches["NoRoom"][0]
        self.assertEqual((c[0], c[2], c[3]), ("pkg/mod.py", "handle", "msg"))

    def test_build_rows_merges_three_sources(self):
        fsd_rows = {"NoRoom": {"dialogID": 7, "dialogType": "info", "suppressable": False, "closable": True,
                               "titleID": None, "bodyID": 22}}
        labels = {"NoRoom": {"num": "7", "body": 22}, "OnlyLabel": {"num": "8", "title": 31, "body": 32}}
        texts = {22: "need {need}", 32: "x"}
        raises = {"NoRoom": [("pkg/mod.py", 4, ["need"])], "FreeText": [("pkg/mod.py", 5, [])]}
        catches = {"NoRoom": [("pkg/mod.py", 9, "handle", "msg", {"dict_keys": {"need"}, "refs": {"Close"}})]}
        rows = UE.build_rows(fsd_rows, labels, texts, raises, catches, {"NoRoom": "prose"})
        by = {r[0]: r for r in rows}
        self.assertEqual([r[0] for r in rows], ["FreeText", "NoRoom", "OnlyLabel"])
        self.assertEqual(by["NoRoom"][1:10], ["fsd+loc+pyc", "7", "info", "modal", "False", "True", "-", "22", "need"])
        self.assertEqual(by["NoRoom"][10:], [1, "pkg/mod.py:4", "need", "need", "pkg/mod.py:9", "handle", "msg", "Close", "prose"])
        self.assertEqual(by["OnlyLabel"][1:9], ["loc", "8", "-", "unknown", "-", "-", "31", "32"])
        self.assertEqual(by["FreeText"][1], "pyc")
        self.assertEqual(by["FreeText"][-1], "-")

    def test_catch_sites_truncate_to_three_with_count(self):
        catches = {"K": [("f%d.py" % i, 1, "fn%d" % i, "msg", {"dict_keys": set(), "refs": set()}) for i in range(5)]}
        row = UE.build_rows({}, {}, {}, {}, catches, {})[0]
        self.assertEqual(row[14], "f0.py:1;f1.py:1;f2.py:1;+2")
        self.assertEqual(row[15], "fn0;fn1;fn2")


# ------------------------------------------------------------------- push events

class ProtoMessagesTests(unittest.TestCase):
    def run_main(self, *argv):
        err = io.StringIO()
        with mock.patch.object(sys, "argv", ["scan_proto_messages.py", *argv]), \
                mock.patch.object(sys, "stderr", err):
            try:
                PM.main()
            except SystemExit as e:
                return e.code, err.getvalue()
        return None, err.getvalue()

    def test_a_missing_tree_is_an_error_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "o.txt")
            code, _ = self.run_main("--pyc", os.path.join(d, "nope"), "--out", out)
            self.assertIn("no eveProto", str(code))
            self.assertFalse(os.path.exists(out))

    def test_an_empty_tree_is_an_error_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "t", "eveProto"))
            out = os.path.join(d, "o.txt")
            code, _ = self.run_main("--pyc", os.path.join(d, "t"), "--out", out)
            self.assertIn("refusing to write an empty table", str(code))
            self.assertFalse(os.path.exists(out))


class PushSeedTests(unittest.TestCase):
    def test_seed_and_old_pyc_go_together(self):
        for argv in (["--seed", "x.tsv"], ["--old-pyc", "somewhere"]):
            with self.assertRaises(SystemExit) as cm:
                PU.main(argv + ["--pyc", "."])
            self.assertIn("go together", str(cm.exception))


class PushTests(unittest.TestCase):
    def _handler(self, name, items, consts, names, varnames, argcount, **kw):
        return mk(name, items, consts=consts, names=names, varnames=varnames, argcount=argcount, **kw)

    def test_arity_text(self):
        f = mk("h", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), argcount=4,
               varnames=("self", "a", "b", "c"), flags=0x04)
        self.assertEqual(PU.arity_text(f, 0), "3+*args")
        self.assertEqual(PU.arity_text(f, 1), "2..3+*args")

    def test_analyse_discard_attr_subscript_unpack(self):
        h = self._handler("OnX", [
            ("LOAD_FAST", 2), ("LOAD_ATTR", 0), ("POP_TOP", None),                      # b.thing
            ("LOAD_FAST", 3), ("LOAD_CONST", 0), ("BINARY_SUBSCR", None), ("POP_TOP", None),   # c['k']
            ("LOAD_FAST", 4), ("UNPACK_SEQUENCE", 2), ("POP_TOP", None), ("POP_TOP", None),   # d unpack_2
            ("LOAD_CONST", 1), ("RETURN_VALUE", None)],
            (b"k", None), ["thing"], ("self", "a", "b", "c", "d"), 5)
        a = PU.analyse_handler(h, "OnX")
        self.assertEqual(a["shapes"], ["discard", "attr", "subscript", "unpack_2"])
        self.assertEqual(a["ev_attr"][1], [".thing"])
        self.assertEqual(a["ev_sub"][2], ["'k'"])
        self.assertIn("strict-arity", a["risk"])
        self.assertEqual(PU.risk_tags(a, "OnX"), "strict-arity;raises-on-shape")

    def test_pass_is_dropped_when_shape_is_pinned(self):
        h = self._handler("OnY", [
            ("LOAD_GLOBAL", 0), ("LOAD_FAST", 1), ("CALL_FUNCTION", 1), ("POP_TOP", None),      # f(a)
            ("LOAD_FAST", 2), ("LOAD_ATTR", 1), ("POP_TOP", None),                              # b.x
            ("LOAD_GLOBAL", 0), ("LOAD_FAST", 2), ("CALL_FUNCTION", 1), ("POP_TOP", None),      # f(b)
            ("LOAD_CONST", 0), ("RETURN_VALUE", None)],
            (None,), ["f", "x"], ("self", "a", "b"), 3)
        a = PU.analyse_handler(h, "OnY")
        self.assertEqual(a["shapes"], ["pass", "attr"])
        self.assertEqual(PU.evidence_text(a), "a >f; b .x >f")   # evidence keeps the call even when the shape drops it

    def test_do_events_and_session_change_risks(self):
        h = self._handler("DoSessionChanging", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], (None,), [], ("self",), 1)
        a = PU.analyse_handler(h, "DoSessionChanging")
        self.assertEqual(PU.risk_tags(a, "DoSessionChanging"), "do-sync-blocktrap;session-change-path")
        self.assertEqual(PU.risk_tags(a, "OnPlain"), "none")

    def test_native_payload_is_not_native_touch(self):
        h = self._handler("OnZ", [
            ("LOAD_GLOBAL", 0), ("LOAD_ATTR", 1), ("LOAD_FAST", 1), ("CALL_FUNCTION", 1), ("POP_TOP", None),
            ("LOAD_CONST", 0), ("RETURN_VALUE", None)],
            (None,), ["blue", "Do"], ("self", "a"), 2)
        a = PU.analyse_handler(h, "OnZ")
        self.assertEqual(a["shapes"], ["native"])
        self.assertEqual(PU.risk_tags(a, "OnZ"), "native-payload")

    def test_declared_events_forms(self):
        def decl(items, consts, names):
            cb = class_body("C", items, consts, names)
            ci = PU.ClassInfo()
            ci.code, ci.ins = cb, list(pyc27.dis(cb))
            return PU.declared_events(ci)
        # plain list
        own, comp = decl([("LOAD_CONST", 0), ("LOAD_CONST", 1), ("BUILD_LIST", 2), ("STORE_NAME", 2)],
                         (b"OnA", b"OnB"), ["__notifyevents__"])
        self.assertEqual((own, comp), ({"OnA", "OnB"}, False))
        # base + own constants
        own, comp = decl([("LOAD_CONST", 0), ("BUILD_LIST", 1), ("LOAD_NAME", 3), ("LOAD_ATTR", 2),
                          ("BINARY_ADD", None), ("STORE_NAME", 2)], (b"OnA",), ["__notifyevents__", "Base"][:1] + ["__notifyevents__", "Base"])
        self.assertEqual(own, {"OnA"})
        # bare string: declares its characters (the client iterates it)
        own, _ = decl([("LOAD_CONST", 0), ("STORE_NAME", 2)], (b"OnA",), ["__notifyevents__"])
        self.assertEqual(own, {"O", "n", "A"})
        # never assigned
        own, comp = decl([("LOAD_CONST", 0), ("STORE_NAME", 2)], (5,), ["other"])
        self.assertIsNone(own)

    def test_alias_of_base_is_computed_unless_mutated(self):
        cb = class_body("C", [("LOAD_NAME", 2), ("LOAD_ATTR", 3), ("STORE_NAME", 3)], (), ["Base", "__notifyevents__"])
        # names: 0 __name__, 1 __module__, 2 Base, 3 __notifyevents__
        ci = PU.ClassInfo()
        ci.code, ci.ins = cb, list(pyc27.dis(cb))
        own, comp = PU.declared_events(ci)
        self.assertEqual((own, comp), (set(), True))

    def _tree(self, d):
        """Base declares OnA; Child has OnA (inherited relation) and Plain has an unrelated OnA."""
        def meth(line):
            return mk("OnA", [("LOAD_CONST", 0), ("RETURN_VALUE", None)], consts=(None,), argcount=2,
                      varnames=("self", "x"), firstlineno=line)
        decl = [("LOAD_CONST", 0), ("BUILD_LIST", 1), ("STORE_NAME", 2),
                ("LOAD_CONST", 1), ("MAKE_FUNCTION", 0), ("STORE_NAME", 3)]
        base = class_body("Base", decl, (b"OnA", meth(10)), ["__notifyevents__", "OnA"])
        write_pyc(d, "pkg/base", module_with_class(base))
        child = class_body("Child", [("LOAD_CONST", 0), ("MAKE_FUNCTION", 0), ("STORE_NAME", 2)], (meth(20),), ["OnA"])
        write_pyc(d, "pkg/child", module_with_class(child, bases=("Base",)))
        plain = class_body("Plain", [("LOAD_CONST", 0), ("MAKE_FUNCTION", 0), ("STORE_NAME", 2)], (meth(30),), ["OnA"])
        write_pyc(d, "pkg/plain", module_with_class(plain, bases=("object",)))
        lonely = class_body("Lonely", [("LOAD_CONST", 0), ("BUILD_LIST", 1), ("STORE_NAME", 2)], (b"OnNobody",), ["__notifyevents__"])
        write_pyc(d, "pkg/lonely", module_with_class(lonely))

    def test_generate_reconciles_and_drops_coincidences(self):
        with tempfile.TemporaryDirectory() as d:
            self._tree(d)
            table, st = PU.generate(d)
        rows = {(r[0], r[1]): r for r in table}
        self.assertEqual(st["classes"], 4)
        self.assertEqual(st["declaring"], 2)
        base = rows[("OnA", "pkg/mod.py:10")]
        child = rows[("OnA", "pkg/mod.py:20")]
        self.assertEqual(base[3], "notifyevents")
        self.assertEqual(child[3], "notifyevents")
        self.assertIn("base:pkg/base.pyc::Base", child[8])
        self.assertNotIn(("OnA", "pkg/mod.py:30"), rows)             # coincidence, dropped
        nohandler = rows[("OnNobody", "(none)")]
        self.assertEqual(nohandler[3], "notifyevents (no handler)")
        self.assertIn("declared by 1 class(es): pkg/lonely.py::Lonely", nohandler[8])
        self.assertEqual([r[0] for r in table], sorted(r[0] for r in table))

    def test_carry_keeps_curated_cells_unless_the_generator_moved(self):
        def row(line, risk, notes="n"):
            return ["OnA", "pkg/mod.py:%d" % line, "S", "notifyevents", "1", "x", "pass", risk, notes]
        old = [row(10, "plain"), row(20, "plain")]
        seed = [row(10, "curated"), row(20, "curated")]
        new = [row(12, "plain"), row(20, "plain", "n2"), row(30, "plain")]
        out, moved = PU.carry(new, old, seed)
        self.assertEqual(out[0], row(12, "curated"))            # line moved, curation kept
        self.assertEqual(out[1], row(20, "curated", "n2"))      # notes moved, risk kept
        self.assertEqual(out[2], row(30, "plain"))              # no counterpart: taken whole
        self.assertEqual(moved, 2)

    def test_resolve_bases_prefers_same_file_then_last(self):
        a1, a2, a3 = PU.ClassInfo(), PU.ClassInfo(), PU.ClassInfo()
        for i, (c, rel) in enumerate(((a1, "x"), (a2, "y"), (a3, "z"))):
            c.uid, c.rel, c.name, c.bases = i, rel, "Dup", []
        child = PU.ClassInfo()
        child.uid, child.rel, child.name, child.bases = 3, "w", "Child", ["Dup"]
        self.assertEqual(PU.resolve_bases([a1, a2, a3, child])[3], [2])      # last in tree order
        child.rel = "y"
        self.assertEqual(PU.resolve_bases([a1, a2, a3, child])[3], [1])      # same file wins


# ---------------------------------------------------------------------- gw_dump

class FakeField:
    def __init__(self, number, name, type_, label=1, type_name="", oneof=None):
        self.number, self.name, self.type, self.label, self.type_name = number, name, type_, label, type_name
        self._oneof = oneof

    def HasField(self, _n):
        return self._oneof is not None

    @property
    def oneof_index(self):
        return self._oneof


class FakeOneof:
    def __init__(self, name):
        self.name = name


class FakeMsg:
    def __init__(self, name, fields=(), oneofs=(), nested=()):
        self.name, self.field, self.oneof_decl, self.nested_type = name, list(fields), list(oneofs), list(nested)


class GwDumpTests(unittest.TestCase):
    def test_dump_message_lines(self):
        inner = FakeMsg("Inner", [FakeField(1, "v", 5)])
        m = FakeMsg("Outer", [FakeField(1, "name", 9), FakeField(2, "items", 11, label=3, type_name=".p.Inner"),
                              FakeField(3, "kind", 14, type_name=".p.Kind", oneof=0), FakeField(4, "odd", 99)],
                    [FakeOneof("which")], [inner])
        out = []
        GW.dump_message(m, "", out)
        self.assertEqual(out, ["message Outer", "  oneof#0 which",
                               "  1 string  name", "  2 rep msg .p.Inner items",
                               "  3 enum .p.Kind kind oneof0", "  4 99  odd",
                               "  message Inner", "    1 int32  v"])


if __name__ == "__main__":
    unittest.main()
