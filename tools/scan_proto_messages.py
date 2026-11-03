"""Generate findings/proto-messages.txt: every top-level protobuf message the
client ships, one `<proto file>\t<full name>` row per message, sorted.

    tools/analysis.sh python3 -I tools/scan_proto_messages.py \
        [--pyc out/pyc] [--out findings/proto-messages.txt]

Each `eveProto/**/*_pb2.pyc` embeds its serialized `FileDescriptorProto`
(`tools/protos.py` finds it). A message in a file with no package is written
as `.Name`, as the committed file has always had it. Nested messages are not
listed. Added at the re-pin to 3662168 (issue #19): the file had no generator,
and reproduces byte for byte on 3619879's tree. Read-only.
"""
import argparse
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

from protos import descriptors  # noqa: E402


def rows(pyc):
    out = []
    for root, _dirs, files in os.walk(os.path.join(pyc, "eveProto")):
        for f in files:
            if not f.endswith("_pb2.pyc"):
                continue
            fd = descriptors(os.path.join(root, f))
            if fd is None:
                continue
            for m in fd.message_type:
                out.append((fd.name, (fd.package or "") + "." + m.name))
    out.sort()
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--pyc", default="/work/out/pyc")
    ap.add_argument("--out", default="/work/findings/proto-messages.txt")
    a = ap.parse_args()
    if not os.path.isdir(os.path.join(a.pyc, "eveProto")):
        sys.exit("scan_proto_messages: no eveProto/ under %s (run tools/extract_code.py first)" % a.pyc)
    rs = rows(a.pyc)
    if not rs:
        sys.exit("scan_proto_messages: no protobuf descriptors found under %s; refusing to write an empty table" % a.pyc)
    with open(a.out, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("".join("%s\t%s\n" % r for r in rs))
    print("proto-messages: %d messages" % len(rs))


if __name__ == "__main__":
    main()
