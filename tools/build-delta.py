"""Measure what changed between two EVE client installs, and say whether it matters.

  python3 -I tools/build-delta.py OLD NEW [--out FILE] [--check FILE]
                                  [--manifest tools/patches/<old>.json]
                                  [--findings DIR] [--no-schemas]

Inside the analysis image, both installs are under ``/client-root``::

  EVE_CLIENT_DIR=tq tools/analysis.sh python3 -I tools/build-delta.py \\
      /client-root/tq-offline /client-root/tq --out /dev/stdout

This is R-BUILD §1 (``out/research/R-BUILD.md``) as one command. Read-only:
it opens every file for reading and writes only ``--out``.

What it compares
----------------
* **files** at the install root and in ``bin64/`` (not recursive). A PE file
  is compared *normalised*: the optional header's ``CheckSum``, the security
  data-directory entry and the certificate table itself are removed first,
  so a file CCP merely re-signed reads ``signature-only``, not changed.
  Where a ``<file>.eveoffline-orig`` backup sits beside a file (our patcher's
  stock copy), the backup is compared, so our own patches are not counted.
* **code.ccp**: every entry decompressed and compared past the 8-byte
  ``.pyc`` header (a rebuild timestamp). A module that differs gets a
  function-level diff (each code object's ``co_code``, ``co_names`` and
  non-code constants, keyed by its dotted path) and a call-surface check:
  does it appear as a call site in ``findings/client-calls.tsv``,
  ``client-callable.tsv`` or ``push-events.tsv``, and does it name
  ``RemoteSvc``/``machoNet``/... at all.
* **resfileindex*.txt**: added/removed/changed entries, bucketed by directory.
* **loader schemas**: a ``*Loader.pyd`` whose code differs is re-read with
  ``tools/pyd_schema.py`` on both sides (needs ``capstone``/``pefile``, i.e.
  the analysis image; ``skipped`` otherwise).
* **patch sites**: every site of ``--manifest`` (default
  ``tools/patches/<old build>.json``) is looked up in NEW at the same offset
  and its 16-byte window classified ``stock``/``patched``/``other``.
* **start.ini**: every key that differs.

The verdict
-----------
``equivalent`` when the protocol layer (``blue.dll``, ``_destiny.dll``,
``exefile.exe``, ``python27.dll`` and the machoNet/GPS modules), the call
surface, the loader schemas and the static data (``res:/staticdata``,
``res:/localizationfsd``, ``*.fsdbinary``) are all unchanged; otherwise
``differs`` with the reasons. The output is TSV with hashes, names and counts
only -- no client bytes -- so a run can be committed as
``findings/build-delta-<old>-<new>.tsv`` and re-checked with ``--check``.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins (see
# tools/dis.py and docs/port-tracker.md 42).
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import hashlib
import io
import json
import os
import struct
import sys
import zipfile
import zlib

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(TOOLS)
if TOOLS not in sys.path:
    sys.path.append(TOOLS)

import pyc27  # noqa: E402

ORIG = ".eveoffline-orig"
#: Suffixes our own tools leave beside client files; never compared.
OURS = (ORIG, ".eveoffline-tmp", ".eveoffline-new")

#: Native files whose code is the wire protocol, marshal, destiny or the
#: code-push interpreter. A code change in any of them is a protocol change.
PROTOCOL_FILES = ("bin64/blue.dll", "bin64/_destiny.dll", "bin64/exefile.exe",
                  "bin64/python27.dll")
#: Module basenames (lower case, no extension) that make up the handshake
#: and machoNet in code.ccp.
PROTOCOL_MODULES = ("gps", "machonet", "machobase", "evemachonetversion",
                    "objectcaching")
#: A path component that marks a whole package as protocol.
PROTOCOL_PACKAGES = ("machonet", "evecrypto")
#: Names whose presence in a module's code means it can reach the server.
MACHO_NAMES = ("RemoteSvc", "ProxySvc", "RemoteSvcBlocking", "GetRemoteSvc",
               "RemoteServiceCall", "Moniker", "ScatterEvent", "machoNet")
#: resfileindex paths that are static data.
STATIC_PREFIXES = ("res:/staticdata/", "res:/localizationfsd/")
STATIC_SUFFIXES = (".fsdbinary", ".static")
#: Differing bytes (same size, after normalisation) at or below which a PE is
#: called ``rebuilt``: a timestamp, a debug GUID and a build-agent path, not
#: code. R-BUILD §1.1 measured 30-35 such bytes on three rebuilt .pyd files.
REBUILT_MAX_BYTES = 64


# --- small helpers -----------------------------------------------------------


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def read(path):
    with open(path, "rb") as fh:
        return fh.read()


def stock_path(path):
    """The stock copy of ``path``: our patcher's backup when there is one."""
    o = path + ORIG
    return o if os.path.isfile(o) else path


