"""Unpack a client code.ccp into a tree of CPython 2.7 .pyc files.

  python3 tools/extract_code.py <path-to-code.ccp> <outdir>

.pyj entries are zlib-compressed, standard 2.7 bytecode underneath.
Nothing from the client is redistributed; this reads your own installation.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys, os, zipfile, zlib

def main(src, out):
    z = zipfile.ZipFile(src)
    ok = bad = 0
    for name in z.namelist():
        if not name.endswith('.pyj'):
            continue
        try:
            raw = zlib.decompress(z.read(name))
        except Exception:
            bad += 1
            continue
        p = os.path.join(out, name[:-4] + '.pyc')
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, 'wb') as f:
            f.write(raw)
        ok += 1
    print('extracted %d, failed %d' % (ok, bad))

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
