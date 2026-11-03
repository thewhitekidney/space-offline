"""Reader for the EVE client's static-data corpus (``res:/staticdata/``), V24.01.

Pure Python 3, standard library only, read-only. Nothing here writes to the
client installation and nothing here needs the client to run.

House style: every claim below is marked **[V]** (verified: read out of the
client's own bytecode, or reproduced by decoding a real file) or **[I]**
(inferred). ``unknown`` is used where neither applies.


Where the files are
-------------------

**[V]** ``$EVE_CLIENT/resfileindex.txt`` is a 5-column CSV::

    res:/staticdata/types.fsdbinary,3c/3cc5bf8ff5e9099a_c0ee...,<md5>,<size>,<csize>

Column 2 is a path relative to ``$EVE_CLIENT/../ResFiles/`` — the cache
directory is a *sibling* of the ``tq`` install directory, not inside it.
**[V]** All 203 ``res:/staticdata/*`` entries resolve, exist, and are stored
uncompressed (on-disk size == column 4 for all 203), so no inflate step is
needed for this corpus.


The three formats
-----------------

**[V]** By extension, of the 203 files: 149 ``.fsdbinary``, 45 ``.static``,
3 ``.pickle``, 6 ``.schema``.

``.pickle`` — **[V]** a plain Python pickle (protocol 2), nothing else.

``.schema`` — **[V]** YAML source schema, human-readable. Ships beside six
``.static`` tables (systems, regions, constellations, jumps,
factionsowningsolarsystems, dialogs).

``.static`` — **[V]** the *classic* FSD binary container, and the client reads
it with pure Python we already have::

    out/pyc/fsd/schemas/binaryLoader.pyc        container + entry points
    out/pyc/fsd/schemas/loaders/__init__.pyc    scalars, vectors, enum, union
    out/pyc/fsd/schemas/loaders/listLoader.pyc  list
    out/pyc/fsd/schemas/loaders/objectLoader.pyc        object
    out/pyc/fsd/schemas/loaders/dictLoader.pyc  dict / index / multi-index
    out/pyc/fsd/schemas/predefinedStructTypes.pyc       struct formats

  This module is a port of exactly those five files. See `Classic format`.

``.fsdbinary`` — **[V]** a second, undocumented container, **not** the classic
one, and not readable by the pure-Python loader. Its *container* is decoded
here (see `CfsdTable`): a 32-byte prologue, then a hash map of fixed-size
records keyed by ``key % bucketCount``, then per-bucket variable-length data.
The per-table **field layout** inside a record lives in generated C++, one
``bin64/<table>Loader.pyd`` per table, described nowhere in the bytecode;
`tools/pyd_schema.py` reads it out of those DLLs into
``findings/pyd-schemas.tsv``, and **`rows(client, stem)` decodes any table
that file describes** -- 140 of the 149, nested containers included
(packet K1, `docs/lanes/fsd-generic.md`). `CFSD_COLUMNS` and
`tools/dogma.py`'s ``SCHEMAS`` are the older hand-confirmed layouts, kept as
cross-checks: ``python3 -I tools/fsd.py generic-check <client>``.

**[V]** *not* the classic container, and *not* readable by
the Python above. It is what the client's own log messages call a **cFSD**
file (``fsdBuiltData/common/base.pyc``, ``__LoadBinaryFromResFolder``:
"Trying to load cFSD file at %s" / "Trying to load regular FSD file at %s" —
the second branch, the one that calls ``binaryLoader.LoadFSDDataForCFG``, is
the fallback taken only when the table's native loader is absent).
**[V]** Each cFSD table has its own generated C extension in
``$EVE_CLIENT/bin64/<table>Loader.pyd`` (e.g. ``typesLoader.pyd``,
``accountingEntryTypesLoader.pyd``, ``dungeonsLoader.pyd``) which the Python
wrapper class assigns to ``__loader__``. So the cFSD *decoder* is native.
See `docs/re/static-corpus.md` for what has and has not been established
about that format; this module exposes the header fields it can read and
the attribute-name list recovered from the matching ``.pyd``, and nothing
more.


Classic format (``.static``)
----------------------------

**[V]** ``binaryLoader.LoadFromString``::

    uint32 schemaSize
    bytes[schemaSize]   a Python 2 cPickle of the *optimized* schema dict
    bytes[...]          the data, parsed with that schema

Offsets inside the data are relative to the start of the *node* being parsed,
and the top-level node starts at ``4 + schemaSize``.

**[V]** The optimized schema is a plain nest of dicts. ``type`` selects a
loader; ``binaryLoader.defaultLoaderFactories`` maps 30-odd type names onto
only eight distinct loaders, because ``typeID``, ``localizationID``,
``dungeonID``, ``npcCorporationID``, ``factionID``, ``marketGroupID``,
``spawnTableID``, ``npcFleetTypeID``, ``groupBehaviorTreeID`` and friends are
all just ``int`` with a name that tells you what it references. That naming is
itself the most useful thing in the schema: it is a foreign-key map.

Node layouts, all **[V]** from the loaders listed above:

``int``
    uint32 if ``min >= 0`` or ``exclusiveMin >= -1``, else int32.
``float``
    float64 if ``precision == 'double'`` else float32.
``bool``
    one byte; true iff the byte is 255.
``string`` / ``resPath``
    uint32 length, then that many bytes. ``unicode`` is the same, UTF-8.
``enum``
    uint8/uint16/uint32 depending on ``maxEnumValue`` (<=255, <=65536, else);
    returns the *name* from ``values`` unless ``readEnumValue`` is set.
``vector2`` / ``vector3`` / ``vector4`` / ``color``
    2/3/4 packed floats (doubles if ``precision == 'double'``); ``aliases``
    maps names such as ``x`` onto indices.
``union``
    uint32 index into ``optionTypes``, then the chosen node at +4.
``list``
    ``length`` in the schema means a fixed count and no count in the data;
    otherwise a uint32 count comes first. With ``fixedItemSize`` the items
    are ``itemTypes['size']`` bytes apart; without it, the count is followed
    by one uint32 offset per item, each relative to the list's own start.
``object``
    ``size`` present  -> every attribute is at a constant offset.
    ``size`` absent   -> constant-offset attributes first (occupying
    ``endOfFixedSizeData`` bytes), then a uint64 presence bitfield, then one
    uint32 per *present* variable attribute, then the variable data. An
    attribute is present when it is not in ``optionalValueLookups`` or its
    mask bit is set in the bitfield. A missing attribute falls back to
    ``default`` if the schema has one, and is otherwise absent.
``dict``
    uint32 ``sizeOfData``; then the values; then the footer; then a uint32
    ``sizeOfFooter`` at ``offset + sizeOfData``. The footer begins at
    ``offset + sizeOfData - sizeOfFooter``. With an ``int`` key type the
    footer is a uint32 count followed by packed ``(int32 key, int32 offset)``
    or ``(int32 key, int32 offset, int32 size)`` records, sorted by key —
    ``size`` is present iff ``keyFooter.itemTypes.attributes`` has a ``size``
    attribute. With any other key type the footer is an ordinary ``list`` of
    objects with ``key``/``offset``/``size`` attributes. A value lives at
    ``offset + 4 + <its footer offset>`` and is parsed with ``valueTypes``.

**[V]** ``buildIndex`` on a dict means the client streams it from the file
instead of loading it whole (``LoadIndexFromFile`` / ``IndexLoader``), and
``multiIndex`` means it also carries secondary indices
(``MultiIndexLoader``). Both describe *access strategy*; the bytes are laid
out the same way, which is why this module reads them with one code path.
Nested ``buildIndex`` dicts are the one place that matters: ``IndexLoader``
slices each value out and parses it at offset 0, which is equivalent to
parsing it in place because every node's offsets are node-relative. **[V]**
reproduced: ``solarsystemcontent.static`` is a nested index three levels deep
and decodes identically either way.


Usage
-----

::

    import fsd
    c = fsd.Client()                       # $EVE_CLIENT, or pass a path
    print(len(c.index))                    # every res: path in the client
    t = c.load('res:/staticdata/blueprints.static')
    print(len(t), t.columns)
    print(t[681])                          # a row, as a plain dict

A note on pickles: the embedded schema and the ``.pickle`` tables are real
pickles, i.e. code. They are loaded through `SafeUnpickler`, which refuses
every global, so a crafted file can allocate but cannot execute.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import io
import os
import pickle
import re
import struct
import sys

__all__ = [
    "Client",
    "FsdError",
    "SafeUnpickler",
    "load_classic",
    "load_pickle",
    "read_cfsd_header",
    "cfsd_attribute_names",
    "describe",
    "PydField",
    "PydSchema",
    "CfsdDecoder",
    "CfsdUnknown",
    "load_pyd_schemas",
    "parse_pyd_schemas",
    "cfsd_generic_rows",
    "schema_for",
    "rows",
]

# --- struct formats, from fsd/schemas/predefinedStructTypes.pyc [V] ---------
_U64 = struct.Struct("<Q")
_I64 = struct.Struct("<q")
_U32 = struct.Struct("<I")
_I32 = struct.Struct("<i")
_KEYED = struct.Struct("<ii")
_KEYED_SZ = struct.Struct("<iii")
_F32 = struct.Struct("<f")
_F64 = struct.Struct("<d")
_VEC = {
    ("vector2", False): struct.Struct("<ff"),
    ("vector2", True): struct.Struct("<dd"),
    ("vector3", False): struct.Struct("<fff"),
    ("vector3", True): struct.Struct("<ddd"),
    ("vector4", False): struct.Struct("<ffff"),
    ("vector4", True): struct.Struct("<dddd"),
    ("color", False): struct.Struct("<ffff"),
    ("color", True): struct.Struct("<dddd"),
}

CLASSIC_EXTENSIONS = (".static",)
CFSD_EXTENSIONS = (".fsdbinary",)
PICKLE_EXTENSIONS = (".pickle",)
SCHEMA_EXTENSIONS = (".schema",)


class FsdError(Exception):
    """Anything this reader cannot decode."""


# --- pickle, with no code execution ---------------------------------------


class SafeUnpickler(pickle.Unpickler):
    """An Unpickler that refuses every global.

    The optimized schemas and the ``.pickle`` tables in this corpus are plain
    data: dicts, lists, tuples, strings, ints, floats, bools, None. Anything
    that wants a class or a function is a file we do not understand, and we
    would rather raise than run it.
    """

    #: The only globals allowed. **[V]** the embedded schemas use
    #: ``collections.OrderedDict`` for ``attributes``, because attribute order
    #: is part of the format.
    ALLOWED = {("collections", "OrderedDict")}

    def find_class(self, module, name):  # noqa: D102
        if (module, name) in self.ALLOWED:
            import collections

            return collections.OrderedDict
        raise FsdError(
            "refusing to resolve %s.%s out of a client pickle" % (module, name)
        )


def _unpickle(data):
    # Python 2 wrote these: protocol 0 for the embedded schemas, protocol 2
    # for the .pickle tables. latin-1 is the lossless byte<->str mapping, so
    # schema keys such as 'valueTypes' come back as str and arbitrary bytes
    # survive. [V] reproduced on all 45 .static schemas and all 3 .pickle.
    return SafeUnpickler(io.BytesIO(data), encoding="latin-1").load()


def load_pickle(path):
    """Load a ``.pickle`` static-data file."""
    with open(path, "rb") as fh:
        return _unpickle(fh.read())


# --- the classic container -------------------------------------------------


class _Node:
    """Lazy view of one classic-format node. Subscript it, or `unwrap` it."""

    __slots__ = ("_d", "_o", "_s")

    def __init__(self, data, offset, schema):
        self._d = data
        self._o = offset
        self._s = schema

    @property
    def schema(self):
        return self._s

    def unwrap(self, depth=16):
        """Materialise this node and everything under it as plain Python."""
        return _unwrap(self, depth)


class FsdObject(_Node):
    """An ``object`` node: a row. Behaves like a read-only mapping."""

    __slots__ = ("_const", "_var", "_varbase")

    def __init__(self, data, offset, schema):
        _Node.__init__(self, data, offset, schema)
        self._const = schema["constantAttributeOffsets"]
        self._var = {}
        self._varbase = 0
        if "size" in schema:
            return
        lookups = schema.get("optionalValueLookups") or {}
        if lookups:
            field = _U64.unpack_from(data, offset + schema["endOfFixedSizeData"])[0]
            present = [
                a
                for a in schema["attributesWithVariableOffsets"]
                if a not in lookups or (field & lookups[a])
            ]
        else:
            present = list(schema["attributesWithVariableOffsets"])
        start = offset + schema.get("endOfFixedSizeData", 0) + 8
        self._varbase = start + 4 * len(present)
        for i, name in enumerate(present):
            self._var[name] = _U32.unpack_from(data, start + 4 * i)[0]

    # -- mapping surface ---------------------------------------------------

    @property
    def columns(self):
        """Every attribute the schema declares, sorted."""
        return sorted(self._s["attributes"])

    def keys(self):
        """Only the attributes actually present on this row."""
        out = []
        for name, sub in self._s["attributes"].items():
            if name in self._const or name in self._var or "default" in sub:
                out.append(name)
        return sorted(out)

    def __contains__(self, key):
        try:
            self[key]
        except KeyError:
            return False
        return True

    def __iter__(self):
        return iter(self.keys())

    def __len__(self):
        return len(self.keys())

    def items(self):
        for k in self.keys():
            yield k, self[k]

    def get(self, key, default=None):
        try:
            return self[key]
        except KeyError:
            return default

    def __getitem__(self, key):
        attrs = self._s["attributes"]
        if key not in attrs:
            raise KeyError(key)
        sub = attrs[key]
        if key in self._const:
            return _node(self._d, self._o + self._const[key], sub)
        if key in self._var:
            return _node(self._d, self._varbase + self._var[key], sub)
        if "default" in sub:
            return sub["default"]
        raise KeyError("%s not present on this row" % key)

    def __getattr__(self, name):
        try:
            return self[name]
        except KeyError as exc:
            raise AttributeError(str(exc)) from None

    def __repr__(self):
        return "<FsdObject %s>" % (", ".join(self.keys()),)


class FsdList(_Node):
    """A ``list`` node."""

    __slots__ = ("_count", "_base", "_fixed", "_item")

    def __init__(self, data, offset, schema, known_length=None):
        _Node.__init__(self, data, offset, schema)
        self._item = schema["itemTypes"]
        length = schema.get("length", known_length)
        if length is None:
            self._count = _U32.unpack_from(data, offset)[0]
            self._base = offset + 4
        else:
            self._count = length
            self._base = offset
        self._fixed = "fixedItemSize" in schema

    def __len__(self):
        return self._count

    def __iter__(self):
        for i in range(self._count):
            yield self[i]

    def __getitem__(self, i):
        if i < 0:
            i += self._count
        if not 0 <= i < self._count:
            raise IndexError(i)
        if self._fixed:
            off = self._base + self._item["size"] * i
        else:
            off = self._o + _U32.unpack_from(self._d, self._base + 4 * i)[0]
        return _node(self._d, off, self._item)

    def __repr__(self):
        return "<FsdList n=%d>" % self._count


class FsdDict(_Node):
    """A ``dict`` node: the table itself, or a nested index inside one."""

    __slots__ = ("_footer", "_value", "_keys")

    def __init__(self, data, offset, schema):
        _Node.__init__(self, data, offset, schema)
        self._value = schema["valueTypes"]
        size_of_data = _U32.unpack_from(data, offset)[0]
        size_of_footer = _U32.unpack_from(data, offset + size_of_data)[0]
        f0 = offset + size_of_data - size_of_footer
        self._footer = _read_footer(data, f0, size_of_footer, schema)
        self._keys = None

    # -- mapping surface ---------------------------------------------------

    @property
    def columns(self):
        """The value schema's attributes, when the values are objects."""
        if self._value.get("type") == "object":
            return sorted(self._value["attributes"])
        return None

    @property
    def value_type(self):
        return self._value.get("type")

    def keys(self):
        if self._keys is None:
            self._keys = [k for k, _o, _s in self._footer]
        return self._keys

    def __len__(self):
        return len(self._footer)

    def __iter__(self):
        return iter(self.keys())

    def __contains__(self, key):
        return self._find(key) is not None

    def _find(self, key):
        lo, hi = 0, len(self._footer) - 1
        while lo <= hi:
            mid = (lo + hi) // 2
            k = self._footer[mid][0]
            if k < key:
                lo = mid + 1
            elif k > key:
                hi = mid - 1
            else:
                return self._footer[mid]
        return None

    def __getitem__(self, key):
        hit = self._find(key)
        if hit is None:
            raise KeyError(key)
        return _node(self._d, self._o + 4 + hit[1], self._value)

    def get(self, key, default=None):
        hit = self._find(key)
        if hit is None:
            return default
        return _node(self._d, self._o + 4 + hit[1], self._value)

    def items(self):
        for k, off, _sz in self._footer:
            yield k, _node(self._d, self._o + 4 + off, self._value)

    def values(self):
        for _k, v in self.items():
            yield v

    def __repr__(self):
        return "<FsdDict n=%d of %s>" % (len(self._footer), self._value.get("type"))


