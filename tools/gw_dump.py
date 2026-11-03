"""Dump the client's public-gateway protobuf schemas as flat text (rescued from
the gitignored `out/gw-dump.py`; lane findings-gen, issue #18).

    tools/analysis.sh python3 -I tools/gw_dump.py [--pyc out/pyc] [--out out/gw-protos.txt]

For every `eveProto/generated/eve_public/**/*_pb2.pyc` it takes the serialized
`FileDescriptorProto` embedded in the module (`tools/protos.py`) and prints, per
file: `FILE <name> pkg <package>`, each message with its oneofs and numbered
fields (`<number> [rep ]<type>[ <message/enum type>] <name>[ oneofN]`), nested
messages indented, then one `rpc Name(In)->Out` line per service method. This
is the file `eve-server/src/gateway.rs` and `gateway_plans.rs` are re-checked
against after a client update (r-port-1109 report, section 6 C): re-run it on the
new `out/pyc` and diff against the previous output.

Directories are walked in sorted order, so the output is deterministic. The
original script walked them in file-system order: same lines, different block
order (the line multiset is identical on the pinned build).

Runs in the analysis container (it needs `google.protobuf`). Read-only.
The output has no trailing newline, exactly like the original script's.
"""
import argparse
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

# FieldDescriptorProto.Type numbers -> the short names used in the dump.
# Anything not listed prints as its number, which is how a new scalar kind in a
# new build shows up as a diff instead of being hidden.
TYPES = {1: 'double', 2: 'float', 3: 'int64', 4: 'uint64', 5: 'int32', 8: 'bool',
         9: 'string', 11: 'msg', 12: 'bytes', 13: 'uint32', 14: 'enum', 103: 'x'}
LABEL_REPEATED = 3
TYPE_MESSAGE, TYPE_ENUM = 11, 14


def dump_message(m, ind, out):
    out.append('%smessage %s' % (ind, m.name))
    for i, o in enumerate(m.oneof_decl):
        out.append('%s  oneof#%d %s' % (ind, i, o.name))
    for fl in m.field:
        out.append('%s  %d %s%s %s%s' % (
            ind, fl.number, 'rep ' if fl.label == LABEL_REPEATED else '',
            TYPES.get(fl.type, fl.type),
            fl.type_name if fl.type in (TYPE_MESSAGE, TYPE_ENUM) else '',
            (' ' + fl.name) + (' oneof%d' % fl.oneof_index if fl.HasField('oneof_index') else '')))
    for n in m.nested_type:
        dump_message(n, ind + '  ', out)


def dump(pyc_root, descriptors):
    """`descriptors(path)` returns a FileDescriptorProto or None (protos.descriptors)."""
    out = []
    base = os.path.join(pyc_root, 'eveProto', 'generated', 'eve_public')
    for root, dns, fs in os.walk(base):
        dns.sort()
        for f in sorted(fs):
            if not f.endswith('_pb2.pyc'):
                continue
            fd = descriptors(os.path.join(root, f))
            if fd is None:
                continue
            out.append('FILE %s pkg %s' % (fd.name, fd.package))
            for m in fd.message_type:
                dump_message(m, '', out)
            for s in fd.service:
                for m in s.method:
                    out.append('rpc %s(%s)->%s' % (m.name, m.input_type, m.output_type))
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--pyc', default='/work/out/pyc')
    ap.add_argument('--out', default='/work/out/gw-protos.txt')
    a = ap.parse_args(argv)
    import protos
    if not os.path.isdir(a.pyc):
        sys.exit('gw_dump: no such tree: %s' % a.pyc)
    out = dump(a.pyc, protos.descriptors)
    with open(a.out, 'w') as fh:
        fh.write('\n'.join(out))
    print('gw_dump: %d lines -> %s' % (len(out), a.out), file=sys.stderr)


if __name__ == '__main__':
    main()
