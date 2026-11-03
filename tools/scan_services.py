import os, pyc27, collections, json
import os, sys
# Walks a *relative* `pyc/` directory and writes its output beside it, so it
# only works from the directory that holds it (`out/`). Checked before the
# output file is opened: it used to be opened first, which left an empty
# `index.txt` or `services.json` in whatever directory it was run from and
# then printed a cheerful `0 modules`.
if not os.path.isdir('pyc'):
    sys.exit('%s: no `pyc/` directory here. Run it from the directory that\n'
             'holds the unpacked bytecode, which in this repo is `out/`:\n'
             '    cd out && python3 ../tools/%s'
             % (os.path.basename(sys.argv[0]), os.path.basename(sys.argv[0])))

DUNDERS=('__servicename__','__guid__','__notifyevents__','__dependencies__','__exportedcalls__',
         '__machoresolve__','__sessionchangeevents__','__startupdependencies__','__update_on_queues__')
found=collections.defaultdict(dict)   # (module, classname) -> {dunder: value}
methods=collections.defaultdict(list)
mods=0
for root,dirs,files in os.walk('pyc'):
    for f in files:
        if not f.endswith('.pyc'): continue
        p=os.path.join(root,f)
        try: top=pyc27.load_pyc(p)
        except Exception: continue
        mods+=1
        rel=os.path.relpath(p,'pyc')[:-4].replace(os.sep,'.')
        for c in pyc27.walk(top):
            # class bodies: look for const -> STORE_NAME dunder
            prev=None; hits={}
            fns=[]
            for off,op,arg,rep in pyc27.dis(c):
                if op=='LOAD_CONST': prev=c.consts[arg]
                elif op=='STORE_NAME' and rep in DUNDERS and prev is not None:
                    hits[rep]=prev
                elif op=='STORE_NAME' and prev is not None and isinstance(prev,pyc27.Code):
                    fns.append(rep)
                if op not in ('LOAD_CONST',): prev=prev if op.startswith('STORE_') else None
            if '__servicename__' in hits or ('__guid__' in hits and str(hits.get('__guid__','')).startswith("b'svc.")):
                key=(rel,c.name.decode())
                found[key]=hits
                methods[key]=fns
print("modules parsed:",mods)
print("service classes:",len(found))
out={}
for (mod,cls),h in sorted(found.items()):
    sn=h.get('__servicename__')
    sn=sn.decode() if isinstance(sn,bytes) else sn
    out[sn or cls]={'module':mod,'class':cls,
        'guid':(h.get('__guid__').decode() if isinstance(h.get('__guid__'),bytes) else h.get('__guid__')),
        'methods':sorted(set(m for m in methods[(mod,cls)] if not m.startswith('_')))}
json.dump(out,open('services.json','w'),indent=1,default=str)
names=sorted(out)
print("=== service names (%d) ==="%len(names))
print(', '.join(names))
