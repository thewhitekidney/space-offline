import os, pyc27, collections
REMOTE=('RemoteSvc','ProxySvc','RemoteSvcBlocking','GetRemoteSvc','RemoteServiceCall','StartServiceAndWaitForRunningState')
calls=collections.Counter()      # remote service name -> count
pairs=collections.Counter()      # (svc, method)
for root,dirs,files in os.walk('pyc'):
    for f in files:
        if not f.endswith('.pyc'): continue
        try: top=pyc27.load_pyc(os.path.join(root,f))
        except Exception: continue
        for c in pyc27.walk(top):
            ins=list(pyc27.dis(c))
            for i,(off,op,arg,rep) in enumerate(ins):
                if op=='LOAD_ATTR' and rep in REMOTE:
                    # next: LOAD_CONST 'svcname' ; CALL_FUNCTION ; then LOAD_ATTR method
                    if i+1<len(ins) and ins[i+1][1]=='LOAD_CONST':
                        k=c.consts[ins[i+1][2]]
                        if isinstance(k,bytes):
                            svc=k.decode('utf8','replace'); calls[svc]+=1
                            for j in range(i+2,min(i+6,len(ins))):
                                if ins[j][1]=='LOAD_ATTR':
                                    pairs[(svc,ins[j][3])]+=1; break
print("=== remote services referenced by client (%d) ==="%len(calls))
for s,n in calls.most_common():
    ms=sorted(m for (sv,m),_ in pairs.items() if sv==s)
    print(f"  {s:28s} refs={n:4d}  methods({len(ms)}): {', '.join(ms[:14])}{' ...' if len(ms)>14 else ''}")
