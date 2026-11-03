import os, pyc27
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

out=open('index.txt','w',encoding='utf8',errors='replace')
n=0
for root,dirs,files in os.walk('pyc'):
    for f in sorted(files):
        if not f.endswith('.pyc'): continue
        p=os.path.join(root,f)
        try: top=pyc27.load_pyc(p)
        except Exception: continue
        rel=os.path.relpath(p,'pyc')
        seen=set()
        for c in pyc27.walk(top):
            for k in (c.consts or ()):
                if isinstance(k,bytes) and 2<len(k)<400:
                    s=k.decode('utf8','replace').replace('\n','\\n')
                    if s not in seen: seen.add(s); out.write(f"{rel}\tC\t{c.name.decode('utf8','replace')}\t{s}\n")
            for nm in (c.names or ())+(c.varnames or ()):
                s=nm.decode('utf8','replace')
                if ('N'+s) not in seen: seen.add('N'+s); out.write(f"{rel}\tN\t{c.name.decode('utf8','replace')}\t{s}\n")
        n+=1
out.close()
print("indexed",n,"modules")