def ini_values(data):
    """``section.key -> value`` for an ini file (latin-1, any line ending)."""
    out, section = {}, ""
    for line in data.decode("latin-1").splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            section = s[1:-1].strip()
            continue
        k, sep, v = line.partition("=")
        if sep and k.strip():
            out["%s.%s" % (section, k.strip()) if section else k.strip()] = v.strip()
    return out


def install_build(root):
    try:
        vals = ini_values(read(stock_path(os.path.join(root, "start.ini"))))
    except OSError:
        return None
    for k, v in vals.items():
        if k.split(".")[-1].lower() == "build":
            return v
    return None


# --- PE normalisation --------------------------------------------------------


def pe_normalise(data):
    """``data`` with the PE checksum, security directory and certificate removed.

    Returns ``(normalised, is_pe)``. A file that is not a PE, or whose headers
    do not parse, comes back unchanged with ``is_pe`` False.
    """
    try:
        if data[:2] != b"MZ":
            return data, False
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        if data[pe:pe + 4] != b"PE\0\0":
            return data, False
        opt = pe + 24
        magic = struct.unpack_from("<H", data, opt)[0]
        if magic not in (0x10B, 0x20B):
            return data, False
        dd = opt + (112 if magic == 0x20B else 96)
        sec = dd + 8 * 4  # IMAGE_DIRECTORY_ENTRY_SECURITY
        coff, csize = struct.unpack_from("<II", data, sec)
        b = bytearray(data)
        b[opt + 64:opt + 68] = b"\0\0\0\0"  # CheckSum
        b[sec:sec + 8] = b"\0" * 8
        if coff and csize and coff + csize <= len(b):
            del b[coff:coff + csize]
        return bytes(b), True
    except struct.error:
        return data, False


def byte_ranges(a, b, gap=8):
    """``(differing bytes, ranges)`` between two equal-length buffers."""
    n = nbytes = 0
    i, end, last = 0, len(a), -gap - 1
    while i < end:
        if a[i] != b[i]:
            nbytes += 1
            if i - last > gap:
                n += 1
            last = i
        i += 1
    return nbytes, n


# --- files ---------------------------------------------------------------------


def list_files(root):
    """Files in the install root, and every file under ``bin64/`` at any
    depth (``bin64/staticdata/mapObjects.db``, ``bin64/packages/*``)."""
    out = []
    for name in sorted(os.listdir(root)) if os.path.isdir(root) else ():
        if os.path.isfile(os.path.join(root, name)) and not name.endswith(OURS):
            out.append(name)
    top = os.path.join(root, "bin64")
    for dirpath, dirs, names in os.walk(top):
        dirs.sort()
        for name in sorted(names):
            if name.endswith(OURS) or not os.path.isfile(os.path.join(dirpath, name)):
                continue
            rel = os.path.relpath(os.path.join(dirpath, name), root)
            out.append(rel.replace(os.sep, "/"))
    return out


