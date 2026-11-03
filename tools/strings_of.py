# Move `tools/` to the end of `sys.path` so the standard library wins. This
# line is load-bearing in every tool here; `tools/dis.py` carries the whole
# explanation and `docs/port-tracker.md` **42** the measurement. Deleting it
# breaks this tool with a traceback that names no file of ours.
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import sys, pyc27
def dump(path, minlen=3):
    top=pyc27.load_pyc(path)
    seen=set(); out=[]
    for c in pyc27.walk(top):
        for k in (c.consts or ()):
            if isinstance(k,bytes) and len(k)>=minlen:
                s=k.decode('utf8','replace')
                if s not in seen: seen.add(s); out.append(s)
        for nm in (c.names or ()):
            s=nm.decode('utf8','replace')
            if s not in seen and len(s)>=minlen: seen.add(s); out.append('.'+s)
    return out
if __name__=='__main__':
    for p in sys.argv[1:]:
        print("="*20, p)
        print(' | '.join(dump(p)))
