#!/usr/bin/env python3
"""Dump an MSVC two-level switch table: tag -> case index -> handler VA.

  python3 tools/re/dump_switch.py <pe> <index_rva> <n_tags> <target_rva> <n_targets> <tag_bias>
"""
import struct
import sys

import pefile


def main():
    path, irva, ntags, trva, ntargets, bias = sys.argv[1:7]
    irva, trva = int(irva, 16), int(trva, 16)
    ntags, ntargets, bias = int(ntags), int(ntargets), int(bias, 16)
    pe = pefile.PE(path, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    tdata = pe.get_data(trva, ntargets * 4)
    idata = pe.get_data(irva, ntags)
    tvas = [base + struct.unpack_from('<I', tdata, i * 4)[0] for i in range(ntargets)]

    print('%d case targets:' % ntargets)
    for i, v in enumerate(tvas):
        print('   case %2d -> 0x%x' % (i, v))

    groups = {}
    print('\n%d tag slots (bias 0x%x):' % (ntags, bias))
    print('  tag  chr  case  handler')
    for b in range(ntags):
        tag = bias + b
        g = idata[b]
        ch = chr(tag) if 32 <= tag < 127 else '.'
        h = tvas[g] if g < ntargets else -1
        print('  0x%02x  %-3s  %3d   0x%x' % (tag, ch, g, h))
        groups.setdefault(g, []).append(tag)

    print('\ngrouped by handler:')
    for g in sorted(groups):
        tags = groups[g]
        chars = ''.join(chr(t) if 32 <= t < 127 else '.' for t in tags)
        print('  case %2d  0x%x  %2d tags: %s   (%s)'
              % (g, tvas[g] if g < ntargets else -1, len(tags),
                 ' '.join('%02x' % t for t in tags), chars))


main()
