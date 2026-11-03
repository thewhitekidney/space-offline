"""Tests for the generic cFSD decoder in `tools/fsd.py` (packet K1).

**Synthetic bytes only.** Every container here is built by `build_cfsd`
below from the format `fsd.CfsdTable` documents; nothing reads the client,
and nothing from the client is in this file. The client-backed checks --
the generic rows against every hand-written path, row counts, foreign-key
closures -- are `python3 -I tools/fsd.py generic-check <client>` and are
recorded in `docs/lanes/fsd-generic.md`.

Run::

    python3 -I tools/test_fsd_generic.py
    tools/analysis.sh python3 -I /work/tools/test_fsd_generic.py
"""

from __future__ import annotations

import os
import struct
import sys
import tempfile
import unittest
from unittest import mock

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import fsd  # noqa: E402

U32 = struct.Struct("<I")
I32 = struct.Struct("<i")
U64 = struct.Struct("<Q")


# --- a synthetic cFSD writer -------------------------------------------------


class Arena:
    """The variable-length area: ``[u64 count][data]`` vectors, 8-aligned."""

    def __init__(self, base):
        self.base = base
        self.buf = bytearray()

    def vector(self, count, data):
        """Write one vector; return the payload offset of its first byte."""
        while len(self.buf) % 8:
            self.buf.append(0)
        self.buf += U64.pack(count)
        ptr = self.base + len(self.buf)
        self.buf += data
        return ptr

    def string(self, s):
        b = s.encode("utf-8")
        return self.vector(len(b), b)

    def dict_(self, buckets, entry):
        """A dict: ``buckets`` is ``[[entry args...] per bucket]``.

        Each bucket's entries are written as one vector, back to back, then
        the bucket-pointer array. Returns ``(pointer, total entries)``.
        """
        ptrs = []
        for items in buckets:
            if not items:
                ptrs.append(0)
                continue
            ptrs.append(self.vector(len(items), b"".join(entry(*it) for it in items)))
        ptr = self.vector(len(buckets), b"".join(U64.pack(p) for p in ptrs))
        return ptr, sum(len(b) for b in buckets)


def build_cfsd(path, rows, nb, stride, str_keys=False):
    """Write a ``.fsdbinary``-shaped file. ``rows`` is ``[(key, fill)]``.

    ``fill(arena, key_ptr)`` returns the record's bytes after the key slot.
    An integer key goes in bucket ``key % nb``, ascending; a string key in
    bucket ``index % nb`` (the real hash is not reproduced, and the reader
    must not need it).
    """
    buckets = [[] for _ in range(nb)]
    for i, (key, fill) in enumerate(rows):
        b = (i % nb) if str_keys else key % nb
        buckets[b].append((key, fill))
    if not str_keys:
        for b in buckets:
            b.sort(key=lambda kv: kv[0])
    pos = 24 + 8 * nb
    offsets = []
    for b in buckets:
        offsets.append(pos + 8)
        pos += 8 + len(b) * stride
    var_base = (pos + 7) // 8 * 8
    arena = Arena(var_base)
    body = bytearray()
    for b in buckets:
        body += U64.pack(len(b))
        for key, fill in b:
            if str_keys:
                head = U64.pack(arena.string(key))
            else:
                head = U32.pack(key)
            rec = head + fill(arena)
            if len(rec) != stride:
                raise AssertionError("record is %d bytes, not %d" % (len(rec), stride))
            body += rec
    payload = bytearray(U64.pack(24) + U64.pack(len(rows)) + U64.pack(nb))
    payload += b"".join(U64.pack(o) for o in offsets)
    payload += body
    payload += b"\0" * (var_base - len(payload))
    payload += arena.buf
    with open(path, "wb") as fh:
        fh.write(b"\x5a" * 24 + U64.pack(len(payload)) + payload)


def tsv(*lines):
    head = "table\tfield\toffset\ttype\tconfidence\thow_derived"
    return [head + "\n"] + [ln + "\n" for ln in lines]


def rec(table, stride, delta=8):
    return "%s\t#record\t-\tstride=%d delta=%d\tV\tsynthetic" % (table, stride, delta)


def fld(table, name, off, kind, opt=None):
    how = "synthetic"
    if opt:
        how += "; optional(bit=%s,word=+%d)" % (hex(opt[0]), opt[1])
    return "%s\t%s\t%d\t%s\tV\t%s" % (table, name, off, kind, how)


