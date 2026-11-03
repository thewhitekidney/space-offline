#!/usr/bin/env python3
"""Fingerprint handler functions by the imports they touch.

  python3 tools/re/handler_sigs.py <pe> <va> [<va>...]

For each address, disassemble until `ret` (or a cap) and report size, the
imported symbols referenced (calls and data refs, resolved through the IAT),
and small immediate constants -- enough to identify what a marshal type
handler does without reading every instruction by hand.
"""
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import read_va, rip_target  # noqa: E402


def iat_map(pe):
    pe.parse_data_directories(directories=[
        pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
    m = {}
    for e in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
        for imp in e.imports:
            if imp.name:
                m[imp.address] = imp.name.decode(errors='replace')
    return m


def main():
    path = sys.argv[1]
    pe = pefile.PE(path, fast_load=True)
    iat = iat_map(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    for a in sys.argv[2:]:
        va = int(a, 16)
        data = read_va(pe, va, 0x400)
        syms, imms, calls = [], [], 0
        size = 0
        for ins in md.disasm(data, va):
            size = ins.address + ins.size - va
            t = rip_target(ins)
            if t in iat:
                nm = iat[t]
                if nm not in syms:
                    syms.append(nm)
            if ins.mnemonic == 'call':
                calls += 1
            if ins.mnemonic == 'cmp' and ',' in ins.op_str:
                o = ins.op_str.split(',')[-1].strip()
                if o.startswith('0x') and len(o) <= 6:
                    imms.append(o)
            if ins.mnemonic in ('ret', 'jmp') and size > 4:
                break
        print('0x%x  %3d bytes  calls=%d  imports=%s%s'
              % (va, size, calls, ','.join(syms[:6]) or '-',
                 ('  cmp=' + ','.join(imms[:4])) if imms else ''))


main()