def compare_file(old_path, new_path):
    """``(class, detail)`` for one file present in both installs."""
    a, b = read(old_path), read(new_path)
    if a == b:
        return "identical", ""
    na, pa = pe_normalise(a)
    nb, pb = pe_normalise(b)
    if pa and pb and na == nb:
        return "signature-only", ""
    if pa and pb and len(na) == len(nb):
        nbytes, nranges = byte_ranges(na, nb)
        cls = "rebuilt" if nbytes <= REBUILT_MAX_BYTES else "code-differs"
        return cls, "%d bytes in %d ranges" % (nbytes, nranges)
    if pa and pb:
        return "code-differs", "size %d -> %d" % (len(a), len(b))
    return "data-differs", "size %d -> %d" % (len(a), len(b))


# --- code.ccp ------------------------------------------------------------------


def ccp_modules(path):
    """``name -> pyc bytes`` for every ``.pyj`` in a code.ccp."""
    out = {}
    with zipfile.ZipFile(path) as z:
        for name in z.namelist():
            if name.endswith(".pyj"):
                out[name[:-4]] = zlib.decompress(z.read(name))
    return out


def code_objects(pyc):
    """``dotted path -> fingerprint`` for every code object in a module."""
    top = pyc27.Unmarshal(pyc, 8).load()
    out = {}

    def visit(c, prefix):
        consts = []
        kids = []
        for k in c.consts or ():
            if isinstance(k, pyc27.Code):
                kids.append(k)
            else:
                consts.append(repr(k))
        key = prefix + _text(c.name)
        n, base = 2, key
        while key in out:
            key = "%s#%d" % (base, n)
            n += 1
        out[key] = sha256(repr((c.code, tuple(c.names or ()), tuple(consts))).encode())
        for k in kids:
            visit(k, key + ".")

    visit(top, "")
    return out


def _text(v):
    return v.decode("latin-1") if isinstance(v, bytes) else str(v)


def module_names(pyc):
    """Every name and string constant in a module's code objects."""
    seen = set()
    for c in pyc27.walk(pyc27.Unmarshal(pyc, 8).load()):
        for n in c.names or ():
            seen.add(_text(n))
        for k in c.consts or ():
            if isinstance(k, (bytes, str)):
                seen.add(_text(k))
    return seen


def function_delta(a, b):
    fa, fb = code_objects(a), code_objects(b)
    changed = sorted(k for k in fa.keys() & fb.keys() if fa[k] != fb[k])
    return changed, sorted(fb.keys() - fa.keys()), sorted(fa.keys() - fb.keys())


def is_protocol_module(name):
    parts = name.lower().split("/")
    return parts[-1] in PROTOCOL_MODULES or any(p in PROTOCOL_PACKAGES for p in parts[:-1])


