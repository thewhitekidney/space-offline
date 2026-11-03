#!/usr/bin/env python3
"""Map instrumented functions by their profiler zone names.

  python3 tools/re/zones.py <pe> <source-filename-string-VA>

Tracy zone begins pass (line, file, function-name). Every function compiled
from one source file therefore references that file's path string. For each
such function, print its bounds, size, and every printable string it
references -- which includes its own zone name.
"""
import sys

import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs

sys.path.insert(0, 'tools')
from pe_functions import disasm_func, functions, load, rip_target  # noqa: E402


def printable(pe, va):
    base = pe.OPTIONAL_HEADER.ImageBase
    try:
        s = pe.get_string_at_rva(va - base)
    except Exception:
        return None
    if not s or len(s) < 3 or len(s) > 120:
        return None
    try:
        t = s.decode('ascii')
    except Exception:
        return None
    return t if all(32 <= ord(c) < 127 for c in t) else None


def main():
    path = sys.argv[1]
    needle = int(sys.argv[2], 16)
    pe = load(path)
    funcs = functions(pe)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    hits = []
    for fb, fe in funcs:
        if fe - fb > 200000:
            continue
        ins = disasm_func(pe, md, fb, fe)
        tgts = {rip_target(i) for i in ins}
        if needle in tgts:
            strs = []
            for t in sorted(x for x in tgts if x):
                s = printable(pe, t)
                if s and 'BuildAgent' not in s:
                    strs.append(s)
            hits.append((fb, fe, strs))
    print('%d functions reference 0x%x' % (len(hits), needle))
    for fb, fe, strs in sorted(hits, key=lambda h: h[1] - h[0], reverse=True):
        print('\n0x%x-0x%x  %6d bytes' % (fb, fe, fe - fb))
        for s in strs[:8]:
            print('      %r' % s)


main()
