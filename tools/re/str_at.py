import sys, pefile
pe=pefile.PE(sys.argv[1], fast_load=True)
base=pe.OPTIONAL_HEADER.ImageBase
for a in sys.argv[2:]:
    va=int(a,16)
    print("0x%x: %r"%(va, pe.get_string_at_rva(va-base)))
