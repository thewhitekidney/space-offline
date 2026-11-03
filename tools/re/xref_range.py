#!/usr/bin/env python3
"""Report a VA's section, and find every function with a RIP-relative ref into a VA range.

  python3 tools/re/xref_range.py <pe> <lo_va> <hi_va>
"""
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import disasm_func, functions, load, rip_target  # noqa: E402


def main():
    path = sys.argv[1]
    lo = int(sys.argv[2], 16)
    hi = int(sys.argv[3], 16)
    pe = load(path)
    base = pe.OPTIONAL_HEADER.ImageBase
    for s in pe.sections:
        start = base + s.VirtualAddress
        end = start + max(s.Misc_VirtualSize, s.SizeOfRawData)
        if start <= lo < end:
            print('0x%x is in %-8s  virt 0x%x raw 0x%x  %s'
                  % (lo, s.Name.rstrip(b'\0').decode(), s.Misc_VirtualSize,
                     s.SizeOfRawData,
                     'UNINITIALIZED TAIL (filled at runtime)'
                     if (lo - start) >= s.SizeOfRawData else 'has file data'))
    funcs = functions(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    print('\nfunctions referencing 0x%x..0x%x:' % (lo, hi))
    for fb, fe in funcs:
        if fe - fb > 200000:
            continue
        ins = disasm_func(pe, md, fb, fe)
        hits = [(i.address, i.mnemonic, i.op_str, rip_target(i))
                for i in ins if (rip_target(i) or 0) and lo <= rip_target(i) < hi]
        if hits:
            print('  func 0x%x-0x%x (%d bytes), %d refs' % (fb, fe, fe - fb, len(hits)))
            for a, m, o, t in hits[:6]:
                print('      0x%x  %-8s %-34s -> 0x%x' % (a, m, o, t))


main()
