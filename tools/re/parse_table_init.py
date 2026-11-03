#!/usr/bin/env python3
"""Reconstruct Marshal's runtime-populated type-handler table from its initializer.

  python3 tools/re/parse_table_init.py <pe> <init_va> <table_va> <stride>

MSVC builds each 16-byte entry {handler:u64, offset:i32} in a stack temp and
stores it with `movaps [rip+slot], xmm0`, software-pipelined. Rather than
zip two sequences (which drifts), this walks the instructions keeping exact
abstract state for registers, the stack temp and xmm0, so each store is
attributed to the handler that was actually staged for it.

Every recovered handler is checked against the .pdata function table; a
handler that is not a known function entry means the reconstruction is wrong.
"""
import sys

from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import containing, disasm_func, functions, load, rip_target  # noqa: E402


def main():
    path = sys.argv[1]
    init_va = int(sys.argv[2], 16)
    tva = int(sys.argv[3], 16)
    stride = int(sys.argv[4])
    pe = load(path)
    funcs = functions(pe)
    fstart = {b for b, _ in funcs}
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    fb, fe = containing(funcs, init_va) or (init_va, init_va + 0x600)
    ins = disasm_func(pe, md, fb, fe)

    regs = {}        # register name -> VA or 0
    mem = {}         # stack text like '[rbp - 0x10]' -> value
    xmm = {}         # 'xmm0' -> value staged
    slots = {}
    zeroed = set()

    def memkey(op):
        i, j = op.find('['), op.find(']')
        return op[i:j + 1] if i >= 0 else None

    for i in ins:
        m, ops = i.mnemonic, i.op_str
        t = rip_target(i)
        parts = [p.strip() for p in ops.split(',')]

        if m == 'xor' and len(parts) == 2 and parts[0] == parts[1]:
            regs[parts[0]] = 0
            if parts[0].startswith('e'):
                regs['r' + parts[0][1:]] = 0
        elif m == 'lea' and t:
            regs[parts[0]] = t
        elif m == 'mov' and len(parts) == 2:
            dst, src = parts
            if t and 'ptr [rip' in dst:                      # store to global
                if src in regs and regs[src] == 0 and t >= tva:
                    zeroed.add((t - tva) // stride)
            elif dst.startswith('qword ptr [rbp') or dst.startswith('[rbp'):
                k = memkey(dst)
                if k:
                    mem[k] = regs.get(src)
            elif not dst.startswith(('qword', 'dword', 'byte', 'word', '[')):
                regs[dst] = regs.get(src) if src in regs else (t if t else None)
        elif m in ('movups', 'movaps'):
            dst, src = parts
            if dst.startswith('xmm') and ('[rbp' in src):
                xmm[dst] = mem.get(memkey(src))
            elif t and ('ptr [rip' in dst) and src.startswith('xmm'):
                if t >= tva:
                    slots[(t - tva) // stride] = xmm.get(src)

    print('init 0x%x -> table 0x%x: %d entry stores, %d explicit NULL slots'
          % (fb, tva, len(slots), len(zeroed)))
    bad = [(s, h) for s, h in slots.items() if h and h not in fstart]
    print('handlers that are NOT .pdata function entries: %d %s'
          % (len(bad), ['0x%02x->0x%x' % (s, h) for s, h in sorted(bad)]))

    print('\n slot   tag / tag|0x40   handler')
    for s in sorted(set(slots) | zeroed):
        h = slots.get(s)
        if not h:
            print('  %2d    0x%02x / 0x%02x       NULL  (invalid type code)' % (s, s, s | 0x40))
        else:
            ok = '' if h in fstart else '   <-- SUSPECT, not a function entry'
            print('  %2d    0x%02x / 0x%02x       0x%x%s' % (s, s, s | 0x40, h, ok))
    valid = sorted(s for s, h in slots.items() if h)
    print('\nvalid type codes (%d): %s' % (len(valid), ' '.join('0x%02x' % s for s in valid)))
    inv = sorted((zeroed | set(slots)) - set(valid))
    print('invalid/NULL (%d): %s' % (len(inv), ' '.join('0x%02x' % s for s in inv)))


main()