class FsdMultiIndex(FsdDict):
    """A ``multiIndex`` dict: a primary index plus named secondary indices.

    **[V]** ``MultiIndexLoader`` (dictLoader.pyc). The primary index is read
    exactly as `FsdDict`; after it come a uint32 ``attributeLookupTableSize``
    and a lookup table parsed with ``subIndexOffsetLookup``, which maps a
    nested-index id onto the (offset, size) of that index's own footer.
    ``indexNameToIds`` names the groups and ``indexableSchemas`` gives each
    one's value schema. `secondary` exposes them by name.
    """

    __slots__ = ("_sub",)

    def __init__(self, data, offset, schema):
        FsdDict.__init__(self, data, offset, schema)
        size_of_data = _U32.unpack_from(data, offset)[0]
        size_of_footer = _U32.unpack_from(data, offset + size_of_data)[0]
        # ...; <lookup table>; <uint32 its size>; <footer>; <uint32 footer size>
        at_size = offset + 4 + size_of_data - 4 - size_of_footer - 4
        lut_size = _U32.unpack_from(data, at_size)[0]
        lut = _node(data, at_size - lut_size, schema["subIndexOffsetLookup"])
        self._sub = {}
        by_id = {}
        for idx, where in lut.items():
            sub_schema = schema["indexableSchemas"][idx]
            by_id[idx] = (
                _read_footer(
                    data, offset + where["offset"], where["size"], sub_schema
                ),
                sub_schema,
            )
        for name, ids in schema["indexNameToIds"].items():
            self._sub[name] = _SubIndex(data, offset, [by_id[i] for i in ids])

    @property
    def secondary(self):
        return self._sub


class _SubIndex:
    """One named secondary index of a `FsdMultiIndex`."""

    def __init__(self, data, offset, parts):
        self._d = data
        self._o = offset
        self._parts = parts

    def __len__(self):
        return sum(len(f) for f, _s in self._parts)

    def keys(self):
        return [k for f, _s in self._parts for k, _o, _sz in f]

    def items(self):
        for footer, schema in self._parts:
            for k, off, _sz in footer:
                yield k, _node(self._d, self._o + 4 + off, schema["valueTypes"])

    def __getitem__(self, key):
        for footer, schema in self._parts:
            for k, off, _sz in footer:
                if k == key:
                    return _node(self._d, self._o + 4 + off, schema["valueTypes"])
        raise KeyError(key)


def _read_footer(data, offset, size, schema):
    """Return ``[(key, offset, size), ...]`` for a dict footer. **[V]**"""
    if schema["keyTypes"]["type"] == "int":
        n = _U32.unpack_from(data, offset)[0]
        has_size = "size" in schema["keyFooter"]["itemTypes"]["attributes"]
        unp = _KEYED_SZ if has_size else _KEYED
        out = []
        at = offset + 4
        for _i in range(n):
            if has_size:
                k, o, sz = unp.unpack_from(data, at)
            else:
                k, o = unp.unpack_from(data, at)
                sz = 0
            out.append((k, o, sz))
            at += unp.size
        return out
    rows = FsdList(data, offset, schema["keyFooter"])
    return [(r["key"], r["offset"], r.get("size", 0)) for r in rows]


def _node(data, offset, schema):
    """Dispatch one schema node. Mirrors ``defaultLoaderFactories``. **[V]**"""
    t = schema.get("type")
    if t in _INT_TYPES:
        if ("min" in schema and schema["min"] >= 0) or (
            "exclusiveMin" in schema and schema["exclusiveMin"] >= -1
        ):
            return _U32.unpack_from(data, offset)[0]
        return _I32.unpack_from(data, offset)[0]
    if t == "object":
        return FsdObject(data, offset, schema)
    if t == "list":
        return FsdList(data, offset, schema)
    if t == "dict":
        if schema.get("multiIndex", False):
            return FsdMultiIndex(data, offset, schema)
        return FsdDict(data, offset, schema)
    if t in ("string", "resPath"):
        n = _U32.unpack_from(data, offset)[0]
        return bytes(data[offset + 4 : offset + 4 + n]).decode("utf-8", "replace")
    if t == "unicode":
        n = _U32.unpack_from(data, offset)[0]
        return bytes(data[offset + 4 : offset + 4 + n]).decode("utf-8")
    if t == "bool":
        return data[offset] == 255
    if t == "float":
        if schema.get("precision", "single") == "double":
            return _F64.unpack_from(data, offset)[0]
        return _F32.unpack_from(data, offset)[0]
    if t == "enum":
        return _enum(data, offset, schema)
    if t == "union":
        i = _U32.unpack_from(data, offset)[0]
        return _node(data, offset + 4, schema["optionTypes"][i])
    if t in ("vector2", "vector3", "vector4", "color"):
        dbl = schema.get("precision", "single") == "double"
        vals = _VEC[t, dbl].unpack_from(data, offset)
        aliases = schema.get("aliases")
        if aliases:
            return {name: vals[i] for name, i in aliases.items()}
        return vals
    raise FsdError("unknown schema node type %r" % (t,))


def _enum(data, offset, schema):
    m = schema["maxEnumValue"]
    if m <= 255:
        v = data[offset]
    elif m <= 65536:
        v = struct.unpack_from("<H", data, offset)[0]
    else:
        v = _U32.unpack_from(data, offset)[0]
    if schema.get("readEnumValue", False):
        return v
    for k, val in schema["values"].items():
        if val == v:
            return k
    return v


# **[V]** binaryLoader.defaultLoaderFactories: all of these are
# IntFromBinaryString. The name is the only thing that differs, and it is a
# foreign-key annotation -- `typeID` points at types, `dungeonID` at dungeons,
# `localizationID` at the localisation corpus, and so on.
_INT_TYPES = frozenset(
    [
        "int",
        "typeID",
        "localizationID",
        "npcTag",
        "deploymentType",
        "npcEnemyFleetTypeID",
        "groupBehaviorTreeID",
        "npcCorporationID",
        "spawnTableID",
        "npcFleetCounterTableID",
        "dungeonID",
        "typeListID",
        "npcFleetTypeID",
        "metaGroupID",
        "fsdReference",
        "raceID",
        "marketGroupID",
        "ShipGroupID",
        "certificateTemplateID",
        "factionID",
    ]
)


def _unwrap(v, depth):
    if depth <= 0:
        return v
    if isinstance(v, FsdObject):
        return {k: _unwrap(x, depth - 1) for k, x in v.items()}
    if isinstance(v, (FsdDict, FsdMultiIndex)):
        return {k: _unwrap(x, depth - 1) for k, x in v.items()}
    if isinstance(v, FsdList):
        return [_unwrap(x, depth - 1) for x in v]
    if isinstance(v, (list, tuple)):
        return [_unwrap(x, depth - 1) for x in v]
    if isinstance(v, dict):
        return {k: _unwrap(x, depth - 1) for k, x in v.items()}
    return v


def load_classic(path, schema=None):
    """Load one classic-format (``.static``) file.

    Returns a `FsdDict`, `FsdList`, `FsdObject` or a scalar, depending on the
    top-level schema node. Pass ``schema`` to override the embedded one.
    """
    with open(path, "rb") as fh:
        raw = fh.read()
    return load_classic_bytes(raw, schema)


def load_classic_bytes(raw, schema=None):
    data = memoryview(raw)
    if schema is None:
        n = _U32.unpack_from(data, 0)[0]
        if n == 0 or n + 4 > len(data):
            raise FsdError("not a classic FSD container (schema size %d)" % n)
        schema = _unpickle(bytes(data[4 : 4 + n]))
        offset = 4 + n
    else:
        offset = 0
    if not isinstance(schema, dict) or "type" not in schema:
        raise FsdError("embedded schema is not an optimized FSD schema")
    return _node(data, offset, schema)


def read_classic_schema(path):
    """Just the embedded optimized schema of a ``.static`` file."""
    with open(path, "rb") as fh:
        n = _U32.unpack(fh.read(4))[0]
        return _unpickle(fh.read(n))


# --- .schema sidecars ------------------------------------------------------
#
# **[V]** Six `.static` tables ship no embedded schema; the uint32 at offset 0
# is already data. They have a sibling `res:/staticdata/<name>.schema` which
# is the *already optimized* schema in YAML -- it carries
# `constantAttributeOffsets`, `endOfFixedSizeData`, `optionalValueLookups`,
# `fixedItemSize` and `size`, which only the optimizer produces. So no
# optimizer port is needed: parse the YAML and hand it to `load_classic`.
# Verified against regions/jumps/systems/constellations/dialogs/
# factionsowningsolarsystems.
#
# The parser below is a deliberately small subset: block mappings, flow
# mappings and sequences (which PyYAML wraps across lines), one anchor/alias
# form, and plain scalars. That is everything these six files use, checked by
# inspection of all six. It is not a YAML implementation and will raise on
# anything else rather than guess.


def parse_simple_yaml(text):
    """Parse the YAML subset used by the ``.schema`` sidecars."""
    lines = []
    for raw in text.splitlines():
        if not raw.strip():
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        lines.append([indent, raw.strip()])
    # Join lines that PyYAML wrapped inside a flow collection.
    joined = []
    for indent, body in lines:
        if joined and _flow_depth(joined[-1][1]) > 0:
            joined[-1][1] += " " + body
        else:
            joined.append([indent, body])
    anchors = {}
    value, pos = _yaml_block(joined, 0, joined[0][0], anchors)
    if pos != len(joined):
        raise FsdError("yaml: trailing content at line %d" % pos)
    return value


def _flow_depth(s):
    depth = 0
    quote = None
    for ch in s:
        if quote:
            if ch == quote:
                quote = None
            continue
        if ch in "'\"":
            quote = ch
        elif ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
    return depth


def _yaml_block(lines, pos, indent, anchors):
    out = {}
    while pos < len(lines) and lines[pos][0] >= indent:
        cur_indent, body = lines[pos]
        if cur_indent > indent:
            raise FsdError("yaml: unexpected indent at line %d" % pos)
        if ":" not in body:
            raise FsdError("yaml: expected 'key:' at line %d: %r" % (pos, body))
        key, _, rest = body.partition(":")
        key = _yaml_scalar(key.strip(), anchors)
        rest = rest.strip()
        pos += 1
        if rest == "":
            if pos < len(lines) and lines[pos][0] > cur_indent:
                out[key], pos = _yaml_block(lines, pos, lines[pos][0], anchors)
            else:
                out[key] = None
            continue
        anchor = None
        if rest.startswith("&"):
            anchor, _, rest = rest.partition(" ")
            anchor = anchor[1:]
            rest = rest.strip()
            if rest == "":
                if pos < len(lines) and lines[pos][0] > cur_indent:
                    val, pos = _yaml_block(lines, pos, lines[pos][0], anchors)
                else:
                    val = None
                anchors[anchor] = val
                out[key] = val
                continue
        val = _yaml_value(rest, anchors)
        if anchor:
            anchors[anchor] = val
        out[key] = val
    return out, pos


def _yaml_value(s, anchors):
    s = s.strip()
    if s.startswith("*"):
        return anchors[s[1:]]
    if s.startswith("{"):
        return _yaml_flow_map(s, anchors)
    if s.startswith("["):
        return _yaml_flow_seq(s, anchors)
    return _yaml_scalar(s, anchors)


def _split_flow(s):
    """Split the inside of a flow collection on top-level commas."""
    parts = []
    depth = 0
    quote = None
    cur = []
    for ch in s:
        if quote:
            cur.append(ch)
            if ch == quote:
                quote = None
            continue
        if ch in "'\"":
            quote = ch
            cur.append(ch)
        elif ch in "[{":
            depth += 1
            cur.append(ch)
        elif ch in "]}":
            depth -= 1
            cur.append(ch)
        elif ch == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if "".join(cur).strip():
        parts.append("".join(cur))
    return parts


def _yaml_flow_map(s, anchors):
    s = s.strip()
    if not s.endswith("}"):
        raise FsdError("yaml: unterminated flow mapping: %r" % s)
    out = {}
    for part in _split_flow(s[1:-1]):
        k, _, v = part.partition(":")
        out[_yaml_scalar(k.strip(), anchors)] = _yaml_value(v, anchors)
    return out


def _yaml_flow_seq(s, anchors):
    s = s.strip()
    if not s.endswith("]"):
        raise FsdError("yaml: unterminated flow sequence: %r" % s)
    return [_yaml_value(p, anchors) for p in _split_flow(s[1:-1])]


def _yaml_scalar(s, anchors):
    s = s.strip()
    if s.startswith("*"):
        return anchors[s[1:]]
    if len(s) >= 2 and s[0] == s[-1] and s[0] in "'\"":
        return s[1:-1].replace("''", "'")
    if s in ("null", "~", ""):
        return None
    if s == "true":
        return True
    if s == "false":
        return False
    try:
        return int(s)
    except ValueError:
        pass
    try:
        return float(s)
    except ValueError:
        pass
    return s


# --- sqlite .static tables -------------------------------------------------

SQLITE_MAGIC = b"SQLite format 3\x00"


def load_sqlite(path):
    """Open a sqlite-backed ``.static`` table read-only.

    **[V]** 15 of the 45 ``.static`` files are not FSD containers at all: they
    are plain SQLite 3 databases (``blueprints``, ``skins``, ``skinlicenses``,
    ``skinmaterials``, ``clonegrades``, ``dbuffcollections``,
    ``fighterabilities``, ``fighterabilitiesbytype``, ``infobubbles``,
    ``industry_*``). ``bin64/staticdata/mapObjects.db`` is the same idea.

    **[V]** These are `fsdlite` caches, not FSD containers at all: the schema
    is ``cache(key, value, time)`` plus ``indexes(key, value)``, and ``value``
    is a **plain JSON document**. ``fsdlite/storage.pyc`` ``_cache_save`` /
    ``_cache_load`` do ``fsdlite.encode(obj, json=True)`` /
    ``fsdlite.decode(data, json=True)`` against exactly that table, and
    ``_cache_save`` writes ``indexes`` rows as
    ``'{indexName}.{indexKey}' -> key``, which is why the index rows read
    ``productTypeID.31292 -> 31293``. So `sqlite_rows` gives you real,
    fully-typed rows with CCP's own field names, and these 14 files are the
    *easiest* part of the corpus rather than the hardest.

    Returns ``{table_name: {'columns': [...], 'rows': int}}`` plus a live
    read-only ``sqlite3`` connection under the key ``'_connection'``.
    """
    import sqlite3

    uri = "file:%s?mode=ro" % path.replace("?", "%3f").replace("#", "%23")
    con = sqlite3.connect(uri, uri=True)
    con.row_factory = sqlite3.Row
    out = {"_connection": con}
    for (name,) in con.execute(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
    ):
        cols = [r[1] for r in con.execute('PRAGMA table_info("%s")' % name)]
        n = con.execute('SELECT COUNT(*) FROM "%s"' % name).fetchone()[0]
        out[name] = {"columns": cols, "rows": n}
    return out


def sqlite_rows(path, table="cache", limit=None):
    """Rows of a sqlite ``.static`` table, with the JSON ``value`` parsed.

    Yields ``(key, value)`` where ``value`` is the decoded JSON document for
    the ``cache`` table, or the raw string for ``indexes``.
    """
    import json

    meta = load_sqlite(path)
    con = meta["_connection"]
    try:
        sql = 'SELECT * FROM "%s"' % table
        if limit:
            sql += " LIMIT %d" % int(limit)
        for row in con.execute(sql):
            key = row["key"]
            val = row["value"]
            if table == "cache" and isinstance(val, str):
                try:
                    val = json.loads(val)
                except ValueError:
                    pass
            yield key, val
    finally:
        con.close()


