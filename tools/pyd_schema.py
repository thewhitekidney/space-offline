#!/usr/bin/env python3
"""Recover cFSD record schemas from the client's generated ``*Loader.pyd``.

What this is
------------
``res:/staticdata/*.fsdbinary`` ("cFSD") is read by native code: ``bin64/``
holds exactly one ``<table>Loader.pyd`` per ``.fsdbinary`` table. The
*container* is decoded in ``tools/fsd.py`` and documented in
``docs/re/static-corpus.md`` §3.4, but the *per-field layout inside a record*
exists only in that generated C++.

This tool reads the layout back out of the DLL. It is a **schema generator**:
it writes ``(name, offset, type)`` triples plus a record stride and a base
delta, and nothing out of any data file. See ``docs/re/native-loaders.md``.

It needs ``pefile`` and ``capstone``, which are already in the analysis image,
so run it there:

    tools/analysis.sh python3 tools/pyd_schema.py table types --client /client
    tools/analysis.sh python3 tools/pyd_schema.py tsv --client /client \\
        --out findings/pyd-schemas.tsv
    tools/analysis.sh python3 tools/pyd_schema.py check --client /client
    tools/analysis.sh python3 tools/pyd_schema.py tsv --client /client \\
        --tables types,dungeons --out findings/pyd-schemas-<build>.tsv   # an overlay

``--client`` may be the install directory or its ``bin64``; it falls back to
``/client/tq``, ``/client`` and ``$EVE_CLIENT``. The client is only ever read.

How it works, and why each step is sound
----------------------------------------
Claims are **[V]** (read out of the binary and reproduced across all 149
loaders, or checked against a layout established independently from the data
files) or **[I]** (inferred). ``unknown`` means unknown.

**1. There is no ``PyGetSetDef`` table. [V]** The obvious plan — find the
``{name, getter, setter, doc, closure}`` array and disassemble 150 tiny
getters — does not apply to these DLLs. The only pointer-to-name arrays in
``.data`` are ``PyMethodDef``s (32 bytes, ``ml_flags`` 1 or 4 in the third
slot) for ``load``, ``__dir__``, ``keys``, ``values``, ``items``, ``get``,
``has_key`` and the iterators; ``tp_getset`` is null everywhere. Instead CCP's
generator emits **one ``tp_getattro`` per record type** containing the whole
attribute set as a linear comparison chain, and the attribute names sit in
``.rdata`` as 8-byte-aligned NUL-padded C strings referenced only by
``lea reg, [rip+disp]`` from inside that function. That is why
``fsd_dump.py names`` could scrape the names but could not order or type them.

**2. The chain has one shape, repeated per attribute. [V]** Every
``tp_getattro`` starts

    mov  rax, qword ptr [rdx + 8]            ; name->ob_type
    mov  BASE, qword ptr [rcx + 0x10]        ; self->data, the record pointer
    test dword ptr [rax + 0xa8], 0x8000000   ; Py_TPFLAGS_STRING_SUBCLASS
    je   <"Attribute is not a string">
    call PyString_AsString

and then, once per attribute and in the schema's own declaration order:

    lea   rdx, [rip + N]                     ; N -> the attribute name
    call  <strcmp>                / or an inlined movzx/cmp byte loop
    jne   <next attribute>          ending in `cmp reg, len+1`
    test  byte ptr [BASE + F], MASK          ; optional fields only:
    je    <return Py_None>                   ;   the presence bitfield
    mov   ecx, dword ptr [BASE + K]          ; the value
    jmp   PyInt_FromLong                     ; the box

So the parser anchors on each name ``lea``, and inside that attribute's span
takes the ``test``-ed presence word, the last base-relative access that is not
the presence word, and the boxing import. The chain ends at
``PyErr_Format(PyExc_AttributeError, "Attribute '%s' does not exist on this
instance", name)``, which is also how the function is recognised.

**3. The box names the width and the kind. [V]** Across all 149 loaders the
python27 imports that can box a field value are exactly five:
``PyInt_FromLong``, ``PyFloat_FromDouble``, ``PyLong_FromLongLong``,
``PyString_FromStringAndSize`` and ``PyUnicodeUCS2_FromUnicode`` (one loader).
Paired with the load instruction that feeds them:

===============================  ==============================  =========
load                             box                             type
===============================  ==============================  =========
``movsd xmm0, qword [B+K]``      ``PyFloat_FromDouble``          ``f64``
``movss xmm0, dword [B+K]``      ``PyFloat_FromDouble``          ``f32``
``mov e_x, dword [B+K]``         ``PyInt_FromLong``              ``i32``
``movzx e_x, byte [B+K]``        ``PyInt_FromLong``              ``u8``
``movzx e_x, word [B+K]``        ``PyInt_FromLong``              ``u16``
``mov r_x, qword [B+K]``         ``PyLong_FromLongLong``         ``i64``
``mov rcx, [B+K]``; ``[rcx-8]``  ``PyString_FromStringAndSize``  ``str``
``mov rcx, [B+K]``; strlen       ``PyUnicodeUCS2_FromUnicode``   ``wstr``
===============================  ==============================  =========

The string case is the loader confirming, rather than this repo assuming, the
arena convention ``fsd.py`` already reads: the record slot holds a
payload-relative pointer and the byte length sits in the eight bytes
**before** the data.

**[V]** No loader imports ``PyBool_FromLong``, so a 1-byte field is boxed with
``PyInt_FromLong`` like any other integer and nothing in the binary says
"bool". Every 1-byte field seen reads as a flag (``published``,
``anchorable``, ``isDynamicType``), so the emitted type is ``u8`` and
"probably bool" is a note rather than a claim. Whether the generator ever
emits a numeric ``u8`` is ``unknown``.

**4. Container and nested-struct fields are the ones that need care.** A field
whose value is not a scalar is handed to ``_PyObject_New(T)`` — sometimes
inline, more often by a tail call to a one-line helper — with the record
pointer offset by a ``lea``:

    test byte ptr [BASE + F], 0x40
    je   <None>
    lea  rcx, [BASE + 0x20]       ; or `mov rcx, BASE` when K == 0
    jmp  <helper that calls _PyObject_New(T)>

``T``'s ``tp_name`` says which it is: ``cfsd.list``, ``cfsd.dict``, or
``<module>.<struct>`` for a nested record whose own ``tp_getattro`` is another
chain this tool parses the same way. Several of those type objects are
zero-initialised, with every slot assigned at run time by the module's init
code, so ``tp_name`` and the sequence slots are resolved through a module-wide
map of ``mov qword ptr [rip+slot], reg`` assignments as well as from ``.data``.

A ``list``'s element type comes from the list type object's ``sq_item``, and a
``dict``'s key and value types from its ``mp_subscript`` — the key from how
the subscript coerces its argument (``PyString_AsString`` vs
``PyInt_AsSsize_t``), the value exactly like a list element. Resolving either
one *by name* would be wrong: a table with several container-valued fields has
several ``cfsd.list`` and ``cfsd.dict`` type objects sharing one pooled
``tp_name`` string, so each field carries the address of the type object the
loader actually hands to ``_PyObject_New``.

**[V] A ``list`` field occupies 8 bytes of the record and a ``dict`` 16.**
Read off the gaps between consecutive fields in ``dungeons`` and ``missions``,
and consistent with the generated ``keys()``, which takes the data pointer
from ``data+0`` and a ``uint32`` count from ``data+8``.

A vector field is a ``PyStructSequence`` filled in a counted loop over
consecutive slots, and it is the one case where the *indexed* memory operand
is the field access rather than a name comparison:

    lea      rcx, [rip + desc]
    call     PyStructSequence_New
    movss    xmm0, dword ptr [rbx + rdi*4 + 0x74]   ; +116, +120, +124
    cvtps2pd xmm0, xmm0
    call     PyFloat_FromDouble
    inc      rdi
    cmp      rdi, 3

so the arity comes from the loop bound, the element width from the load, and
the member names from the ``PyStructSequence_Desc``: ``vec3<f32>`` with
members ``x,y,z``. This check runs **before** the scalar-box branch, because
the loop boxes each slot with ``PyFloat_FromDouble`` and a box-first parser
drops the field.

**5. Stride and base delta. [V]** The offsets in ``tp_getattro`` are *not*
record offsets: the row object's ``[self+0x10]`` points past the record's
primary key. The factory states both numbers literally —

    imul rbx, rcx, 0x98                 ; types: stride 152
    add  rbx, qword ptr [rax + rdx*8]   ; + the bucket base
    lea  rcx, [rbx + 8]                 ; types: delta 8
    mov  qword ptr [rax + 0x10], rcx

— but the stride is often folded into address arithmetic instead
(``lea rcx,[rcx+rcx*4]`` then ``lea rbx,[rax+rcx*8]`` is a multiply by 40), so
the stride is **also** computed from the recovered layout as
``align_up(delta + span, align)``, where ``span`` is the end of the last field
or presence word and ``align`` is the widest field (floor 4, the key's width).
Both are reported, and ``check`` lists any disagreement.

``delta`` is that ``uint32`` key at record ``+0``, padded up to the first
field's alignment — which is why it is 4 for ``groups`` (all fields 4-byte)
and 8 for ``types``, ``categories`` and ``accountingentrytypes`` (which have
8-byte fields). **[V]** Adding it reproduces the ``fsd-reader`` lane's
data-derived offsets exactly, and the computed stride matches that lane's
data-derived stride for every table where it had one.

The ``offset`` column of the TSV is therefore **record-relative** and directly
usable by ``tools/fsd.py``. Nested-struct offsets are struct-relative, because
a nested instance's ``[self+0x10]`` points at the struct itself with no delta.

Known limits
------------
- **[V]** 10 of the 149 loaders have no ``tp_getattro`` anywhere. Their row is
  a bare scalar or a bare list, not a struct, so there is nothing to name;
  reported ``no-attributes``, which is an answer rather than a failure.
- The 4-byte word some optional scalars carry beside the value (``factionID``
  in ``types`` reads ``[B+0x44]`` and returns the int at ``[B+0x40]`` only when
  that word is 0 or 1) is noted in ``how_derived`` as a tag of ``unknown``
  meaning.
- Enum fields are indistinguishable from ``i32`` here: the value-to-name
  mapping is not in the loader.
- A field name is CCP's own, but what the integer *references* (``typeID`` vs
  ``localizationID`` vs a plain count) is not stated in the binary. Names that
  end in ``NameID``/``DescriptionID`` are message ids in every case checked,
  but this tool does not claim that: it emits ``i32``.
- Signedness of a 4-byte integer is not observable: the load is a plain ``mov``
  and ``PyInt_FromLong`` is signed-agnostic. Emitted ``i32``.
- ``dict<...,unknown>`` appears where a dict's ``mp_subscript`` neither boxes a
  single scalar nor builds a recognisable object.
- ``seasonselection`` gets a layout but no stride or delta: it is the
  header-only singleton with no hash map, so there is no row factory with
  index arithmetic to read them out of.
- A nested record type CCP's generator left anonymous is named
  ``unnamed_<64-bit>`` after the generator's own hash. Which field holds it is
  in the referring row, not in the name.

One more shape to know about, documented here because it cost an afternoon:
MSVC gives one function several ``RUNTIME_FUNCTION`` entries when it chains
unwind info, and the split lands mid-body. ``Module.dis`` joins consecutive
``.pdata`` ranges while the next one's start is not referenced anywhere — not
a call or jump target, not a pointer in ``.data``/``.rdata`` — because a
continuation range is by definition unreachable except by falling into it.
Joining on "the previous range did not end in ``ret``" is **not** equivalent
and misses the three-range ``dungeons`` object record, since every arm of a
``tp_getattro`` ends in ``ret``.
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys

try:
    import capstone
    import pefile
except ImportError:  # pragma: no cover
    sys.exit(
        "pyd_schema: needs pefile and capstone; run it in the analysis image:\n"
        "  tools/analysis.sh python3 tools/pyd_schema.py ..."
    )

ATTR_ERR = b"Attribute '%s' does not exist on this instance"

#: PyTypeObject slots, x86-64 CPython 2.7. **[V]** confirmed in
#: ``typesLoader.pyd``, whose init stores its ``PyMethodDef`` array into
#: ``<rowtype>+0xe8`` (``tp_methods``) with ``tp_name`` at ``+0x18``.
TP_NAME = 0x18
TP_BASICSIZE = 0x20
TP_AS_SEQUENCE = 0x68
TP_AS_MAPPING = 0x70
TP_GETATTRO = 0x90
TP_FLAGS = 0xA8
#: PySequenceMethods: sq_length, sq_concat, sq_repeat, sq_item, ...
SQ_ITEM = 0x18
#: PyMappingMethods: mp_length, mp_subscript, mp_ass_subscript
MP_SUBSCRIPT = 0x08

#: ``Py_TPFLAGS_DEFAULT`` on this build. **[V]** every statically initialised
#: generated type object in all 149 loaders carries exactly this, and the
#: flags word is what keeps the ``.data`` scan from picking up stray qwords.
TPFLAGS = 0x301EB

IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
RIP = re.compile(r"rip ([+-]) (0x[0-9a-f]+|\d+)")
DISP = re.compile(r"\+ (0x[0-9a-f]+|\d+)\]$")
STORE_DATA = re.compile(r"^qword ptr \[rax \+ 0x10\], (r[a-z0-9]+)$")
SELF_DATA = re.compile(r"^r[a-z0-9]+, qword ptr \[rcx \+ 0x10\]$")
SLOT_STORE = re.compile(
    r"^qword ptr \[rip [+-] (?:0x[0-9a-f]+|\d+)\], (r[a-z0-9]+)$"
)

#: The boxing imports, as a census over all 149 loaders. ``None`` means "take
#: the width from the load instruction".
BOXES = {
    "PyFloat_FromDouble": "f64",
    "PyInt_FromLong": None,
    "PyLong_FromLongLong": "i64",
    "PyString_FromStringAndSize": "str",
    "PyUnicodeUCS2_FromUnicode": "wstr",
}

#: Strings inside a ``tp_getattro`` that read as identifiers but are not
#: attribute names.
NOT_ATTRS = {"s", "rb", "std", "type"}

#: Byte size per emitted type, for the span and stride computation.
WIDTH = {
    "f64": 8,
    "f32": 4,
    "i64": 8,
    "i32": 4,
    "i16": 2,
    "i8": 1,
    "u16": 2,
    "u8": 1,
    "str": 8,
    "wstr": 8,
    "list": 8,
    #: **[V]** a list field is one 8-byte arena pointer with its element count
    #: in the eight bytes before the data, exactly like a string; a dict field
    #: is 16 bytes (bucket pointer plus row count). Both were read off the
    #: gaps between consecutive fields in `dungeons` and `missions`, where a
    #: dict is always followed 16 bytes later and a list 8 bytes later.
    "dict": 16,
    "structseq": 8,
    "container": 8,
}


def _imm(text):
    t = text.strip()
    try:
        return int(t, 16) if t.startswith("0x") else int(t)
    except ValueError:
        return None


def _align_up(n, a):
    return (n + a - 1) // a * a


# --- the PE ----------------------------------------------------------------


class Module:
    """One ``*Loader.pyd``: sections, imports, functions, type objects."""

    _MD = None

    def __init__(self, path):
        if Module._MD is None:
            Module._MD = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
            Module._MD.detail = True
        self.path = path
        pe = pefile.PE(path, fast_load=True)
        pe.parse_data_directories(
            directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]]
        )
        self.base = pe.OPTIONAL_HEADER.ImageBase
        self.secs = [
            (
                s.Name.decode("latin1").rstrip("\0"),
                s.VirtualAddress,
                max(s.Misc_VirtualSize, s.SizeOfRawData),
                s.get_data(),
            )
            for s in pe.sections
        ]
        self.iat = {}
        for d in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []) or []:
            for imp in d.imports:
                if imp.name:
                    self.iat[imp.address - self.base] = imp.name.decode("latin1")
        self.funcs = {}
        for name, _va, _sz, d in self.secs:
            if name != ".pdata":
                continue
            for off in range(0, len(d) - 11, 12):
                b, e, _u = struct.unpack_from("<III", d, off)
                if b and e > b:
                    self.funcs[b] = e
        self._dis = {}
        self._entries = None
        self._slots = None
        self._types = None
        self._wrappers = None

    # -- raw access --

    def section_of(self, rva):
        for name, va, size, _d in self.secs:
            if va <= rva < va + size:
                return name
        return None

    def read(self, rva, n):
        for _name, va, size, d in self.secs:
            if va <= rva < va + size:
                return d[rva - va : rva - va + n]
        return b""

    def u64(self, rva):
        b = self.read(rva, 8)
        return struct.unpack("<Q", b)[0] if len(b) == 8 else 0

    def static_ptr(self, rva):
        q = self.u64(rva)
        if q < self.base:
            return 0
        r = q - self.base
        return r if self.section_of(r) else 0

    def cstr(self, rva, limit=256):
        b = self.read(rva, limit)
        i = b.find(b"\0")
        return b[:i] if i >= 0 else b

    def text(self, rva, limit=96):
        try:
            return self.cstr(rva, limit).decode("ascii")
        except UnicodeDecodeError:
            return ""

    # -- disassembly --

    def dis(self, begin):
        """The logical function at ``begin``, across split ``.pdata`` ranges.

        MSVC gives one function several ``RUNTIME_FUNCTION`` entries when it
        chains unwind info, and the split lands mid-body: ``dungeonsLoader``'s
        object-record ``tp_getattro`` is three ranges, and a parser that stops
        at the first loses two thirds of the record. Consecutive ranges are
        joined while the next one's start address is not *referenced* anywhere
        — not a call or jump target, not a pointer in ``.data``/``.rdata``.
        A continuation range is by definition unreachable except by falling
        into it, so that test is exact rather than a guess about padding.
        """
        if begin not in self.funcs:
            return []
        if begin in self._dis:
            return self._dis[begin]
        self._dis[begin] = []  # guard against recursion through entries()
        out = []
        cur = begin
        for _hop in range(16):
            end = self.funcs.get(cur)
            if end is None:
                break
            out.extend(Module._MD.disasm(self.read(cur, end - cur), cur))
            if end not in self.funcs or end in self.entries:
                break
            cur = end
        self._dis[begin] = out
        return out

    def _raw(self, begin):
        end = self.funcs.get(begin)
        if end is None:
            return []
        return list(Module._MD.disasm(self.read(begin, end - begin), begin))

    @property
    def entries(self):
        """Function starts something refers to: call/jmp targets and pointers.

        Everything else that ``.pdata`` lists is a continuation of the range
        before it.
        """
        if self._entries is None:
            self._entries = set()  # set first: _raw does not recurse
            ent = set()
            for begin in self.funcs:
                for ins in self._raw(begin):
                    if ins.mnemonic in ("call", "jmp") and ins.op_str.startswith(
                        "0x"
                    ):
                        v = _imm(ins.op_str)
                        if v is not None:
                            ent.add(v)
            for name, va, size, d in self.secs:
                if name not in (".data", ".rdata"):
                    continue
                for off in range(0, min(len(d), size) - 8, 8):
                    p = self.static_ptr(va + off)
                    if p in self.funcs:
                        ent.add(p)
            for slot, val in self.slots.items():
                if val in self.funcs:
                    ent.add(val)
            self._entries = ent
        return self._entries

    def rip_target(self, ins):
        m = RIP.search(ins.op_str)
        if not m:
            return None
        v = _imm(m.group(2))
        if v is None:
            return None
        return ins.address + ins.size + (v if m.group(1) == "+" else -v)

    def import_at(self, ins):
        """The import name for ``call/jmp qword ptr [rip+N]``."""
        if ins.mnemonic not in ("call", "jmp") or "qword ptr" not in ins.op_str:
            return None
        return self.iat.get(self.rip_target(ins))

    def imports_of(self, begin):
        return {self.import_at(i) for i in self.dis(begin)} - {None}

    # -- run-time slot assignments --

    @property
    def slots(self):
        """``{slot rva: value rva}`` for ``mov qword ptr [rip+S], reg``.

        Many generated type objects are zero-initialised and filled in by the
        module's init code, so their ``tp_name`` and sequence slots are not in
        ``.data`` at all. This is the only way to read them.
        """
        if self._slots is None:
            out = {}
            for begin in self.funcs:
                lea = {}
                for cur in self.dis(begin):
                    if cur.mnemonic == "lea" and "rip" in cur.op_str:
                        t = self.rip_target(cur)
                        if t is not None:
                            lea[cur.op_str.split(",")[0]] = t
                    elif cur.mnemonic == "mov" and "rip" in cur.op_str:
                        m = SLOT_STORE.match(cur.op_str)
                        if m and m.group(1) in lea:
                            t = self.rip_target(cur)
                            if t is not None:
                                out.setdefault(t, lea[m.group(1)])
            self._slots = out
        return self._slots

    def ptr(self, rva):
        """A pointer slot: the static value, or the one init assigns."""
        return self.static_ptr(rva) or self.slots.get(rva, 0)

    def type_name(self, type_rva):
        nm = self.ptr(type_rva + TP_NAME)
        return self.text(nm, 96) if nm else ""

    # -- generated type objects --

    @property
    def types(self):
        """``{rva: tp_name}`` for every generated ``PyTypeObject``.

        Static objects are found by their ``tp_flags``; the zero-initialised
        ones are found by their ``tp_name`` assignment, which is the only slot
        guaranteed to be written.
        """
        if self._types is None:
            out = {}
            for name, va, size, d in self.secs:
                if name not in (".data", ".rdata"):
                    continue
                for off in range(0, min(len(d), size) - 8, 8):
                    r = va + off
                    if self.u64(r + TP_FLAGS) != TPFLAGS:
                        continue
                    if not 0 < self.u64(r + TP_BASICSIZE) < 0x400:
                        continue
                    nm = self.type_name(r)
                    if nm and IDENT.match(nm.replace(".", "_")):
                        out[r] = nm
            for slot, val in self.slots.items():
                if slot % 8 or slot < TP_NAME or (slot - TP_NAME) in out:
                    continue
                nm = self.text(val, 96)
                if nm and "." in nm and IDENT.match(nm.replace(".", "_")):
                    out[slot - TP_NAME] = nm
            self._types = out
        return self._types

    def new_sites(self, begin):
        """``[(index, type rva, tp_name)]`` for ``_PyObject_New`` in a function."""
        ins = self.dis(begin)
        out = []
        for k, cur in enumerate(ins):
            if self.import_at(cur) != "_PyObject_New":
                continue
            for back in range(k - 1, max(-1, k - 8), -1):
                p = ins[back]
                if p.mnemonic == "lea" and "rip" in p.op_str:
                    t = self.rip_target(p)
                    if t is not None and t in self.types:
                        out.append((k, t, self.types[t]))
                    break
        return out

    @property
    def wrappers(self):
        """``{function: (type rva, tp_name)}`` for record-object factories.

        A factory calls ``_PyObject_New(T)`` and parks a pointer in
        ``[obj+0x10]``. Most loaders split the work in two — one function
        multiplies the index by the stride, a one-liner wraps the pointer —
        and a few inline both; both shapes land here.
        """
        if self._wrappers is None:
            out = {}
            for begin, end in self.funcs.items():
                if end - begin > 16384:
                    continue
                ins = self.dis(begin)
                for k, trva, tname in self.new_sites(begin):
                    for j in range(k, min(k + 7, len(ins))):
                        if ins[j].mnemonic == "mov" and STORE_DATA.match(
                            ins[j].op_str
                        ):
                            out.setdefault(begin, (trva, tname))
                            break
            self._wrappers = out
        return self._wrappers

    def thin_wrapper(self, begin):
        """True when a factory merely forwards its first argument.

        Most loaders split the row factory: a caller computes
        ``bucket + index*stride + delta`` and a one-liner turns that pointer
        into an object with ``mov rbx, rcx`` / ``_PyObject_New`` /
        ``mov [rax+0x10], rbx``. Such a function knows nothing about the delta,
        so it must not be asked; its call sites must be.
        """
        ins = self.dis(begin)
        for k, cur in enumerate(ins):
            m = STORE_DATA.match(cur.op_str) if cur.mnemonic == "mov" else None
            if not m:
                continue
            arg = m.group(1)
            for back in range(k - 1, -1, -1):
                p = ins[back]
                if not p.op_str.startswith(arg + ","):
                    continue
                return p.mnemonic == "mov" and p.op_str == arg + ", rcx"
        return False

    def structseq_fields(self, desc_rva):
        """Member names of a ``PyStructSequence_Desc``, or ``None``.

        ``{char *name, char *doc, PyStructSequence_Field *fields,
        int n_in_sequence}``; each field is ``{char *name, char *doc}``.
        """
        fields = self.ptr(desc_rva + 16)
        n = self.u64(desc_rva + 24) & 0xFFFFFFFF
        if not fields or not 0 < n <= 16:
            return None
        out = []
        for i in range(n):
            nm = self.ptr(fields + 16 * i)
            s = self.text(nm, 32) if nm else ""
            if not s or not IDENT.match(s):
                return None
            out.append(s)
        return tuple(out)

    def getattro_funcs(self):
        """Every function that is a ``tp_getattro`` attribute chain."""
        out = []
        for begin in sorted(self.funcs):
            for cur in self.dis(begin):
                if cur.mnemonic != "lea" or "rip" not in cur.op_str:
                    continue
                t = self.rip_target(cur)
                if t is not None and self.read(t, len(ATTR_ERR)) == ATTR_ERR:
                    out.append(begin)
                    break
        return out


# --- parsing one attribute chain -------------------------------------------


class Field:
    __slots__ = ("name", "offset", "type", "presence", "how", "type_rva")

    def __init__(self, name, offset, type_, presence, how, type_rva=None):
        self.name = name
        self.offset = offset
        self.type = type_
        self.presence = presence
        self.how = how
        #: for a container field, the ``PyTypeObject`` the loader instantiates
        self.type_rva = type_rva


def _base_reg(mod, begin):
    for ins in mod.dis(begin)[:28]:
        if ins.mnemonic == "mov" and SELF_DATA.match(ins.op_str):
            return ins.op_str.split(",")[0]
    return None


def _base_mem_indexed(ins, base):
    """``(disp, size, scale)`` for a ``[base + idx*scale + disp]`` operand.

    The vector fields are read in a three-iteration loop
    (``movss xmm0, dword ptr [rbx + rdi*4 + 0x74]``), so they are the one case
    where an indexed operand *is* the field access.
    """
    for op in ins.operands:
        if op.type != capstone.x86.X86_OP_MEM:
            continue
        if op.mem.index == 0 or op.mem.segment != 0:
            continue
        if ins.reg_name(op.mem.base) != base:
            continue
        return op.mem.disp, op.size, op.mem.scale
    return None


def _base_mem(ins, base):
    """``(disp, size)`` for a ``[base + disp]`` operand with no index register.

    Rejecting indexed operands matters: the inlined name comparison reads
    ``byte ptr [name_reg + rcx]``, and the register holding the attribute name
    in one loader is the register holding ``self->data`` in the next, so an
    index-blind parser reads the comparison loop as a field access.
    """
    for op in ins.operands:
        if op.type != capstone.x86.X86_OP_MEM:
            continue
        if op.mem.index != 0 or op.mem.segment != 0:
            continue
        if ins.reg_name(op.mem.base) != base:
            continue
        return op.mem.disp, op.size
    return None


def _scalar_type(ins, size, box):
    named = BOXES.get(box)
    if ins.mnemonic == "movsd":
        return "f64"
    if ins.mnemonic == "movss":
        return "f32"
    if named:
        return named
    if ins.mnemonic == "movzx":
        return {1: "u8", 2: "u16"}.get(size, "unknown")
    if ins.mnemonic in ("movsx", "movsxd"):
        return {1: "i8", 2: "i16", 4: "i32"}.get(size, "unknown")
    if ins.mnemonic == "mov":
        return {1: "u8", 2: "u16", 4: "i32", 8: "i64"}.get(size, "unknown")
    return "unknown"


def _container_type(tname, nested):
    if tname == "cfsd.list":
        return "list"
    if tname == "cfsd.dict":
        return "dict"
    if tname == "cfsd.dict?":
        return "dict"
    if tname:
        nested.add(tname)
        return "struct:" + tname.split(".")[-1]
    return "container"


def parse_chain(mod, begin, nested=None):
    """Parse one ``tp_getattro`` into fields, in declaration order.

    Offsets are relative to whatever the type's instances carry in
    ``[self+0x10]``: for a row type that is ``record + delta``; for a nested
    struct it is the struct itself.
    """
    nested = nested if nested is not None else set()
    ins = mod.dis(begin)
    base = _base_reg(mod, begin)
    if not ins or base is None:
        return []
    type_by_func = dict(mod.wrappers)

    anchors = []
    for i, cur in enumerate(ins):
        if cur.mnemonic != "lea" or "rip" not in cur.op_str:
            continue
        t = mod.rip_target(cur)
        if t is None or mod.section_of(t) not in (".rdata", ".data"):
            continue
        s = mod.text(t, 80)
        if not s or not IDENT.match(s) or s in NOT_ATTRS:
            continue
        # Confirm it is a name being compared: either a call to the shared
        # string compare, or the inlined loop's `cmp reg, len+1`.
        for j in range(i + 1, min(i + 16, len(ins))):
            nx = ins[j]
            if nx.mnemonic == "call" and nx.op_str.startswith("0x"):
                anchors.append((i, s))
                break
            if nx.mnemonic == "cmp" and nx.op_str.endswith(", %d" % (len(s) + 1)):
                anchors.append((i, s))
                break
    if not anchors:
        return []

    presence_words = set()
    out = []
    for a, (i, name) in enumerate(anchors):
        stop = anchors[a + 1][0] if a + 1 < len(anchors) else len(ins)
        span = ins[i:stop]
        presence = None
        mems = []
        leas = []
        reg_copy = False
        box = None
        newtype = None
        tail = None
        structseq = None
        idx_mems = []
        bound = None
        for k, cur in enumerate(span):
            mem = _base_mem(cur, base)
            if cur.mnemonic == "test" and mem:
                imm = None
                for op in cur.operands:
                    if op.type == capstone.x86.X86_OP_IMM:
                        imm = op.imm
                if imm is not None:
                    presence = (mem[0], imm)
                    presence_words.add(mem[0])
                    continue
            if mem and cur.mnemonic in (
                "mov",
                "movzx",
                "movsx",
                "movsxd",
                "movsd",
                "movss",
            ):
                mems.append((cur, mem[0], mem[1]))
            elif mem and cur.mnemonic == "lea":
                leas.append(mem[0])
            elif cur.mnemonic == "mov" and cur.op_str.endswith(", " + base):
                reg_copy = True
            if mem is None and cur.mnemonic in ("movss", "movsd", "mov", "movzx"):
                im = _base_mem_indexed(cur, base)
                if im:
                    idx_mems.append((cur, im[0], im[1], im[2]))
            if cur.mnemonic == "cmp" and "," in cur.op_str:
                v = _imm(cur.op_str.rsplit(",", 1)[1])
                if v is not None and 1 < v <= 16:
                    bound = v
            imp = mod.import_at(cur)
            if imp in BOXES:
                box = imp
            elif imp == "PyStructSequence_New":
                structseq = True
                for back in range(k - 1, max(-1, k - 10), -1):
                    p = span[back]
                    if (
                        p.mnemonic == "lea"
                        and "rip" in p.op_str
                        and p.op_str.startswith("rdx,")
                    ):
                        t = mod.rip_target(p)
                        if t:
                            structseq = mod.structseq_fields(t) or True
                        break
            elif imp == "_PyObject_New":
                for back in range(k - 1, max(-1, k - 8), -1):
                    p = span[back]
                    if p.mnemonic == "lea" and "rip" in p.op_str:
                        t = mod.rip_target(p)
                        if t is not None and t in mod.types:
                            newtype = (t, mod.types[t])
                        break
            if cur.mnemonic in ("call", "jmp") and cur.op_str.startswith("0x"):
                t = _imm(cur.op_str)
                if t in type_by_func:
                    tail = type_by_func[t]
        mems = [m for m in mems if m[1] not in presence_words]
        leas = [d for d in leas if d not in presence_words]

        # A struct sequence is checked before the scalar box because its
        # loop boxes each slot with PyFloat_FromDouble too.
        if structseq:
            # A struct sequence is read in a counted loop over consecutive
            # slots, so its field access is the one indexed operand this
            # parser otherwise rejects, and the loop bound is its arity.
            if idx_mems:
                cur, disp, size, _scale = idx_mems[0]
                n = bound or len(structseq) if isinstance(structseq, tuple) else bound
                elem = _scalar_type(cur, size, "PyFloat_FromDouble")
                ftype = "vec%s<%s>" % (n or "?", elem)
                how = "getattro chain, PyStructSequence_New over a %s-slot loop" % (
                    n or "?"
                )
                if isinstance(structseq, tuple):
                    how += "; members %s" % ",".join(structseq)
                out.append(Field(name, disp, ftype, presence, how))
                continue
            if mems or leas:
                out.append(
                    Field(
                        name,
                        mems[0][1] if mems else leas[0],
                        "structseq",
                        presence,
                        "getattro chain, PyStructSequence_New; contents unknown",
                    )
                )
            continue
        if box:
            if not mems:
                continue
            cur, disp, size = mems[-1]
            ftype = _scalar_type(cur, size, box)
            how = "getattro chain, %s" % box
            if ftype == "u8":
                how += "; 1-byte field, probably bool"
            if len(mems) > 1 and mems[0][1] != disp:
                how += "; 4-byte tag at +%d of unknown meaning" % mems[0][1]
            out.append(Field(name, disp, ftype, presence, how))
            continue
        ctype = newtype or tail
        if ctype is not None:
            trva, tname = ctype
            if leas:
                disp = leas[-1]
            elif mems:
                disp = mems[-1][1]
            elif reg_copy:
                disp = 0
            else:
                continue
            out.append(
                Field(
                    name,
                    disp,
                    _container_type(tname, nested),
                    presence,
                    "getattro chain, _PyObject_New(%s)" % tname,
                    trva,
                )
            )
            continue
        if mems or leas:
            out.append(
                Field(
                    name,
                    mems[-1][1] if mems else leas[-1],
                    "unknown",
                    presence,
                    "getattro chain; base-relative access with no recognised box",
                )
            )
    return out


def list_element(mod, list_type_rva, nested=None):
    """Element type of a ``cfsd.list`` type object, from its ``sq_item``.

    ``sq_item`` is the generated indexer: it multiplies by the element stride
    and then either boxes a scalar or builds an object, exactly like the row
    factory. When it builds a nested record, that record's own attribute chain
    is parsed too, so a ``list<struct:x>`` field comes with ``x``'s layout.
    """
    seq = mod.ptr(list_type_rva + TP_AS_SEQUENCE)
    item = mod.ptr(seq + SQ_ITEM) if seq else 0
    if not item or item not in mod.funcs:
        return "unknown"
    got = _first_wrapper(mod, item)
    if got is not None:
        nm = got[1]
        if nm == "cfsd.list":
            return "list"
        if nm == "cfsd.dict":
            return "dict"
        if nm:
            if nested is not None:
                nested.add(nm)
            return "struct:" + nm.split(".")[-1]
    boxes = {b for b in mod.imports_of(item) if b in BOXES}
    if len(boxes) == 1:
        return BOXES[boxes.pop()] or "i32"
    return "unknown"


def dict_types(mod, dict_type_rva, nested=None):
    """``(key type, value type)`` for a ``cfsd.dict`` type, from ``mp_subscript``.

    The key type is read from how the subscript coerces its argument
    (``PyString_AsString`` for a string key, ``PyInt_AsSsize_t`` /
    ``PyLong_AsSsize_t`` for an integer one), and the value type the same way a
    list's element type is: whatever object the lookup builds. This is what
    opens `dungeons`' ``rooms`` and the room records' ``objects``, which are
    dicts rather than lists.
    """
    mp = mod.ptr(dict_type_rva + TP_AS_MAPPING)
    sub = mod.ptr(mp + MP_SUBSCRIPT) if mp else 0
    if not sub or sub not in mod.funcs:
        return "unknown", "unknown"
    imps = mod.imports_of(sub)
    if "PyString_AsString" in imps:
        kt = "str"
    elif "PyInt_AsSsize_t" in imps or "PyLong_AsSsize_t" in imps:
        kt = "i32"
    else:
        kt = "unknown"
    vt = "unknown"
    got = _first_wrapper(mod, sub)
    if got is not None:
        nm = got[1]
        if nm == "cfsd.list":
            vt = "list"
        elif nm == "cfsd.dict":
            vt = "dict"
        elif nm:
            if nested is not None:
                nested.add(nm)
            vt = "struct:" + nm.split(".")[-1]
    if vt == "unknown":
        boxes = {b for b in imps if b in BOXES}
        if len(boxes) == 1:
            vt = BOXES[boxes.pop()] or "i32"
    return kt, vt


# --- the row type, the stride and the delta --------------------------------


def top_dict(mod):
    """The ``cfsd.dict`` type the module-level ``load()`` returns.

    A table with dict-valued fields has several ``cfsd.dict`` type objects, so
    picking one by name is wrong. ``load`` is the function that opens the
    ``.fsdbinary`` file — the only one importing ``fopen_s`` — and the dict it
    builds is the table itself.
    """
    for begin in sorted(mod.funcs):
        imps = mod.imports_of(begin)
        if "fopen_s" not in imps and "fread_s" not in imps:
            continue
        for _k, trva, tname in mod.new_sites(begin):
            if tname == "cfsd.dict":
                return trva
    return 0


def _first_wrapper(mod, root):
    """The first object factory reachable from ``root`` over direct calls."""
    depth = {root: 0}
    queue = [root]
    seen = set()
    while queue:
        f = queue.pop(0)
        if f in seen or f not in mod.funcs or depth[f] > 4:
            continue
        seen.add(f)
        if f in mod.wrappers:
            return mod.wrappers[f]
        for cur in mod.dis(f):
            if cur.mnemonic in ("call", "jmp") and cur.op_str.startswith("0x"):
                t = _imm(cur.op_str)
                if t in mod.funcs and t not in depth:
                    depth[t] = depth[f] + 1
                    queue.append(t)
    return None


def find_row(mod):
    """``(rowtype rva, tp_name, delta, code_stride)``, or ``None``.

    Starts at ``cfsd.dict``'s ``mp_subscript`` — the path ``loader[key]`` takes
    — and walks direct calls to the first object factory. When the value is
    itself a container (``effectsequences`` maps a key to a *list* of records)
    the walk continues through that container type's ``sq_item`` or
    ``mp_subscript``, so the record is whatever the table ultimately holds and
    never a nested struct that some other field happens to build.
    """
    rowtype = None
    td = top_dict(mod)
    cur_type = td
    for _hop in range(4):
        if not cur_type:
            break
        if mod.types.get(cur_type) == "cfsd.list":
            seq = mod.ptr(cur_type + TP_AS_SEQUENCE)
            root = mod.ptr(seq + SQ_ITEM) if seq else 0
        else:
            mp = mod.ptr(cur_type + TP_AS_MAPPING)
            root = mod.ptr(mp + MP_SUBSCRIPT) if mp else 0
        if not root:
            break
        got = _first_wrapper(mod, root)
        if got is None:
            break
        if not got[1].startswith("cfsd."):
            rowtype = got
            break
        cur_type = got[0]
    if rowtype is None:
        # Fall back to the non-container type with the longest attribute chain.
        best = None
        for _f, (trva, tname) in mod.wrappers.items():
            if tname.startswith("cfsd."):
                continue
            g = mod.ptr(trva + TP_GETATTRO)
            n = len(parse_chain(mod, g)) if g in mod.funcs else 0
            if best is None or n > best[0]:
                best = (n, trva, tname)
        if best is None or best[0] == 0:
            return None
        rowtype = (best[1], best[2])

    wrapper_funcs = {f for f, v in mod.wrappers.items() if v[0] == rowtype[0]}
    thin = {f for f in wrapper_funcs if mod.thin_wrapper(f)}
    fat = wrapper_funcs - thin
    deltas = []
    strides = []
    for begin, end in mod.funcs.items():
        if end - begin > 16384:
            continue
        ins = mod.dis(begin)
        stride = None
        for cur in ins:
            if cur.mnemonic == "imul" and cur.op_str.count(",") == 2:
                v = _imm(cur.op_str.rsplit(",", 1)[1])
                if v and v % 4 == 0 and v < (1 << 16):
                    stride = v
        for k, cur in enumerate(ins):
            arg = None
            # Only two sites know the delta: a factory that computes the
            # record address itself (so it must carry the stride), and a call
            # into a thin wrapper, which is handed the offset pointer. A thin
            # wrapper's own `mov rbx, rcx` says nothing and must not vote.
            if cur.mnemonic == "mov" and begin in fat:
                m = STORE_DATA.match(cur.op_str)
                if m:
                    arg = m.group(1)
            elif cur.mnemonic in ("call", "jmp") and cur.op_str.startswith("0x"):
                if _imm(cur.op_str) in thin:
                    arg = "rcx"
            if arg is None:
                continue
            # Walk back to whatever last wrote the register being handed over.
            # `lea arg, [x + D]` makes the delta explicit; any other write
            # (`add`, `imul`, a plain `mov`) means the pointer is the record
            # start and the delta is zero, which is the shape list-element
            # records have, since they carry no primary key to skip.
            for back in range(k - 1, max(-1, k - 13), -1):
                p = ins[back]
                if not p.op_str.startswith(arg + ","):
                    continue
                if p.mnemonic == "lea":
                    m = DISP.search(p.op_str)
                    deltas.append(_imm(m.group(1)) if m else 0)
                elif p.mnemonic in ("mov", "add", "imul", "sub", "movsxd", "or"):
                    deltas.append(0)
                else:
                    continue
                break
            if stride:
                strides.append(stride)
    delta = max(set(deltas), key=deltas.count) if deltas else None
    code_stride = max(set(strides), key=strides.count) if strides else None
    return rowtype[0], rowtype[1], delta, code_stride


# --- a table's whole schema ------------------------------------------------


class TableSchema:
    def __init__(self, stem):
        self.stem = stem
        self.row_type = None
        self.delta = None
        self.code_stride = None
        self.stride = None
        self.fields = []
        self.structs = {}
        self.status = "ok"
        self.note = ""
        self.struct_sizes = {}


def _width(f, sizes):
    base = f.type.split("<")[0]
    if base == "dict":
        return 16
    if base.startswith("vec"):
        try:
            n = int(base[3:])
        except ValueError:
            return 8
        return n * (8 if f.type.endswith("<f64>") else 4)
    if base.startswith("struct:"):
        return sizes.get(base[len("struct:") :], 8)
    return WIDTH.get(base, 8)


def _span(fields, sizes):
    end = 0
    for f in fields:
        end = max(end, f.offset + _width(f, sizes))
        if f.presence:
            end = max(end, f.presence[0] + 4)
    return end


def _align(fields, sizes):
    """The record's alignment: the widest *scalar* in it, floor 4.

    A dict field is two pointers and a nested struct can be any size, but
    neither raises the record's alignment above 8 — capping here is what makes
    the computed stride agree with the ``imul`` immediate on every table that
    states one.
    """
    a = 4  # the primary key is a uint32, so 4 is the floor
    for f in fields:
        w = _width(f, sizes)
        if f.type.startswith("vec"):
            w = 8 if f.type.endswith("<f64>") else 4
        a = max(a, min(w, 8))
    return a


def _resolve_lists(mod, fields, nested=None):
    """Name each container field's element type from *its own* type object.

    A table with several list-valued fields has several ``cfsd.list`` type
    objects, one per element type, all sharing the one pooled ``tp_name``
    string. Resolving by name would give every list the first one's element
    type, so each field carries the type object the loader actually hands to
    ``_PyObject_New``.
    """
    for f in fields:
        if f.type == "list":
            f.type = "list<%s>" % (
                list_element(mod, f.type_rva, nested) if f.type_rva else "unknown"
            )
        elif f.type == "dict":
            kt, vt = (
                dict_types(mod, f.type_rva, nested)
                if f.type_rva
                else ("unknown", "unknown")
            )
            f.type = "dict<%s,%s>" % (kt, vt)


def schema_for(path, stem):
    out = TableSchema(stem)
    mod = Module(path)
    row = find_row(mod)
    if row is None:
        if not mod.getattro_funcs():
            out.status = "no-attributes"
            out.note = "no tp_getattro in the module: the row is a scalar or a list"
        else:
            out.status = "no-row-type"
            out.note = "attribute chains present but no row factory was identified"
        return out
    rowtype, out.row_type, out.delta, out.code_stride = row
    nested = set()
    g = mod.ptr(rowtype + TP_GETATTRO)
    if g in mod.funcs:
        out.fields = parse_chain(mod, g, nested)
    if not out.fields:
        best = []
        for f in mod.getattro_funcs():
            cand = parse_chain(mod, f, nested)
            if len(cand) > len(best):
                best = cand
        out.fields = best
        if best:
            out.note = "chain located by function scan, not via the tp_getattro slot"
    by_name = {}
    for rva, nm in mod.types.items():
        by_name.setdefault(nm, rva)
    _resolve_lists(mod, out.fields, nested)
    done = set()
    while nested - done:
        tname = sorted(nested - done)[0]
        done.add(tname)
        trva = by_name.get(tname)
        if trva is None:
            out.structs[tname] = []
            continue
        g2 = mod.ptr(trva + TP_GETATTRO)
        fl = parse_chain(mod, g2, nested) if g2 in mod.funcs else []
        _resolve_lists(mod, fl, nested)
        out.structs[tname] = fl
    # Nested struct sizes, innermost first, so a struct-valued field in the
    # row record contributes its real width to the row's span.
    sizes = {}
    for _round in range(4):
        for tname, fl in out.structs.items():
            if not fl:
                continue
            short = tname.split(".")[-1]
            sizes[short] = _align_up(_span(fl, sizes), _align(fl, sizes))
    out.struct_sizes = sizes
    if out.fields and out.delta is not None:
        out.stride = _align_up(
            out.delta + _span(out.fields, sizes), _align(out.fields, sizes)
        )
    return out


# --- client discovery ------------------------------------------------------


def find_bin64(arg):
    cands = []
    if arg:
        cands += [os.path.join(arg, "bin64"), arg]
    cands += [
        "/client/tq/bin64",
        "/client/bin64",
        os.path.join(os.environ.get("EVE_CLIENT", ""), "bin64"),
        os.environ.get("EVE_CLIENT", ""),
    ]
    for c in cands:
        if (
            c
            and os.path.isdir(c)
            and any(n.lower().endswith("loader.pyd") for n in os.listdir(c))
        ):
            return c
    return None


def loaders(bin64):
    out = {}
    for n in sorted(os.listdir(bin64)):
        if n.lower().endswith("loader.pyd"):
            out[n[: -len("Loader.pyd")].lower()] = os.path.join(bin64, n)
    return out


# --- output ----------------------------------------------------------------


def _opt(f, delta):
    """The machine-readable optional marker appended to ``how_derived``.

    Strict format, so a consumer can pick it up with one regex:
    ``optional(bit=0x40,word=+112)``. ``word`` is the record-relative offset
    of the presence bitfield; the field is present when
    ``u32_at(record + word) & bit``.
    """
    if not f.presence:
        return ""
    return "; optional(bit=%s,word=+%d)" % (hex(f.presence[1]), f.presence[0] + delta)


def select_tables(schemas, names):
    """The schemas whose stem is in ``names`` (comma list or iterable, case and
    `.fsdbinary` ignored); all when empty.

    Raises ValueError naming any wanted table that has no loader: an overlay
    file silently missing a table would be read as the base layout.
    """
    if isinstance(names, str):
        names = names.split(",")
    want = [n.strip().lower().replace(".fsdbinary", "") for n in names if n.strip()]
    if not want:
        return list(schemas)
    have = {s.stem for s in schemas}
    missing = [n for n in want if n not in have]
    if missing:
        raise ValueError("no loader for: %s" % ", ".join(missing))
    return [s for s in schemas if s.stem in want]


def emit_tsv(schemas, fh):
    fh.write("table\tfield\toffset\ttype\tconfidence\thow_derived\n")
    for s in sorted(schemas, key=lambda x: x.stem):
        if s.status != "ok" or not s.fields:
            fh.write(
                "%s\t-\t-\t-\tV\t%s: %s\n"
                % (s.stem, s.status, s.note or "see the tools/pyd_schema.py docstring")
            )
            continue
        if s.code_stride == s.stride:
            agree = "stride confirmed by the imul immediate in the row factory"
        elif s.code_stride:
            agree = "DISAGREEMENT: imul immediate says %s" % s.code_stride
        else:
            agree = "stride computed from the layout; not an immediate in this loader"
        fh.write(
            "%s\t#record\t-\tstride=%s delta=%s\tV\t"
            "every offset in this table's rows is RECORD-relative and already "
            "includes delta -- do not add it again; delta is the lea immediate "
            "in the row factory and is published only so a reader can "
            "reconstruct the struct frame; %s\n"
            % (s.stem, s.stride, s.delta, agree)
        )
        d = s.delta or 0
        for f in s.fields:
            fh.write(
                "%s\t%s\t%d\t%s\t%s\t%s%s\n"
                % (
                    s.stem,
                    f.name,
                    f.offset + d,
                    f.type,
                    "V" if f.type != "unknown" else "I",
                    f.how,
                    _opt(f, d),
                )
            )
        for tname, fl in sorted(s.structs.items()):
            short = tname.split(".")[-1]
            for f in fl:
                fh.write(
                    "%s\tstruct:%s/%s\t%d\t%s\t%s\t%s (struct-relative)%s\n"
                    % (
                        s.stem,
                        short,
                        f.name,
                        f.offset,
                        f.type,
                        "V" if f.type != "unknown" else "I",
                        f.how,
                        _opt(f, 0),
                    )
                )


def print_table(s):
    print(
        "# %s  row_type=%s stride=%s (imul says %s) delta=%s status=%s"
        % (s.stem, s.row_type, s.stride, s.code_stride, s.delta, s.status)
    )
    if s.note:
        print("# %s" % s.note)
    d = s.delta or 0
    for f in s.fields:
        print("  +%-5d %-26s %-22s%s" % (f.offset + d, f.name, f.type, _opt(f, d)))
    for tname, fl in sorted(s.structs.items()):
        print("  -- nested %s (offsets struct-relative)" % tname)
        for f in fl:
            print("    +%-5d %-26s %-22s%s" % (f.offset, f.name, f.type, _opt(f, 0)))


def main(argv=None):
    ap = argparse.ArgumentParser(description="cFSD schemas out of the native loaders")
    ap.add_argument("command", choices=["table", "all", "tsv", "check"])
    ap.add_argument("name", nargs="?", help="table stem, for `table`")
    ap.add_argument("--client", help="client install directory, or its bin64")
    ap.add_argument("--resfiles", help="accepted and ignored; for tool symmetry")
    ap.add_argument("--out", help="write the TSV here instead of stdout")
    ap.add_argument("--tables", help="`tsv` only: comma list of table stems; the "
                    "rest are left out (an overlay file for a build, "
                    "findings/pyd-schemas-<build>.tsv)")
    a = ap.parse_args(argv)

    bin64 = find_bin64(a.client)
    if not bin64:
        sys.exit("pyd_schema: no bin64 with *Loader.pyd found; pass --client")
    pyds = loaders(bin64)

    if a.command == "table":
        if not a.name:
            sys.exit("pyd_schema: `table` needs a table name")
        key = a.name.lower().replace(".fsdbinary", "")
        if key not in pyds:
            sys.exit("pyd_schema: no loader for %r in %s" % (a.name, bin64))
        print_table(schema_for(pyds[key], key))
        return 0

    schemas = []
    for stem, path in pyds.items():
        try:
            schemas.append(schema_for(path, stem))
        except Exception as exc:  # a loader we cannot parse is itself a finding
            s = TableSchema(stem)
            s.status = "error"
            s.note = "%s: %s" % (type(exc).__name__, exc)
            schemas.append(s)

    if a.command == "all":
        for s in schemas:
            print_table(s)
            print()
        return 0
    if a.command == "check":
        full = [s for s in schemas if s.status == "ok" and s.fields]
        print("loaders                  %d" % len(schemas))
        print("with a field layout      %d" % len(full))
        print("fields                   %d" % sum(len(s.fields) for s in full))
        print(
            "fields typed `unknown`   %d"
            % sum(1 for s in full for f in s.fields if f.type == "unknown")
        )
        print("nested struct types      %d" % sum(len(s.structs) for s in full))
        print("--- not fully recovered:")
        for s in schemas:
            if s.status != "ok" or not s.fields:
                print("  %-48s %-14s %s" % (s.stem, s.status, s.note))
        print("--- stride disagreements (imul immediate vs recovered layout):")
        for s in full:
            if s.code_stride and s.code_stride != s.stride:
                print("  %-48s imul=%s layout=%s" % (s.stem, s.code_stride, s.stride))
        return 0
    try:
        schemas = select_tables(schemas, a.tables or "")
    except ValueError as exc:
        sys.exit("pyd_schema: %s" % exc)
    fh = open(a.out, "w") if a.out else sys.stdout
    try:
        emit_tsv(schemas, fh)
    finally:
        if a.out:
            fh.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
