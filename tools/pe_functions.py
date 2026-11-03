"""Locate functions in a PE via .pdata, and find which function references a string.

  python3 tools/pe_functions.py <pe> --locate "<string>" [...]
  python3 tools/pe_functions.py <pe> --disasm <va> [--limit N]

x64 PEs carry a .pdata exception directory: an array of RUNTIME_FUNCTION
{BeginAddress, EndAddress, UnwindInfo} RVAs. That gives exact function bounds,
so we can disassemble a real function from its real entry point instead of
guessing an instruction boundary mid-stream.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import bisect
import struct
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs


def load(path):
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(directories=[
        pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_EXCEPTION']])
    return pe


def functions(pe):
    """Sorted list of (begin_va, end_va) from .pdata."""
    base = pe.OPTIONAL_HEADER.ImageBase
    out = []
    d = pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]  # IMAGE_DIRECTORY_ENTRY_EXCEPTION
    if not d.VirtualAddress or not d.Size:
        return out
    raw = pe.get_data(d.VirtualAddress, d.Size)
    for off in range(0, len(raw) - 11, 12):
        b, e, _u = struct.unpack_from('<III', raw, off)
        if b and e > b:
            out.append((base + b, base + e))
    out.sort()
    return out


def section_of(pe, va):
    base = pe.OPTIONAL_HEADER.ImageBase
    for s in pe.sections:
        start = base + s.VirtualAddress
        if start <= va < start + max(s.Misc_VirtualSize, s.SizeOfRawData):
            return s
    return None


def read_va(pe, va, n):
    s = section_of(pe, va)
    if s is None:
        return b''
    base = pe.OPTIONAL_HEADER.ImageBase
    off = va - (base + s.VirtualAddress)
    d = s.get_data()
    return d[off:off + n]


def find_strings(pe, needles):
    """VA of each needle, searched in every initialized section."""
    base = pe.OPTIONAL_HEADER.ImageBase
    found = {}
    for s in pe.sections:
        d = s.get_data()
        for nd in needles:
            pat = nd.encode()
            off = d.find(pat)
            while off != -1:
                found.setdefault(nd, []).append(base + s.VirtualAddress + off)
                off = d.find(pat, off + 1)
    return found


def containing(funcs, va):
    i = bisect.bisect_right(funcs, (va, float('inf'))) - 1
    if i >= 0 and funcs[i][0] <= va < funcs[i][1]:
        return funcs[i]
    return None


def disasm_func(pe, md, begin, end, limit=None):
    data = read_va(pe, begin, end - begin)
    out = []
    for ins in md.disasm(data, begin):
        out.append(ins)
        if limit and len(out) >= limit:
            break
    return out


def rip_target(ins):
    """VA referenced by a RIP-relative operand, or None."""
    if 'rip +' not in ins.op_str and 'rip -' not in ins.op_str:
        return None
    try:
        frag = ins.op_str.split('rip ')[1]
        sign = 1 if frag[0] == '+' else -1
        hexpart = frag[1:].strip().split(']')[0].strip()
        return ins.address + ins.size + sign * int(hexpart, 16)
    except Exception:
        return None


def main():
    path = sys.argv[1]
    pe = load(path)
    funcs = functions(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    print('functions in .pdata: %d' % len(funcs))

    if '--locate' in sys.argv:
        needles = sys.argv[sys.argv.index('--locate') + 1:]
        svas = find_strings(pe, needles)
        for nd in needles:
            vas = svas.get(nd, [])
            print('\n=== %r -> %s' % (nd, [hex(v) for v in vas]))
            if not vas:
                continue
            targets = set(vas)
            # scan every function for a real instruction referencing it
            for fb, fe in funcs:
                if fe - fb > 200000:
                    continue
                for ins in disasm_func(pe, md, fb, fe):
                    t = rip_target(ins)
                    if t in targets:
                        print('    referenced by func 0x%x-0x%x  at 0x%x: %s %s'
                              % (fb, fe, ins.address, ins.mnemonic, ins.op_str))
    if '--range' in sys.argv:
        i = sys.argv.index('--range')
        lo = int(sys.argv[i + 1], 16)
        hi = int(sys.argv[i + 2], 16)
        sel = [(b, e) for b, e in funcs if lo <= b < hi]
        print('%d functions in 0x%x-0x%x, largest first:' % (len(sel), lo, hi))
        for b, e in sorted(sel, key=lambda f: f[1] - f[0], reverse=True)[:20]:
            print('   0x%x-0x%x  %6d bytes' % (b, e, e - b))

    if '--disasm' in sys.argv:
        va = int(sys.argv[sys.argv.index('--disasm') + 1], 16)
        limit = None
        if '--limit' in sys.argv:
            limit = int(sys.argv[sys.argv.index('--limit') + 1])
        f = containing(funcs, va) or (va, va + 0x400)
        print('function 0x%x-0x%x (%d bytes)' % (f[0], f[1], f[1] - f[0]))
        for ins in disasm_func(pe, md, f[0], f[1], limit):
            mark = '>>' if ins.address == va else '  '
            t = rip_target(ins)
            extra = '   ; -> 0x%x' % t if t else ''
            print(' %s 0x%x  %-10s %s%s' % (mark, ins.address, ins.mnemonic, ins.op_str, extra))


if __name__ == '__main__':
    main()
