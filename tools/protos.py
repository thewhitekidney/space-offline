import os, pyc27, sys
from google.protobuf import descriptor_pb2
def descriptors(path):
    top=pyc27.load_pyc(path)
    best=None
    for c in pyc27.walk(top):
        for k in (c.consts or ()):
            if isinstance(k,bytes) and len(k)>40 and (b'.proto' in k[:200]):
                if best is None or len(k)>len(best): best=k
    if best is None: return None
    fd=descriptor_pb2.FileDescriptorProto()
    try: fd.ParseFromString(best)
    except Exception: return None
    return fd
if __name__=='__main__':
    ok=0; svcs=0; msgs=0; files=[]
    for root,d,fs in os.walk('pyc/eveProto'):
        for f in fs:
            if not f.endswith('_pb2.pyc'): continue
            fd=descriptors(os.path.join(root,f))
            if fd is None: continue
            ok+=1; msgs+=len(fd.message_type); svcs+=len(fd.service)
            if fd.service: files.append(fd)
    print("parsed descriptors:",ok,"messages:",msgs,"files with rpc services:",len(files))
    for fd in files[:6]:
        print("---",fd.name,"package",fd.package)
        for s in fd.service:
            print("  service",s.name)
            for m in s.method[:8]:
                print("    rpc %s(%s) -> %s"%(m.name,m.input_type,m.output_type))
