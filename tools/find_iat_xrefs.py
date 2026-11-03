"""Find code that reads/writes an imported data symbol in a PE.

  python3 tools/find_iat_xrefs.py <pe> <symbol> [<symbol>...]

Locates each symbol's IAT slot, scans .text for RIP-relative references to it,
and disassembles a window around every hit. Used to answer questions like
"does this binary set Py_NoSiteFlag to 1".
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys
import pefile
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

def iat_slots(pe):
    out = {}
    base = pe.OPTIONAL_HEADER.ImageBase
    for entry in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
        dll = entry.dll.decode(errors='replace')
        for imp in entry.imports:
            if imp.name:
                out[imp.name.decode(errors='replace')] = (base + imp.address - base, dll)
    # imp.address is already a VA
    out2 = {}
    for entry in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
        dll = entry.dll.decode(errors='replace')
        for imp in entry.imports:
            if imp.name:
                out2[imp.name.decode(errors='replace')] = (imp.address, dll)
    return out2

def main():
    path, names = sys.argv[1], sys.argv[2:]
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(directories=[
        pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
    base = pe.OPTIONAL_HEADER.ImageBase
    slots = iat_slots(pe)

    text = None
    for s in pe.sections:
        if s.Name.rstrip(b'\0') == b'.text':
            text = s
    data = text.get_data()
    tbase = base + text.VirtualAddress
    md = Cs(CS_ARCH_X86, CS_MODE_64)

    for name in names:
        if name not in slots:
            print('%s: NOT IMPORTED' % name)
            continue
        va, dll = slots[name]
        print('=' * 70)
        print('%s  (from %s)  IAT slot VA 0x%x' % (name, dll, va))
        hits = 0
        for i in range(len(data) - 7):
            if data[i] != 0x48:
                continue
            if data[i + 1] not in (0x8B, 0x8D):
                continue
            if (data[i + 2] & 0xC7) != 0x05:
                continue
            disp = int.from_bytes(data[i + 3:i + 7], 'little', signed=True)
            if tbase + i + 7 + disp != va:
                continue
            hits += 1
            start = max(0, i - 24)
            print('  -- hit at VA 0x%x' % (tbase + i))
            for ins in md.disasm(data[start:i + 40], tbase + start):
                mark = '>>' if ins.address == tbase + i else '  '
                print('   %s 0x%x  %-9s %s' % (mark, ins.address, ins.mnemonic, ins.op_str))
            if hits >= 6:
                print('   ... (stopping after 6 hits)')
                break
        if hits == 0:
            print('  no RIP-relative references found in .text')

main()