# --- the widgets table: every kind the decoder handles, in one record -------

WIDGETS = tsv(
    rec("widgets", 128),
    fld("widgets", "name", 8, "str"),
    fld("widgets", "tags", 16, "list<str>"),
    fld("widgets", "parts", 24, "list<struct:part>"),
    fld("widgets", "byRoom", 32, "dict<str,struct:room>"),
    fld("widgets", "labels", 48, "dict<str,i32>"),
    fld("widgets", "rates", 64, "dict<str,f64>"),
    fld("widgets", "weight", 80, "f64"),
    fld("widgets", "big", 88, "i64"),
    fld("widgets", "pos", 96, "vec3<f32>"),
    fld("widgets", "count", 108, "i32"),
    fld("widgets", "ratio", 112, "f32", (0x1, 120)),
    fld("widgets", "level", 116, "i32", (0x2, 120)),
    fld("widgets", "flag", 124, "u8"),
    fld("widgets", "struct:part/typeID", 0, "i32"),
    fld("widgets", "struct:part/qty", 4, "i32"),
    fld("widgets", "struct:part/note", 8, "str", (0x1, 16)),
    fld("widgets", "struct:room/roomID", 0, "i32"),
    fld("widgets", "struct:room/scale", 4, "f32"),
)


def widget(name, parts, rooms, labels, rates, opt):
    """A `fill` for one widgets record. ``opt`` is ``(ratio, level)``."""

    def fill(a):
        tags = [a.string(t) for t in ("a", "bb")]
        tags_ptr = a.vector(len(tags), b"".join(U64.pack(p) for p in tags))
        pdata = b""
        for tid, qty, note in parts:
            np = a.string(note) if note is not None else 0
            pdata += struct.pack("<iiQI4x", tid, qty, np, 0x1 if note is not None else 0)
        parts_ptr = a.vector(len(parts), pdata)
        nb = 2
        rb = [[], []]
        for rid, scale in rooms:
            rb[rid % nb].append((rid, scale))
        room_ptr, room_n = a.dict_(
            rb, lambda rid, scale: struct.pack("<I", rid) + struct.pack("<if", rid, scale))
        lb = [[(a.string(k), v)] for k, v in labels]
        lab_ptr, lab_n = a.dict_(lb, lambda kp, v: struct.pack("<Qi4x", kp, v))
        # f32 values in an "f64" dict: 8-byte entries, which pack.
        fb = [[], [], []]
        for k, v in rates:
            fb[k % 3].append((k, v))
        rate_ptr, rate_n = a.dict_(fb, lambda k, v: struct.pack("<If", k, v))
        ratio, level = opt
        pres = (0x1 if ratio is not None else 0) | (0x2 if level is not None else 0)
        return (
            b"\0" * 4
            + U64.pack(a.string(name))
            + U64.pack(tags_ptr)
            + U64.pack(parts_ptr)
            + U64.pack(room_ptr) + U64.pack(room_n)
            + U64.pack(lab_ptr) + U64.pack(lab_n)
            + U64.pack(rate_ptr) + U64.pack(rate_n)
            + struct.pack("<d", 2.5)
            + struct.pack("<q", -(1 << 40))
            + struct.pack("<3f", 1.0, -2.0, 0.5)
            + struct.pack("<i", 7)
            + struct.pack("<f", ratio or 0.0)
            + struct.pack("<i", level or 0)
            + struct.pack("<I", pres)
            + b"\x01" + b"\0" * 3
        )

    return fill


