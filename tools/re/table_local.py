#!/usr/bin/env python3
"""Recover Marshal's type-handler table by strictly local backward pairing.

  python3 tools/re/table_local.py <pe> <init_va> <table_va> <stride>

For each `movaps [rip+slot], xmmN` store, walk backwards:
  store  <-  `movups xmmN, [rbp-X]`  <-  `mov [rbp-X], reg`  <-  `lea reg,[rip+V]`
Each hop is the nearest preceding match, so nothing accumulates drift.
"""
import sys

from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import containing, disasm_func, functions, load, rip_target  # noqa: E402


def memkey(op):
    i, j = op.find('['), op.find(']')
    return op[i:j + 1] if i >= 0 else None


def main():
    path, init_va, tva, stride = sys.argv[1], int(sys.argv[2], 16), int(sys.argv[3], 16), int(sys.argv[4])
    pe = load(path)
    funcs = functions(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    fb, fe = containing(funcs, init_va)
    ins = disasm_func(pe, md, fb, fe)

    slots, zeroed, unresolved = {}, set(), []
    for k, i in enumerate(ins):
        t = rip_target(i)
        if i.mnemonic == 'mov' and t and 'qword ptr [rip' in i.op_str and t >= tva:
            src = i.op_str.split(',')[-1].strip()
            # rax is zeroed at function top and not reloaded before these stores
            if src in ('rax', 'eax'):
                zeroed.add((t - tva) // stride)
            continue
        if not (i.mnemonic == 'movaps' and t and i.op_str.startswith('xmmword ptr [rip')):
            continue
        slot = (t - tva) // stride
        xreg = i.op_str.split(',')[-1].strip()
        # hop 1: nearest preceding load of that xmm from a stack temp
        mk = None
        for j in range(k - 1, max(0, k - 12), -1):
            p = ins[j]
            if p.mnemonic in ('movups', 'movaps') and p.op_str.split(',')[0].strip() == xreg \
                    and '[rbp' in p.op_str:
                mk = memkey(p.op_str.split(',')[-1])
                break
        if mk is None:
            unresolved.append((slot, 'no xmm load'))
            continue
        # hop 2: nearest preceding qword store into that temp
        reg = None
        for j2 in range(j - 1, max(0, j - 12), -1):
            p = ins[j2]
            if p.mnemonic == 'mov' and memkey(p.op_str.split(',')[0]) == mk \
                    and not p.op_str.startswith('dword'):
                reg = p.op_str.split(',')[-1].strip()
                break
        if reg is None:
            unresolved.append((slot, 'no temp store'))
            continue
        # hop 3: nearest preceding lea into that register
        val = None
        for j3 in range(j2 - 1, max(0, j2 - 12), -1):
            p = ins[j3]
            if p.mnemonic == 'lea' and p.op_str.split(',')[0].strip() == reg:
                val = rip_target(p)
                break
        if val is None:
            unresolved.append((slot, 'no lea for %s' % reg))
            continue
        slots[slot] = val

    print('%d resolved, %d explicit NULL, %d unresolved' % (len(slots), len(zeroed), len(unresolved)))
    if unresolved:
        print('unresolved:', ['0x%02x:%s' % u for u in unresolved])
    dupes = {}
    for s, h in slots.items():
        dupes.setdefault(h, []).append(s)
    d = {h: ss for h, ss in dupes.items() if len(ss) > 1}
    print('duplicate handlers (should be none):', {hex(h): ['0x%02x' % x for x in ss] for h, ss in d.items()})
    print('\n slot   tag / tag|0x40   handler')
    for s in sorted(set(slots) | zeroed):
        h = slots.get(s)
        print('  %2d    0x%02x / 0x%02x       %s' % (s, s, s | 0x40, ('0x%x' % h) if h else 'NULL (invalid)'))


main()
