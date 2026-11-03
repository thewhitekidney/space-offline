"""Minimal Python 2.7 .pyc reader: marshal parser + 2.7 disassembler. Read-only analysis."""
import struct, sys

class Code(object):
    __slots__=('argcount','nlocals','stacksize','flags','code','consts','names',
               'varnames','freevars','cellvars','filename','name','firstlineno','lnotab')
    def __repr__(self): return '<code %s @%s:%s>'%(self.name,self.filename,self.firstlineno)

class Unmarshal(object):
    def __init__(self, data, pos=0):
        self.d=data; self.p=pos; self.refs=[]
    def u8(self):
        b=self.d[self.p]; self.p+=1; return b
    def i32(self):
        v=struct.unpack_from('<i',self.d,self.p)[0]; self.p+=4; return v
    def i64(self):
        v=struct.unpack_from('<q',self.d,self.p)[0]; self.p+=8; return v
    def f64(self):
        v=struct.unpack_from('<d',self.d,self.p)[0]; self.p+=8; return v
    def nbytes(self,n):
        v=self.d[self.p:self.p+n]; self.p+=n; return v
    def load(self):
        t=chr(self.u8())
        if t=='0': return None
        if t=='N': return None
        if t=='F': return False
        if t=='T': return True
        if t=='S': return StopIteration
        if t=='.': return Ellipsis
        if t=='i': return self.i32()
        if t=='I': return self.i64()
        if t=='f': return float(self.nbytes(self.u8()))
        if t=='g': return self.f64()
        if t=='x': return complex(float(self.nbytes(self.u8())),float(self.nbytes(self.u8())))
        if t=='y': return complex(self.f64(),self.f64())
        if t=='l':
            n=self.i32(); sign=-1 if n<0 else 1; n=abs(n); v=0
            for i in range(n):
                d=struct.unpack_from('<H',self.d,self.p)[0]; self.p+=2
                v |= d << (15*i)
            return sign*v
        if t=='s': return self.nbytes(self.i32())
        if t=='t':
            v=self.nbytes(self.i32()); self.refs.append(v); return v
        if t=='R': return self.refs[self.i32()]
        if t=='u': return self.nbytes(self.i32()).decode('utf-8','replace')
        if t in '([':
            n=self.i32(); return tuple(self.load() for _ in range(n)) if t=='(' else [self.load() for _ in range(n)]
        if t in '<>':
            n=self.i32(); return frozenset(self.load() for _ in range(n))
        if t=='{':
            d={}
            while True:
                k=self.load()
                if k is None and self.d[self.p-1]==0x30: break
                v=self.load(); d[k]=v
            return d
        if t=='c':
            c=Code()
            c.argcount=self.i32(); c.nlocals=self.i32(); c.stacksize=self.i32(); c.flags=self.i32()
            c.code=self.load(); c.consts=self.load(); c.names=self.load(); c.varnames=self.load()
            c.freevars=self.load(); c.cellvars=self.load(); c.filename=self.load(); c.name=self.load()
            c.firstlineno=self.i32(); c.lnotab=self.load()
            return c
        raise ValueError('bad marshal type %r at %d'%(t,self.p-1))

def load_pyc(path):
    d=open(path,'rb').read()
    assert d[:4]==b'\x03\xf3\r\n', (path, d[:4])
    return Unmarshal(d,8).load()

def walk(c, out=None):
    if out is None: out=[]
    out.append(c)
    for k in (c.consts or ()):
        if isinstance(k,Code): walk(k,out)
    return out

