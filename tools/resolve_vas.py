"""Resolve VAs to imported symbol names, and dump a string at a VA.

  python3 tools/resolve_vas.py <pe> <va> [<va>...]
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys, pefile
pe = pefile.PE(sys.argv[1], fast_load=True)
pe.parse_data_directories(directories=[
    pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
base = pe.OPTIONAL_HEADER.ImageBase
slots = {}
for entry in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
    dll = entry.dll.decode(errors='replace')
    for imp in entry.imports:
        if imp.name:
            slots[imp.address] = (imp.name.decode(errors='replace'), dll)
for a in sys.argv[2:]:
    va = int(a, 16)
    if va in slots:
        print('0x%x  IMPORT  %s  (%s)' % (va, slots[va][0], slots[va][1]))
        continue
    try:
        s = pe.get_string_at_rva(va - base)
        print('0x%x  STRING  %r' % (va, s))
    except Exception as e:
        print('0x%x  unresolved (%s)' % (va, e))