def sqlite_json_fields(path, sample=400):
    """Union of top-level JSON field names in a sqlite ``.static`` cache."""
    fields = set()
    for i, (_k, v) in enumerate(sqlite_rows(path, "cache", limit=sample)):
        if isinstance(v, dict):
            fields |= set(v)
        if i >= sample:
            break
    return sorted(fields)


# --- the cFSD container (.fsdbinary) --------------------------------------
#
# The container. Record layouts come from `findings/pyd-schemas.tsv` through
# `rows` and `cfsd_generic_rows` further down.

CFSD_HEADER = struct.Struct("<24sQ")
CFSD_PROLOGUE = 32


def read_cfsd_header(path):
    """Read the 32-byte prologue of a ``.fsdbinary`` file.

    Returns ``{'tag': bytes24, 'payload_size': int, 'file_size': int,
    'ok': bool}``.

    **[V]** measured on all 149 ``.fsdbinary`` files in V24.01: the first 24
    bytes are high-entropy and differ per file, and the uint64 at offset 24
    equals ``file_size - 32`` for every one of them. So the prologue is 24
    bytes of something plus the size of the rest.

    **[I]** the 24 bytes are a schema or content fingerprint, by analogy with
    the schema-compatibility check the classic loader gets for free from its
    embedded schema. Not verified; it is not the md5 in ``resfileindex.txt``.
    """
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        tag, payload = CFSD_HEADER.unpack(fh.read(32))
    return {
        "tag": tag,
        "payload_size": payload,
        "file_size": size,
        "ok": payload == size - 32,
    }


