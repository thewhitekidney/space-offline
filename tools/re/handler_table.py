#!/usr/bin/env python3
"""Dump Marshal's type-handler table: 64 slots x 16 bytes {handler ptr, int32 off}.

  python3 tools/re/handler_table.py <pe> <table_va> [n_slots]

A non-NULL handler means the type code is valid in this build. Relocated
pointers are stored in the file with the default image base applied, so raw
reads are meaningful; zeros mean the slot is filled at init instead.
"""
import struct
import sys

import pefile

sys.path.insert(0, 'tools')
from pe_functions import functions  # noqa: E402


def main():
    path = sys.argv[1]
    tva = int(sys.argv[2], 16)
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 64
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(directories=[
        pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_EXCEPTION']])
    base = pe.OPTIONAL_HEADER.ImageBase
    raw = pe.get_data(tva - base, n * 16)
    funcs = functions(pe)
    fstart = {b for b, _ in funcs}

    print('table 0x%x, %d slots of 16 bytes' % (tva, n))
    print(' idx  tag(s)                handler        in .pdata?  extra_i32')
    nonnull = 0
    for i in range(n):
        ptr, off = struct.unpack_from('<Qi', raw, i * 16)
        if ptr == 0:
            continue
        nonnull += 1
        known = 'yes' if ptr in fstart else '-'
        # a tag byte maps here via (tag & 0x3f) == i, so tag i and i|0x40
        print('  %2d  0x%02x / 0x%02x          0x%012x   %-9s  %d'
              % (i, i, i | 0x40, ptr, known, off))
    print('\nnon-NULL slots: %d of %d' % (nonnull, n))
    nulls = [i for i in range(n)
             if struct.unpack_from('<Q', raw, i * 16)[0] == 0]
    print('NULL (invalid) type codes: %s' % (' '.join('0x%02x' % i for i in nulls) or 'none'))


main()
