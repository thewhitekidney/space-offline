"""Find .text code that forms a pointer to a given string literal (RIP-relative lea).

  python3 tools/find_str_xref.py <pe> <needle>
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys, pefile
from capstone import Cs, CS_ARCH_X86, CS_MODE_64
pe = pefile.PE(sys.argv[1], fast_load=True)
needle = sys.argv[2].encode()
base = pe.OPTIONAL_HEADER.ImageBase
# locate the string in any initialized section
targets = []
for s in pe.sections:
    d = s.get_data()
    off = d.find(needle)
    while off != -1:
        targets.append(base + s.VirtualAddress + off)
        off = d.find(needle, off + 1)
print('string VAs:', [hex(t) for t in targets])
text = next(s for s in pe.sections if s.Name.rstrip(b'\0') == b'.text')
data = text.get_data(); tbase = base + text.VirtualAddress
md = Cs(CS_ARCH_X86, CS_MODE_64)
tset = set(targets)
for i in range(len(data) - 7):
    # lea reg, [rip+disp]  (48/4C 8D xx)
    if data[i] in (0x48, 0x4C) and data[i+1] == 0x8D and (data[i+2] & 0xC7) == 0x05:
        disp = int.from_bytes(data[i+3:i+7], 'little', signed=True)
        if tbase + i + 7 + disp in tset:
            start = max(0, i - 8)
            print('  -- lea at 0x%x' % (tbase + i))
            for ins in md.disasm(data[start:i + 24], tbase + start):
                print('     0x%x  %-9s %s' % (ins.address, ins.mnemonic, ins.op_str))