# --- 2.7 opcode table ---
OPNAME={}
def _o(n,v): OPNAME[v]=n
for n,v in [('STOP_CODE',0),('POP_TOP',1),('ROT_TWO',2),('ROT_THREE',3),('DUP_TOP',4),('ROT_FOUR',5),
 ('NOP',9),('UNARY_POSITIVE',10),('UNARY_NEGATIVE',11),('UNARY_NOT',12),('UNARY_CONVERT',13),
 ('UNARY_INVERT',15),('BINARY_POWER',19),('BINARY_MULTIPLY',20),('BINARY_DIVIDE',21),('BINARY_MODULO',22),
 ('BINARY_ADD',23),('BINARY_SUBTRACT',24),('BINARY_SUBSCR',25),('BINARY_FLOOR_DIVIDE',26),
 ('BINARY_TRUE_DIVIDE',27),('INPLACE_FLOOR_DIVIDE',28),('INPLACE_TRUE_DIVIDE',29),('SLICE+0',30),
 ('SLICE+1',31),('SLICE+2',32),('SLICE+3',33),('STORE_SLICE+0',40),('STORE_SLICE+1',41),('STORE_SLICE+2',42),
 ('STORE_SLICE+3',43),('DELETE_SLICE+0',50),('DELETE_SLICE+1',51),('DELETE_SLICE+2',52),('DELETE_SLICE+3',53),
 ('STORE_MAP',54),('INPLACE_ADD',55),('INPLACE_SUBTRACT',56),('INPLACE_MULTIPLY',57),('INPLACE_DIVIDE',58),
 ('INPLACE_MODULO',59),('STORE_SUBSCR',60),('DELETE_SUBSCR',61),('BINARY_LSHIFT',62),('BINARY_RSHIFT',63),
 ('BINARY_AND',64),('BINARY_XOR',65),('BINARY_OR',66),('INPLACE_POWER',67),('GET_ITER',68),('PRINT_EXPR',70),
 ('PRINT_ITEM',71),('PRINT_NEWLINE',72),('PRINT_ITEM_TO',73),('PRINT_NEWLINE_TO',74),('INPLACE_LSHIFT',75),
 ('INPLACE_RSHIFT',76),('INPLACE_AND',77),('INPLACE_XOR',78),('INPLACE_OR',79),('BREAK_LOOP',80),
 ('WITH_CLEANUP',81),('LOAD_LOCALS',82),('RETURN_VALUE',83),('IMPORT_STAR',84),('EXEC_STMT',85),
 ('YIELD_VALUE',86),('POP_BLOCK',87),('END_FINALLY',88),('BUILD_CLASS',89),
 ('STORE_NAME',90),('DELETE_NAME',91),('UNPACK_SEQUENCE',92),('FOR_ITER',93),('LIST_APPEND',94),
 ('STORE_ATTR',95),('DELETE_ATTR',96),('STORE_GLOBAL',97),('DELETE_GLOBAL',98),('DUP_TOPX',99),
 ('LOAD_CONST',100),('LOAD_NAME',101),('BUILD_TUPLE',102),('BUILD_LIST',103),('BUILD_SET',104),
 ('BUILD_MAP',105),('LOAD_ATTR',106),('COMPARE_OP',107),('IMPORT_NAME',108),('IMPORT_FROM',109),
 ('JUMP_FORWARD',110),('JUMP_IF_FALSE_OR_POP',111),('JUMP_IF_TRUE_OR_POP',112),('JUMP_ABSOLUTE',113),
 ('POP_JUMP_IF_FALSE',114),('POP_JUMP_IF_TRUE',115),('LOAD_GLOBAL',116),('CONTINUE_LOOP',119),
 ('SETUP_LOOP',120),('SETUP_EXCEPT',121),('SETUP_FINALLY',122),('LOAD_FAST',124),('STORE_FAST',125),
 ('DELETE_FAST',126),('RAISE_VARARGS',130),('CALL_FUNCTION',131),('MAKE_FUNCTION',132),('BUILD_SLICE',133),
 ('MAKE_CLOSURE',134),('LOAD_CLOSURE',135),('LOAD_DEREF',136),('STORE_DEREF',137),('CALL_FUNCTION_VAR',140),
 ('CALL_FUNCTION_KW',141),('CALL_FUNCTION_VAR_KW',142),('SETUP_WITH',143),('EXTENDED_ARG',145),
 ('SET_ADD',146),('MAP_ADD',147)]:
    _o(n,v)
HAS_ARG=lambda op: op>=90

def dis(c):
    """yield (offset, opname, arg, argrepr)"""
    code=c.code; i=0; ext=0
    while i < len(code):
        off=i; op=code[i]; i+=1
        name=OPNAME.get(op,'<%d>'%op)
        if HAS_ARG(op):
            arg=code[i]|(code[i+1]<<8); i+=2
            arg|=ext; ext=0
            if name=='EXTENDED_ARG': ext=arg<<16; continue
            rep=''
            try:
                if name in ('LOAD_CONST',):
                    k=c.consts[arg]; rep=repr(k) if not isinstance(k,Code) else '<code %s>'%k.name
                elif name in ('LOAD_NAME','STORE_NAME','LOAD_GLOBAL','STORE_GLOBAL','LOAD_ATTR',
                              'STORE_ATTR','IMPORT_NAME','IMPORT_FROM','DELETE_NAME'):
                    rep=c.names[arg].decode('utf8','replace')
                elif name in ('LOAD_FAST','STORE_FAST','DELETE_FAST'):
                    rep=c.varnames[arg].decode('utf8','replace')
                elif name in ('LOAD_DEREF','STORE_DEREF','LOAD_CLOSURE'):
                    fv=(c.cellvars or ())+(c.freevars or ())
                    rep=fv[arg].decode('utf8','replace')
            except Exception: rep='?'
            yield off,name,arg,rep
        else:
            yield off,name,None,''