def call_site_modules(findings):
    """Module paths (``a/b/c``, no extension) that hold a known call site."""
    out = set()

    def add(path):
        path = path.split(":")[0].strip()
        if path.endswith(".py"):
            out.add(path[:-3])

    for fname, cols in (("client-calls.tsv", ("call_site",)),
                        ("client-callable.tsv", ("module",)),
                        ("push-events.tsv", ("handler_file",))):
        p = os.path.join(findings, fname)
        if not os.path.isfile(p):
            continue
        header = None
        with open(p, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if line.startswith("#") or not line.strip():
                    continue
                row = line.rstrip("\n").split("\t")
                if header is None:
                    header = row
                    continue
                for c in cols:
                    if c in header and header.index(c) < len(row):
                        add(row[header.index(c)])
    return out


# --- resfileindex --------------------------------------------------------------


def res_index(path):
    out = {}
    with open(path, encoding="latin-1") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            k, _, v = line.partition(",")
            out[k.lower()] = v
    return out


def res_bucket(key):
    return key.rsplit("/", 1)[0] + "/*" if "/" in key else key


def is_static(key):
    return key.startswith(STATIC_PREFIXES) or key.endswith(STATIC_SUFFIXES)


# --- loader schemas ------------------------------------------------------------


def load_pyd_schema():
    try:
        import pyd_schema  # noqa: PLC0415
    except (ImportError, SystemExit):
        return None
    return pyd_schema


def schema_tsv(mod, path, stem):
    buf = io.StringIO()
    mod.emit_tsv([mod.schema_for(path, stem)], buf)
    return buf.getvalue()


# --- the run -------------------------------------------------------------------


class Report:
    def __init__(self):
        self.rows = []
        self.reasons = []

    def row(self, layer, item, cls, old="-", new="-", detail=""):
        self.rows.append((layer, item, cls, str(old), str(new), detail or "-"))

    def because(self, why):
        if why not in self.reasons:
            self.reasons.append(why)


def run(old, new, manifest=None, findings=None, schemas=True, report=None):
    r = report or Report()
    old_build, new_build = install_build(old), install_build(new)
    r.row("build", "start.ini", "builds", old_build or "?", new_build or "?")

    # files
    fo, fn = set(list_files(old)), set(list_files(new))
    counts = {}
    code_loaders = []
    for rel in sorted(fo | fn):
        po, pn = os.path.join(old, rel), os.path.join(new, rel)
        if rel not in fn:
            cls, detail, ho, hn = "removed", "", sha256(read(stock_path(po))), "-"
        elif rel not in fo:
            cls, detail, ho, hn = "added", "", "-", sha256(read(stock_path(pn)))
        else:
            so, sn = stock_path(po), stock_path(pn)
            cls, detail = compare_file(so, sn)
            ho, hn = sha256(read(so)), sha256(read(sn))
        counts[cls] = counts.get(cls, 0) + 1
        r.row("file", rel, cls, ho, hn, detail)
        # Any change past the signature counts here, ``rebuilt`` included: a
        # few bytes in blue.dll can be a marshal tag.
        # A bin64 subdirectory file (mapObjects.db, packages/*) that changed:
        # static data when under staticdata/, else a bundled package.
        if cls not in ("identical", "signature-only") and rel.startswith("bin64/") \
                and rel.count("/") > 1:
            if rel.startswith("bin64/staticdata/"):
                r.because("static data %s %s" % (rel, cls))
            else:
                r.because("bin64 subdirectory file %s %s" % (rel, cls))
        if cls not in ("identical", "signature-only") and rel in PROTOCOL_FILES:
            r.because("protocol file %s %s" % (rel, cls))
        if cls in ("code-differs", "rebuilt") and rel.lower().endswith("loader.pyd"):
            code_loaders.append(rel)
        if cls in ("added", "removed") and rel.lower().endswith("loader.pyd"):
            r.because("loader %s %s" % (rel, cls))
    r.row("summary", "files", "counts", len(fo), len(fn),
          ", ".join("%s=%d" % kv for kv in sorted(counts.items())))

    # code.ccp
    co, cn = os.path.join(old, "code.ccp"), os.path.join(new, "code.ccp")
    if os.path.isfile(stock_path(co)) and os.path.isfile(stock_path(cn)):
        mo, mn = ccp_modules(stock_path(co)), ccp_modules(stock_path(cn))
        sites = call_site_modules(findings or os.path.join(REPO, "findings"))
        same = 0
        for name in sorted(mo.keys() | mn.keys()):
            a, b = mo.get(name), mn.get(name)
            if a is not None and b is not None and a[8:] == b[8:]:
                same += 1
                continue
            if a is None or b is None:
                cls = "added" if a is None else "removed"
                r.row("module", name, cls, "-" if a is None else sha256(a[8:]),
                      "-" if b is None else sha256(b[8:]))
                touched = b if b is not None else a
                detail = ""
            else:
                cls = "changed"
                try:
                    ch, add, rem = function_delta(a, b)
                    parts = []
                    if ch:
                        parts.append("changed " + ",".join(ch))
                    if add:
                        parts.append("added " + ",".join(add))
                    if rem:
                        parts.append("removed " + ",".join(rem))
                    detail = "; ".join(parts) or "constants only"
                except (ValueError, IndexError, struct.error) as e:
                    detail = "unparsed: %s" % e
                r.row("module", name, cls, sha256(a[8:]), sha256(b[8:]), detail)
                touched = b
            hits = []
            if name in sites:
                hits.append("call site in findings")
            try:
                names = module_names(touched) | (module_names(a) if a else set())
                hits += ["names %s" % n for n in MACHO_NAMES if n in names]
            except (ValueError, IndexError, struct.error):
                hits.append("unparsed")
            if is_protocol_module(name):
                hits.append("protocol module")
                r.because("protocol module %s %s" % (name, cls))
            if hits:
                r.because("call surface: %s (%s)" % (name, "; ".join(hits)))
            r.row("callsurface", name, "hit" if hits else "none", "-", "-",
                  "; ".join(hits))
        r.row("summary", "code.ccp", "counts", len(mo), len(mn),
              "identical past header=%d, other=%d" % (same, len(mo.keys() | mn.keys()) - same))
    else:
        r.row("summary", "code.ccp", "missing")

    # resfileindex
    for idx in ("resfileindex.txt", "resfileindex_Windows.txt", "resfileindex_prefetch.txt"):
        po, pn = stock_path(os.path.join(old, idx)), stock_path(os.path.join(new, idx))
        if not (os.path.isfile(po) and os.path.isfile(pn)):
            continue
        a, b = res_index(po), res_index(pn)
        buckets = {}
        # Sorted: the reasons land in the verdict row, which --check compares,
        # and a str set's order changes per process.
        for k in sorted(a.keys() | b.keys()):
            if k in a and k in b and a[k] == b[k]:
                continue
            kind = "+" if k not in a else "-" if k not in b else "~"
            d = buckets.setdefault(res_bucket(k), {"+": 0, "-": 0, "~": 0})
            d[kind] += 1
            if is_static(k):
                r.because("static data %s %s" % ({"+": "added", "-": "removed",
                                                    "~": "changed"}[kind], k))
        for bk in sorted(buckets):
            d = buckets[bk]
            r.row("res", "%s:%s" % (idx, bk), "static" if is_static(bk.rstrip("*"))
                  else "other", "-", "-", "+%d -%d ~%d" % (d["+"], d["-"], d["~"]))
        r.row("summary", idx, "counts", len(a), len(b), "%d buckets changed" % len(buckets))

    # loader schemas
    if code_loaders:
        mod = load_pyd_schema() if schemas else None
        for rel in code_loaders:
            stem = os.path.basename(rel)[:-len("Loader.pyd")].lower()
            if mod is None:
                r.row("schema", stem, "skipped", "-", "-", "pyd_schema needs the analysis image")
                r.because("schema of %s not checked" % stem)
                continue
            sa = schema_tsv(mod, stock_path(os.path.join(old, rel)), stem)
            sb = schema_tsv(mod, stock_path(os.path.join(new, rel)), stem)
            same = sa == sb
            r.row("schema", stem, "identical" if same else "differs",
                  sha256(sa.encode()), sha256(sb.encode()))
            if not same:
                r.because("loader schema %s differs" % stem)

    # patch sites
    mpath = manifest or os.path.join(TOOLS, "patches", "%s.json" % old_build)
    if os.path.isfile(mpath):
        import eve_patch  # noqa: PLC0415

        m = json.loads(read(mpath))
        for rel, entry in sorted(m.get("files", {}).items()):
            pn = stock_path(os.path.join(new, rel))
            if not os.path.isfile(pn):
                r.row("patch", "%s %s" % (entry.get("id", "?"), rel), "missing")
                continue
            data = read(pn)
            if entry.get("kind") == "ini":
                try:
                    eve_patch.ini_set(data, entry["set"])
                    st = "ini keys present"
                except ValueError as e:
                    st = "ini keys missing: %s" % e
                r.row("patch", "%s %s" % (entry.get("id", "?"), rel), st,
                      entry.get("sha256_stock", "-"), sha256(data),
                      "size %d -> %d" % (entry.get("size", -1), len(data)))
                continue
            if entry.get("kind") == "ccp":
                for name, st in eve_patch.ccp_stock_members(entry, data):
                    r.row("patch", "%s %s:%s" % (entry.get("id", "?"), rel, name), st,
                          entry.get("sha256_stock", "-"), sha256(data),
                          "size %d -> %d" % (entry.get("size", -1), len(data)))
                continue
            for s in entry.get("sites", ()):
                st = eve_patch.window_state(data, s)
                r.row("patch", "%s %s@%d" % (entry.get("id", "?"), rel, s["offset"]), st,
                      entry.get("sha256_stock", "-"), sha256(data),
                      "size %d -> %d" % (entry.get("size", -1), len(data)))
    else:
        r.row("patch", os.path.basename(mpath), "no manifest")

    # start.ini
    ia = stock_path(os.path.join(old, "start.ini"))
    ib = stock_path(os.path.join(new, "start.ini"))
    if os.path.isfile(ia) and os.path.isfile(ib):
        a, b = ini_values(read(ia)), ini_values(read(ib))
        for k in sorted(a.keys() | b.keys()):
            if a.get(k) != b.get(k):
                r.row("ini", k, "differs", a.get(k, "-"), b.get(k, "-"))
                if k.split(".")[-1].lower() in ("version", "codename", "region"):
                    r.because("release line: start.ini %s" % k)

    r.row("verdict", "%s->%s" % (old_build, new_build),
          "differs" if r.reasons else "equivalent", "-", "-",
          "; ".join(r.reasons) or "protocol, call surface, schemas and static data unchanged")
    return r


HEADER = ("# build-delta: tools/build-delta.py OLD NEW. Hashes, names and counts only;\n"
          "# no client bytes. Columns: layer item class old new detail.\n")


def render(r):
    out = [HEADER, "layer\titem\tclass\told\tnew\tdetail\n"]
    for row in r.rows:
        out.append("\t".join(c.replace("\t", " ").replace("\n", " ") for c in row) + "\n")
    return "".join(out)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--out", help="write the TSV here (default stdout)")
    ap.add_argument("--check", help="compare the result with this committed TSV")
    ap.add_argument("--manifest", help="patch manifest whose sites to look up in NEW")
    ap.add_argument("--findings", help="findings directory (default the repo's)")
    ap.add_argument("--no-schemas", action="store_true",
                    help="do not re-read loader schemas (no capstone needed)")
    a = ap.parse_args(argv)
    for d in (a.old, a.new):
        if not os.path.isfile(os.path.join(d, "start.ini")):
            print("build-delta: %s has no start.ini; not a client install" % d, file=sys.stderr)
            return 2
    r = run(a.old, a.new, a.manifest, a.findings, not a.no_schemas)
    text = render(r)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as fh:
            fh.write(text)
    elif not a.check:
        sys.stdout.write(text)
    verdict = r.rows[-1]
    print("build-delta: %s %s (%s)" % (verdict[1], verdict[2], verdict[5]), file=sys.stderr)
    if a.check:
        with open(a.check, encoding="utf-8") as fh:
            want = fh.read()
        if want != text:
            print("build-delta: CHECK FAILED: output differs from %s" % a.check, file=sys.stderr)
            return 1
        print("build-delta: check ok against %s" % a.check, file=sys.stderr)
    return 0 if verdict[2] == "equivalent" else 3


if __name__ == "__main__":
    sys.exit(main())
