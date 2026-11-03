#!/usr/bin/env python3
"""Regenerate the marshal string table Rust source from a client's blue.dll.

  tools/extract_string_table.sh                     # writes string_table.rs
  tools/extract_string_table.sh --print             # just list the table

WHY NOT marshalstrings.py. The obvious source is
`eve/common/lib/marshalstrings.py`, which carries a `rawstrings` blob and a
`DoIt()` that installs it. Reading that module's bytecode shows DoIt does:

    oldTables = SwapTables((smap, strings))   # install the Python table,
                                              # keeping what was there
    SwapTables(oldTables)                     # ...and put that straight back

so the Python table is built, installed, and immediately replaced by the
native one again. The module is a no-op, and a table taken from it is wrong:
its entries are a different, longer, differently ordered set. Using it put
`machoVersion` at index 148 where the real client sends 135.

WHERE THE REAL TABLE IS. `blue.dll` holds an array of pointers to
NUL-terminated strings, and an initialisation loop walks it:

    0x18018a971  inc   edi
    0x18018a973  add   rsi, 8          ; next pointer
    0x18018a977  cmp   edi, 0xc3       ; 195
    0x18018a97d  jl    <loop top>

so the table is exactly 195 entries. The decoder bounds-checks the same way
(`cmp eax, 0xc3; jb ok`). Both the array and the count are found here by
disassembling that loop rather than hardcoding an address, so this works on
whatever build the owner supplies.

CHECK. `machoVersion` must land on 1-based index 135: that pairing was read
off the wire from a real client (its first `GetServiceInfo` call sends
`{machoVersion: 1}` with the key as tag 0x11, byte 0x87). The script refuses
to write a table that fails it.

Stdlib + pefile + capstone; runs in the analysis container, never installs.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import pathlib
import struct
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs

# An entry we have confirmed from a real client's own traffic.
ANCHOR = b"machoVersion"
ANCHOR_WIRE_INDEX = 135


def sections(pe):
    base = pe.OPTIONAL_HEADER.ImageBase
    out = []
    for s in pe.sections:
        out.append(
            (
                s.PointerToRawData,
                s.PointerToRawData + s.SizeOfRawData,
                base + s.VirtualAddress,
            )
        )
    return out


def find_table(path):
    """Return (entries, array_va) for the native marshal string table."""
    pe = pefile.PE(str(path), fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    data = pe.__data__[:]
    secs = sections(pe)

    def off_to_va(off):
        for lo, hi, va in secs:
            if lo <= off < hi:
                return va + (off - lo)
        return None

    def va_to_off(va):
        for lo, hi, sva in secs:
            if sva <= va < sva + (hi - lo):
                return lo + (va - sva)
        return None

    def cstr(off, limit=512):
        end = data.find(b"\x00", off, off + limit)
        return None if end < 0 else data[off:end]

    # 1. The anchor string, then the pointer array that references it.
    anchor_vas = []
    start = 0
    needle = ANCHOR + b"\x00"
    while True:
        i = data.find(needle, start)
        if i < 0:
            break
        va = off_to_va(i)
        if va is not None:
            anchor_vas.append(va)
        start = i + 1
    if not anchor_vas:
        raise LookupError("no %r in this binary" % ANCHOR)

    slots = []
    for va in anchor_vas:
        ptr = struct.pack("<Q", va)
        start = 0
        while True:
            i = data.find(ptr, start)
            if i < 0:
                break
            start = i + 1
            if off_to_va(i) is not None:
                slots.append(i)
    if not slots:
        raise LookupError("no pointer to %r; not a pointer table" % ANCHOR)

    # 2. The array base: walk back while the previous slot still points at a
    #    plausible string. Only the START needs finding this way -- the LENGTH
    #    comes from the code, because an unrelated string array sits directly
    #    after this one and a data-only walk runs straight through it.
    def entry(off):
        if off < 0 or off + 8 > len(data):
            return None
        (va,) = struct.unpack_from("<Q", data, off)
        if va == 0:
            return None
        o = va_to_off(va)
        if o is None:
            return None
        s = cstr(o)
        if not s or len(s) > 128 or any(c < 0x20 or c > 0x7E for c in s):
            return None
        return s

    bases = set()
    for slot in slots:
        if entry(slot) is None:
            continue
        lo = slot
        while entry(lo - 8) is not None:
            lo -= 8
        bases.add(lo)
    if len(bases) != 1:
        raise LookupError("found %d candidate table bases; refusing to guess" % len(bases))
    array_off = bases.pop()
    array_va = off_to_va(array_off)

    count = find_count(pe, data, secs, array_va)

    entries = []
    for n in range(count):
        s = entry(array_off + 8 * n)
        if s is None:
            raise LookupError("entry %d of the table does not point at a string" % (n + 1))
        entries.append(s)
    return entries, array_va, count


def find_count(pe, data, secs, array_va):
    """Read the entry count off the loop that initialises the table.

    The loop ends `inc edi; add rsi, 8; cmp edi, N; jl top`, so N is the
    count. Located from the `lea rsi, [rip+disp]` that loads the array.
    """
    base = pe.OPTIONAL_HEADER.ImageBase
    text = next(s for s in pe.sections if b".text" in s.Name)
    tstart = text.PointerToRawData
    tva = base + text.VirtualAddress
    code = data[tstart : tstart + text.SizeOfRawData]

    leas = []
    for o in range(len(code) - 7):
        # REX.W lea r64, [rip+disp32]; any destination register.
        if code[o] == 0x48 and code[o + 1] == 0x8D and (code[o + 2] & 0xC7) == 0x05:
            disp = struct.unpack_from("<i", code, o + 3)[0]
            if tva + o + 7 + disp == array_va:
                leas.append(o)

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    counts = set()
    for o in leas:
        window = code[o : o + 0x400]
        insns = list(md.disasm(window, tva + o))
        for i, ins in enumerate(insns):
            # The shape we want: `add rsi, 8` then `cmp <reg>, N` then `jl`.
            if ins.mnemonic != "add" or not ins.op_str.endswith(", 8"):
                continue
            nxt = insns[i + 1] if i + 1 < len(insns) else None
            jmp = insns[i + 2] if i + 2 < len(insns) else None
            if not nxt or not jmp or nxt.mnemonic != "cmp" or jmp.mnemonic != "jl":
                continue
            try:
                counts.add(int(nxt.op_str.split(",")[1].strip(), 0))
            except ValueError:
                pass
    if len(counts) != 1:
        raise LookupError("could not read a unique entry count (found %r)" % sorted(counts))
    return counts.pop()


def emit(entries, array_va, count):
    out = [
        "//! The marshal shared string table for the targeted client build.",
        "//!",
        "//! GENERATED by tools/extract_string_table.py from the client's",
        "//! `bin64/blue.dll`. Do not edit by hand.",
        "//!",
        "//! **Not** from `eve/common/lib/marshalstrings.py`. That module looks like",
        "//! the source of this table and is not: its `DoIt()` installs its table and",
        "//! then swaps the native one straight back, so the list it carries is never",
        "//! the one on the wire. Taking it from there put `machoVersion` at index 148",
        "//! where a real client sends 135.",
        "//!",
        "//! Tag `0x11` carries a 1-based index into this table, so",
        "//! `STRING_TABLE[i - 1]` is entry `i`. Build-specific: regenerate it for a",
        "//! different client.",
        "//!",
        "//! Table at VA %#x in this build, %d entries, both read out of the",
        "//! initialisation loop rather than hardcoded.",
        "",
        "/// 1-based on the wire; index 0 of this array is table entry 1.",
        "pub static STRING_TABLE: [&str; %d] = [" % len(entries),
    ]
    out[15] = out[15] % (array_va, count)
    for e in entries:
        s = e.decode("latin1").replace("\\", "\\\\").replace('"', '\\"')
        out.append('    "%s",' % s)
    out.append("];")
    out.append("")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("dll", type=pathlib.Path)
    ap.add_argument("out", type=pathlib.Path, nargs="?")
    ap.add_argument("--print", dest="show", action="store_true")
    args = ap.parse_args()

    entries, array_va, count = find_table(args.dll)

    # The check that makes this trustworthy: a pairing read off a real client.
    got = [i for i, e in enumerate(entries, 1) if e == ANCHOR]
    if got != [ANCHOR_WIRE_INDEX]:
        print(
            "refusing: %r is at %r, but a real client sends index %d for it."
            % (ANCHOR.decode(), got, ANCHOR_WIRE_INDEX),
            file=sys.stderr,
        )
        return 1

    if args.show or not args.out:
        for n, e in enumerate(entries, 1):
            print("%4d  %s" % (n, e.decode("latin1")))
        print("\n%d entries, array at %#x" % (len(entries), array_va), file=sys.stderr)
        return 0

    args.out.write_text(emit(entries, array_va, count))
    print("wrote %s with %d entries (array %#x)" % (args.out, len(entries), array_va))
    return 0


if __name__ == "__main__":
    sys.exit(main())
