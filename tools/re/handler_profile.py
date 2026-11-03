#!/usr/bin/env python3
"""Profile marshal type handlers: size, recursion into ReadObject, callees.

  python3 tools/re/handler_profile.py <pe> <readobject_va> <handler_va>...

A handler that calls ReadObject is a container type (it decodes children);
one that does not is a scalar. Call counts and sizes separate the families.
"""
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import containing, functions, load, read_va, rip_target  # noqa: E402


def main():
    path = sys.argv[1]
    ro = int(sys.argv[2], 16)
    pe = load(path)
    funcs = functions(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    pe.parse_data_directories(directories=[
        pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
    iat = {}
    for e in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
        for imp in e.imports:
            if imp.name:
                iat[imp.address] = imp.name.decode(errors='replace')

    print('%-14s %6s %5s %7s  %s' % ('handler', 'bytes', 'ret?', 'ReadObj', 'notes'))
    for a in sys.argv[3:]:
        va = int(a, 16)
        f = containing(funcs, va)
        n = (f[1] - f[0]) if f else 0x300
        data = read_va(pe, va, max(n, 0x40))
        size = 0
        ro_calls = 0
        direct = []
        imports = []
        widths = []
        for ins in md.disasm(data, va):
            size = ins.address + ins.size - va
            t = rip_target(ins)
            if t in iat and iat[t] not in imports:
                imports.append(iat[t])
            if ins.mnemonic in ('call', 'jmp'):
                op = ins.op_str
                if op.startswith('0x'):
                    tgt = int(op, 16)
                    if tgt == ro:
                        ro_calls += 1
                    elif tgt not in direct and not (va <= tgt < va + size):
                        direct.append(tgt)
            if ins.mnemonic == 'cmp' and ',' in ins.op_str:
                o = ins.op_str.split(',')[-1].strip()
                if o in ('1', '2', '4', '8', '0x10', '0x20'):
                    widths.append(o)
            if ins.mnemonic == 'ret' and not f:
                break
            if f and ins.address + ins.size >= f[1]:
                break
        notes = []
        if widths:
            notes.append('reads=' + ','.join(dict.fromkeys(widths)))
        if imports:
            notes.append('imp=' + ','.join(imports[:3]))
        if direct:
            notes.append('calls=' + ','.join('0x%x' % d for d in direct[:3]))
        print('0x%-12x %6d %5s %7d  %s'
              % (va, size, 'pdata' if f else 'scan', ro_calls, '  '.join(notes)))


main()
