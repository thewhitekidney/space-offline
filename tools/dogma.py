#!/usr/bin/env python3
"""Reader for the client's dogma tables: the attribute-and-effect system.

Five ``.fsdbinary`` (cFSD) tables plus two that come off the same machinery:

===========================  ======  ==========================================
table                        rows    what it is
===========================  ======  ==========================================
``typedogma``                27,160  per-``typeID`` attribute values and effects
``dogmaattributes``           2,871  attribute definitions
``dogmaeffects``              3,427  effect definitions, incl. ``modifierInfo``
``dogmaattributecategories``     37  attribute grouping, for the UI
``dogmaunits``                   60  display units
``requiredskillsfortypes``    9,573  ``{typeID: {skillTypeID: level}}``
``marketgroups``              2,114  the market tree (bonus; same machinery)
``npccharacters``            11,424  agents and NPC pilots (bonus)
``types``                    53,288  every type: mass, volume, group, published
``schematics``                   68  the PI production recipes
``globaltypes``                   8  planet type -> resource distribution
===========================  ======  ==========================================

Every column below was **read out of the client's own generated code** --
``$EVE_CLIENT/bin64/<table>Loader.pyd`` -- and then confirmed against data.
See ``docs/re/dogma.md`` for how, and for what each column is for. Section 12
of that file adds the rule that makes a derived layout *checkable*: the
generated struct orders its fields by size class and then alphabetically, and
the presence bits are alphabetical and contiguous from ``0x1``, so a field
this reader missed would leave a visible gap in one of the two.

The record model, corrected
---------------------------
``tools/fsd.py`` decoded the cFSD *container*: a hash map on
``key % bucketCount`` of fixed-size records, plus a per-bucket
variable-length arena. What it could not say was how a record is laid out.
It is this, and `derive_schema` reads it straight out of the loader:

**[V]** A record is ``uint32 key``, then padding to the row struct's own
alignment, then the generated C++ row struct. The struct's alignment is 8 when
it holds any pointer or ``double`` and 4 otherwise, so the struct starts at
record ``+8`` for most tables and at ``+4`` for an all-``int32`` table like
``marketgroups``. ``8 + sizeof(struct)`` (or ``4 + ...``) equals the stride
``fsd.CfsdTable.stride`` finds, for every table here -- which is how the model
was checked rather than assumed.

**[V]** Inside the struct: pointers first, then ``int32``/``float``, then
``bool``, then one ``uint32`` **presence bitfield**. A field with a presence
bit reads as ``None`` when the bit is clear; the generated getter returns
``Py_None`` in exactly that case, so ``None`` here means what it means in the
client.

**[V]** A pointer field holds a payload-relative offset to the *first element*
with the element count in the ``uint64`` eight bytes *before* it. That is the
same convention `fsd.CfsdTable.string` already uses for strings. Note this is
off by eight from `fsd.cfsd_list`, which takes the slot to point at the count
and the elements to follow: both read the same bytes, but the loader's own
arithmetic is ``elements = p``, ``count = *(uint64*)(p - 8)``, and that is what
is implemented here.

**[V]** A *dict* field is a nested hash map: the pointer leads to an array of
``bucketCount`` bucket pointers (count at ``-8``), and each bucket is an array
of fixed-size entries (count at ``-8``). ``requiredskillsfortypes`` uses
8-byte entries, ``{uint32 key, uint32 value}``, and carries the dict's total
entry count in the record slot after the pointer.

Stdlib only. Read-only: nothing here writes to the client. `derive_schema`
needs ``pefile`` and ``capstone``, which are in the analysis image; everything
else needs neither.

Usage
-----
::

    tools/analysis.sh python3 tools/dogma.py --client /client \\
        --resfiles /client-root/ResFiles <cmd>

    schema [table]        the column layout, from the table below
    derive <table>        re-read the layout out of the loader .pyd
    attributes [-n N]     dogmaattributes rows
    effects [-n N]        dogmaeffects rows, with modifierInfo
    type <typeID>         one type's attributes, effects and required skills
    skills <typeID>       one type's required skills
    shape <table>         the TSV shape line used by findings/corpus-*.tsv
    validate              every closure and cross-check in docs/re/dogma.md

    --client DIR          which install (no default; or $EVE_CLIENT)
    --build N | any       the build it must be (default: the pinned one)

Flags work on either side of the subcommand -- they did not, see
`resolve_client` and `main`. Every run names on stderr which install and
which build it read, and refuses a build other than the pinned one (3662168)
unless ``--build`` says otherwise. There is no default install.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import os
import struct
import sys

# `tools/` itself is already on the path -- the rotation above put it last,
# behind the stdlib, which is the whole point. See that comment.

import fsd  # noqa: E402

_U32 = struct.Struct("<I")
_I32 = struct.Struct("<i")
_I64 = struct.Struct("<q")
_U64 = struct.Struct("<Q")
_F32 = struct.Struct("<f")
_F64 = struct.Struct("<d")


# --- the schemas ----------------------------------------------------------
#
# ``fields`` is ``(name, struct-relative offset, kind, presence bit or None)``.
# Kinds:
#   u32   uint32
#   i32   int32   (``operation`` in modifierInfo really does use -1)
#   f32   float
#   f64   double
#   bool  one byte, 0/1
#   str   pointer to length-prefixed bytes (length in the uint64 at ptr-8)
#   msgid uint32 global localisation message id -- resolve with loctable
#   list  pointer to an element array; see ``element``
#   dict  pointer to a nested hash map; see ``entry``
#
# ``struct_at`` is where the row struct begins inside the record, and
# ``size`` is ``struct_at + sizeof(struct)``, i.e. the stride the container
# reports. Both are **[V]**: see the module docstring.

SCHEMAS = {
    "dogmaattributes": {
        "struct_at": 8,
        "presence": 0x44,
        "size": 80,
        "fields": [
            ("description", 0x00, "str", 0x4),
            ("name", 0x08, "str", None),
            ("attributeID", 0x10, "u32", None),
            ("categoryID", 0x14, "u32", 0x1),
            ("chargeRechargeTimeID", 0x18, "u32", 0x2),
            ("dataType", 0x1C, "u32", None),
            ("defaultValue", 0x20, "f32", None),
            ("displayNameID", 0x24, "msgid", 0x8),
            ("iconID", 0x28, "u32", 0x20),
            ("maxAttributeID", 0x2C, "u32", 0x40),
            ("minAttributeID", 0x30, "u32", 0x80),
            ("tooltipDescriptionID", 0x34, "msgid", 0x100),
            ("tooltipTitleID", 0x38, "msgid", 0x200),
            ("unitID", 0x3C, "u32", 0x400),
            ("displayWhenZero", 0x40, "bool", 0x10),
            ("highIsGood", 0x41, "bool", None),
            ("published", 0x42, "bool", None),
            ("stackable", 0x43, "bool", None),
        ],
    },
    "dogmaeffects": {
        "struct_at": 8,
        "presence": 0x64,
        "size": 112,
        "fields": [
            ("effectName", 0x00, "str", None),
            ("guid", 0x08, "str", 0x80),
            ("modifierInfo", 0x10, "list", 0x200),
            ("sfxName", 0x18, "str", 0x4000),
            ("descriptionID", 0x20, "msgid", 0x1),
            ("dischargeAttributeID", 0x24, "u32", 0x2),
            ("displayNameID", 0x28, "msgid", 0x4),
            ("distribution", 0x2C, "u32", 0x8),
            ("durationAttributeID", 0x30, "u32", 0x10),
            ("effectCategory", 0x34, "u32", None),
            ("effectID", 0x38, "u32", None),
            ("falloffAttributeID", 0x3C, "u32", 0x20),
            ("fittingUsageChanceAttributeID", 0x40, "u32", 0x40),
            ("iconID", 0x44, "u32", 0x100),
            ("npcActivationChanceAttributeID", 0x48, "u32", 0x400),
            ("npcUsageChanceAttributeID", 0x4C, "u32", 0x800),
            ("rangeAttributeID", 0x50, "u32", 0x1000),
            ("resistanceAttributeID", 0x54, "u32", 0x2000),
            ("trackingSpeedAttributeID", 0x58, "u32", 0x8000),
            ("disallowAutoRepeat", 0x5C, "bool", None),
            ("electronicChance", 0x5D, "bool", None),
            ("isAssistance", 0x5E, "bool", None),
            ("isOffensive", 0x5F, "bool", None),
            ("isWarpSafe", 0x60, "bool", None),
            ("propulsionChance", 0x61, "bool", None),
            ("published", 0x62, "bool", None),
            ("rangeChance", 0x63, "bool", None),
        ],
        "element": {
            "modifierInfo": {
                "stride": 48,
                "presence": 0x28,
                "fields": [
                    ("domain", 0x00, "str", None),
                    ("func", 0x08, "str", None),
                    ("effectID", 0x10, "u32", 0x1),
                    ("groupID", 0x14, "u32", 0x2),
                    ("modifiedAttributeID", 0x18, "u32", 0x4),
                    ("modifyingAttributeID", 0x1C, "u32", 0x8),
                    ("operation", 0x20, "i32", 0x10),
                    ("skillTypeID", 0x24, "u32", 0x20),
                ],
            }
        },
    },
    "typedogma": {
        "struct_at": 8,
        "presence": None,
        "size": 24,
        "fields": [
            ("dogmaAttributes", 0x00, "list", None),
            ("dogmaEffects", 0x08, "list", None),
        ],
        "element": {
            "dogmaAttributes": {
                "stride": 16,
                "presence": None,
                "fields": [
                    ("value", 0x00, "f64", None),
                    ("attributeID", 0x08, "u32", None),
                ],
            },
            "dogmaEffects": {
                "stride": 8,
                "presence": None,
                "fields": [
                    ("effectID", 0x00, "u32", None),
                    ("isDefault", 0x04, "bool", None),
                ],
            },
        },
    },
    "dogmaattributecategories": {
        "struct_at": 8,
        "presence": 0x10,
        "size": 32,
        "fields": [
            ("description", 0x00, "str", 0x1),
            ("name", 0x08, "str", None),
        ],
    },
    "dogmaunits": {
        "struct_at": 8,
        "presence": 0x10,
        "size": 32,
        "fields": [
            ("name", 0x00, "str", None),
            ("descriptionID", 0x08, "msgid", 0x1),
            ("displayNameID", 0x0C, "msgid", 0x2),
        ],
    },
    # requiredskillsfortypes has no attribute names in its loader at all,
    # because the row value is a bare dict rather than a generated object.
    # Its shape was read out of the dict template's index arithmetic and then
    # confirmed against typedogma; see docs/re/dogma.md section 6.
    "requiredskillsfortypes": {
        "struct_at": 8,
        "presence": None,
        "size": 24,
        "fields": [
            ("skills", 0x00, "dict", None),
            ("skillCount", 0x08, "u64", None),
        ],
        "entry": {"skills": {"stride": 8, "key": "u32", "value": "u32"}},
    },
    # --- bonus: same machinery, not dogma -------------------------------
    # types.fsdbinary, stride 152. Derived from typesLoader.pyd and
    # reconciled against the stride: the struct is 144 bytes (presence
    # bitfield at +140, rounded to the 8-byte alignment its f64s force) and
    # 8 + 144 = 152. **[V]**
    #
    # `tools/fsd.py`'s `CFSD_COLUMNS["types"]` found 15 of these by profiling
    # and every one of the 15 agrees with this, offset for offset, including
    # `portionSize` being an int64 rather than a u32.
    #
    # **The 4-byte "hole" at struct +0x40 (record +72) was `factionID`**
    # (2026-10-25, lane K1, `docs/lanes/fsd-generic.md`). On all 1,383 rows
    # with presence bit 0x20 set, +0x40 resolves into `factions` or
    # `npccorporations`, and +0x44 is zero on all 53,288 rows. Reading +0x44
    # returned 0 for every faction-owned type. The 4-byte slot at +0x44 is the
    # unnamed one now: it has no presence bit and no Python getter.
    "types": {
        "struct_at": 8,
        "presence": 0x8C,
        "size": 152,
        "fields": [
            ("basePrice", 0x00, "f64", 0x1),
            ("capacity", 0x08, "f64", 0x2),
            # A pointer, and deliberately not followed: the element layout is
            # unestablished and the field is set for almost nothing.
            ("designerIDs", 0x10, "ref", 0x10),
            ("mass", 0x18, "f64", 0x800),
            ("portionSize", 0x20, "i64", 0x4000),
            ("radius", 0x28, "f64", 0x80000),
            ("volume", 0x30, "f64", 0x800000),
            ("certificateTemplate", 0x38, "u32", 0x4),
            ("descriptionID", 0x3C, "msgid", 0x8),
            ("factionID", 0x40, "u32", 0x20),
            # 0x44: unnamed, always zero. See the note above.
            ("graphicID", 0x48, "u32", 0x40),
            ("groupID", 0x4C, "u32", None),
            ("iconID", 0x50, "u32", 0x80),
            ("isisGroupID", 0x54, "u32", 0x200),
            ("marketGroupID", 0x58, "u32", 0x400),
            ("metaGroupID", 0x5C, "u32", 0x1000),
            ("metaLevel", 0x60, "u32", 0x2000),
            ("quoteAuthorID", 0x64, "msgid", 0x10000),
            ("quoteID", 0x68, "msgid", 0x20000),
            ("raceID", 0x6C, "u32", 0x40000),
            ("soundID", 0x70, "u32", 0x100000),
            ("techLevel", 0x74, "u32", 0x200000),
            ("typeID", 0x78, "u32", None),
            ("typeNameID", 0x7C, "msgid", None),
            ("variationParentTypeID", 0x80, "u32", 0x400000),
            ("wreckTypeID", 0x84, "u32", 0x1000000),
            ("isDynamicType", 0x88, "bool", 0x100),
            ("published", 0x89, "bool", 0x8000),
        ],
    },
    # schematics.fsdbinary, stride 40, 68 rows: the PI production recipes.
    # Struct is 32 bytes and 8 + 32 = 40. **[V]**
    #
    # `pins` is a **list** of the facility type ids the schematic can run in
    # (count in the payload at pointer-8, eight of them for every row seen).
    # `types` is a **dict** of `{typeID: {quantity, isInput}}` and
    # `typeCount` is its total entry count -- the same "dict pointer, then
    # the count in the next slot" shape `requiredskillsfortypes` has.
    #
    # Checked against a published recipe rather than only against the stride:
    # key 84 reads inputs 2395 and 2396 at 40 each, output 3725 at 5, cycle
    # 3600s. That is a P2 schematic consuming two P1s, which is what EVE's P2
    # recipes are.
    "schematics": {
        "struct_at": 8,
        "presence": None,
        "size": 40,
        "fields": [
            ("pins", 0x00, "list", None),
            ("types", 0x08, "dict", None),
            ("typeCount", 0x10, "u64", None),
            ("cycleTime", 0x18, "u32", None),
            ("nameID", 0x1C, "msgid", None),
        ],
        "element": {
            "pins": {"stride": 4, "presence": None,
                     "fields": [("typeID", 0x00, "u32", None)]},
        },
        # A dict entry is `key, padding to the value struct's alignment, then
        # the struct` -- the record model one level down. The struct here is
        # all 4-byte, so the entry is 4 + 8 = 12 rather than 8 + 8.
        "entry": {
            "types": {
                "stride": 12,
                "key": "u32",
                "value": {
                    "at": 4,
                    "presence": None,
                    "fields": [("quantity", 0x00, "u32", None),
                               ("isInput", 0x04, "bool", None)],
                },
            },
        },
    },
    # globaltypes.fsdbinary, stride 24, **8 rows** -- and the name is the only
    # misleading thing about it. The eight keys are 11, 12, 13, 2014, 2015,
    # 2016, 2017 and 2063: **planet type ids**. Each row carries fifteen
    # `{resource typeID: weight}` pairs, which is the planetary-interaction
    # resource distribution, from the client rather than from a wiki.
    #
    # Struct is 16 bytes (dict pointer, then the entry count) and 8 + 16 = 24.
    # The count is 15 for all eight rows. **[V]**
    "globaltypes": {
        "struct_at": 8,
        "presence": None,
        "size": 24,
        "fields": [
            ("resourceDistribution", 0x00, "dict", None),
            ("resourceCount", 0x08, "u64", None),
        ],
        # 4-byte key, 4 bytes of padding, then an f64: alignment 8, so the
        # entry is 16.
        "entry": {
            "resourceDistribution": {"stride": 16, "key": "u32",
                                     "value": "f64", "at": 8},
        },
    },
    "marketgroups": {
        "struct_at": 4,
        "presence": 0x14,
        "size": 28,
        "fields": [
            ("descriptionID", 0x00, "msgid", 0x1),
            ("iconID", 0x04, "u32", 0x2),
            ("nameID", 0x08, "msgid", None),
            ("parentGroupID", 0x0C, "u32", 0x4),
            ("hasTypes", 0x10, "bool", None),
        ],
    },
    "npccharacters": {
        "struct_at": 8,
        "presence": 0x78,
        "size": 136,
        "fields": [
            ("agent", 0x00, "ref", 0x1),
            ("description", 0x20, "str", 0x8),
            ("skills", 0x28, "dict", 0x20),
            ("startDate", 0x30, "str", 0x80),
            ("ancestryID", 0x38, "u32", 0x2),
            ("bloodlineID", 0x3C, "u32", None),
            ("careerID", 0x40, "u32", 0x4),
            ("charisma", 0x44, "u32", None),
            ("corporationID", 0x48, "u32", None),
            ("intelligence", 0x4C, "u32", None),
            ("memory", 0x50, "u32", None),
            ("nameID", 0x54, "msgid", None),
            ("perception", 0x58, "u32", None),
            ("raceID", 0x5C, "u32", None),
            ("schoolID", 0x60, "u32", 0x10),
            ("specialityID", 0x64, "u32", 0x40),
            ("stationID", 0x68, "u32", 0x100),
            ("willpower", 0x6C, "u32", None),
            ("ceo", 0x70, "bool", None),
            ("deleted", 0x71, "bool", None),
            ("employmentRecord", 0x72, "bool", None),
            ("gender", 0x73, "bool", None),
            ("uniqueName", 0x74, "bool", None),
        ],
    },
}


# --- reading a record -----------------------------------------------------


class Table:
    """One cFSD table read with a `SCHEMAS` entry.

    ``rows()`` yields ``(key, dict)``. Message-id columns come back as plain
    integers; `Dogma.text` resolves them.
    """

    def __init__(self, client, stem, schema=None):
        self.stem = stem
        self.schema = schema or SCHEMAS[stem]
        self.cfsd = fsd.CfsdTable(
            client.path("res:/staticdata/%s.fsdbinary" % stem), stem
        )
        if not self.cfsd.ok:
            raise fsd.FsdError("%s: container not understood (%s)"
                               % (stem, self.cfsd.why))
        stride = self.cfsd.stride
        want = self.schema["size"]
        if stride != want:
            raise fsd.FsdError(
                "%s: schema says stride %d, container says %r -- the layout "
                "and the file disagree, so nothing here is trustworthy"
                % (stem, want, stride)
            )
        self.pl = self.cfsd.pl

    # -- primitives, all payload-relative ---------------------------------

    def _u32(self, o):
        return _U32.unpack_from(self.pl, o)[0]

    def _u64(self, o):
        return _U64.unpack_from(self.pl, o)[0]

    def _str(self, o):
        p = self._u64(o)
        if not 8 <= p < len(self.pl):
            return None
        n = self._u64(p - 8)
        if p + n > len(self.pl):
            return None
        return bytes(self.pl[p:p + n]).decode("utf-8", "replace")

    def _array(self, o):
        """``(first_element_offset, count)`` for a pointer field."""
        p = self._u64(o)
        if not 8 <= p < len(self.pl):
            return None, 0
        return p, self._u64(p - 8)

    def _scalar(self, kind, o):
        if kind in ("u32", "msgid"):
            return self._u32(o)
        if kind == "i32":
            return _I32.unpack_from(self.pl, o)[0]
        # `types.portionSize` is one: the getter is `PyLong_FromLongLong`.
        if kind == "i64":
            return _I64.unpack_from(self.pl, o)[0]
        if kind == "u64":
            return self._u64(o)
        if kind == "f32":
            return _F32.unpack_from(self.pl, o)[0]
        if kind == "f64":
            return _F64.unpack_from(self.pl, o)[0]
        if kind == "bool":
            return bool(self.pl[o])
        if kind == "str":
            return self._str(o)
        raise fsd.FsdError("unknown kind %r" % kind)

    def _fields(self, base, fields, presence):
        pres = self._u32(base + presence) if presence is not None else None
        out = {}
        for name, off, kind, bit in fields:
            if bit is not None and pres is not None and not pres & bit:
                out[name] = None
                continue
            if kind in ("list", "dict", "ref"):
                out[name] = (kind, base + off)
            else:
                out[name] = self._scalar(kind, base + off)
        return out

    def _elements(self, name, slot):
        spec = self.schema["element"][name]
        first, n = self._array(slot)
        if first is None:
            return []
        out = []
        for i in range(n):
            e = first + i * spec["stride"]
            out.append(self._fields(e, spec["fields"], spec["presence"]))
        return out

    def _dict(self, name, slot):
        """``{key: value}`` for a dict field.

        The value is a scalar kind, or a nested struct spec -- `schematics`
        has `{typeID: {quantity, isInput}}`, which the one-scalar form could
        not express. ``at`` is where the value starts inside the entry, which
        is the key's width rounded up to the value's alignment and so is 4 for
        a 4-byte value and 8 for an ``f64`` or a pointer; it defaults to 4,
        which is what every entry read before this had.
        """
        spec = self.schema["entry"][name]
        value = spec["value"]
        at = spec.get("at", 4)
        buckets, nb = self._array(slot)
        if buckets is None:
            return {}
        out = {}
        for b in range(nb):
            bp = self._u64(buckets + 8 * b)
            if not 8 <= bp < len(self.pl):
                continue
            for i in range(self._u64(bp - 8)):
                e = bp + i * spec["stride"]
                key = self._scalar(spec["key"], e)
                if isinstance(value, dict):
                    out[key] = self._fields(
                        e + value.get("at", at), value["fields"],
                        value["presence"])
                else:
                    out[key] = self._scalar(value, e + at)
        return out

    def rows(self, limit=None):
        at = self.schema["struct_at"]
        n = 0
        for key, rec in self.cfsd._locate():  # noqa: SLF001  (no public API yet)
            row = self._fields(rec + at, self.schema["fields"],
                               self.schema["presence"])
            for name, val in list(row.items()):
                if not isinstance(val, tuple):
                    continue
                kind, slot = val
                if kind == "list":
                    row[name] = self._elements(name, slot)
                elif kind == "dict":
                    row[name] = self._dict(name, slot)
                else:  # 'ref': a nested object this lane did not follow
                    row[name] = "unknown"
            yield key, row
            n += 1
            if limit and n >= limit:
                return

    def keys(self):
        return self.cfsd.keys()


# --- the corpus -----------------------------------------------------------


class Dogma:
    """Lazy access to the dogma tables of one client installation."""

    def __init__(self, client=None, resfiles=None):
        if isinstance(client, fsd.Client):
            self.client = client
        else:
            self.client = fsd.Client(client, resfiles)
        self._cache = {}
        self._msgs = None

    def table(self, stem):
        return Table(self.client, stem)

    def _dict_of(self, stem, key):
        if stem not in self._cache:
            self._cache[stem] = {k: v for k, v in self.table(stem).rows()}
        return self._cache[stem]

    @property
    def attributes(self):
        """``{attributeID: row}`` -- 2,871 attribute definitions."""
        return self._dict_of("dogmaattributes", "attributeID")

    @property
    def effects(self):
        """``{effectID: row}`` -- 3,427 effect definitions."""
        return self._dict_of("dogmaeffects", "effectID")

    @property
    def units(self):
        return self._dict_of("dogmaunits", "unitID")

    @property
    def attribute_categories(self):
        return self._dict_of("dogmaattributecategories", "categoryID")

    @property
    def type_dogma(self):
        """``{typeID: {'dogmaAttributes': [...], 'dogmaEffects': [...]}}``.

        27,160 rows and 648,726 attribute values, so this is ~0.4 GB of Python
        objects. `of_type` reads one row instead.
        """
        return self._dict_of("typedogma", "typeID")

    @property
    def required_skills(self):
        return self._dict_of("requiredskillsfortypes", "typeID")

    def attribute_names(self):
        """``{attributeID: name}`` -- cheap, and what you usually want."""
        return {k: v["name"] for k, v in self.attributes.items()}

    def type_names(self):
        """``{typeID: English name}``, for labelling skills and groups.

        Uses ``fsd.CFSD_COLUMNS['types']['typeNameID']``, which the
        `fsd-reader` lane confirmed against 53,288 resolving message ids.
        """
        if "typenames" not in self._cache:
            t = fsd.CfsdTable(
                self.client.path("res:/staticdata/types.fsdbinary"), "types")
            out = {}
            for key, rec in t._locate():  # noqa: SLF001
                mid = _U32.unpack_from(t.pl, rec + 132)[0]
                out[key] = self.text(mid, width=80) or mid
            self._cache["typenames"] = out
        return self._cache["typenames"]

    def of_type(self, type_id):
        """One type's dogma: ``{'attributes': {name: value}, ...}``.

        Attribute ids are resolved to CCP's own attribute names and effect ids
        to CCP's own effect names, because a bare id is not evidence of
        anything to a reader.
        """
        names = self.attribute_names()
        effnames = {k: v["effectName"] for k, v in self.effects.items()}
        attrs, effects = {}, []
        for key, row in self.table("typedogma").rows():
            if key != type_id:
                continue
            for e in row["dogmaAttributes"]:
                attrs[names.get(e["attributeID"], e["attributeID"])] = e["value"]
            for e in row["dogmaEffects"]:
                effects.append((effnames.get(e["effectID"], e["effectID"]),
                                e["isDefault"]))
            break
        else:
            raise KeyError(type_id)
        skills = {}
        for key, row in self.table("requiredskillsfortypes").rows():
            if key == type_id:
                skills = row["skills"]
                break
        return {"typeID": type_id, "attributes": attrs, "effects": effects,
                "requiredSkills": skills}

    # -- localisation -----------------------------------------------------

    @property
    def messages(self):
        """The en-us message table, or ``None`` if loctable is unavailable.

        ``tools/loctable.py`` owns the localisation corpus; this only consumes
        it, and resolves the resource through `fsd.Client` rather than through
        loctable's own root search, which assumes ``ResFiles`` is a sibling of
        the directory it was handed.
        """
        if self._msgs is None:
            try:
                import loctable
            except ImportError:
                return None
            res = "res:/localizationfsd/localization_fsd_en-us.pickle"
            try:
                path = self.client.path(res)
            except fsd.FsdError:
                return None
            self._msgs = loctable.load_lang("en-us", path=path)[1]
        return self._msgs

    def text(self, message_id, width=60):
        msgs = self.messages
        if not msgs or message_id not in msgs:
            return None
        e = msgs[message_id]
        t = e[0] if isinstance(e, (tuple, list)) else e
        return str(t).replace("\n", " ")[:width]


# --- re-deriving a schema from the loader ---------------------------------
#
# This is the part that makes the schemas above checkable rather than
# believable. It needs pefile and capstone (both in the analysis image).

_IDENT_SKIP = frozenset(
    "dll edata pdb pyd xdata zzzdbg bss text rdata data pdata rsrc reloc "
    "idata rtc std".split()
)


def derive_schema(pyd_path):
    """Read a cFSD table's row layout out of its generated loader. **[V]**

    Returns a list of structs, each a list of
    ``(name, struct offset, kind, presence bit, presence offset)``.

    How it works, and why it is sound. The generated ``.pyd`` resolves
    attributes with a ``tp_getattro`` that is a chain of ``strcmp`` against
    the attribute-name literals; each arm then loads the field and boxes it.
    Four things fall out of that code with no guessing:

    * the **name**, from the ``lea reg, [rip + x]`` that points at the literal;
    * the **offset**, from the displacement of the load that follows;
    * the **type**, from the load's operand size and form -- ``mov dword``
      is int32, ``movzx byte`` is bool, ``movss``/``movsd`` are float and
      double, a ``mov qword`` followed by ``mov rdx, [reg-8]`` is a string
      (the loader reading the length that sits before the data), and a ``lea``
      or a store of the field address into a new wrapper object is a list or
      dict;
    * the **presence bit**, from the ``test ptr [reg + P], BIT`` that guards
      the arm and jumps to the ``Py_None`` return.

    The struct pointer is identified by the function prologue, which is always
    ``mov <reg>, qword ptr [rcx + 0x10]`` -- loading ``self->data``. One
    struct is emitted per such prologue, so a table with list fields yields
    the row struct and one struct per element type.

    The generated ``__dir__`` tests every presence bit in turn against one
    offset; those groups are dropped, since all their "offsets" are the
    bitfield's.
    """
    import re

    import capstone
    import pefile

    pe = pefile.PE(pyd_path, fast_load=True)
    pe.parse_data_directories()
    base = pe.OPTIONAL_HEADER.ImageBase
    secs = [(s.Name.rstrip(b"\0").decode(), s.VirtualAddress + base,
             s.Misc_VirtualSize, s.get_data()) for s in pe.sections]
    strs = {}
    for name, start, _size, data in secs:
        if name not in (".rdata", ".data"):
            continue
        for m in re.finditer(rb"[\x20-\x7e]{2,}\x00", data):
            strs[start + m.start()] = m.group()[:-1].decode()

    imps = {}
    for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []) or []:
        for imp in entry.imports:
            if imp.name:
                imps[imp.address] = imp.name.decode()

    ident = re.compile(r"^[a-z_][A-Za-z0-9_]*$")
    mem = re.compile(r"\[(r[a-z0-9]+)(?: \+ (0x[0-9a-f]+|\d+))?\]$")
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    text = [s for s in secs if s[0] == ".text"][0]
    # Resync on undecodable bytes. `.text` is not all instructions -- some
    # loaders carry jump tables and alignment blobs in it, and capstone's
    # `disasm` stops dead at the first byte it cannot decode. Without this,
    # `missionsLoader.pyd` yields 731 instructions out of ~37,000 and the
    # extractor silently reports an empty schema.
    blob = bytes(text[3])
    ins, pos = [], 0
    while pos < len(blob):
        got = list(md.disasm(blob[pos:], text[1] + pos))
        if not got:
            pos += 1
            continue
        ins.extend(got)
        pos = (got[-1].address - text[1]) + got[-1].size
        if pos < len(blob):
            pos += 1  # the byte that stopped it

    groups, cur, datareg, pending = [], None, None, None
    bit = presoff = None
    for i, x in enumerate(ins):
        t = "%s %s" % (x.mnemonic, x.op_str)
        if x.mnemonic == "int3":
            continue
        m = re.match(r"mov (r[a-z0-9]+), qword ptr \[rcx \+ 0x10\]$", t)
        if m:
            datareg = m.group(1)
            cur = []
            groups.append(cur)
            pending = bit = presoff = None
            continue
        if cur is None:
            continue
        m = re.match(r"lea (r[a-z0-9]+), \[rip \+ (0x[0-9a-f]+)\]$", t)
        if m:
            tgt = x.address + x.size + int(m.group(2), 16)
            s = strs.get(tgt)
            if s and ident.match(s) and s not in _IDENT_SKIP:
                pending, bit, presoff = s, None, None
            continue
        if pending is None:
            continue
        m = re.match(
            r"test (?:byte|word|dword|qword) ptr \[%s \+ (0x[0-9a-f]+|\d+)\], "
            r"(0x[0-9a-f]+|\d+)$" % datareg, t)
        if m:
            presoff, bit = int(m.group(1), 0), int(m.group(2), 0)
            continue
        if re.match(r"mov qword ptr \[r[a-z0-9]+ \+ 0x10\], %s$" % datareg, t):
            cur.append((pending, 0, "ref", bit, presoff))
            pending = bit = presoff = None
            continue
        mm = mem.search(t)
        if not mm or mm.group(1) != datareg:
            continue
        off = int(mm.group(2), 0) if mm.group(2) else 0
        kind = None
        if x.mnemonic == "mov" and " dword ptr " in t:
            kind = "i32"
        elif x.mnemonic == "movzx" and " byte ptr " in t:
            kind = "bool"
        elif x.mnemonic == "movss":
            kind = "f32"
        elif x.mnemonic == "movsd":
            kind = "f64"
        elif x.mnemonic == "mov" and " qword ptr " in t:
            # Three cases, told apart by what the arm does next: reading the
            # length that precedes the bytes (a string), boxing with
            # PyLong_From* (a real 64-bit integer field), or neither (a
            # pointer to a list, dict or nested object).
            kind = "ptr"
            for j in range(i + 1, min(i + 7, len(ins))):
                nxt = "%s %s" % (ins[j].mnemonic, ins[j].op_str)
                if re.search(r"qword ptr \[r[a-z0-9]+ - 8\]$", nxt):
                    kind = "str"
                    break
                m2 = re.match(r"(?:jmp|call) qword ptr \[rip \+ (0x[0-9a-f]+)\]$",
                              nxt)
                if m2:
                    tgt = ins[j].address + ins[j].size + int(m2.group(1), 16)
                    if (imps.get(tgt) or "").startswith("PyLong_From"):
                        kind = "i64"
                    break
        elif x.mnemonic == "lea":
            kind = "ref"
        if kind:
            cur.append((pending, off, kind, bit, presoff))
            pending = bit = presoff = None

    out = []
    for fields in groups:
        if not fields:
            continue
        if len(fields) > 1 and len({f[1] for f in fields}) == 1:
            continue  # the generated __dir__
        seen = {}
        for f in fields:
            seen.setdefault(f[0], f)
        out.append(sorted(seen.values(), key=lambda f: f[1]))
    return out


def loader_path(client, stem):
    """``$EVE_CLIENT/bin64/<stem>Loader.pyd``, case-insensitively."""
    return fsd._find_loader_pyd(client.bin64, stem)  # noqa: SLF001


def client_constants(path=None):
    """``{name: int}`` from the client's own ``dogma/const.py``. **[V]**

    The second, independent source this lane validates names against: 1,341
    ``attributeXxx = N`` and 305 ``effectXxx = N`` module-level constants,
    extracted from the *bytecode*, not from FSD. Returns ``{}`` when
    ``out/pyc`` has not been unpacked, since that is a generated tree.
    """
    if path is None:
        here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        path = os.path.join(here, "out", "pyc", "dogma", "const.pyc")
    if not os.path.isfile(path):
        return {}
    try:
        import pyc27
    except ImportError:
        return {}
    co = pyc27.load_pyc(path)
    out, pend = {}, None
    for _off, name, arg, rep in pyc27.dis(co):
        if name == "LOAD_CONST":
            pend = co.consts[arg]
        elif name == "STORE_NAME" and pend is not None:
            if isinstance(pend, int) and not isinstance(pend, bool):
                out[rep] = pend
            pend = None
        else:
            pend = None
    return out


# --- validation -----------------------------------------------------------


def validate(d, out=print):
    """Every check `docs/re/dogma.md` claims, re-run against the client.

    Returns the number of failures. Each check is a closure into another
    table, a 100%-resolution test through the localisation corpus, or an
    agreement between two independently decoded tables -- not a spot check
    against values the layout was calibrated on.
    """
    fails = 0

    def ck(label, ok, detail=""):
        nonlocal fails
        if not ok:
            fails += 1
        out("%s %-58s %s" % ("ok  " if ok else "FAIL", label, detail))

    attrs = d.attributes
    ck("dogmaattributes: rows", len(attrs) == 2871, len(attrs))
    ck("dogmaattributes: attributeID == bucket key",
       all(k == v["attributeID"] for k, v in attrs.items()))
    cats = d.attribute_categories
    ck("dogmaattributes: categoryID closes into dogmaattributecategories",
       all(v["categoryID"] in cats for v in attrs.values()
           if v["categoryID"] is not None))
    units = d.units
    ck("dogmaattributes: unitID closes into dogmaunits",
       all(v["unitID"] in units for v in attrs.values()
           if v["unitID"] is not None))
    msgs = d.messages
    if msgs:
        ids = [v["displayNameID"] for v in attrs.values() if v["displayNameID"]]
        ck("dogmaattributes: displayNameID resolves (en-us)",
           all(i in msgs for i in ids), "%d ids" % len(ids))

    effs = d.effects
    ck("dogmaeffects: rows", len(effs) == 3427, len(effs))
    ck("dogmaeffects: effectID == bucket key",
       all(k == v["effectID"] for k, v in effs.items()))
    for col in ("dischargeAttributeID", "durationAttributeID",
                "falloffAttributeID", "rangeAttributeID",
                "resistanceAttributeID", "trackingSpeedAttributeID",
                "fittingUsageChanceAttributeID",
                "npcActivationChanceAttributeID", "npcUsageChanceAttributeID"):
        vals = [v[col] for v in effs.values() if v[col]]
        ck("dogmaeffects: %s closes into dogmaattributes" % col,
           all(v in attrs for v in vals), "%d values" % len(vals))
    mods = [m for v in effs.values() for m in v["modifierInfo"] or []]
    ck("dogmaeffects: modifierInfo entries", len(mods) == 5178, len(mods))
    ck("dogmaeffects: modifier attribute ids close into dogmaattributes",
       all(m[c] in attrs for m in mods
           for c in ("modifiedAttributeID", "modifyingAttributeID")
           if m[c] is not None))
    ck("dogmaeffects: modifier domain vocabulary",
       {m["domain"] for m in mods} <= {"shipID", "charID", "structureID",
                                       "itemID", "targetID", "otherID",
                                       "target"},
       sorted({m["domain"] for m in mods}))
    ck("dogmaeffects: modifier func vocabulary",
       {m["func"] for m in mods} <= {"ItemModifier", "LocationModifier",
                                     "LocationGroupModifier",
                                     "LocationRequiredSkillModifier",
                                     "OwnerRequiredSkillModifier",
                                     "EffectStopper"},
       sorted({m["func"] for m in mods}))

    types = fsd.CfsdTable(
        d.client.path("res:/staticdata/types.fsdbinary"), "types")
    tkeys = set(types.keys())
    groups = fsd.CfsdTable(
        d.client.path("res:/staticdata/groups.fsdbinary"), "groups")
    gkeys = set(groups.keys())
    ck("dogmaeffects: modifier groupID closes into groups",
       all(m["groupID"] in gkeys for m in mods if m["groupID"] is not None))
    ck("dogmaeffects: modifier skillTypeID closes into types",
       all(m["skillTypeID"] in tkeys for m in mods
           if m["skillTypeID"] is not None))

    td = {}
    nrows = nattr = neff = 0
    seen_attr, seen_eff = set(), set()
    for key, row in d.table("typedogma").rows():
        nrows += 1
        a = {e["attributeID"]: e["value"] for e in row["dogmaAttributes"]}
        nattr += len(a)
        neff += len(row["dogmaEffects"])
        seen_attr.update(a)
        seen_eff.update(e["effectID"] for e in row["dogmaEffects"])
        td[key] = a
    ck("typedogma: rows", nrows == 27160, nrows)
    ck("typedogma: attribute values", nattr == 648726, nattr)
    ck("typedogma: effect entries", neff == 54307, neff)
    ck("typedogma: keys close into types", set(td) <= tkeys)
    ck("typedogma: every attributeID is a dogmaattributes key",
       seen_attr <= set(attrs), "%d distinct" % len(seen_attr))
    ck("typedogma: every effectID is a dogmaeffects key",
       seen_eff <= set(effs), "%d distinct" % len(seen_eff))

    rs = {k: v["skills"] for k, v in d.table("requiredskillsfortypes").rows()}
    counts = {k: v["skillCount"]
              for k, v in d.table("requiredskillsfortypes").rows()}
    ck("requiredskillsfortypes: rows", len(rs) == 9573, len(rs))
    ck("requiredskillsfortypes: stored count == decoded dict size",
       all(len(rs[k]) == counts[k] for k in rs))
    ck("requiredskillsfortypes: skill ids close into types",
       all(s in tkeys for v in rs.values() for s in v))
    pairs = [(182, 277), (183, 278), (184, 279),
             (1285, 1286), (1289, 1287), (1290, 1288)]
    disagree = 0
    for tid, skills in rs.items():
        a = td.get(tid, {})
        want = {int(a[sa]): int(a.get(la, 0))
                for sa, la in pairs if a.get(sa)}
        if want != skills:
            disagree += 1
    ck("requiredskillsfortypes agrees with typedogma requiredSkillN",
       disagree == 0, "%d of %d rows disagree" % (disagree, len(rs)))

    names = {k: v["name"] for k, v in attrs.items()}
    k = client_constants()
    if k:
        import re
        hit = bad = 0
        worst = []
        for cname, val in k.items():
            m = re.match(r"^attribute([A-Z]\w*)$", cname)
            if not m or val not in names:
                continue
            want = m.group(1)[0].lower() + m.group(1)[1:]
            if names[val].lower() == want.lower():
                hit += 1
            else:
                bad += 1
                if len(worst) < 4:
                    worst.append("%s=%d is %r" % (cname, val, names[val]))
        ck("dogma/const.py attributeXxx names agree with the data",
           hit >= 1200 and bad < 120,
           "%d agree, %d differ (%s)" % (hit, bad, "; ".join(worst)))
        uhit = ubad = 0
        for cname, val in k.items():
            m = re.match(r"^unit([A-Z]\w*)$", cname)
            if not m or val not in units:
                continue
            want = m.group(1).lower()
            got = units[val]["name"].replace(" ", "").lower()
            if got == want:
                uhit += 1
            else:
                ubad += 1
        ck("dogma/const.py unitXxx names agree with dogmaunits",
           uhit >= 25, "%d agree, %d differ" % (uhit, ubad))
    for aid, want in ((280, "skillLevel"), (276, "skillPoints"),
                      (275, "skillTimeConstant"), (14, "hiSlots"),
                      (13, "medSlots"), (12, "lowSlots"),
                      (552, "signatureRadius")):
        ck("dogma/const.py attribute %d is %r" % (aid, want),
           names.get(aid) == want, names.get(aid))
    for eid, want in ((11, "loPower"), (12, "hiPower"), (13, "medPower")):
        ck("effect %d is %r" % (eid, want),
           effs[eid]["effectName"] == want, effs[eid]["effectName"])

    rifter = {names.get(k, k): v for k, v in td[587].items()}
    for k, want in (("hiSlots", 3.0), ("medSlots", 3.0), ("lowSlots", 4.0),
                    ("turretSlotsLeft", 3.0), ("launcherSlotsLeft", 2.0),
                    ("signatureRadius", 35.0), ("hp", 350.0),
                    ("shieldCapacity", 450.0), ("armorHP", 450.0),
                    ("capacitorCapacity", 250.0), ("maxVelocity", 365.0),
                    ("powerOutput", 41.0), ("cpuOutput", 130.0)):
        ck("Rifter (587) %s == %g" % (k, want), rifter.get(k) == want,
           rifter.get(k))

    out("")
    out("%d failure(s)" % fails)
    return fails


# --- CLI ------------------------------------------------------------------


def _fmt(v, d=None):
    if isinstance(v, float):
        return "%g" % v
    return str(v)


def cmd_schema(a, d):
    stems = [a.table] if a.table else sorted(SCHEMAS)
    for stem in stems:
        s = SCHEMAS[stem]
        print("%s.fsdbinary  record %d bytes = uint32 key + pad to %d + struct"
              % (stem, s["size"], s["struct_at"]))
        if s["presence"] is not None:
            print("  presence bitfield: uint32 at struct +0x%02x (record +%d)"
                  % (s["presence"], s["struct_at"] + s["presence"]))
        for name, off, kind, bit in s["fields"]:
            print("  +%-4d %-6s %-32s %s"
                  % (s["struct_at"] + off, kind, name,
                     "optional, bit 0x%x" % bit if bit else "always present"))
        for which in ("element", "entry"):
            for fname, spec in (s.get(which) or {}).items():
                print("  %s %s: stride %d" % (which, fname, spec["stride"]))
                for f in spec.get("fields", []):
                    print("    +%-4d %-6s %-30s %s"
                          % (f[1], f[2], f[0],
                             "optional, bit 0x%x" % f[3] if f[3] else ""))
                if "key" in spec:
                    print("    key %s at +0, value %s at +4"
                          % (spec["key"], spec["value"]))
        print()


def cmd_derive(a, d):
    pyd = loader_path(d.client, a.table)
    if pyd is None:
        raise SystemExit("dogma: no bin64/%sLoader.pyd" % a.table)
    print("# %s" % pyd)
    for g in derive_schema(pyd):
        pres = sorted({f[4] for f in g if f[4] is not None})
        print("-- struct, %d field(s), presence bitfield at %s"
              % (len(g), ["+%d" % p for p in pres] or "none"))
        for name, off, kind, bit, _p in g:
            print("   +%-4d %-6s %-32s %s"
                  % (off, kind, name, "bit 0x%x" % bit if bit else ""))


def _print_rows(d, stem, limit, resolve):
    t = d.table(stem)
    for key, row in t.rows(limit=limit):
        print("%s %d" % (stem, key))
        for name, off, kind, _bit in t.schema["fields"]:
            v = row[name]
            if kind == "msgid" and resolve and v:
                print("   %-30s %s  %r" % (name, v, d.text(v)))
            elif isinstance(v, list):
                print("   %-30s %d entries" % (name, len(v)))
                for e in v[:limit or 4]:
                    print("      %s" % ", ".join(
                        "%s=%s" % (k, _fmt(x)) for k, x in e.items()))
            elif isinstance(v, dict):
                print("   %-30s %s" % (name, v))
            else:
                print("   %-30s %s" % (name, _fmt(v)))


def cmd_attributes(a, d):
    _print_rows(d, "dogmaattributes", a.n, a.loc)


def cmd_effects(a, d):
    _print_rows(d, "dogmaeffects", a.n, a.loc)


def cmd_units(a, d):
    _print_rows(d, "dogmaunits", a.n, a.loc)


def cmd_type(a, d):
    row = d.of_type(a.type_id)
    print("typeID %d: %d attributes, %d effects, %d required skills"
          % (a.type_id, len(row["attributes"]), len(row["effects"]),
             len(row["requiredSkills"])))
    for k in sorted(row["attributes"], key=str):
        print("   %-34s %s" % (k, _fmt(row["attributes"][k])))
    for name, is_default in row["effects"]:
        print("   effect %-27s default=%s" % (name, is_default))
    if row["requiredSkills"]:
        tn = d.type_names()
        for s, lvl in sorted(row["requiredSkills"].items()):
            print("   skill  %-27s level %d" % (tn.get(s, s), lvl))


def cmd_skills(a, d):
    for key, row in d.table("requiredskillsfortypes").rows():
        if key == a.type_id:
            print(row["skills"])
            return
    raise SystemExit("typeID %d has no required-skills row" % a.type_id)


def cmd_shape(a, d):
    """The one TSV line a ``findings/corpus-<table>.tsv`` header carries."""
    s = SCHEMAS[a.table]
    t = d.table(a.table)
    cols = ";".join(
        "%s@+%d:%s%s" % (n, s["struct_at"] + o, k, "?" if b else "")
        for n, o, k, b in s["fields"])
    print("\t".join(["res:/staticdata/%s.fsdbinary" % a.table,
                     str(t.cfsd.row_count), str(s["size"]), cols]))


def cmd_validate(a, d):
    raise SystemExit(1 if validate(d) else 0)


# --- the client this run is reading --------------------------------------
#
# Every ``[V]`` mark in this repository is measured against the pinned build
# (**3662168** since 2026-10-31; **3619879** before, and still the build of every
# older mark; `docs/archive/plan-subsystems.md` §2a). On 2026-10-24 there were
# three installs on the reference machine -- ``tq-offline`` and ``tq-offline-2`` at 3619879 and the live Steam
# ``tq`` at 3628513 -- and `fsd.Client`'s built-in fallback path points at
# ``tq``, the *wrong* one. So "which client did this run actually read" is not
# a detail; it decides whether the output is evidence.

#: Where the expected build comes from, so there is one number, not two.
PIN_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "client-pin.sha256")


def pinned_build(path=None):
    """The build `tools/client-pin.sha256` pins, or ``None``.

    The manifest's header carries ``# build:   3662168``;
    `tools/pin-client.sh` writes it and reads it back the same way.
    """
    try:
        with open(path or PIN_FILE, "r", encoding="utf-8") as fh:
            for line in fh:
                if not line.startswith("#"):
                    break
                k, _, v = line[1:].partition(":")
                if k.strip() == "build" and v.strip():
                    return v.strip()
    except OSError:
        pass
    return None


#: The build registry (R-BUILD §3.4): which builds the tools and the server
#: accept, and why. `client-pin.sha256` pins FILES; this pins MEANING.
BUILDS_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "builds.tsv")
#: Registry statuses a tool accepts without ``--build``: the pinned build
#: and any build `tools/build-delta.py` measured ``equivalent`` to it.
ACCEPTED_STATUSES = ("pinned", "equivalent")


def registry(path=None):
    """``{build: {"line", "status", "data_hash", "evidence"}}`` from
    `tools/builds.tsv`, or ``{}`` when it is missing.

    Format: ``#``-comment lines, then one row per build with the columns
    ``build  line  status  data_hash  evidence`` (``data_hash`` ``-`` when
    none is registered). A malformed row is an error, not a skip: a
    registry that silently lost a row would refuse a build for no reason.
    """
    out = {}
    try:
        with open(path or BUILDS_FILE, "r", encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except OSError:
        return out
    for n, line in enumerate(lines, 1):
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t")
        if len(f) != 5 or not f[0].isdigit() or f[2] not in (
                "pinned", "equivalent", "unverified"):
            raise SystemExit("dogma.py: %s line %d is not `build line status "
                             "data_hash evidence`: %r" % (path or BUILDS_FILE, n, line))
        out[f[0]] = {"line": f[1], "status": f[2],
                     "data_hash": None if f[3] == "-" else f[3], "evidence": f[4]}
    return out


def accepted_builds(path=None, pin=None):
    """The builds a tool accepts by default: every registry row with an
    accepted status, or, with no registry, the pin alone."""
    reg = registry(path)
    if reg:
        return [b for b, e in reg.items() if e["status"] in ACCEPTED_STATUSES]
    pin = pin if pin is not None else pinned_build()
    return [pin] if pin else []


def install_build(root):
    """The ``build =`` line of ``<root>/start.ini``, or ``None``."""
    try:
        with open(os.path.join(root, "start.ini"), "r", encoding="utf-8",
                  errors="replace") as fh:
            for line in fh:
                k, _, v = line.partition("=")
                if k.strip() == "build" and v.strip():
                    return v.strip()
    except OSError:
        pass
    return None


def resolve_client(client=None, resfiles=None, want_build=None, out=None):
    """Open one client install, announcing it, and refuse the wrong build.

    Returns an `fsd.Client`. Raises `SystemExit` rather than let a run
    continue against an install nobody chose:

    * nothing named the install -- no ``--client``, no ``$EVE_CLIENT``. The
      inherited fallback pointed at Steam's ``tq``, i.e. the wrong build, so
      there is no default to fall back *to*;
    * the named root has no ``resfileindex.txt`` -- say which path was tried
      and **which of the two sources it came from**, since they fail in very
      different places;
    * ``start.ini`` names a build `tools/builds.tsv` does not accept
      (``pinned`` or ``equivalent``), or names none. With no registry, the
      pin's build is the only one. Pass ``--build <n>`` for another build on
      purpose, or ``--build any`` to stop checking.

    The announcement goes to stderr unconditionally -- stdout is the data --
    the way `tools/analysis.sh` names the build it mounts, and for the same
    reason: inside a container one install is indistinguishable from another.
    """
    out = out or sys.stderr
    if client:
        origin = "--client"
    elif os.environ.get("EVE_CLIENT"):
        client, origin = os.environ["EVE_CLIENT"], "$EVE_CLIENT"
    else:
        # **No implicit default, on purpose.** `fsd.Client`'s fallback is the
        # Steam `tq` directory, which on the reference machine is build
        # 3628513 -- not the pinned one. Guessing it is how a run reads an
        # install nobody chose, so this one asks instead.
        raise SystemExit(
            "dogma.py: no client named, and there is no default\n"
            "dogma.py:   pass --client DIR, or set $EVE_CLIENT. The built-in\n"
            "dogma.py:   fallback path (Steam's `tq`) is deliberately NOT\n"
            "dogma.py:   used here: on the reference machine it is build\n"
            "dogma.py:   3628513 and the corpus is pinned to 3662168.\n"
            "dogma.py: inside tools/analysis.sh:\n"
            "dogma.py:   --client /client --resfiles /client-root/ResFiles")
    root = os.path.abspath(os.path.expanduser(client))

    if not os.path.isfile(os.path.join(root, "resfileindex.txt")):
        raise SystemExit(
            "dogma.py: no client install at %s (no resfileindex.txt)\n"
            "dogma.py:   that path came from: %s\n"
            "dogma.py: inside tools/analysis.sh the client is mounted at\n"
            "dogma.py:   --client /client --resfiles /client-root/ResFiles"
            % (root, origin))

    build = install_build(root)
    print("dogma.py: client = %s (from %s), build = %s"
          % (root, origin, build or "unknown"), file=out)

    if want_build is None:
        accepted = accepted_builds()
        if accepted and build in accepted:
            entry = registry().get(build)
            if entry and entry["status"] != "pinned":
                print("dogma.py: build %s is %s to the pinned build (%s)"
                      % (build, entry["status"], entry["evidence"]), file=out)
            want = None
        else:
            want = ",".join(accepted) or None
    else:
        want = want_build
    if want and want != "any" and build != want:
        raise SystemExit(
            "dogma.py: WRONG BUILD -- this install says %s, the RE corpus is\n"
            "dogma.py:   anchored to %s (docs/archive/plan-subsystems.md 2a;\n"
            "dogma.py:   accepted builds are in tools/builds.tsv).\n"
            "dogma.py:   Measured 2026-10-24/26 for 3628513: 0 of 203\n"
            "dogma.py:   res:/staticdata rows differ and the 149\n"
            "dogma.py:   bin64/*Loader.pyd differ only in signature (one\n"
            "dogma.py:   relinked, same schema) -- but a build nobody has\n"
            "dogma.py:   measured may not, so a schema `derive` reads here\n"
            "dogma.py:   is THIS install's and nothing says so but me.\n"
            "dogma.py:   Pass --build %s to mean it, or --build any to skip."
            % (build or "nothing readable", want, build or "<n>"))

    c = fsd.Client(root, resfiles)
    if not os.path.isdir(c.resfiles):
        raise SystemExit(
            "dogma.py: no ResFiles at %s\n"
            "dogma.py:   it is a *sibling* of the install, not a child, so a\n"
            "dogma.py:   container that mounts only the install cannot see\n"
            "dogma.py:   it. Pass --resfiles, or $EVE_RESFILES." % c.resfiles)
    return c


def main(argv=None):
    import argparse

    def add_common(parser, sub):
        """The flags every subcommand takes, on both sides of the subcommand.

        ``sub`` is the whole fix for a bug two lanes had to document as a
        trap instead. These flags came from one ``parents=[common]`` parser
        passed to *both* the top-level parser and every subparser. A
        subparser parses into a namespace of its own and then copies every
        key of it over the outer namespace -- its *defaults* included -- so
        ``--client X type 670`` set ``client=X`` and then immediately set it
        back to ``None``.
        The flag was accepted, echoed in ``--help``, and silently discarded.

        `argparse.SUPPRESS` as the subparser-side default is the fix: a
        subparser then creates a key only for a flag actually given, so
        either placement works and, if both are given, the later one wins.
        """
        d = (lambda v: argparse.SUPPRESS) if sub else (lambda v: v)
        parser.add_argument("--client", default=d(None),
                            help="client install dir; no default, or "
                                 "set $EVE_CLIENT")
        parser.add_argument("--resfiles", default=d(None),
                            help="ResFiles dir (default: sibling of the "
                                 "install)")
        parser.add_argument("-n", type=int, default=d(4),
                            help="how many rows to show")
        parser.add_argument("--loc", action="store_true", default=d(False),
                            help="resolve message ids via tools/loctable.py")
        parser.add_argument("--build", default=d(None), metavar="N",
                            help="the build this install must be, or "
                                 "'any' (default: any build tools/builds.tsv "
                                 "marks pinned or equivalent)")

    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    add_common(p, sub=False)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name, fn, arg in (
        ("schema", cmd_schema, "table?"),
        # `derive` takes any cFSD table name, not just the ones with a
        # committed schema: it is how the next table gets one.
        ("derive", cmd_derive, "any_table"),
        ("attributes", cmd_attributes, None),
        ("effects", cmd_effects, None),
        ("units", cmd_units, None),
        ("type", cmd_type, "type_id"),
        ("skills", cmd_skills, "type_id"),
        ("shape", cmd_shape, "table"),
        ("validate", cmd_validate, None),
    ):
        s = sub.add_parser(name)
        add_common(s, sub=True)
        if arg == "any_table":
            s.add_argument("table", help="any .fsdbinary table's stem")
        elif arg == "table":
            s.add_argument("table", choices=sorted(SCHEMAS))
        elif arg == "table?":
            s.add_argument("table", nargs="?", choices=sorted(SCHEMAS))
        elif arg == "type_id":
            s.add_argument("type_id", type=int)
        s.set_defaults(func=fn)
    a = p.parse_args(argv)
    a.func(a, Dogma(resolve_client(a.client, a.resfiles, a.build)))


if __name__ == "__main__":
    main()