class CfsdTable:
    """A ``.fsdbinary`` (cFSD) table, as far as the container is understood.

    What is **[V]** verified, by reproducing it on all 149 files of V24.01 and
    cross-checking one table against an independent source:

    ::

        bytes[24]   tag          high-entropy, per-file; [I] a fingerprint
        uint64      payloadSize  == filesize - 32, on all 149
        -- payload starts here; every offset below is payload-relative --
        uint64      headerSize   24 on 147 of 149
        uint64      rowCount
        uint64      bucketCount
        uint64[bucketCount]      bucketDataOffset
        ...
        <bucket 0>  uint64 rowsInBucket, then rowsInBucket fixed-size records,
                    then that bucket's variable-length data (strings, lists)
        <bucket 1>  ...

      The ``rowsInBucket`` count sits at ``bucketDataOffset - 8``, so the first
      bucket's offset is always ``headerSize + 8*bucketCount + 8``. The
      primary key is a **hash map keyed by ``key % bucketCount``**: every
      record in bucket *b* has ``key % bucketCount == b``, and within a bucket
      keys ascend. **[V]** on every table where a stride could be determined:
      ``sum(rowsInBucket) == rowCount`` and the key invariant holds for every
      record.

      A record is a fixed number of bytes (`stride`), the same for every row
      of a table, and begins with ``uint32 key``. The rest is the row, and
      **its field layout is `unknown`**: it comes from generated C++ in
      ``bin64/<table>Loader.pyd`` and is not described anywhere in the
      bytecode. What this class gives you instead is the raw slots plus two
      safe readings: `strings` resolves a slot that is a valid
      variable-length-data pointer, and `u32_pairs` splits a slot into the two
      32-bit halves the generator clearly uses for id columns.

      **[V]** Variable-length data is addressed by a payload-relative uint64
      offset; the eight bytes before it are a uint64 length. Strings are the
      bytes at that offset, and are not NUL-terminated in the length.

    Worked, fully cross-checked example: `accounting_entry_types`.
    """

    def __init__(self, path, name=None):
        with open(path, "rb") as fh:
            raw = fh.read()
        self.name = name or os.path.basename(path)
        self.path = path
        self.raw = raw
        tag, payload_size = CFSD_HEADER.unpack_from(raw, 0)
        self.tag = tag
        self.payload_size = payload_size
        self.prologue_ok = payload_size == len(raw) - CFSD_PROLOGUE
        self.pl = memoryview(raw)[CFSD_PROLOGUE:]
        self.header_size = self._u64(0)
        self.row_count = self._u64(8)
        self.bucket_count = self._u64(16)
        self.ok = False
        self.why = None
        self.offsets = []
        self.counts = []
        self._stride = None
        self._validate()

    # -- primitives --------------------------------------------------------

    def _u64(self, i):
        return _U64.unpack_from(self.pl, i)[0]

    def _u32(self, i):
        return _U32.unpack_from(self.pl, i)[0]

    def _validate(self):
        n = len(self.pl)
        if self.header_size < 24 or self.header_size > 4096:
            self.why = "headerSize=%d" % self.header_size
            return
        if not 0 < self.bucket_count <= 1 << 22:
            self.why = "bucketCount=%d" % self.bucket_count
            return
        first = self.header_size + 8 * self.bucket_count + 8
        if first > n:
            self.why = "bucket table does not fit"
            return
        offs = [
            self._u64(self.header_size + 8 * i) for i in range(self.bucket_count)
        ]
        if offs != sorted(offs) or offs[0] != first or offs[-1] > n:
            self.why = "bucket offsets implausible (first=%d want %d)" % (
                offs[0],
                first,
            )
            return
        counts = [self._u64(o - 8) for o in offs]
        if sum(counts) != self.row_count:
            self.why = "bucket counts sum to %d, header says %d" % (
                sum(counts),
                self.row_count,
            )
            return
        self.offsets = offs
        self.counts = counts
        self.ok = True

    # -- stride ------------------------------------------------------------

    @property
    def stride(self):
        """Record size in bytes, or None if it could not be determined.

        Found, not guessed. In a bucket holding at least two rows, the second
        row's key is some 8-aligned uint64 after the first that is ``== b``
        modulo ``bucketCount`` and larger than the first key. Each such bucket
        therefore yields a *set* of candidate strides; the real stride is in
        every set, so they are intersected, and each survivor is then checked
        against every row of every bucket. Returns None when no candidate
        survives or more than one does.
        """
        if self._stride is not None:
            return self._stride or None
        if not self.ok:
            self._stride = 0
            return None
        ends = self.offsets[1:] + [len(self.pl)]
        shared = None
        for b, (o, e, c) in enumerate(zip(self.offsets, ends, self.counts)):
            if c < 2:
                continue
            k0 = self._u32(o)
            room = (e - o) // c  # a record cannot be wider than this
            cands = set()
            p = o + 4
            while p + 4 <= e and p - o <= room:
                v = self._u32(p)
                if v > k0 and v % self.bucket_count == b:
                    cands.add(p - o)
                p += 4
            if not cands:
                self._stride = 0
                return None
            shared = cands if shared is None else (shared & cands)
            if not shared:
                self._stride = 0
                return None
            if len(shared) == 1:
                break
        if not shared:
            self._stride = 0
            return None
        survivors = [s for s in sorted(shared) if self._stride_consistent(s)]
        if len(survivors) != 1:
            self._stride = 0
            self.stride_candidates = survivors
            return None
        self._stride = survivors[0]
        return self._stride

    def _stride_consistent(self, stride):
        ends = self.offsets[1:] + [len(self.pl)]
        for b, (o, e, c) in enumerate(zip(self.offsets, ends, self.counts)):
            if c == 0:
                continue
            if o + c * stride > e:
                return False
            prev = -1
            for i in range(c):
                k = self._u32(o + i * stride)
                if k % self.bucket_count != b or k <= prev:
                    return False
                prev = k
        return True

    # -- rows --------------------------------------------------------------

    def keys(self):
        """Every primary key, sorted. Requires `stride`."""
        return sorted(k for k, _o in self._locate())

    def records(self):
        """``(key, payload-relative record offset)`` for every row."""
        stride = self.stride
        if stride is None:
            raise FsdError("%s: record stride unknown" % self.name)
        for b, (o, c) in enumerate(zip(self.offsets, self.counts)):
            for i in range(c):
                r = o + i * stride
                yield self._u32(r), r

    def slots(self, key=None):
        """Raw uint64 slots of one row, or ``{key: slots}`` for all rows."""
        stride = self.stride
        if key is not None:
            for k, r in self._locate():
                if k == key:
                    return [self._u64(r + 8 * j) for j in range(stride // 8)]
            raise KeyError(key)
        return {
            k: [self._u64(r + 8 * j) for j in range(stride // 8)]
            for k, r in self._locate()
        }

    def u32_pairs(self, key):
        """One row's slots split into 32-bit halves."""
        out = []
        for v in self.slots(key):
            out.append((v & 0xFFFFFFFF, v >> 32))
        return out

    # The old private name, kept working for one round: every consumer in
    # this file and in `tools/dogma.py` reached for it with a `noqa`.
    _locate = records

    def string(self, offset):
        """Resolve a variable-length-data pointer to its bytes. **[V]**"""
        n = self._u64(offset - 8)
        return bytes(self.pl[offset : offset + n])

    def maybe_string(self, offset, limit=4096):
        """`string`, but None when ``offset`` cannot be a data pointer."""
        if not 8 <= offset < len(self.pl):
            return None
        try:
            n = self._u64(offset - 8)
        except struct.error:
            return None
        if n > limit or offset + n > len(self.pl):
            return None
        blob = bytes(self.pl[offset : offset + n])
        try:
            s = blob.decode("utf-8")
        except UnicodeDecodeError:
            return None
        if any(ch < " " and ch not in "\t\n\r" for ch in s):
            return None
        return s

    def profile(self, sample=400):
        """Per-slot summary over a sample of rows: what each slot might be.

        Returns one dict per slot with ``min``/``max`` of the uint64 value, how
        often it resolves as a string, and the two 32-bit halves' ranges.
        This is the honest substitute for a schema: it tells you which slots
        are ids, which are pointers and which are flags, without naming them.
        """
        stride = self.stride
        if stride is None:
            raise FsdError("%s: record stride unknown" % self.name)
        nslots = stride // 8
        acc = [
            {
                "min": None,
                "max": None,
                "strings": 0,
                "zero": 0,
                "lo_max": 0,
                "hi_max": 0,
                "examples": [],
            }
            for _ in range(nslots)
        ]
        rows = 0
        for k, r in self._locate():
            rows += 1
            if rows > sample:
                break
            for j in range(nslots):
                v = self._u64(r + 8 * j)
                a = acc[j]
                a["min"] = v if a["min"] is None else min(a["min"], v)
                a["max"] = v if a["max"] is None else max(a["max"], v)
                if v == 0:
                    a["zero"] += 1
                a["lo_max"] = max(a["lo_max"], v & 0xFFFFFFFF)
                a["hi_max"] = max(a["hi_max"], v >> 32)
                s = self.maybe_string(v) if j else None
                if s is not None:
                    a["strings"] += 1
                    if len(a["examples"]) < 3 and s:
                        a["examples"].append(s[:60])
        for a in acc:
            a["sampled"] = min(rows, sample)
        return acc


def accounting_entry_types(client):
    """``accountingentrytypes.fsdbinary``, decoded. The worked cFSD example.

    Record, 40 bytes, slot by slot::

        +0   uint64  entryTypeID              [V] the bucket key
        +8   uint64  -> description string     [V]
        +16  uint64  -> name string            [V]
        +24  uint32  optional id A             [I] see below
        +28  uint32  optional id B             [I] see below
        +32  uint32  entryTypeNameID           [V]
        +36  uint32  presence bitfield         [V] bit0 <-> A, bit1 <-> B

    **[V]** All 218 rows decode; ``sum(bucket counts) == 218 == rowCount``;
    the two pointers resolve to the English name and description, and
    ``entryTypeNameID == 59792 + entryTypeID`` for all 103 ids <= 103, which
    is the `localization-corpus` lane's independently derived run. **[V]** the
    presence bitfield matches field A / field B being non-zero on all 218
    rows, with no exception.

    **[I]** A and B are ``entryJournalMessageID`` and
    ``entryTypeDescriptionID`` -- those are the two remaining attribute names
    in ``bin64/accountingEntryTypesLoader.pyd`` -- **in an order this lane did
    not establish**. The evidence leans A = description id (for the newest
    rows A is exactly ``entryTypeNameID - 1``, i.e. allocated beside the name)
    and B = journal message id, but that is a lean, not a finding. Resolving
    it costs one lookup each in the localisation corpus.

    Returns ``{entryTypeID: {...}}``.
    """
    t = CfsdTable(client.path("res:/staticdata/accountingentrytypes.fsdbinary"))
    if t.stride != 40:
        raise FsdError("accountingentrytypes: stride is %r, expected 40" % t.stride)
    out = {}
    for key, r in t._locate():  # noqa: SLF001  (same module)
        out[key] = {
            "entryTypeID": key,
            "description": t.string(t._u64(r + 8)).decode("utf-8", "replace"),
            "name": t.string(t._u64(r + 16)).decode("utf-8", "replace"),
            "optionalIdA": t._u32(r + 24),
            "optionalIdB": t._u32(r + 28),
            "entryTypeNameID": t._u32(r + 32),
            "presenceFlags": t._u32(r + 36),
        }
    return out


#: Record columns established for cFSD tables, as ``{byte offset: (name,
#: kind)}``. ``kind`` is ``u32``, ``f64``, ``msgid`` (a global localisation
#: message id -- see ``tools/loctable.py``), ``str`` (a pointer to
#: length-prefixed bytes), ``listc`` (a pointer to a uint64 element count) or
#: ``listd`` (a pointer to that list's first element; always ``listc + 8``).
#:
#: These are **[V]**: each was found by profiling every row of the table and
#: then confirmed against something outside this file -- foreign-key closure
#: into another cFSD table, resolution through the localisation corpus, or a
#: published EVE value. Nothing in here is a guess; a column that could not be
#: confirmed is simply absent, and most columns of most tables are absent.
CFSD_COLUMNS = {
    # types.fsdbinary, stride 152. **The whole row, from typesLoader.pyd**,
    # and the fifteen columns that were here before -- found by profiling --
    # agree with it offset for offset. `tools/dogma.py`'s `SCHEMAS["types"]`
    # is the authoritative copy (it can also follow the one pointer field);
    # this one is here because `fsd.py` must not import `dogma.py`, and what
    # keeps the two honest is that each reconciles against the same stride.
    #
    # Spot-checked against published values rather than only against the
    # layout: Rifter mass 1,067,000 / volume 27,289 / cargo 140 / group 25,
    # Dominix group 27, Avatar volume 155,000,000, Veldspar portionSize 100,
    # Tritanium volume 0.01.
    "types": {
        0: ("typeID", "u32"),
        8: ("basePrice", "f64", 0x1),
        16: ("capacity", "f64", 0x2),
        # 24: designerIDs, a pointer. `dogma.py` has it; this reader has no
        # kind for a nested object, so it is left out rather than mislabelled.
        32: ("mass", "f64", 0x800),
        # **int64, not u32**: the getter is `PyLong_FromLongLong`. Harmless
        # today because values run 1..5000, but the declared type was wrong
        # and a declared type is what the next reader trusts.
        40: ("portionSize", "i64", 0x4000),
        48: ("radius", "f64", 0x80000),
        56: ("volume", "f64", 0x800000),
        64: ("certificateTemplate", "u32", 0x4),
        68: ("descriptionID", "msgid", 0x8),
        # **factionID is at +72, not +76** (K1, 2026-10-25): the loader says
        # +72, and the data agrees -- of the 1,383 rows with bit 0x20 set,
        # all 1,383 values at +72 close into `factions` U `npccorporations`
        # and +76 is zero on every one of the 53,288 rows. This column, and
        # `tools/dogma.py`'s `SCHEMAS["types"]` (struct +0x44), read +76, so
        # both returned 0 for every faction-owned type. The 4-byte hole of
        # `docs/re/dogma.md` section 12 is at +76.
        72: ("factionID", "u32", 0x20),
        80: ("graphicID", "u32", 0x40),
        84: ("groupID", "u32"),
        88: ("iconID", "u32", 0x80),
        92: ("isisGroupID", "u32", 0x200),
        96: ("marketGroupID", "u32", 0x400),
        100: ("metaGroupID", "u32", 0x1000),
        104: ("metaLevel", "u32", 0x2000),
        108: ("quoteAuthorID", "msgid", 0x10000),
        112: ("quoteID", "msgid", 0x20000),
        116: ("raceID", "u32", 0x40000),
        120: ("soundID", "u32", 0x100000),
        124: ("techLevel", "u32", 0x200000),
        128: ("typeID (repeated)", "u32"),
        132: ("typeNameID", "msgid"),
        136: ("variationParentTypeID", "u32", 0x400000),
        140: ("wreckTypeID", "u32", 0x1000000),
        144: ("isDynamicType", "bool", 0x100),
        145: ("published", "bool", 0x8000),
    },
    # groups.fsdbinary, stride 32. categoryID closes into categories for all
    # 1,611 rows; groupNameID resolves for all of them.
    "groups": {
        0: ("groupID", "u32"),
        4: ("categoryID", "u32"),
        8: ("groupID (repeated)", "u32"),
        12: ("groupNameID", "msgid"),
    },
    # categories.fsdbinary, stride 40.
    "categories": {
        0: ("categoryID", "u32"),
        16: ("categoryID (repeated)", "u32"),
        20: ("categoryNameID", "msgid"),
    },
    # accountingentrytypes.fsdbinary, stride 40. See
    # `accounting_entry_types` for the full, cross-checked story.
    "accountingentrytypes": {
        0: ("entryTypeID", "u32"),
        8: ("description", "str"),
        16: ("name", "str"),
        32: ("entryTypeNameID", "msgid"),
    },
    # dungeons.fsdbinary, stride 120. dungeonNameID resolves for all 5,516
    # rows; factionID closes into factions; the room list's ids are the room
    # ids that asteroidcountbytypeidroomiddungeonid.static is keyed by.
    "dungeons": {
        0: ("dungeonID", "u32"),
        32: ("rooms", "listc"),
        40: ("rooms", "listd"),
        68: ("descriptionID", "msgid"),
        76: ("dungeonID (repeated)", "u32"),
        80: ("dungeonNameID", "msgid"),
        88: ("typeID", "u32"),
        92: ("factionID", "u32"),
        96: ("prose A", "msgid"),
        100: ("objectiveTextID", "msgid"),
    },
    # missions.fsdbinary, stride 224. missionNameID resolves for all 2,892
    # rows; dungeonID closes into dungeons; the (key, messageID) lists resolve
    # to full briefing prose.
    "missions": {
        0: ("missionID", "u32"),
        48: ("contentTemplateKey", "str"),
        56: ("messages", "listc"),
        64: ("messages", "listd"),
        92: ("npcCorporationID", "u32"),
        120: ("factionID", "u32"),
        144: ("dungeonID", "u32"),
        196: ("missionNameID", "msgid"),
    },
    # graphiclocations.fsdbinary, stride 40, 68 rows. **The whole row**, from
    # `graphicLocationsLoader.pyd` via `findings/pyd-schemas.tsv`, and it
    # reconciles with the stride: three 8-byte fields (alphabetically
    # `directionalLocators`, `hull`, `locators`) then the uint32 presence
    # bitfield at +32, which is `8 + 24 + 8 = 40`.
    #
    # Only `hull` is a scalar. The other two are **lists of inline structs**,
    # which no `cfsd_rows` kind can express, so they are not columns here:
    # `graphic_locations` below decodes them and is the entry point to use.
    #
    # This is the undock-point geometry: `ballparkCommon/docking.py`
    # reaches it as `GetDirectionalLocators(GetGraphicLocationID(
    # evetypes.GetGraphicID(stationTypeID)), default=[])`. **[V]**
    "graphiclocations": {
        0: ("graphicLocationID", "u32"),
        16: ("hull", "str"),
    },
}

#: Where a table's **presence bitfield** sits, by record offset. **[V]**
#:
#: A field with a presence bit reads as `None` when the bit is clear, because
#: the generated getter returns `Py_None` in exactly that case -- so `None`
#: here means what it means in the client, and is not the same answer as `0`.
#: A table absent from this map has no presence handling, and its columns must
#: not carry bits.
#:
#: The bits themselves are not arbitrary: they are assigned **alphabetically**
#: over the presence-gated fields, which is how a derived layout can be shown
#: to be complete. `docs/re/dogma.md` section 12.
CFSD_PRESENCE = {"types": 148, "graphiclocations": 32}

#: Within one dungeon room record (104 bytes), **[V]**.
CFSD_ROOM_COLUMNS = {0: ("roomID", "u32"), 8: ("objects", "listc"), 40: ("objects", "listd")}

#: Within one dungeon object record, **[V]** for the typeID only: at ``+148``
#: 97% of non-zero values are valid typeIDs and they resolve to exactly the
#: things a dungeon is made of -- Veldspar, Beacon, Acceleration Gate,
#: Habitation Module, Tower Sentry. The record's own width and its position
#: and rotation fields are `unknown`.
CFSD_OBJECT_COLUMNS = {148: ("typeID", "u32")}


def cfsd_list(table, count_offset):
    """Elements of a cFSD list field. **[V]**

    A list occupies two uint64 slots of a record, 8 bytes apart: the first
    points at a uint64 element count, the second at the first element (so it
    is always ``first + 8``). Elements are uint64 -- for a list of objects,
    each one is a payload-relative pointer to that object's record. A string
    is the same shape with the count being a byte length, which is why
    `CfsdTable.string` reads its length from ``offset - 8``.
    """
    p = _U32.unpack_from(table.pl, count_offset)[0]
    if not 8 <= p < len(table.pl) - 8:
        return []
    n = _U64.unpack_from(table.pl, p)[0]
    if n > 1 << 20 or p + 8 + 8 * n > len(table.pl):
        return []
    return [_U64.unpack_from(table.pl, p + 8 + 8 * i)[0] for i in range(n)]


def cfsd_list_at(table, data_slot):
    """Elements of a cFSD list from the slot the **loader** reads. **[V]**

    `cfsd_list` above takes the slot holding a pointer to the element *count*;
    this takes the slot holding a pointer to the first *element*, with the
    count in the uint64 immediately before it. That is the shape the
    generated loader uses, and it is the one to prefer:

    **`cfsd_list` at the count slot is right only by accident when the
    preceding list field is empty.** The generator writes each vector into the
    arena as `[uint64 count][elements...]`, back to back, so an empty
    predecessor leaves its element pointer one word past its own count --
    exactly where the next field's count then sits. When the predecessor is
    *not* empty the two diverge and the read returns garbage: for
    `dungeons.fsdbinary` that is **86 of 5,516 rows** -- precisely those with a
    non-empty `contentTags` -- and dungeon 12454 came back claiming 644,680
    rooms where it has 2. Measured, not inferred; found by the `dogma-corpus`
    lane reading the loader's own generated `tp_getattro`.
    """
    p = _U64.unpack_from(table.pl, data_slot)[0]
    if not 8 <= p < len(table.pl):
        return []
    n = _U64.unpack_from(table.pl, p - 8)[0]
    if n > 1 << 20 or p + 8 * n > len(table.pl):
        return []
    return [_U64.unpack_from(table.pl, p + 8 * i)[0] for i in range(n)]


def cfsd_dict(table, slot, entry_stride):
    """``[(key, value-struct offset)]`` for a cFSD **dict** field. **[V]**

    `slot` holds a payload-relative pointer `p`; `u64(p-8)` is the bucket
    count and `p + 8*b` the b'th bucket pointer; a bucket's own `u64(bp-8)` is
    its entry count, and each entry is `uint32 key`, four bytes of padding,
    then the value struct inline -- so `entry_stride` is `8 + sizeof(struct)`.

    **This is what `dungeons.rooms` and a room's `objects` actually are**, and
    reading them as lists was wrong twice over: `cfsd_list` on the rooms slot
    returned only **the first entry of each hash bucket** (dungeon 47's two
    buckets are `[12778, 12780, 12782]` and `[12779, 12781, 12783]`, so it
    reported 12778 and 12779 and lost four rooms), and `cfsd_list(room + 8)`
    landed on the empty `assetManipulations` pointer -- whose value happens to
    be `objects_ptr - 8` -- so the "object counts" it returned were the
    objects dict's **bucket count** and bucket-pointer array.

    Validated by the `dungeon-model` lane against the record's own count
    field: the entry count this yields equals `u32` at the slot 8 bytes after
    `slot` for **all 5,516 dungeons and all 8,483 rooms**, and the per-room
    object counts then reproduce
    `asteroidcountbytypeidroomiddungeonid.static` **exactly** -- 3,054 of
    3,054 cells, no room missing.
    """
    p = _U64.unpack_from(table.pl, slot)[0]
    if not 8 <= p < len(table.pl):
        return []
    nb = _U64.unpack_from(table.pl, p - 8)[0]
    if nb > 1 << 22 or p + 8 * nb > len(table.pl):
        return []
    out = []
    for b in range(nb):
        bp = _U64.unpack_from(table.pl, p + 8 * b)[0]
        if not 8 <= bp < len(table.pl):
            continue
        n = _U64.unpack_from(table.pl, bp - 8)[0]
        if n > 1 << 22 or bp + n * entry_stride > len(table.pl):
            continue
        for i in range(n):
            e = bp + i * entry_stride
            out.append((_U32.unpack_from(table.pl, e)[0], e + 8))
    return out


#: Room struct is 96 bytes and the object struct 152; a dict entry is
#: `8 + sizeof(struct)`. See `docs/re/dungeon-model.md`.
CFSD_ROOM_STRIDE = 8 + 96
CFSD_OBJECT_STRIDE = 8 + 152


def cfsd_rows(table, stem=None, columns=None, limit=None):
    """Decode the *known* columns of a cFSD table into ``{key: {name: value}}``.

    Columns come from `CFSD_COLUMNS` (keyed by the table's file stem) unless
    ``columns`` is given. ``msgid`` columns come back as integers: resolve them
    with ``tools/loctable.py``, which owns the localisation corpus. Columns
    that this lane could not confirm are **not** in the result, by design --
    ``fsd_dump.py raw`` is how you look at the rest.
    """
    stem = stem or table.name.split(".")[0]
    cols = columns if columns is not None else CFSD_COLUMNS.get(stem)
    if not cols:
        raise FsdError("no confirmed columns for %s" % stem)
    presence = CFSD_PRESENCE.get(stem)
    out = {}
    for key, r in table._locate():  # noqa: SLF001
        row = {}
        pres = table._u32(r + presence) if presence is not None else None  # noqa: SLF001
        for off, col in sorted(cols.items()):
            name, kind = col[0], col[1]
            # **A column with a bit whose table has no presence offset is a
            # mistake, not a column to read anyway.** Reading it would return
            # whatever is in the struct, which for a clear bit is not a value
            # the client would ever show.
            bit = col[2] if len(col) > 2 else None
            if bit is not None:
                if pres is None:
                    raise FsdError(
                        "%s: column %r has presence bit 0x%x but the table has "
                        "no CFSD_PRESENCE entry" % (table.name, name, bit)
                    )
                if not pres & bit:
                    row[name] = None
                    continue
            if kind == "u32":
                row[name] = table._u32(r + off)  # noqa: SLF001
            elif kind == "i64":
                row[name] = _I64.unpack_from(table.pl, r + off)[0]
            elif kind == "msgid":
                row[name] = table._u32(r + off)  # noqa: SLF001
            elif kind == "f64":
                row[name] = _F64.unpack_from(table.pl, r + off)[0]
            elif kind == "str":
                p = table._u32(r + off)  # noqa: SLF001
                row[name] = table.maybe_string(p)
            elif kind == "bool":
                row[name] = bool(table.pl[r + off])
            elif kind == "listc":
                row[name] = cfsd_list(table, r + off)
            elif kind == "listd":
                pass
            else:
                # **Not `pass`.** An unrecognised kind used to drop the column
                # silently -- declaring `portionSize` as `i64` before this arm
                # existed removed it from every row of `types` with nothing
                # said, and the only reason it was noticed was a spot-check of
                # unrelated output. A typo in a column table must be loud.
                raise FsdError(
                    "%s: column %r has unknown kind %r" % (table.name, name, kind)
                )
        out[key] = row
        if limit and len(out) >= limit:
            break
    return out


def dungeon_rooms(table, dungeon_id):
    """``[(roomID, [objectTypeID, ...])]`` for one dungeon. **[V]** partly.

    The room ids are confirmed: they are the ids
    ``asteroidcountbytypeidroomiddungeonid.static`` is keyed by, and the
    object typeIDs for those rooms include the ore the asteroid table names
    (dungeon 47, room 12779: 12 Veldspar objects, asteroid table says
    Veldspar). **Caveat, and it is a real one:** for some dungeons this list
    is a *subset* of the rooms the asteroid table knows about (dungeon 49
    **Both of the caveats that used to be here are closed.** They were one
    bug each, and neither was in the data:

    - *"the room list is sometimes a subset"* -- dungeon 49 yielding rooms
      8650-8651 while the asteroid table named 8652-8654 -- was `cfsd_list`
      returning **the first entry of each hash bucket**. Rooms are a dict.
    - *"the object counts disagree"* -- the asteroid table saying 51 of type
      1230 against this function's 12 of 1031140 -- was two separate
      mistakes. `cfsd_list(room + 8)` landed on the empty
      `assetManipulations` pointer, whose value is exactly `objects_ptr - 8`,
      so the counts returned were the objects dict's *bucket count*. And
      **there was never a second type id**: 1031140 is Veldspar's
      `typeNameID`, printed by `fsd_dump.py`'s own label instead of the
      typeID. This function always said 1230.

    Read as dicts, the per-room object counts reproduce
    `asteroidcountbytypeidroomiddungeonid.static` exactly -- 3,054 of 3,054
    cells with no room missing -- and `object.typeID` closes 6,249 of 6,249
    distinct values into `types`. Dungeon 47 is six rooms, not two.
    """
    for key, r in table.records():
        if key != dungeon_id:
            continue
        out = []
        # **Record +40 is the rooms *dict*.** `+32` is `contentTags`. Reading
        # +40 as a list returned one room per hash bucket; see `cfsd_dict`.
        #
        # `typeID` at struct `+140` is the same byte as the old reading's
        # record `+148` counted from the entry, which is why that one column
        # validated while the counts did not.
        for rid, room in cfsd_dict(table, r + 40, CFSD_ROOM_STRIDE):
            types = [
                _U32.unpack_from(table.pl, obj + 140)[0]
                for _oid, obj in cfsd_dict(table, room + 32, CFSD_OBJECT_STRIDE)
            ]
            out.append((rid, types))
        return out
    raise KeyError(dungeon_id)


#: One ``graphiclocations`` list field: ``(record offset, element stride,
#: presence bit)``. **[V]**, `graphicLocationsLoader.pyd`.
#:
#: Both are lists of **inline structs**, not of pointers: the record slot holds
#: a payload-relative pointer to element 0 and the element count sits in the
#: uint64 at ``pointer - 8``, exactly as for an ordinary list, but the elements
#: are the structs themselves laid end to end.
CFSD_GRAPHIC_LOCATION_LISTS = {
    "directionalLocators": (8, 40, 0x1),
    "locators": (24, 32, 0x2),
}

#: A ``graphiclocations`` **directional locator**, 40 bytes. **[V]**
#:
#: ``8 + 8 + 12 + 12 = 40``, and the field order is the generator's own rule
#: (`docs/re/dogma.md` section 12): the two 8-byte string pointers first,
#: alphabetically, then the two 4-byte vectors, alphabetically.
CFSD_DIRECTIONAL_LOCATOR = {
    0: ("category", "str"),
    8: ("name", "str"),
    16: ("direction", "vec3"),
    28: ("position", "vec3"),
}

#: A ``graphiclocations`` **locator**, 32 bytes: the same shape with no
#: ``direction``, so ``8 + 8 + 12`` padded to the 8-byte alignment of the
#: pointers. **[V]**
CFSD_LOCATOR = {
    0: ("category", "str"),
    8: ("name", "str"),
    16: ("position", "vec3"),
}


def cfsd_struct_list(table, slot, elem_stride):
    """Payload offsets of a cFSD list whose elements are **inline structs**.

    Same addressing as `cfsd_list_at` -- the slot holds a pointer to element
    0 and ``u64(pointer - 8)`` is the count -- but the elements are structs
    laid end to end rather than uint64s, so what comes back is the offset of
    each element rather than its value.
    """
    p = _U64.unpack_from(table.pl, slot)[0]
    if not 8 <= p < len(table.pl):
        return []
    n = _U64.unpack_from(table.pl, p - 8)[0]
    if n > 1 << 20 or p + n * elem_stride > len(table.pl):
        return []
    return [p + i * elem_stride for i in range(n)]


def _locator(table, base, layout):
    row = {}
    for off, (name, kind) in sorted(layout.items()):
        if kind == "str":
            row[name] = table.maybe_string(_U64.unpack_from(table.pl, base + off)[0])
        elif kind == "vec3":
            row[name] = tuple(
                _F32.unpack_from(table.pl, base + off + 4 * i)[0] for i in range(3)
            )
        else:  # pragma: no cover - the loud arm, as in `cfsd_rows`
            raise FsdError("locator field %r has unknown kind %r" % (name, kind))
    return row


def graphic_locations(table):
    """``{graphicLocationID: {hull, locators, directionalLocators}}``. **[V]**

    The whole of ``graphiclocations.fsdbinary``, including the two
    inline-struct lists `cfsd_rows` cannot express. A list whose presence bit
    is clear comes back as ``None`` rather than ``[]``, for the same reason
    `cfsd_rows` does it: that is what the client's own getter returns.

    This is the undock geometry. ``directionalLocators`` carries one entry per
    docking mouth, with ``category`` one of ``undockPoint``,
    ``undockPointSmall``, ``undockPointLarge``, ``undockPointCapital`` or
    ``undockPointSupercapital`` -- `eve/common/lib/appConst`'s five
    ``*_UNDOCKPOINT_CATEGORY`` constants -- and ``position``/``direction`` in
    the **model's own space**, in metres. ``locators`` is the non-directional
    set: service points such as ``cloningService`` and ``factoryService``.

    Checked across all 68 rows: 235 directional locators, every ``category``
    one of the five ``undockPoint*`` constants or ``graphicSpawnPoint``, and
    every one of the **232 ``undockPoint*`` directions a unit vector to
    within 1e-5**. The three that are not are the three
    ``graphicSpawnPoint`` rows -- ``cynoField`` at ``(0, 0, -180)`` and two
    placement markers at ``(0, 0, 0)`` -- which are a different feature and
    are not consumed by ``ballparkCommon/docking``. Reached from a type as
    ``types.graphicID -> graphicids.graphicLocationID -> here``.
    """
    presence = CFSD_PRESENCE["graphiclocations"]
    out = {}
    for key, r in table.records():
        row = {"hull": table.maybe_string(table._u32(r + 16))}  # noqa: SLF001
        pres = table._u32(r + presence)  # noqa: SLF001
        for name, (off, stride, bit) in CFSD_GRAPHIC_LOCATION_LISTS.items():
            if not pres & bit:
                row[name] = None
                continue
            layout = (
                CFSD_DIRECTIONAL_LOCATOR if stride == 40 else CFSD_LOCATOR
            )
            row[name] = [
                _locator(table, e, layout)
                for e in cfsd_struct_list(table, r + off, stride)
            ]
        out[key] = row
    return out


# --- the generic cFSD decoder: every table, from the native loaders' layouts --
#
# Packet K1 (`docs/content/C1-corpus-map.md` section 7), applying the patch
# `docs/re/native-loaders.md` section 7 proposed. `findings/pyd-schemas.tsv` is
# the per-table record layout `tools/pyd_schema.py` reads out of
# `bin64/<table>Loader.pyd`; this reads that file and decodes any table it
# describes, nested containers included. `CFSD_COLUMNS` and `tools/dogma.py`'s
# `SCHEMAS` stay as they are and become cross-checks:
# `tools/test_fsd_generic.py` and `docs/lanes/fsd-generic.md` record that the
# generic rows equal theirs.

#: Where `tools/pyd_schema.py` leaves the layouts. Format:
#: `docs/re/native-loaders.md` section 3. **Offsets are record-relative and
#: already include `delta`; add nothing to them.**
PYD_SCHEMA_TSV = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    os.pardir,
    "findings",
    "pyd-schemas.tsv",
)

_PYD_OPTIONAL = re.compile(r"optional\(bit=(0x[0-9a-f]+),word=\+(\d+)\)")

#: Byte width of each scalar kind in the TSV. ``str``/``wstr`` are an 8-byte
#: arena pointer; ``u64`` is not a TSV kind and appears only in
#: `CFSD_BARE_SCHEMAS`.
_PYD_WIDTH = {
    "f64": 8, "f32": 4, "i64": 8, "u64": 8, "i32": 4, "u16": 2, "u8": 1,
    "str": 8, "wstr": 8,
}


class PydField:
    """One field of a cFSD record or nested struct, as the loader reads it.

    ``offset`` is record-relative for a row field (``delta`` included) and
    struct-relative for a field of a nested struct. ``bit``/``word`` are set
    for an optional field: it is present when ``u32_at(base + word) & bit``,
    which is the branch the generated getter takes to ``Py_None``.
    """

    __slots__ = ("name", "offset", "kind", "bit", "word")

    def __init__(self, name, offset, kind, bit=None, word=None):
        self.name = name
        self.offset = offset
        self.kind = kind
        self.bit = bit
        self.word = word

    def __repr__(self):
        return "PydField(%r, %d, %r)" % (self.name, self.offset, self.kind)


class PydSchema:
    """A table's layout: ``stride``, ``delta``, row fields, nested structs.

    ``sizes`` is ``{struct name: byte size}`` for every nested struct, and is
    **computed, not read**: the TSV has no ``#struct`` row, so a struct's size
    is ``align_up(span, align)`` -- the rule `tools/pyd_schema.py` uses for
    the record stride, one level down (`docs/content/C1-corpus-map.md`
    section 4, **[I]**). A size that cannot be computed (a struct with no
    recovered fields) is absent, and reading through it raises.
    ``status`` is ``ok``, or the TSV's reason for a table with no fields.
    """

    __slots__ = ("table", "stride", "delta", "fields", "structs", "status",
                 "sizes", "aligns", "bare")

    def __init__(self, table):
        self.table = table
        self.stride = None
        self.delta = None
        self.fields = []
        self.structs = {}
        self.status = "ok"
        self.sizes = {}
        self.aligns = {}
        self.bare = None

    def finish(self):
        """Compute the nested struct sizes, innermost first. Idempotent."""
        sizes = {}
        aligns = {}
        # `tools/pyd_schema.py` iterates four rounds with an unresolved struct
        # counted as 8 wide; doing the same keeps the record strides here
        # equal to the TSV's for exactly the reason they are equal there.
        for _round in range(6):
            for name, fl in self.structs.items():
                if not fl:
                    continue
                sizes[name] = _align_up(_pyd_span(fl, sizes), _pyd_align(fl, sizes))
                aligns[name] = _pyd_align(fl, sizes)
        self.sizes = sizes
        self.aligns = aligns
        return self

    def computed_stride(self):
        """``align_up(span, align)`` over the row fields, or None."""
        if not self.fields:
            return None
        return _align_up(_pyd_span(self.fields, self.sizes),
                         _pyd_align(self.fields, self.sizes))


def _align_up(n, a):
    return (n + a - 1) // a * a


def _pyd_width(kind, sizes):
    """Bytes a field of ``kind`` occupies in its record. None if unknown."""
    base = kind.split("<", 1)[0]
    if base == "dict":
        return 16  # data pointer, then the entry count. [V] section 3.
    if base == "list":
        return 8
    if base.startswith("vec"):
        try:
            n = int(base[3:])
        except ValueError:
            return None
        return n * (8 if kind.endswith("<f64>") else 4)
    if base.startswith("struct:"):
        return sizes.get(base[len("struct:"):])
    return _PYD_WIDTH.get(base, 8 if base == "unknown" else None)


def _pyd_span(fields, sizes):
    end = 0
    for f in fields:
        w = _pyd_width(f.kind, sizes)
        end = max(end, f.offset + (8 if w is None else w))
        if f.word is not None:
            end = max(end, f.word + 4)
    return end


def _pyd_align(fields, sizes):
    """Widest scalar, capped at 8, floor 4 -- `pyd_schema._align`, verbatim."""
    a = 4
    for f in fields:
        w = _pyd_width(f.kind, sizes)
        w = 8 if w is None else w
        if f.kind.startswith("vec"):
            w = 8 if f.kind.endswith("<f64>") else 4
        a = max(a, min(w, 8))
    return a


def _value_align(kind, schema):
    """Alignment of a container *element* or dict *value* of ``kind``."""
    if kind.startswith("struct:"):
        return schema.aligns.get(kind[len("struct:"):], 8)
    if kind.startswith("vec"):
        return 8 if kind.endswith("<f64>") else 4
    w = _pyd_width(kind, schema.sizes)
    return min(w or 8, 8)


#: Presence bits `pyd-schemas.tsv` misses, as ``{table: {(owner, field):
#: (bit, word)}}``; ``owner`` is ``'row'`` or ``'struct:<name>'``, and
#: ``word`` is relative like the TSV's. **[V]** each one: found by
#: `CfsdDecoder`'s presence audit as a bit set in the data that no field
#: claims, and assigned by the generator's own rule -- bits go
#: **alphabetically** over the gated fields, carrying into the next word after
#: ``0x80000000`` (`docs/re/dogma.md` section 12) -- which leaves exactly one
#: candidate field for each. With them applied the audit is clean for these
#: owners. `spacecomponentsbytype`'s three are the ones that matter -- the
#: three components whose names end a word's alphabetical run, so the
#: extractor never saw their ``0x80000000`` test -- because without the bit
#: every type reads, e.g., an ``entityBounty`` struct of zeros.
CFSD_PRESENCE_FIXES = {
    "archetypes": {("row", "titleID"): (0x8, 32)},
    "groupgraphics": {("row", "typeIDs"): (0x8, 64)},
    "spacecomponentsbytype": {
        ("row", "entityBounty"): (0x80000000, 1808),
        ("row", "proximityDisplacement"): (0x80000000, 1812),
        ("row", "underConstruction"): (0x80000000, 1816),
        ("struct:cameraLimitation", "minZoom"): (0x8, 20),
    },
}

#: The same fixes for the former reference, build 3619879 (and 3628513, which
#: shares its layout). Since the re-pin to 3662168 the table above is 3662168's
#: and this one is the older loader's: its `spaceComponentsByTypeLoader.pyd`
#: still had the component ``tagSubscriber`` (stride 1832, the presence words
#: at 1816/1820/1824/1828), so ``turboShield`` ends a word and
#: ``underConstruction`` does not. 3662168 dropped ``tagSubscriber`` (stride
#: 1824, the words move down 8, every gated field after it drops one bit).
#: **[V]** by re-extraction: `npc/components.tsv` has the same 28,280 rows and
#: every per-component count equal in both builds (`r-port-diff` E1). The other
#: two tables' loaders are identical.
CFSD_PRESENCE_FIXES_3619879 = {
    "archetypes": CFSD_PRESENCE_FIXES["archetypes"],
    "groupgraphics": CFSD_PRESENCE_FIXES["groupgraphics"],
    "spacecomponentsbytype": {
        ("row", "entityBounty"): (0x80000000, 1816),
        ("row", "proximityDisplacement"): (0x80000000, 1820),
        ("row", "turboShield"): (0x80000000, 1824),
        ("struct:cameraLimitation", "minZoom"): (0x8, 20),
    },
}

#: **The loader layout of each client build**, ``{build: (overlay, fixes)}``.
#: ``overlay`` is a `findings/` TSV whose tables *replace* the same tables of
#: `pyd-schemas.tsv` (None: the base file as it is), and ``fixes`` is that
#: build's presence-bit table. A build absent here is **refused**
#: (`layout_for_build`), never read with another build's offsets: the schema
#: guard sees stride and offsets but not `CFSD_PRESENCE_FIXES`, and a stale
#: fix extracts without an error into a wrong bundle.
#:
#: To add a build (the 11-09 freeze): regenerate only the tables whose loader
#: differs in `findings/build-delta-*.tsv` into ``findings/pyd-schemas-<build>.tsv``
#: (never overwrite the hand-corrected ``dungeons .../messageTextID`` row),
#: recheck every fix by hand, and add one line here. At a re-pin the new build's
#: tables replace the base rows and the old ones move to an overlay (3619879's
#: is ``pyd-schemas-3619879.tsv``). 3628513 shares 3619879's layout.
CFSD_BUILD_LAYOUTS = {
    3619879: ("pyd-schemas-3619879.tsv", CFSD_PRESENCE_FIXES_3619879),
    3628513: ("pyd-schemas-3619879.tsv", CFSD_PRESENCE_FIXES_3619879),
    3662168: (None, CFSD_PRESENCE_FIXES),
}

#: The build `pyd-schemas.tsv` itself is the layout of, and what a caller
#: with no client (a test, a doc tool) gets.
CFSD_REFERENCE_BUILD = 3662168


def layout_for_build(build):
    """``(overlay, fixes)`` for ``build`` (int or str); None is the reference.

    Raises `FsdError` for a build with no entry in `CFSD_BUILD_LAYOUTS`.
    """
    if build is None:
        build = CFSD_REFERENCE_BUILD
    try:
        key = int(build)
    except (TypeError, ValueError):
        key = None
    if key not in CFSD_BUILD_LAYOUTS:
        raise FsdError(
            "no cFSD loader layout for client build %s (known: %s): its "
            "spacecomponentsbytype stride and presence bits may differ, so "
            "it is refused rather than read with another build's offsets. "
            "Add an entry to CFSD_BUILD_LAYOUTS (see its comment)"
            % (build, ", ".join(str(b) for b in sorted(CFSD_BUILD_LAYOUTS))))
    return CFSD_BUILD_LAYOUTS[key]


def install_build(root):
    """The ``build =`` line of ``<root>/start.ini`` as an int, or None."""
    try:
        with open(os.path.join(root, "start.ini"), "r", encoding="utf-8",
                  errors="replace") as fh:
            for line in fh:
                k, _, v = line.partition("=")
                if k.strip() == "build" and v.strip().isdigit():
                    return int(v.strip())
    except OSError:
        pass
    return None


def client_layout_build(client):
    """The build ``client`` is, for layout selection; None if it has no
    readable start.ini (then the reference layout, as before)."""
    return install_build(client.root) if getattr(client, "root", None) else None


def parse_pyd_schemas(lines, fixes=None):
    """``{table: PydSchema}`` from the lines of a `pyd-schemas.tsv`.

    ``fixes`` defaults to `CFSD_PRESENCE_FIXES`; pass ``{}`` for the TSV as
    written.
    """
    fixes = CFSD_PRESENCE_FIXES if fixes is None else fixes
    it = iter(lines)
    header = next(it, "")
    if not header.startswith("table\tfield\toffset\ttype"):
        raise FsdError("pyd-schemas.tsv: unexpected header %r" % header[:60])
    out = {}
    for line in it:
        parts = line.rstrip("\n").split("\t")
        if len(parts) != 6:
            continue
        table, field, offset, kind, _conf, how = parts
        sch = out.get(table)
        if sch is None:
            sch = out[table] = PydSchema(table)
        if field == "#record":
            for item in kind.split():
                key, _eq, val = item.partition("=")
                if key in ("stride", "delta"):
                    setattr(sch, key, None if val == "None" else int(val))
            continue
        if field == "-":
            sch.status = how.split(":", 1)[0]
            continue
        bit = word = None
        m = _PYD_OPTIONAL.search(how)
        if m:
            bit, word = int(m.group(1), 16), int(m.group(2))
        if field.startswith("struct:"):
            name, _slash, member = field[len("struct:"):].partition("/")
            sch.structs.setdefault(name, []).append(
                PydField(member, int(offset), kind, bit, word))
        else:
            sch.fields.append(PydField(field, int(offset), kind, bit, word))
    for table, items in fixes.items():
        sch = out.get(table)
        if sch is None:
            continue
        for (owner, name), (bit, word) in items.items():
            fl = sch.fields if owner == "row" else sch.structs.get(owner[7:], ())
            for f in fl:
                if f.name == name and f.bit is None:
                    f.bit, f.word = bit, word
    for sch in out.values():
        sch.finish()
    return out


_PYD_CACHE = {}


def load_pyd_schemas(path=None, build=None):
    """``{table: PydSchema}`` from `findings/pyd-schemas.tsv`, cached.

    ``build`` picks the loader layout (`CFSD_BUILD_LAYOUTS`): that build's
    overlay tables replace the base ones and its presence fixes apply. None is
    the reference build. An unknown build raises `FsdError`.

    Generated, not hand-maintained: every name, offset and type comes from
    the client's own ``bin64/<table>Loader.pyd``. See
    `docs/re/native-loaders.md` for the method and its validation. The ten
    tables whose loader has no attribute chain come back with ``status``
    ``no-attributes``; `CFSD_BARE_SCHEMAS` covers the ones that are decoded.
    """
    overlay, fixes = layout_for_build(build)
    path = os.path.abspath(path or PYD_SCHEMA_TSV)
    key = (path, overlay, id(fixes))
    if key not in _PYD_CACHE:
        with open(path, "r", encoding="utf-8") as fh:
            lines = fh.readlines()
        if overlay is not None:
            with open(os.path.join(os.path.dirname(path), overlay), "r",
                      encoding="utf-8") as fh:
                extra = fh.readlines()[1:]
            names = {ln.split("\t", 1)[0] for ln in extra}
            lines = lines[:1] + [ln for ln in lines[1:]
                                 if ln.split("\t", 1)[0] not in names] + extra
        _PYD_CACHE[key] = parse_pyd_schemas(lines, fixes=fixes)
    return _PYD_CACHE[key]


#: The tables whose loader has **no attribute chain** -- the row is a bare
#: value, so `pyd-schemas.tsv` has nothing to say -- with the value's kind and
#: record offset. Read off the data, not the loader, so the confidence is
#: stated per entry. ``delta`` follows the generator's rule: 4 when the value
#: is 4 bytes or narrower, 8 when it holds a pointer.
#:
#: - ``agenttypes`` **[V]**: a string at ``+8`` -- the 13 values are the
#:   agent-type names (``NonAgent``, ``BasicAgent``, ``ResearchAgent``,
#:   ``CONCORDAgent``, ...), keyed 1..13.
#: - ``compressibletypes`` **[V]**: an ``i32`` at ``+4``, the compressed
#:   variant's typeID; key and value both close into ``types``.
#: - ``requiredskillsfortypes`` **[V]**: a ``dict<i32,i32>`` of
#:   ``{skillTypeID: level}`` at ``+8`` with its entry count at ``+16`` -- the
#:   shape `tools/dogma.py` reads (`docs/re/dogma.md` section 6). The row is
#:   the dict itself, as ``loader[typeID]`` returns it.
#:
#: Not decoded, and `unknown`: ``requiredskillsindex``,
#: ``challengeconditions``, ``challengeevents``, ``manufacturableitems`` and
#: the three ``ship_skin_design_*`` tables. None is on a content path.
CFSD_BARE_SCHEMAS = {
    "agenttypes": ("str", 8, 16),
    "compressibletypes": ("i32", 4, 8),
    "requiredskillsfortypes": ("dict<i32,i32>", 8, 24),
}


def bare_schema(stem):
    """A `PydSchema` for a `CFSD_BARE_SCHEMAS` table, or None."""
    spec = CFSD_BARE_SCHEMAS.get(stem)
    if spec is None:
        return None
    kind, off, stride = spec
    sch = PydSchema(stem)
    sch.stride = stride
    sch.delta = off
    sch.bare = PydField("value", off, kind)
    return sch.finish()


class CfsdUnknown:
    """A value the layout does not describe well enough to decode.

    Where it occurs (**[V]**, from the TSV): the ``dict<str,unknown>``
    ``assetManipulations`` fields of ``dungeons``, and the element type of
    every ``dict<str,list>``/``dict<str,dict>`` and the one bare ``unknown``
    (``objectivechains`` ``contextParameterTrigger.value``). ``count`` is the
    container's length where it can be read, else None.
    """

    __slots__ = ("kind", "offset", "count")

    def __init__(self, kind, offset, count=None):
        self.kind = kind
        self.offset = offset
        self.count = count

    def __eq__(self, other):
        return (isinstance(other, CfsdUnknown) and other.kind == self.kind
                and other.count == self.count)

    def __hash__(self):
        return hash((self.kind, self.count))

    def __repr__(self):
        return "CfsdUnknown(%r, count=%r)" % (self.kind, self.count)


def _split_kv(inner):
    """``'str,struct:x'`` -> ``('str', 'struct:x')``, at the top-level comma."""
    depth = 0
    for i, ch in enumerate(inner):
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
        elif ch == "," and depth == 0:
            return inner[:i], inner[i + 1:]
    raise FsdError("malformed dict kind %r" % inner)


#: Fields the TSV types as a container but which occupy **32 bytes** of their
#: record -- four times a list's 8 -- so they are not what the TSV says.
#: Every one is named ``value`` or ``defaultValue`` and sits in a
#: parameter-like struct: **[I]** a tagged variant (int, float, string or
#: list), whose tag and arms this lane did not decode. Found mechanically --
#: a container field whose room up to the next field or presence word exceeds
#: its width rounded to 8 -- and the only container fields that rule flags.
#: Read as the TSV says, `atoms` and `nodegraphs` return ASCII as integers or
#: run off the payload. They decode as `CfsdUnknown('variant')`.
CFSD_VARIANT_FIELDS = {
    "atoms": {("struct:parameter", "defaultValue")},
    "dungeonobjectives": {("struct:unnamed_18405745329423081530", "defaultValue")},
    "nodegraphs": {("struct:unnamed_8643665391442253824", "value")},
    "nodetypes": {("struct:unnamed_8985353217801030564", "defaultValue")},
    "objectivechains": {("struct:contextParameterTrigger", "value")},
}


class CfsdDecoder:
    """Decode values of a `PydSchema` out of one `CfsdTable`'s payload.

    Every offset is payload-relative. Pointers are payload-relative
    ``uint64``s; a pointed-to vector has its ``uint64`` length at
    ``pointer - 8`` (**[V]**, `CfsdTable` docstring).

    A **dict** field is 16 bytes: a pointer to ``bucketCount`` bucket
    pointers (count at ``-8``), then the entry count. A bucket is an array of
    entries (count at ``-8``); an entry is a ``uint32`` key, padding up to the
    value's alignment, then the value inline (**[V]** `cfsd_dict`,
    `dogma.Table._dict`). The key is a ``uint32`` or an 8-byte string
    pointer, and which one is decided from the data -- see `dict`.

    ``path`` names where a value sits (``row.rooms``,
    ``struct:<name>.objects``); it keys `key_modes` and
    `CFSD_VARIANT_FIELDS`.
    """

    #: Refuse a vector longer than this: a wild pointer, not data.
    MAX_COUNT = 1 << 22

    def __init__(self, table, schema):
        self.t = table
        self.pl = table.pl
        self.n = len(table.pl)
        self.schema = schema
        self.variants = CFSD_VARIANT_FIELDS.get(schema.table, ())
        #: ``{field path: {'int': n, 'str': n, 'int-hash': n, ...}}`` -- how
        #: each dict field's key width was decided; see `dict`.
        self.key_modes = {}
        #: ``{owner: [presence words read, words with a stray bit,
        #: stray bits...]}``.
        self.presence = {}
        #: ``{field path: '<key>/<value>'}`` -- the majority verdict, imposed
        #: on a second pass when a field's decisive verdicts disagree.
        self.forced = {}

    # -- primitives ----------------------------------------------------------

    def _u64(self, o):
        return _U64.unpack_from(self.pl, o)[0]

    def _vector(self, p):
        """``(first, count)`` for an arena pointer, or ``(None, 0)``."""
        if not 8 <= p <= self.n:
            return None, 0
        n = self._u64(p - 8)
        if n > self.MAX_COUNT:
            return None, 0
        return p, n

    def string(self, o):
        p = self._u64(o)
        first, n = self._vector(p)
        if first is None or first + n > self.n:
            return None
        return bytes(self.pl[first:first + n]).decode("utf-8", "replace")

    def wstring(self, o):
        # **[V]** for `dungeons` room names: the length word counts UTF-16
        # code units, not bytes -- `docs/lanes/fsd-generic.md`.
        p = self._u64(o)
        first, n = self._vector(p)
        if first is None or first + 2 * n > self.n:
            return None
        return bytes(self.pl[first:first + 2 * n]).decode("utf-16-le", "replace")

    # -- values --------------------------------------------------------------

    def value(self, kind, o, path=None):
        """The value of ``kind`` stored at payload offset ``o``."""
        if kind == "i32":
            return _I32.unpack_from(self.pl, o)[0]
        if kind == "f32":
            return _F32.unpack_from(self.pl, o)[0]
        if kind == "f64":
            return _F64.unpack_from(self.pl, o)[0]
        if kind == "u8":
            return self.pl[o]
        if kind == "str":
            return self.string(o)
        if kind == "i64":
            return _I64.unpack_from(self.pl, o)[0]
        if kind == "u64":
            return self._u64(o)
        if kind == "u16":
            return struct.unpack_from("<H", self.pl, o)[0]
        if kind == "wstr":
            return self.wstring(o)
        if kind.startswith("struct:"):
            return self.struct(kind[len("struct:"):], o)
        if kind.startswith("list<"):
            return self.list(kind[5:-1], o, path)
        if kind.startswith("dict<"):
            return self.dict(kind, o, path)
        if kind.startswith("vec"):
            n = int(kind[3])
            fmt = "<%dd" % n if kind.endswith("<f64>") else "<%df" % n
            return struct.unpack_from(fmt, self.pl, o)
        if kind in ("list", "dict", "unknown"):
            # A container whose element type the loader did not state.
            p = self._u64(o)
            return CfsdUnknown(kind, o, self._vector(p)[1] if kind != "unknown" else None)
        raise FsdError("%s: no reader for kind %r" % (self.schema.table, kind))

    def fields(self, fields, base, owner="row"):
        """``{name: value}`` for ``fields`` at ``base``; absent optionals None.

        Also audits the presence words: a set bit that no field claims is
        counted in ``presence[owner]`` as ``[words read, words with a stray
        bit, each stray bit set...]``. A struct read at the wrong stride lands its presence word on
        some other field's bytes, so this is the cheapest witness that a
        **computed** nested size is right.
        """
        row = {}
        pl = self.pl
        masks = {}
        for f in fields:
            if f.word is not None:
                masks[f.word] = masks.get(f.word, 0) | f.bit
        if masks:
            acc = self.presence.setdefault(owner, [0, 0])
            for word, mask in masks.items():
                acc[0] += 1
                extra = _U32.unpack_from(pl, base + word)[0] & ~mask
                if extra:
                    acc[1] += 1
                    acc.append(extra)
        for f in fields:
            if f.bit is not None and not (_U32.unpack_from(pl, base + f.word)[0] & f.bit):
                row[f.name] = None
                continue
            if (owner, f.name) in self.variants:
                row[f.name] = CfsdUnknown("variant", base + f.offset)
                continue
            row[f.name] = self.value(f.kind, base + f.offset, owner + "." + f.name)
        return row

    def struct(self, name, o):
        fl = self.schema.structs.get(name)
        if not fl:
            return CfsdUnknown("struct:" + name, o)
        return self.fields(fl, o, "struct:" + name)

    def elem_stride(self, kind):
        """Bytes between consecutive list elements of ``kind``. **[I]** for structs."""
        if kind.startswith("struct:"):
            size = self.schema.sizes.get(kind[len("struct:"):])
            if size is None:
                raise FsdError("%s: no size for %s" % (self.schema.table, kind))
            return size
        w = _pyd_width(kind, self.schema.sizes)
        if w is None:
            raise FsdError("%s: no width for %s" % (self.schema.table, kind))
        return w

    def list(self, elem, o, path=None):
        first, n = self._vector(self._u64(o))
        if first is None:
            return []
        if elem in ("list", "dict", "unknown"):
            return CfsdUnknown("list<%s>" % elem, o, n)
        stride = self.elem_stride(elem)
        if first + n * stride > self.n:
            raise FsdError("%s: list<%s> at %d runs past the payload"
                           % (self.schema.table, elem, o))
        return [self.value(elem, first + i * stride, path) for i in range(n)]

    def dict_entry(self, vkind, key_width=4):
        """``(value offset in entry, entry stride)``. **[I]** for structs.

        The entry is the key, padding up to the value's alignment, then the
        value; the stride is that rounded up to the larger of the two
        alignments. ``key_width`` is 4 for an integer key, 8 for a string.
        """
        if vkind in ("list", "dict", "unknown"):
            return None, None
        al = max(key_width, _value_align(vkind, self.schema))
        at = _align_up(key_width, al)
        if vkind.startswith("struct:"):
            w = self.schema.sizes.get(vkind[len("struct:"):])
        else:
            w = _pyd_width(vkind, self.schema.sizes)
        if w is None:
            return None, None
        return at, _align_up(at + w, al)

    def _key_string(self, e, allow_empty=False):
        """The string an 8-byte dict/record key points at, or None.

        An empty string is refused unless ``allow_empty``: a zero word sits
        before half the arena, so "points at an empty string" says nothing.
        """
        p = self._u64(e)
        if not 8 <= p < self.n:
            return None
        n = self._u64(p - 8)
        if not (0 if allow_empty else 1) <= n <= 1024 or p + n > self.n:
            return None
        try:
            s = bytes(self.pl[p:p + n]).decode("utf-8")
        except UnicodeDecodeError:
            return None
        if not s.isprintable():
            return None
        return s

    def _buckets(self, buckets, nb, stride):
        """``([(bucket, first entry, count)], packed pairs)``, or None.

        None when a bucket runs past the payload or into the next bucket's
        count word -- the arena never overlaps two vectors. ``packed`` counts
        the consecutive bucket pairs where the next bucket's count word sits
        exactly at the end of this one's entries (rounded to 8): the arena
        usually writes a dict's buckets back to back, so the right entry
        stride packs and a wrong one does not. **[V]** `npccorporations`
        ``corporationTrades``: 19,777 of 19,777 pairs pack at the true
        stride 16 and 8,021 of 19,777 at 8.
        """
        out = []
        for b in range(nb):
            ptr = self._u64(buckets + 8 * b)
            if not ptr:
                continue
            bp, cnt = self._vector(ptr)
            if bp is None or bp + cnt * stride > self.n:
                return None
            out.append((b, bp, cnt))
        packed = 0
        order = sorted(out, key=lambda x: x[1])
        for (_b, bp, cnt), (_b2, nxt, _c2) in zip(order, order[1:]):
            end = bp + cnt * stride
            if end > nxt - 8:
                return None
            if _align_up(end, 8) == nxt - 8:
                packed += 1
        return out, packed

    #: Value kinds the TSV cannot tell apart: the box is ``PyFloat_FromDouble``
    #: for a ``float`` and a ``double`` alike, so a dict the TSV calls
    #: ``dict<..,f64>`` may hold ``f32``s. **[V]** `npccorporations`
    #: ``exchangeRates`` does (``{1000010: 0.8, ...}``, 8-byte entries that
    #: pack), while ``corporationTrades`` really is ``f64``.
    _VALUE_ALTERNATES = {"f64": ("f64", "f32")}

    def _doubles_sane(self, cand):
        _m, _vk, at, stride, bl, _p = cand
        for _b, bp, cnt in bl:
            for i in range(cnt):
                v = _F64.unpack_from(self.pl, bp + i * stride + at)[0]
                if v != v or (v != 0.0 and abs(v) < 1e-300) or abs(v) > 1e300:
                    return False
        return True

    def _dict_candidates(self, vkind, buckets, nb, allow_empty):
        """Every ``(key, value kind, at, stride, buckets, packed)`` that fits."""
        cands = []
        for mode, kw in (("int", 4), ("str", 8)):
            for vk in self._VALUE_ALTERNATES.get(vkind, (vkind,)):
                at, stride = self.dict_entry(vk, kw)
                if at is None:
                    return None
                got = self._buckets(buckets, nb, stride)
                if got is None:
                    continue
                bl, packed = got
                ok = True
                for b, bp, cnt in bl:
                    for i in range(cnt):
                        e = bp + i * stride
                        if mode == "int":
                            ok = nb == 1 or _U32.unpack_from(self.pl, e)[0] % nb == b
                        else:
                            ok = self._key_string(e, allow_empty) is not None
                        if not ok:
                            break
                    if not ok:
                        break
                if ok:
                    cands.append((mode, vk, at, stride, bl, packed))
        return cands

    def dict(self, kind, o, path=None):
        """``{key: value}`` for a dict field at ``o``.

        **The entry layout is decided from the data, per dict**, because the
        TSV cannot state it:

        - *the key's width.* The TSV writes ``dict<str,...>`` for every dict
          (each generated ``mp_subscript`` imports ``PyString_AsString``), but
          `dungeons.rooms` is keyed by integer room ids and
          `nodegraphs.nodes` by GUID strings. An integer key is a ``uint32``
          and, with more than one bucket, must hash as the container's own
          keys do, ``key % bucketCount == bucket``; a string key is an 8-byte
          arena pointer to a printable string.
        - *a float value's width*: see `_VALUE_ALTERNATES`.

        Every candidate must fit (`_buckets`), and a ``f64`` reading that
        yields denormals, NaN or huge values is dropped while an ``f32`` one
        fits. Among several left, the one whose buckets pack best wins, then
        the verdict this field reached when only one candidate fitted, then a
        string key over an integer one, then the TSV's own value kind. Each
        decision is counted in ``key_modes`` (``'<key>/<value>'``, with a
        ``!`` copy for the decisive ones) so a field that flips is visible.
        """
        _kt, vkind = _split_kv(kind[5:-1])
        buckets, nb = self._vector(self._u64(o))
        if buckets is None or nb == 0:
            return {}
        if vkind in ("list", "dict", "unknown"):
            return CfsdUnknown(kind, o, self._u64(o + 8))
        forced = self.forced.get(path or kind)
        cands = self._dict_candidates(vkind, buckets, nb, forced is not None)
        if not cands and forced is None:
            # Last resort before giving up: an empty-string key.
            cands = self._dict_candidates(vkind, buckets, nb, True)
        if cands is None:
            return CfsdUnknown(kind, o, self._u64(o + 8))
        if not cands:
            raise FsdError("%s: %s at %d: no entry layout fits"
                           % (self.schema.table, kind, o))
        # A float read at the wrong width is not subtle: a 4-byte float read
        # as a double is a denormal (5.97e-315). Drop such a reading while a
        # narrower one fits.
        if len({c[1] for c in cands}) > 1:
            sane = [c for c in cands if c[1] != "f64" or self._doubles_sane(c)]
            cands = sane or cands
        seen = self.key_modes.setdefault(path or kind, {})
        if forced is not None:
            cands = [c for c in cands if "%s/%s" % (c[0], c[1]) == forced] or cands
        decisive = len(cands) == 1
        if not decisive:
            best = max(c[5] for c in cands)
            cands = [c for c in cands if c[5] == best]
        if len(cands) > 1:
            hist = [c for c in cands if seen.get("%s/%s!" % (c[0], c[1]))]
            if len(hist) == 1:
                cands = hist
        if len(cands) > 1:
            # A key that resolves to a length-prefixed printable string is a
            # string: an integer id almost never does, while a pointer's low
            # half passes the integer hash test by chance whenever the dict
            # has few buckets (`effects`, `objectivechains`).
            cands = [c for c in cands if c[0] == "str"] or cands
            cands = [c for c in cands if c[1] == vkind] or cands
        mode, vk, at, stride, bl, _p = cands[0]
        tag = "%s/%s" % (mode, vk)
        seen[tag] = seen.get(tag, 0) + 1
        if decisive:
            seen[tag + "!"] = seen.get(tag + "!", 0) + 1
        out = {}
        for _b, bp, cnt in bl:
            for i in range(cnt):
                e = bp + i * stride
                key = (_I32.unpack_from(self.pl, e)[0] if mode == "int"
                       else self._key_string(e, True))
                out[key] = self.value(vk, e + at, path)
        return out

    # -- rows ------------------------------------------------------------------

    def row(self, rec):
        if self.schema.bare is not None:
            return self.value(self.schema.bare.kind, rec + self.schema.bare.offset)
        return self.fields(self.schema.fields, rec)


def cfsd_generic_rows(table, schema, limit=None, decoder=None):
    """``{key: row}`` for a `CfsdTable`, decoded with a `PydSchema`.

    A row is ``{CCP field name: value}`` in the TSV's field order; for a
    `CFSD_BARE_SCHEMAS` table it is the bare value. Values: ``i32``, ``u8``
    and ``i64`` come back as ints (``u8`` is not turned into a bool, and
    ``i32`` is signed -- the binary states neither, `native-loaders.md`
    section 6), ``f32``/``f64`` as floats, strings as ``str``, vectors as
    tuples, lists as lists, dicts as dicts, nested structs as dicts. An absent
    optional field is ``None``, which is what the client's getter returns.

    The loader's stride must equal the container's: a disagreement raises,
    because a wrong layout decodes garbage silently. When the container could
    not measure its own stride (too few multi-row buckets), the loader's is
    used after checking it against every row's key.
    """
    if schema.status != "ok" and schema.bare is None:
        raise FsdError("%s: %s" % (schema.table, schema.status))
    if not table.ok:
        raise FsdError("%s: container not understood (%s)" % (table.name, table.why))
    want = schema.stride
    if want is None:
        raise FsdError("%s: the loader states no stride" % schema.table)
    dec = decoder or CfsdDecoder(table, schema)
    recs = list(cfsd_records(table, want, decoder=dec))
    if limit:
        recs = recs[:limit]
    out = {key: dec.row(rec) for key, rec in recs}
    # A dict field whose verdicts disagree is decoded again with the majority
    # of its *decisive* verdicts imposed. **[V]** one instance in this build:
    # `objectivechains` `taskOverrides`, 4 string-keyed against 1 that only
    # an integer reading fitted -- because its key is the empty string.
    redo = {}
    for path, seen in dec.key_modes.items():
        votes = {k[:-1]: v for k, v in seen.items() if k.endswith("!")}
        tags = [k for k in seen if not k.endswith("!")]
        if votes and (len(votes) > 1 or len(tags) > 1):
            redo[path] = max(sorted(votes), key=votes.get)
    if redo:
        dec.forced.update(redo)
        dec.key_modes.clear()
        out = {key: dec.row(rec) for key, rec in recs}
    return out


def cfsd_records(table, stride, decoder=None):
    """``(key, record offset)`` for every row, given the loader's stride.

    **Two key shapes.** Most tables key on a ``uint32`` at record ``+0``, and
    then the container's own measured stride must equal the loader's -- a
    disagreement raises, because a wrong layout decodes garbage silently --
    or, where the data could not measure one, the loader's must satisfy the
    hash invariant on every row.

    **[V] Seven tables key on a string** -- ``contenttags``,
    ``contenttagtypes``, ``contributionmethods``, ``effects``,
    ``explosionids``, ``huecategories`` and ``traveleffects``: record ``+0``
    is an 8-byte arena pointer to the key's text (``huecategories`` ->
    ``'royal'``, ``'cheese'``), and the bucket is a string hash this module
    does not reproduce. Those are accepted when every record's key resolves
    to a printable string and every bucket's records fit before the next
    bucket's data; the key comes back as ``str``. The stride is rounded up to
    8 there, because the key is a pointer.
    """
    if not table.ok:
        raise FsdError("%s: container not understood (%s)" % (table.name, table.why))
    got = table.stride
    if got is not None and got != stride:
        raise FsdError("%s: loader says stride %d, container says %d"
                       % (table.name, stride, got))
    if got is None and table._stride_consistent(stride):
        table._stride = stride
        got = stride
    if got is not None:
        yield from table.records()
        return
    dec = decoder or CfsdDecoder(table, PydSchema(table.name))
    # An 8-byte pointer key makes the record 8-aligned, which the loader's
    # stride rule (floor 4, for a uint32 key) does not know: **[V]**
    # `contenttagtypes` is stated as 12 and its records are 16 apart.
    stride = _align_up(stride, 8)
    ends = table.offsets[1:] + [len(table.pl)]
    found = []
    for o, e, c in zip(table.offsets, ends, table.counts):
        if o + c * stride > e:
            raise FsdError("%s: loader stride %d does not fit the container"
                           % (table.name, stride))
        for i in range(c):
            r = o + i * stride
            k = dec._key_string(r)
            if k is None:
                raise FsdError("%s: loader stride %d does not fit the "
                               "container, and the keys are not strings"
                               % (table.name, stride))
            found.append((k, r))
    yield from found


def schema_for(stem, path=None, build=None):
    """The `PydSchema` for a table stem: the TSV's, or a bare one, or None.

    ``build`` selects the loader layout (`load_pyd_schemas`)."""
    stem = stem.lower()
    sch = load_pyd_schemas(path, build).get(stem)
    if sch is not None and sch.status == "ok" and sch.fields:
        return sch
    return bare_schema(stem)


def rows(client, stem, limit=None, schema_path=None):
    """``{key: row}`` for any cFSD table with a layout, by file stem.

    ``fsd.rows(client, 'npccorporations')[1000126]['tickerName']``. The
    layout comes from `findings/pyd-schemas.tsv` (or `CFSD_BARE_SCHEMAS`);
    see `cfsd_generic_rows` for the value conventions.
    """
    sch = schema_for(stem, schema_path, client_layout_build(client))
    if sch is None:
        raise FsdError("%s: no recovered layout (see CFSD_BARE_SCHEMAS)" % stem)
    table = CfsdTable(client.path("res:/staticdata/%s.fsdbinary" % stem), stem)
    return cfsd_generic_rows(table, sch, limit=limit)


# --- cross-checking the generic decoder against the hand-written paths ------


def same_value(a, b):
    """Equality across the two vocabularies, `native-loaders.md` section 5.2.

    The hand paths say ``u32``/``msgid``/``bool`` where the loader says
    ``i32``/``u8``; the binary does not distinguish them. So ints compare
    modulo 2**32, a bool against its byte, NaN against NaN, and containers
    element by element. Nothing else is forgiven.
    """
    if a is None or b is None:
        return a is None and b is None
    if isinstance(a, bool) or isinstance(b, bool):
        return bool(a) == bool(b)
    if isinstance(a, int) and isinstance(b, int):
        return a == b or (a - b) % (1 << 32) == 0
    if isinstance(a, float) and isinstance(b, float):
        return a == b or (a != a and b != b)
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(same_value(a[k], b[k]) for k in a)
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
        return len(a) == len(b) and all(same_value(x, y) for x, y in zip(a, b))
    return a == b


def _by_offset(fields, base=0):
    return {base + f.offset: f for f in fields}


def _generic_at(sch, row, off):
    """``(field path, value, optional?)`` of the scalar at record ``off``.

    Descends into an inline nested struct, which is how `CFSD_COLUMNS`'
    ``missions`` ``dungeonID @+144`` is the generic ``killMission.dungeonID``
    (``+140`` and ``+4``). None when no field starts there.
    """
    for f in sch.fields:
        if f.offset == off:
            return f.name, row[f.name], f.bit is not None
        if f.kind.startswith("struct:"):
            size = sch.sizes.get(f.kind[7:], 0)
            if f.offset < off < f.offset + size:
                sub = row[f.name]
                for g in sch.structs.get(f.kind[7:], ()):
                    if f.offset + g.offset == off:
                        val = None if sub is None else sub[g.name]
                        return ("%s.%s" % (f.name, g.name), val,
                                f.bit is not None or g.bit is not None)
    return None


def _xcheck_columns(client, stem, out):
    """`cfsd_rows` (``CFSD_COLUMNS``) against the generic rows, by offset."""
    path = client.path("res:/staticdata/%s.fsdbinary" % stem)
    hand = cfsd_rows(CfsdTable(path, stem), stem)
    sch = schema_for(stem, build=client_layout_build(client))
    gen = cfsd_generic_rows(CfsdTable(path, stem), sch)
    stats = {"cells": 0, "equal": 0, "absent": 0, "absent0": 0,
             "skipped": set(), "bad": []}
    if hand.keys() != gen.keys():
        stats["bad"].append(("keys", len(hand), len(gen)))
    for key, hrow in hand.items():
        grow = gen.get(key, {})
        for off, col in CFSD_COLUMNS[stem].items():
            name, kind = col[0], col[1]
            if kind in ("listc", "listd"):
                stats["skipped"].add(name)
                continue
            got = (None, key, False) if off == 0 else _generic_at(sch, grow, off)
            if got is None:
                stats["skipped"].add(name)
                continue
            _gname, g, optional = got
            h = hrow[name]
            stats["cells"] += 1
            if same_value(h, g):
                stats["equal"] += 1
            elif g is None and optional:
                # The hand path ignores this table's presence bits and reads
                # the raw slot; the generic one returns None, as the client.
                stats["absent"] += 1
                stats["absent0"] += h in (0, "", None)
            elif len(stats["bad"]) < 5:
                stats["bad"].append((key, name, h, g))
    out("  %-24s cfsd_rows  %7d cells, %7d equal, %5d None-vs-raw (%d of "
        "them raw 0), %d bad%s"
        % (stem, stats["cells"], stats["equal"], stats["absent"],
           stats["absent0"], len(stats["bad"]),
           ("; not compared: " + ", ".join(sorted(stats["skipped"])))
           if stats["skipped"] else ""))
    for b in stats["bad"]:
        out("      MISMATCH %r" % (b,))
    return not stats["bad"]


def _xcheck_dogma(client, stem, out):
    """`dogma.Table` (``dogma.SCHEMAS``) against the generic rows, by offset."""
    dogma = _import_dogma()
    spec = dogma.SCHEMAS[stem]
    note = ""
    # A ``dict`` field with no ``entry`` spec makes `dogma.Table` raise
    # (`npccharacters.skills`): compare the rest, and say so.
    broken = [f[0] for f in spec["fields"]
              if f[2] == "dict" and f[0] not in spec.get("entry", {})]
    if broken:
        spec = dict(spec, fields=[f for f in spec["fields"] if f[0] not in broken])
        note = "; dogma.Table raises on %s, compared without it" % ", ".join(broken)
    hand = dict(dogma.Table(client, stem, spec).rows())
    sch = schema_for(stem, build=client_layout_build(client))
    path = client.path("res:/staticdata/%s.fsdbinary" % stem)
    gen = cfsd_generic_rows(CfsdTable(path, stem), sch)
    bad = []
    cells = equal = 0
    names = set()
    moved = {}
    if hand.keys() != gen.keys():
        bad.append(("keys", len(hand), len(gen)))
    if sch.bare is not None:
        # The row *is* the dict; dogma wraps it with its entry count.
        for key, hrow in hand.items():
            cells += 2
            equal += same_value(hrow["skills"], gen[key])
            equal += hrow["skillCount"] == len(gen[key])
            if not same_value(hrow["skills"], gen[key]) and len(bad) < 5:
                bad.append((key, hrow["skills"], gen[key]))
    else:
        fmap = _by_offset(sch.fields)
        for key, hrow in hand.items():
            for name, off, kind, _bit in spec["fields"]:
                if kind == "ref":
                    continue  # dogma does not follow it; the generic does
                f = fmap.get(spec["struct_at"] + off)
                if f is None:
                    if kind == "u64":
                        continue  # a dict's entry count: not a getter
                    # The hand layout puts a field where the loader has none:
                    # compare by name and report the rows that differ.
                    g = gen[key].get(name, "missing")
                    m = moved.setdefault(name, [spec["struct_at"] + off, 0, 0])
                    m[1] += 1
                    m[2] += not same_value(hrow[name], g)
                    continue
                if f.name != name:
                    names.add((name, f.name))
                cells += 1
                ok = same_value(hrow[name], gen[key][f.name])
                if not ok and isinstance(hrow[name], list):
                    # dogma wraps a scalar element in a one-field struct
                    # (`schematics.pins` -> ``{'typeID': n}``); the loader
                    # returns the bare scalar.
                    ok = same_value(
                        [next(iter(e.values())) if isinstance(e, dict)
                         and len(e) == 1 else e for e in hrow[name]],
                        gen[key][f.name])
                equal += ok
                if not ok and len(bad) < 5:
                    bad.append((key, name, hrow[name], gen[key][f.name]))
    out("  %-24s dogma.Table %7d cells, %7d equal, %d bad%s%s"
        % (stem, cells, equal, len(bad),
           ("; renamed: " + ", ".join("%s->%s" % p for p in sorted(names)))
           if names else "", note))
    for b in bad:
        out("      MISMATCH %s" % repr(b)[:300])
    for name, (off, n, diff) in sorted(moved.items()):
        out("      OFFSET %s: dogma reads record +%d where the loader has no "
            "field; by name, %d of %d rows differ" % (name, off, diff, n))
    return not bad and not any(m[2] for m in moved.values())


def _xcheck_rooms(client, out):
    """`dungeon_rooms` (the corrected dict path) against the generic rooms."""
    path = client.path("res:/staticdata/dungeons.fsdbinary")
    t = CfsdTable(path, "dungeons")
    gen = cfsd_generic_rows(CfsdTable(path, "dungeons"), schema_for("dungeons"))
    n = bad = 0
    for key in t.keys():
        hand = sorted((rid, sorted(types)) for rid, types in dungeon_rooms(t, key))
        rooms = gen[key]["rooms"] or {}
        mine = sorted(
            (rid, sorted(o["typeID"] for o in (room["objects"] or {}).values()))
            for rid, room in rooms.items()
        )
        n += 1
        if hand != mine:
            bad += 1
    out("  %-24s dungeon_rooms %d dungeons, %d differ" % ("dungeons", n, bad))
    return not bad


def _xcheck_locations(client, out):
    """`graphic_locations` against the generic rows."""
    path = client.path("res:/staticdata/graphiclocations.fsdbinary")
    hand = graphic_locations(CfsdTable(path, "graphiclocations"))
    gen = cfsd_generic_rows(CfsdTable(path, "graphiclocations"),
                            schema_for("graphiclocations"))
    bad = sum(not same_value(hand[k], {n: gen[k][n] for n in hand[k]})
              for k in hand)
    out("  %-24s graphic_locations %d rows, %d differ"
        % ("graphiclocations", len(hand), bad))
    return not bad


def _import_dogma():
    """`tools/dogma.py`, imported late: it imports this module."""
    here = os.path.dirname(os.path.abspath(__file__))
    if here not in sys.path:
        sys.path.append(here)  # last, behind the stdlib, as at the top
    import dogma  # noqa: PLC0415

    return dogma


def generic_crosscheck(client, out=print):
    """Every hand-written decode path against the generic one. True if equal.

    ``python3 -I tools/fsd.py generic-check <client>`` runs it;
    `docs/lanes/fsd-generic.md` records the result.
    """
    dogma = _import_dogma()
    ok = True
    out("hand path -> generic, row for row:")
    for stem in sorted(CFSD_COLUMNS):
        ok &= _xcheck_columns(client, stem, out)
    ok &= _xcheck_rooms(client, out)
    ok &= _xcheck_locations(client, out)
    for stem in sorted(dogma.SCHEMAS):
        ok &= _xcheck_dogma(client, stem, out)
    return ok


def cfsd_attribute_names(client, resource):
    """Attribute names for a cFSD table, taken from its native loader.

    **[V]** Each ``.fsdbinary`` table has a generated C extension at
    ``$EVE_CLIENT/bin64/<table>Loader.pyd`` which the table's Python wrapper
    assigns to ``__loader__`` (``fsdBuiltData/common/base.pyc``; e.g.
    ``paperdoll/common/npc.pyc`` -> ``paperdollsLoader``). The generated code
    resolves attributes by name, so the names are present in the module as
    plain ASCII.

    This returns the identifier-shaped strings found in that ``.pyd`` minus
    the ones that belong to the CRT, the Python 2.7 C API and the generated
    container boilerplate. It is a *name list*, not a schema: there is no
    order, no type and no offset in it. Treat it as a hint, mark it **[I]**,
    and do not present it as a column list.
    """
    stem = resource.rsplit("/", 1)[-1]
    stem = stem[: -len(".fsdbinary")]
    pyd = _find_loader_pyd(client.bin64, stem)
    if pyd is None:
        return None
    with open(pyd, "rb") as fh:
        blob = fh.read()
    out = set()
    cur = bytearray()
    for b in blob:
        if 0x30 <= b <= 0x39 or 0x41 <= b <= 0x5A or 0x61 <= b <= 0x7A or b == 0x5F:
            cur.append(b)
            continue
        if b == 0 and len(cur) >= 3:
            s = cur.decode("ascii")
            # Keep only lowerCamel/snake identifiers. Everything the MSVC
            # mangler and the CRT contribute starts with a capital or is all
            # caps (`QEBADXZ`, `IEAAPEADXZ`, `GCTL`), and CCP's attribute
            # names never do.
            if (s[0].islower() or s[0] == "_") and any(c.islower() for c in s):
                out.add(s)
        cur = bytearray()
    return sorted(out - _CFSD_BOILERPLATE)


def _find_loader_pyd(bin64, stem):
    if not bin64 or not os.path.isdir(bin64):
        return None
    want = (stem + "loader.pyd").lower()
    for name in os.listdir(bin64):
        if name.lower() == want:
            return os.path.join(bin64, name)
    return None


# Names every generated loader carries: CRT, Python 2.7 C API, Win32, and the
# mapping/iterator boilerplate the generator emits for each table.
_CFSD_BOILERPLATE = frozenset(
    """
    DisableThreadLibraryCalls GetCurrentProcess GetCurrentProcessId
    GetCurrentThreadId GetModuleHandleW GetStartupInfoW GetSystemTimeAsFileTime
    InitializeSListHead IsDebuggerPresent IsProcessorFeaturePresent
    QueryPerformanceCounter RtlCaptureContext RtlLookupFunctionEntry
    RtlVirtualUnwind SetUnhandledExceptionFilter TerminateProcess
    UnhandledExceptionFilter VirtualQuery
    _callnewh _cexit _cexit_ _configure_narrow_argv _crt_atexit
    _crt_at_quick_exit _execute_onexit_table _initialize_narrow_environment
    _initialize_onexit_table _initterm _initterm_e
    _invalid_parameter_noinfo_noreturn _seh_filter_dll _register_onexit_function
    assembly manifestVersion requestedExecutionLevel requestedPrivileges
    security standalone trustInfo urn version xmlns asInvoker uiAccess false
    true
    fclose fopen_s fread_s free fseek ftell malloc memcpy memmove memset
    realloc strcmp strlen terminate
    bad_alloc bad_array_new_length exception type_info allocator basic_ios
    basic_iostream basic_istream basic_ostream basic_streambuf basic_string
    basic_stringbuf basic_stringstream char_traits ios_base std
    Py_InitModule4_64 PyArg_ParseTuple PyArg_ParseTupleAndKeywords
    PyBool_FromLong PyCObject_AsVoidPtr PyDict_New PyDict_SetItem
    PyDict_SetItemString PyErr_Clear PyErr_Format PyErr_NoMemory
    PyErr_Occurred PyErr_SetNone PyErr_SetObject PyErr_SetString
    PyExc_AttributeError PyExc_IOError PyExc_IndexError PyExc_KeyError
    PyExc_MemoryError PyExc_RuntimeError PyExc_StopIteration PyExc_TypeError
    PyExc_ValueError PyFloat_AsDouble PyFloat_FromDouble PyInt_AsLong
    PyInt_AsSsize_t PyInt_FromLong PyInt_FromSize_t PyList_Append PyList_New
    PyList_SetItem PyLong_AsLongLong PyLong_AsUnsignedLong
    PyLong_FromLongLong PyLong_FromUnsignedLong PyLong_FromUnsignedLongLong
    PyMem_Free PyMem_Malloc PyModule_AddObject PyObject_GenericGetAttr
    PyObject_GetAttrString PyObject_Init PyObject_IsTrue PyObject_Repr
    PyObject_SelfIter PyObject_Str PySequence_Fast PyString_AsString
    PyString_FromFormat PyString_FromString PyString_FromStringAndSize
    PyTuple_New PyTuple_SetItem PyTuple_Size PyType_IsSubtype PyType_Ready
    PyUnicodeUCS2_FromString PyUnicodeUCS2_FromStringAndSize
    Py_BuildValue Py_FatalError Py_InitModule4 _Py_NoneStruct
    python27 KERNEL32 api
    __C_specific_handler __CxxFrameHandler3 _CxxThrowException __dir__
    __getattr__ __init__ __len__ __repr__ __iter__ __module__ __name__
    __doc__ __dict__ __class__ __reduce__
    get has_key items iteritems iterkeys itervalues keys next load values
    Get GetIfExists GetData Load Keys Items
    _PyObject_New _Py_TrueStruct _Py_ZeroStruct _items_
    __std_exception_copy __std_exception_destroy __std_terminate
    __std_type_info_destroy_list
    allocation bss data rdata text pdata rsrc reloc
    dict dictIterator dictItemsIterator dictValuesIterator
    iterator listIterator value vector
    """.split()
)


# --- the client installation ----------------------------------------------


class Client:
    """A client installation: the resource index, plus loaders for it."""

    def __init__(self, root=None, resfiles=None):
        if root is None:
            root = os.environ.get("EVE_CLIENT") or os.path.expanduser(
                "~/.local/share/Steam/steamapps/compatdata/8500/pfx/"
                "drive_c/CCP/EVE/tq"
            )
        self.root = os.path.abspath(root)
        if resfiles is None:
            resfiles = self._find_resfiles()
        self.resfiles = resfiles
        self.bin64 = os.path.join(self.root, "bin64")
        self.index = self._read_index()

    def _find_resfiles(self):
        """Locate the ``ResFiles`` cache directory.

        **[V]** It is a *sibling* of the install directory
        (``.../CCP/EVE/ResFiles`` beside ``.../CCP/EVE/tq``), so a container
        that bind-mounts only ``$EVE_CLIENT`` cannot see it. ``EVE_RESFILES``
        overrides, and ``tools/analysis.sh`` mounts the parent at
        ``/client-root``, which is the last candidate tried.
        """
        cands = [
            os.environ.get("EVE_RESFILES"),
            os.path.join(os.path.dirname(self.root), "ResFiles"),
            "/client-root/ResFiles",
        ]
        for c in cands:
            if c and os.path.isdir(c):
                return c
        return os.path.join(os.path.dirname(self.root), "ResFiles")

    def _read_index(self):
        """Parse ``resfileindex.txt``. **[V]** 5 columns, see module docstring."""
        path = os.path.join(self.root, "resfileindex.txt")
        out = {}
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                parts = line.strip().split(",")
                if len(parts) < 5:
                    continue
                out[parts[0]] = {
                    "cache": parts[1],
                    "md5": parts[2],
                    "size": int(parts[3]) if parts[3].isdigit() else -1,
                    "csize": int(parts[4]) if parts[4].isdigit() else -1,
                }
        return out

    def staticdata(self):
        """Every ``res:/staticdata/*`` resource name, sorted."""
        return sorted(k for k in self.index if k.startswith("res:/staticdata/"))

    def path(self, resource):
        """Resolve a resource name to a file on disk."""
        try:
            entry = self.index[resource]
        except KeyError:
            raise FsdError("no such resource: %s" % resource) from None
        return os.path.join(self.resfiles, entry["cache"])

    def kind(self, resource):
        """What this resource is, by extension and then by magic.

        One of ``'classic'`` (embedded schema), ``'classic-sidecar'`` (schema
        in a sibling ``.schema``), ``'sqlite'``, ``'cfsd'``, ``'pickle'``,
        ``'schema'``, ``'other'``.
        """
        low = resource.lower()
        if low.endswith(CFSD_EXTENSIONS):
            return "cfsd"
        if low.endswith(PICKLE_EXTENSIONS):
            return "pickle"
        if low.endswith(SCHEMA_EXTENSIONS):
            return "schema"
        if low.endswith(CLASSIC_EXTENSIONS):
            with open(self.path(resource), "rb") as fh:
                head = fh.read(16)
            if head == SQLITE_MAGIC:
                return "sqlite"
            if self.sidecar(resource) is not None:
                return "classic-sidecar"
            return "classic"
        return "other"

    def sidecar(self, resource):
        """The ``.schema`` resource beside a ``.static`` one, or None."""
        cand = resource[: -len(".static")] + ".schema"
        return cand if cand in self.index else None

    def load(self, resource):
        """Load a static-data resource.

        ``.static`` (FSD, with or without a sidecar schema, or SQLite) and
        ``.pickle`` decode. ``.schema`` comes back parsed. ``.fsdbinary``
        raises `FsdError`: the format is native, see the module docstring.
        """
        path = self.path(resource)
        kind = self.kind(resource)
        if kind == "classic":
            return load_classic(path)
        if kind == "classic-sidecar":
            return load_classic(path, self.schema(resource))
        if kind == "sqlite":
            return load_sqlite(path)
        if kind == "pickle":
            return load_pickle(path)
        if kind == "schema":
            return parse_simple_yaml(self._text(path))
        if kind == "cfsd":
            raise FsdError(
                "%s is a cFSD (.fsdbinary) file; decode it with "
                "fsd.rows(client, stem). See docs/lanes/fsd-generic.md."
                % resource
            )
        raise FsdError("do not know how to load %s" % resource)

    @staticmethod
    def _text(path):
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()

    def schema(self, resource):
        """The optimized schema of a ``.static`` resource, embedded or sidecar."""
        kind = self.kind(resource)
        if kind == "classic":
            return read_classic_schema(self.path(resource))
        if kind == "classic-sidecar":
            return parse_simple_yaml(self._text(self.path(self.sidecar(resource))))
        raise FsdError("%s has no FSD schema" % resource)


# --- description helpers used by fsd_dump.py -------------------------------


def describe(schema, depth=0, maxdepth=3):
    """One-line-per-field rendering of an optimized schema node."""
    out = []
    pad = "  " * depth
    t = schema.get("type")
    if t == "dict":
        kt = schema.get("keyTypes", {}).get("type")
        flags = []
        if schema.get("buildIndex"):
            flags.append("buildIndex")
        if schema.get("multiIndex"):
            flags.append("multiIndex")
        out.append("%sdict[%s]%s" % (pad, kt, (" " + ",".join(flags)) if flags else ""))
        if depth < maxdepth:
            out += describe(schema["valueTypes"], depth + 1, maxdepth)
    elif t == "list":
        out.append(
            "%slist%s"
            % (pad, " fixedItemSize" if "fixedItemSize" in schema else "")
        )
        if depth < maxdepth:
            out += describe(schema["itemTypes"], depth + 1, maxdepth)
    elif t == "object":
        out.append("%sobject" % pad)
        for name in sorted(schema["attributes"]):
            sub = schema["attributes"][name]
            st = sub.get("type")
            extra = []
            if "isOptional" in sub:
                extra.append("optional")
            if "default" in sub:
                extra.append("default=%r" % (sub["default"],))
            label = "%s  %s: %s%s" % (
                pad,
                name,
                st,
                (" (" + ", ".join(extra) + ")") if extra else "",
            )
            out.append(label)
            if st in ("object", "list", "dict") and depth + 1 < maxdepth:
                sub_node = sub
                out += describe(sub_node, depth + 2, maxdepth)
    elif t == "union":
        out.append("%sunion of %d" % (pad, len(schema.get("optionTypes", ()))))
    elif t == "enum":
        vals = sorted(schema.get("values", {}), key=lambda k: schema["values"][k])
        out.append("%senum{%s}" % (pad, ",".join(vals[:12])))
    else:
        out.append("%s%s" % (pad, t))
    return out


def column_types(schema):
    """``[(name, type)]`` for a dict-of-objects table, else ``None``."""
    if schema.get("type") != "dict":
        return None
    v = schema["valueTypes"]
    if v.get("type") != "object":
        return None
    return [(n, v["attributes"][n].get("type")) for n in sorted(v["attributes"])]


if __name__ == "__main__" and sys.argv[1:2] == ["generic-check"]:
    # `python3 -I tools/fsd.py generic-check [client root]`: the generic
    # decoder against every hand path. Exit status 1 on any difference.
    sys.exit(0 if generic_crosscheck(Client(*sys.argv[2:3])) else 1)

if __name__ == "__main__":  # a smoke test, not a CLI; see fsd_dump.py
    c = Client(*sys.argv[1:2])
    names = c.staticdata()
    print("%d staticdata resources under %s" % (len(names), c.root))
    for n in names:
        k = c.kind(n)
        if k == "cfsd":
            continue
        try:
            t = c.load(n)
            if isinstance(t, dict) and "_connection" in t:
                t = {kk: vv for kk, vv in t.items() if kk != "_connection"}
                rep = "sqlite %s" % sorted(t)
            elif isinstance(t, dict):
                rep = "%s n=%d" % (type(t).__name__, len(t))
            else:
                rep = repr(t)
            print("  %-12s %-50s %s" % (k, n.split("/")[-1], rep[:90]))
        except Exception as exc:  # noqa: BLE001
            print("  %-12s %-50s FAILED %s" % (k, n.split("/")[-1], exc))