class Tmp(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.dir = self._dir.name

    def tearDown(self):
        self._dir.cleanup()

    def table(self, name, rows, nb, stride, str_keys=False):
        path = os.path.join(self.dir, name + ".fsdbinary")
        build_cfsd(path, rows, nb, stride, str_keys)
        return fsd.CfsdTable(path, name)


# --- the schema file ----------------------------------------------------------


class SchemaParse(unittest.TestCase):
    def test_record_fields_structs_and_optional_markers(self):
        s = fsd.parse_pyd_schemas(WIDGETS)["widgets"]
        self.assertEqual((s.stride, s.delta, s.status), (128, 8, "ok"))
        self.assertEqual([f.name for f in s.fields][:3], ["name", "tags", "parts"])
        ratio = [f for f in s.fields if f.name == "ratio"][0]
        self.assertEqual((ratio.bit, ratio.word), (0x1, 120))
        self.assertEqual(sorted(s.structs), ["part", "room"])

    def test_nested_sizes_are_align_up_span_align(self):
        s = fsd.parse_pyd_schemas(WIDGETS)["widgets"]
        # part: i32, i32, str@8, presence word @16 -> span 20, align 8 -> 24.
        # room: i32, f32 -> span 8, align 4 -> 8.
        self.assertEqual(s.sizes, {"part": 24, "room": 8})
        self.assertEqual(s.computed_stride(), 128)

    def test_struct_inside_struct_is_sized_innermost_first(self):
        lines = tsv(
            rec("t", 40),
            fld("t", "outer", 8, "struct:outer"),
            fld("t", "struct:outer/inner", 0, "struct:inner"),
            fld("t", "struct:outer/n", 12, "i32"),
            fld("t", "struct:inner/a", 0, "i32"),
            fld("t", "struct:inner/b", 4, "i32"),
            fld("t", "struct:inner/c", 8, "u8"),
        )
        s = fsd.parse_pyd_schemas(lines)["t"]
        self.assertEqual(s.sizes["inner"], 12)
        self.assertEqual(s.sizes["outer"], 16)

    def test_no_attribute_table_has_a_status_not_fields(self):
        lines = tsv("bare\t-\t-\t-\tV\tno-attributes: no tp_getattro")
        s = fsd.parse_pyd_schemas(lines)["bare"]
        self.assertEqual((s.status, s.fields), ("no-attributes", []))

    def test_bad_header_is_loud(self):
        with self.assertRaises(fsd.FsdError):
            fsd.parse_pyd_schemas(["nope\n"])

    def test_presence_fixes_apply_and_can_be_switched_off(self):
        lines = tsv(rec("archetypes", 40), fld("archetypes", "titleID", 28, "i32"))
        fixed = fsd.parse_pyd_schemas(lines)["archetypes"].fields[0]
        self.assertEqual((fixed.bit, fixed.word), (0x8, 32))
        raw = fsd.parse_pyd_schemas(lines, fixes={})["archetypes"].fields[0]
        self.assertIsNone(raw.bit)


# --- decoding ----------------------------------------------------------------


class Decode(Tmp):
    def widgets(self):
        rows = [
            (10, widget("ten", [(34, 400, None), (35, 3, "x")],
                        [(12778, 1.5), (12779, 0.25), (12780, 2.0)],
                        [("alpha", 1), ("beta", -2)],
                        [(1000010, 0.8), (1000011, 0.5), (1000012, 0.25), (1000013, 1.0)],
                        (0.75, None))),
            (11, widget("eleven", [], [], [], [], (None, 42))),
        ]
        t = self.table("widgets", rows, nb=2, stride=128)
        sch = fsd.parse_pyd_schemas(WIDGETS)["widgets"]
        dec = fsd.CfsdDecoder(t, sch)
        return fsd.cfsd_generic_rows(t, sch, decoder=dec), dec

    def test_every_kind_decodes(self):
        got, _dec = self.widgets()
        ten = got[10]
        self.assertEqual(ten["name"], "ten")
        self.assertEqual(ten["tags"], ["a", "bb"])
        self.assertEqual(ten["parts"], [
            {"typeID": 34, "qty": 400, "note": None},
            {"typeID": 35, "qty": 3, "note": "x"},
        ])
        self.assertEqual(ten["byRoom"], {
            12778: {"roomID": 12778, "scale": 1.5},
            12779: {"roomID": 12779, "scale": 0.25},
            12780: {"roomID": 12780, "scale": 2.0},
        })
        self.assertEqual(ten["labels"], {"alpha": 1, "beta": -2})
        self.assertEqual(ten["weight"], 2.5)
        self.assertEqual(ten["big"], -(1 << 40))
        self.assertEqual(ten["pos"], (1.0, -2.0, 0.5))
        self.assertEqual(ten["count"], 7)
        self.assertEqual(ten["ratio"], 0.75)
        self.assertIsNone(ten["level"])
        self.assertEqual(ten["flag"], 1)

    def test_empty_containers_and_the_other_optional(self):
        got, _dec = self.widgets()
        eleven = got[11]
        self.assertEqual((eleven["parts"], eleven["byRoom"], eleven["labels"]), ([], {}, {}))
        self.assertIsNone(eleven["ratio"])
        self.assertEqual(eleven["level"], 42)

    def test_f64_dict_holding_f32_is_read_as_f32(self):
        got, dec = self.widgets()
        rates = got[10]["rates"]
        self.assertEqual(sorted(rates), [1000010, 1000011, 1000012, 1000013])
        self.assertAlmostEqual(rates[1000010], 0.8, places=6)
        self.assertEqual(rates[1000013], 1.0)
        self.assertIn("int/f32", dec.key_modes["row.rates"])

    def test_key_widths_are_recorded(self):
        _got, dec = self.widgets()
        self.assertIn("int/struct:room", dec.key_modes["row.byRoom"])
        self.assertIn("str/i32", dec.key_modes["row.labels"])

    def test_presence_audit_is_clean_on_a_good_layout(self):
        _got, dec = self.widgets()
        self.assertEqual(dec.presence["row"][:2], [2, 0])
        self.assertEqual(dec.presence["struct:part"][:2], [2, 0])

    def test_presence_audit_counts_a_bit_no_field_claims(self):
        lines = tsv(rec("p", 16), fld("p", "a", 8, "i32", (0x1, 12)))
        sch = fsd.parse_pyd_schemas(lines)["p"]
        t = self.table("p", [(1, lambda a: b"\0" * 4 + I32.pack(5) + U32.pack(0x5))], 1, 16)
        dec = fsd.CfsdDecoder(t, sch)
        self.assertEqual(fsd.cfsd_generic_rows(t, sch, decoder=dec), {1: {"a": 5}})
        self.assertEqual(dec.presence["row"], [1, 1, 0x4])


class Strides(Tmp):
    LINES = tsv(rec("s", 16), fld("s", "a", 8, "i32"), fld("s", "b", 12, "i32"))

    def rows(self):
        # Two rows per bucket, so the container can measure its own stride.
        return [(k, lambda a, k=k: b"\0" * 4 + I32.pack(1) + I32.pack(2))
                for k in (100, 102, 101, 103)]

    def test_loader_and_container_agree(self):
        t = self.table("s", self.rows(), 2, 16)
        self.assertEqual(t.stride, 16)
        sch = fsd.parse_pyd_schemas(self.LINES)["s"]
        self.assertEqual(len(fsd.cfsd_generic_rows(t, sch)), 4)

    def test_a_disagreement_raises(self):
        t = self.table("s", self.rows(), 2, 16)
        lines = [ln.replace("stride=16", "stride=24") for ln in self.LINES]
        sch = fsd.parse_pyd_schemas(lines)["s"]
        with self.assertRaises(fsd.FsdError):
            fsd.cfsd_generic_rows(t, sch)

    def test_undetermined_container_stride_uses_the_loaders(self):
        # One row per bucket: the data cannot constrain the stride.
        rows = [(k, lambda a: b"\0" * 4 + I32.pack(1) + I32.pack(2)) for k in (5, 6, 7)]
        t = self.table("s", rows, 3, 16)
        self.assertIsNone(t.stride)
        sch = fsd.parse_pyd_schemas(self.LINES)["s"]
        self.assertEqual(sorted(fsd.cfsd_generic_rows(t, sch)), [5, 6, 7])

    def test_limit(self):
        t = self.table("s", self.rows(), 2, 16)
        sch = fsd.parse_pyd_schemas(self.LINES)["s"]
        self.assertEqual(len(fsd.cfsd_generic_rows(t, sch, limit=2)), 2)


class StringKeys(Tmp):
    def test_string_keyed_records_round_the_stride_up_to_8(self):
        # The loader states 12 (8-byte key pointer + one i32); records are 16.
        lines = tsv(rec("hues", 12), fld("hues", "hue", 8, "i32"))
        rows = [(name, lambda a, v=v: I32.pack(v) + b"\0" * 4)
                for name, v in (("royal", 214), ("cheese", 50), ("pea", 83))]
        t = self.table("hues", rows, 2, 16, str_keys=True)
        sch = fsd.parse_pyd_schemas(lines)["hues"]
        self.assertEqual(fsd.cfsd_generic_rows(t, sch),
                         {"royal": {"hue": 214}, "cheese": {"hue": 50}, "pea": {"hue": 83}})

    def test_garbage_keys_are_refused(self):
        lines = tsv(rec("g", 12), fld("g", "v", 8, "i32"))
        rows = [(k, lambda a: b"\0" * 12) for k in (1, 2, 3)]
        t = self.table("g", rows, 2, 16)
        t._stride = None
        sch = fsd.parse_pyd_schemas(lines)["g"]
        with self.assertRaises(fsd.FsdError):
            fsd.cfsd_generic_rows(t, sch)


class Unknowns(Tmp):
    def test_untyped_containers_and_variants_are_cfsdunknown(self):
        lines = tsv(
            rec("atoms", 56),
            fld("atoms", "loose", 8, "dict<str,list>"),
            fld("atoms", "p", 24, "struct:parameter"),
            fld("atoms", "struct:parameter/defaultValue", 0, "list<i32>"),
        )
        sch = fsd.parse_pyd_schemas(lines)["atoms"]

        def fill(a):
            inner = a.vector(1, U64.pack(0))
            buckets = a.vector(1, U64.pack(a.vector(1, struct.pack("<I4xQ", 3, inner))))
            return b"\0" * 4 + U64.pack(buckets) + U64.pack(1) + b"\xff" * 32

        t = self.table("atoms", [(1, fill)], 1, 56)
        row = fsd.cfsd_generic_rows(t, sch)[1]
        self.assertEqual(row["loose"], fsd.CfsdUnknown("dict<str,list>", 0, 1))
        self.assertEqual(row["p"]["defaultValue"], fsd.CfsdUnknown("variant", 0))

    def test_bare_tables(self):
        sch = fsd.bare_schema("agenttypes")
        t = self.table("agenttypes", [(4, lambda a: b"\0" * 4 + U64.pack(a.string("ResearchAgent")))], 1, 16)
        self.assertEqual(fsd.cfsd_generic_rows(t, sch), {4: "ResearchAgent"})
        self.assertIsNone(fsd.bare_schema("challengeevents"))


class DictMajority(Tmp):
    def test_a_field_that_flips_is_redecoded_with_the_majority(self):
        # Three rows string-keyed; one whose only key is "" -- which a lone
        # integer reading also fits. The majority must win.
        lines = tsv(rec("oc", 24), fld("oc", "m", 8, "dict<str,i32>"))
        sch = fsd.parse_pyd_schemas(lines)["oc"]

        def mk(keys, nb=2):
            def fill(a):
                items = [(a.string(k), i) for i, k in enumerate(keys)]
                ptr, n = a.dict_([items[b::nb] for b in range(nb)],
                                 lambda kp, v: struct.pack("<Qi4x", kp, v))
                return b"\0" * 4 + U64.pack(ptr) + U64.pack(n)
            return fill

        # Rows 1-3: two buckets, so the even pointers fail the integer hash
        # and the string reading is decisive. Row 4: one bucket holding "",
        # where only the integer reading fits on the first pass.
        rows = [(1, mk(["a", "b"])), (2, mk(["cc", "d"])), (3, mk(["e", "ff"])),
                (4, mk([""], nb=1))]
        t = self.table("oc", rows, 4, 24)
        dec = fsd.CfsdDecoder(t, sch)
        got = fsd.cfsd_generic_rows(t, sch, decoder=dec)
        self.assertEqual(dec.forced, {"row.m": "str/i32"})
        self.assertEqual(got[4]["m"], {"": 0})
        self.assertEqual(got[1]["m"], {"a": 0, "b": 1})

    def test_an_empty_string_key_alone_still_decodes(self):
        lines = tsv(rec("oc", 24), fld("oc", "m", 8, "dict<str,i32>"))
        sch = fsd.parse_pyd_schemas(lines)["oc"]

        def fill(a):
            items = [(a.string(""), 5), (a.string("zz"), 6)]
            ptr, n = a.dict_([items[:1], items[1:]],
                             lambda kp, v: struct.pack("<Qi4x", kp, v))
            return b"\0" * 4 + U64.pack(ptr) + U64.pack(n)

        t = self.table("oc", [(1, fill)], 1, 24)
        self.assertEqual(fsd.cfsd_generic_rows(t, sch), {1: {"m": {"": 5, "zz": 6}}})


class SameValue(unittest.TestCase):
    def test_vocabulary_differences_are_forgiven_and_nothing_else(self):
        sv = fsd.same_value
        self.assertTrue(sv(4294967295, -1))
        self.assertTrue(sv(True, 1))
        self.assertTrue(sv(float("nan"), float("nan")))
        self.assertTrue(sv({1: [2, 3]}, {1: [2, 3]}))
        self.assertFalse(sv(None, 0))
        self.assertFalse(sv(1, 2))
        self.assertFalse(sv([1], [1, 2]))
        self.assertFalse(sv("a", "b"))


class RealSchemaFile(unittest.TestCase):
    """The committed TSV (a schema, not client data) parses and sizes."""

    def test_committed_tsv_loads_and_computes_its_own_strides(self):
        if not os.path.exists(fsd.PYD_SCHEMA_TSV):
            self.skipTest("findings/pyd-schemas.tsv absent")
        schemas = fsd.load_pyd_schemas()
        ok = [s for s in schemas.values() if s.status == "ok" and s.fields]
        self.assertGreaterEqual(len(ok), 139)
        for s in ok:
            if s.stride is not None:
                self.assertEqual(s.computed_stride(), s.stride, s.table)


class BuildLayouts(unittest.TestCase):
    """`fsd.CFSD_BUILD_LAYOUTS`: the loader layout is chosen by client build."""

    def tmp(self):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        return d.name

    def test_no_build_is_the_reference_and_a_known_one_is_found(self):
        self.assertEqual(fsd.layout_for_build(None), fsd.layout_for_build(fsd.CFSD_REFERENCE_BUILD))
        self.assertEqual(fsd.layout_for_build("3628513"), fsd.layout_for_build(3619879))
        self.assertIsNotNone(fsd.layout_for_build(3619879)[0])
        self.assertIsNone(fsd.layout_for_build(3662168)[0])  # the base file is its layout

    def test_an_unregistered_build_is_refused_not_guessed(self):
        for bad in (1, "7", "x"):
            with self.assertRaises(fsd.FsdError) as cm:
                fsd.layout_for_build(bad)
            self.assertIn("CFSD_BUILD_LAYOUTS", str(cm.exception))
        with self.assertRaises(fsd.FsdError):
            fsd.load_pyd_schemas(build=1)
        with self.assertRaises(fsd.FsdError):
            fsd.schema_for("types", build=1)

    def test_rows_refuses_a_client_whose_build_has_no_entry(self):
        root = self.tmp()
        with open(os.path.join(root, "start.ini"), "w") as fh:
            fh.write("[main]\nbuild = 1234\n")

        class Fake:
            pass

        c = Fake()
        c.root = root
        self.assertEqual(fsd.client_layout_build(c), 1234)
        with self.assertRaises(fsd.FsdError):
            fsd.rows(c, "types")

    def test_a_client_with_no_start_ini_gets_the_reference(self):
        class Fake:
            root = self.tmp()

        self.assertIsNone(fsd.client_layout_build(Fake()))

    def test_overlay_replaces_only_its_tables_and_uses_its_own_fixes(self):
        d = self.tmp()
        base = os.path.join(d, "base.tsv")
        over = os.path.join(d, "over-77.tsv")
        with open(base, "w") as fh:
            fh.writelines(tsv(
                rec("alpha", 24), fld("alpha", "a", 8, "i32"),
                rec("beta", 24), fld("beta", "b", 8, "i32"), fld("beta", "gate", 12, "i32")))
        with open(over, "w") as fh:
            fh.writelines(tsv(rec("beta", 16), fld("beta", "b", 8, "i32"),
                              fld("beta", "gate", 12, "i32")))
        mine = {"beta": {("row", "gate"): (0x80000000, 12)}}
        with mock.patch.dict(fsd.CFSD_BUILD_LAYOUTS,
                             {77: ("over-77.tsv", mine), 78: (None, {})}):
            ref = fsd.load_pyd_schemas(base, 78)
            new = fsd.load_pyd_schemas(base, 77)
        self.assertEqual((ref["beta"].stride, new["beta"].stride), (24, 16))
        self.assertEqual(new["alpha"].stride, 24)
        gate = [f for f in new["beta"].fields if f.name == "gate"][0]
        self.assertEqual((gate.bit, gate.word), (0x80000000, 12))
        self.assertIsNone([f for f in ref["beta"].fields if f.name == "gate"][0].bit)

    def test_the_cached_layouts_do_not_leak_between_builds(self):
        a = fsd.load_pyd_schemas(build=3619879)["spacecomponentsbytype"]
        b = fsd.load_pyd_schemas(build=3662168)["spacecomponentsbytype"]
        self.assertNotEqual(a.stride, b.stride)
        self.assertIs(b, fsd.load_pyd_schemas()["spacecomponentsbytype"])

    def _fields(self, sch, owner, name):
        fl = sch.fields if owner == "row" else sch.structs.get(owner[7:], ())
        return [f for f in fl if f.name == name]

    def test_every_fix_of_every_build_lands_inside_its_record(self):
        if not os.path.exists(fsd.PYD_SCHEMA_TSV):
            self.skipTest("findings/pyd-schemas.tsv absent")
        for build, (_over, fixes) in fsd.CFSD_BUILD_LAYOUTS.items():
            schemas = fsd.load_pyd_schemas(build=build)
            for table, items in fixes.items():
                sch = schemas[table]
                for (owner, name), (bit, word) in items.items():
                    got = self._fields(sch, owner, name)
                    self.assertEqual(len(got), 1, (build, table, name))
                    self.assertEqual((got[0].bit, got[0].word), (bit, word), (build, table, name))
                    # A stale word past the record is the one error a wrong
                    # fix can show on its own (the other is content_npc's
                    # universal-component check).
                    stride = sch.stride if owner == "row" else sch.sizes.get(owner[7:])
                    if stride is not None:
                        self.assertLessEqual(word + 4, stride, (build, table, name))

    def test_the_two_builds_have_their_own_spacecomponents_layout(self):
        if not os.path.exists(fsd.PYD_SCHEMA_TSV):
            self.skipTest("findings/pyd-schemas.tsv absent")
        old = fsd.load_pyd_schemas(build=3619879)["spacecomponentsbytype"]
        new = fsd.load_pyd_schemas(build=3662168)["spacecomponentsbytype"]
        self.assertEqual((old.stride, new.stride), (1832, 1824))
        self.assertEqual(self._fields(old, "row", "tagSubscriber")[0].kind.startswith("struct"), True)
        self.assertEqual(self._fields(new, "row", "tagSubscriber"), [])
        self.assertEqual(self._fields(new, "row", "underConstruction")[0].word, 1816)
        uc = self._fields(old, "row", "underConstruction")[0]
        self.assertNotEqual((uc.bit, uc.word), (0x80000000, 1816))

    def test_the_hand_corrected_dungeons_row_survives_in_every_build(self):
        if not os.path.exists(fsd.PYD_SCHEMA_TSV):
            self.skipTest("findings/pyd-schemas.tsv absent")
        for build in fsd.CFSD_BUILD_LAYOUTS:
            sch = fsd.load_pyd_schemas(build=build)["dungeons"]
            hits = [f for fl in sch.structs.values() for f in fl if f.name == "messageTextID"]
            self.assertEqual([f.offset for f in hits], [72], build)


class PydSchemaTables(unittest.TestCase):
    """`pyd_schema.select_tables`: one command for an overlay file."""

    def setUp(self):
        try:
            import pyd_schema
        except SystemExit:  # its pefile and capstone live in the analysis image
            self.skipTest("pyd_schema needs the analysis image")
        self.ps = pyd_schema

    def schemas(self):
        return [self.ps.TableSchema(stem) for stem in ("alpha", "beta", "gamma")]

    def test_no_filter_keeps_all(self):
        sch = self.schemas()
        self.assertEqual(self.ps.select_tables(sch, ""), sch)

    def test_filter_keeps_named_in_stem_order_and_ignores_case_and_suffix(self):
        sch = self.schemas()
        got = self.ps.select_tables(sch, "Gamma.fsdbinary, alpha")
        self.assertEqual([s.stem for s in got], ["alpha", "gamma"])

    def test_an_unknown_table_is_refused_by_name(self):
        with self.assertRaises(ValueError) as cm:
            self.ps.select_tables(self.schemas(), "alpha,nope")
        self.assertIn("nope", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
