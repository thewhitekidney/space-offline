#!/usr/bin/env python3
"""Authoring helper for eve_patch manifests (O17).

    tools/analysis.sh python3 -I tools/eve_patch_author.py check  /client [--manifest M]
    tools/analysis.sh python3 -I tools/eve_patch_author.py locate /client [--only C1,B3]
                                                                  [--merge BASE.json]
    tools/analysis.sh python3 -I tools/eve_patch_author.py hosts  /client
                                                                  [--known FILE] [--write-known]

`check` reads a client install (read-only) and reports, for every file in
the build's manifest, whether the stock file (or its `.eveoffline-orig`
backup) hashes to `sha256_stock`, and whether eve_patch's own transform of
those bytes is the manifest's patched form and moves every site's window
from stock to patched. A manifest that passes here and is then applied by
`tools/eve_patch.py` cannot disagree with it.

`locate` finds the recipes of a NEW build structurally, from the client's
STOCK bytes (a file with an `.eveoffline-orig` backup is read from the
backup), and prints the manifest entries it found, with every hash taken by
applying its own transform to the stock bytes. A rule that does not hit
exactly the expected number of times REFUSES (exit 2) and prints nothing.
Rules: C1 (code.ccp, the anchor table CCP_RULES), B1 (the Crashpad URL
length), B2 (the six LaunchDarkly hosts), B3 (the Streamline OTA flags).
B0 and P0 stay with `tools/client-patch-manifest.py`; `--merge BASE.json`
puts the located entries into a copy of an existing manifest. The anchor
table holds host names, as manifests do; keep real names out of tests.

`hosts` lists every host-like string in the client (the marshal strings of
every `code.ccp` member, and the strings of every file in `bin64`) and diffs
it against a reviewed list (default `out/census/known-hosts.txt`, outside the
repository). A host that list does not hold exits 1: it is the alarm for a
new build that grew a new outbound path. `--write-known` writes the current
set as the new list; the human review is what makes it "reviewed".

Stdlib only, plus capstone (the analysis image has it) for B3 only.
"""

import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import json
import re
import struct
import sys
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.append(str(HERE))  # last: nothing in tools/ shadows the stdlib

import eve_patch as ep  # noqa: E402

# eve_patch's own "tools/ last" shuffle pops sys.path[0] once more, which now
# is the real stdlib directory; put tools/ back at the very end.
sys.path[:] = [p for p in sys.path if p != str(HERE)] + [str(HERE)]

WINDOW = 16


_MANIFEST_CACHE = {}


def trusted_backup(client, rel):
    """The `.eveoffline-orig` of `rel` when it is a stock file the current
    build's manifest vouches for (it hashes to `sha256_stock`), else None.

    A backup left by an older build must never stand in for the files of the
    build being read: a launcher update leaves one behind.
    """
    b = ep.backup_of(Path(client) / rel)
    if not b.is_file():
        return None
    build = ep.read_build(client)
    mp = ep.PATCHES / f"{build}.json" if build else None
    if mp is None or not mp.is_file():
        return None
    if mp not in _MANIFEST_CACHE:
        try:
            _MANIFEST_CACHE[mp] = ep.load_manifest(mp)
        except ep.Refuse:
            _MANIFEST_CACHE[mp] = None
    m = _MANIFEST_CACHE[mp]
    e = m["files"].get(rel) if m else None
    if e and ep.sha256_file(b) == e["sha256_stock"]:
        return b
    return None


def stock_bytes(client, rel):
    """(bytes, source name): a trusted backup when there is one, else the file
    itself (with a warning when an untrusted backup is lying beside it)."""
    p = Path(client) / rel
    src = trusted_backup(client, rel)
    if src is None:
        if ep.backup_of(p).exists():
            print(f"eve_patch_author: warning: {rel}{ep.BACKUP} does not match this build's manifest; reading {rel} itself", file=sys.stderr)
        src = p
    if not src.is_file():
        raise ep.Refuse(f"{rel}: missing")
    return src.read_bytes(), src.name


def check(client, manifest_path):
    m = ep.load_manifest(manifest_path)
    ok = True
    for rel, e in m["files"].items():
        p = Path(client) / rel
        b = ep.backup_of(p)
        src = b if b.exists() else p
        if not src.is_file():
            print(f"FAIL {rel}: missing")
            ok = False
            continue
        data = src.read_bytes()
        if ep.sha256(data) != e["sha256_stock"]:
            print(f"FAIL {rel}: {src.name} is not this manifest's stock file")
            ok = False
            continue
        try:
            out = ep.transform(e, data)
        except ep.Refuse as r:
            print(f"FAIL {rel} [{e['id']}]: {r}")
            ok = False
            continue
        good = ep.is_patched(e, out)
        for s in e.get("sites", []):
            good = good and ep.window_state(data, s) == "stock" and ep.window_state(out, s) == "patched"
        if e["kind"] == "ccp":
            good = good and ep.ccp_canon(e, data) is None  # stock must not read as patched
        print(f"{'ok  ' if good else 'FAIL'} {rel} [{e['id']}] (read from {src.name})")
        ok = ok and good
    return 0 if ok else 1


# --------------------------------------------------------------------------
# locate: code.ccp (tier C)

# member, [(anchor, expected hits, exact value instead of substring)], replacement
# The replacement REPLACES the whole marshal string. Each is a dead local
# address or a reserved name, so the lookup or connect fails at once.
DEAD = "http://127.0.0.1:9/"
CCP_RULES = [
    ("autoexec_client_core.pyj", [(b"@sentry.io/", 1, False)], "http://0@127.0.0.1:9/0"),
    ("autoexec_client_core.pyj", [(b"clientresources.", 1, False)], DEAD),
    ("carbon/common/lib/autoexec_client_core.pyj", [(b"@sentry.io/", 1, False)], "http://0@127.0.0.1:9/0"),
    ("carbon/common/lib/autoexec_client_core.pyj", [(b"clientresources.", 1, False)], DEAD),
    ("remotefilecache/__init__.pyj", [(b"clientresources.", 1, False), (b"res.eveprobe.", 1, False)], DEAD),
    ("monolithipify/__init__.pyj", [(b"api.ipify.org", 1, False)], "http://0.0.0.0:9/"),
    ("charactercreator/client/characterCreationMetrics.pyj", [(b"google-analytics.com/collect", 1, False)], DEAD),
    ("eve/client/script/ui/services/bugReporting.pyj", [(b"ebr-service.", 1, False)], "http://127.0.0.1:9/v1"),
    ("victoriametrics/metrics_reporter.pyj", [(b"vm-single.", 1, False)], "http://127.0.0.1:9/?"),
    ("publicGateway/grpc/connectionConfig.pyj", [(b"evetech.net", 2, True)], "gateway.invalid"),
]


def marshal_strings(raw):
    """[(offset of the type byte, type char, value bytes)] of every string
    (`s`, `t`, `u`) in a Python 2.7 .pyc, with offsets in `raw`."""
    import pyc27  # noqa: PLC0415  (tools/ is last on sys.path, so the stdlib wins)

    class Pos(pyc27.Unmarshal):
        def load(self):
            st = self.p
            t = chr(self.d[st])
            v = super().load()
            if t in "stu":
                self.strs.append((st, t, v if isinstance(v, bytes) else v.encode("utf-8")))
            return v

    if raw[:4] != b"\x03\xf3\r\n":
        raise ep.Refuse("a member is not a Python 2.7 .pyc")
    u = Pos(raw, 8)
    u.strs = []
    u.load()
    return u.strs


def inflate_member(data, idx, name):
    mem = {"name": name, "slot": 0}
    got = idx.get(name, [])
    if len(got) != 1:
        raise ep.Refuse(f"code.ccp: {len(got)} members named {name}, want exactly 1")
    mem["slot"] = got[0]["cs"]
    z = ep.ccp_member(data, idx, mem)
    raw, _ = ep.ccp_inflate(data, z, mem)
    return raw, mem["slot"]


def locate_ccp(data):
    """The `code.ccp` manifest entry (id C1, entry-level hashes included)."""
    idx = ep.zip_index(data)
    by_member = {}
    for name, anchors, repl in CCP_RULES:
        raw, slot = inflate_member(data, idx, name)
        m = by_member.setdefault(name, {"name": name, "slot": slot, "raw": raw, "sites": []})
        strs = marshal_strings(raw)
        for anchor, want, exact in anchors:
            hits = [(o, t, v) for o, t, v in strs if (v == anchor if exact else anchor in v)]
            if len(hits) != want or raw.count(anchor) != want:
                raise ep.Refuse(f"locate C1: {name}: anchor {anchor!r} hits {len(hits)} string(s), {raw.count(anchor)} raw; want exactly {want}")
            for o, t, v in hits:
                if t not in "st":
                    raise ep.Refuse(f"locate C1: {name}: anchor {anchor!r} is in a {t!r} string, not s or t")
                m["sites"].append({"offset": o, "old_len": len(v), "after_ascii": repl, "note": anchor.decode()})
    members = []
    for m in by_member.values():
        m["sites"].sort(key=lambda s: s["offset"])
        members.append({"name": m["name"], "slot": m["slot"], "raw_sha256_stock": ep.sha256(m["raw"]), "raw_sha256_patched": "0" * 64,
                        "sites": [{k: s[k] for k in ("offset", "old_len", "after_ascii")} | {"note": s["note"]} for s in m["sites"]]})
    entry = {"id": "C1", "tier": "C", "kind": "ccp", "mark": "I", "size": len(data), "sha256_stock": ep.sha256(data),
             "sha256_patched": "0" * 64,
             "why": "dead-address constants for the Sentry DSN, the resource CDN (three of them), the public-IP lookup, Google Analytics, the bug-report service, VictoriaMetrics and the gateway discovery suffix, so the client reaches nothing even without the launcher's env [I: the boot gate proves the zip edit]",
             "members": members}
    # Patched inflated hashes come from applying our own recipe to the stock raw bytes.
    for mem in members:
        raw = by_member[mem["name"]]["raw"]
        new = bytearray(raw)
        for s in sorted(mem["sites"], key=lambda s: -s["offset"]):
            a = s["after_ascii"].encode("ascii")
            new[s["offset"] + 5 : s["offset"] + 5 + s["old_len"]] = a
            new[s["offset"] + 1 : s["offset"] + 5] = struct.pack("<I", len(a))
        mem["raw_sha256_patched"] = ep.sha256(bytes(new))
    out = ep.ccp_transform(entry, data)  # refuses when a member does not fit its slot
    entry["sha256_patched"] = ep.ccp_canon(entry, out)
    if entry["sha256_patched"] is None:
        raise ep.Refuse("locate C1: the patched file is not its own canonical form")
    return entry


# --------------------------------------------------------------------------
# locate: PE files (B1, B3)


class PE:
    def __init__(self, data):
        if data[:2] != b"MZ":
            raise ep.Refuse("not a PE file")
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        if data[pe : pe + 4] != b"PE\0\0":
            raise ep.Refuse("not a PE file")
        nsec, = struct.unpack_from("<H", data, pe + 6)
        osz, = struct.unpack_from("<H", data, pe + 20)
        self.base, = struct.unpack_from("<Q", data, pe + 24 + 24)
        self.secs = []
        p = pe + 24 + osz
        for i in range(nsec):
            name, vsz, va, rsz, roff, _, _, _, _, ch = struct.unpack_from("<8sIIIIIIHHI", data, p + 40 * i)
            self.secs.append({"name": name.rstrip(b"\0"), "va": va, "vsz": vsz, "roff": roff, "rsz": rsz, "exec": bool(ch & 0x20000000)})

    def off2rva(self, off):
        for s in self.secs:
            if s["roff"] <= off < s["roff"] + s["rsz"]:
                return s["va"] + off - s["roff"]
        return None

    def rva2off(self, rva):
        for s in self.secs:
            if s["va"] <= rva < s["va"] + min(s["vsz"], s["rsz"]):
                return s["roff"] + rva - s["va"]
        return None


def lea_sites(data, pe, target_rva, regs=None):
    """File offsets of every `lea r64,[rip+rel32]` in executable sections that
    resolves to `target_rva`."""
    out = []
    pat = re.compile(rb"[\x48\x4c]\x8d[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]", re.S)
    for s in pe.secs:
        if not s["exec"]:
            continue
        lo, hi = s["roff"], s["roff"] + s["rsz"]
        for m in pat.finditer(data, lo, hi):
            o = m.start()
            if o + 7 > hi:
                continue
            rel, = struct.unpack_from("<i", data, o + 3)
            if pe.off2rva(o) + 7 + rel == target_rva:
                out.append(o)
    return out


def window_entry(stock, patched, off, n):
    return {"window": WINDOW,
            "window_sha256_stock": ep.sha256(stock[off - WINDOW : off + n + WINDOW]),
            "window_sha256_patched": ep.sha256(patched[off - WINDOW : off + n + WINDOW])}


def bytes_entry(fid, rel, why, mark, stock, sites, requires_note=None):
    """A `bytes` entry from (offset, after bytes, note) sites, hashes by our own transform."""
    patched = bytearray(stock)
    for o, a, _ in sites:
        patched[o : o + len(a)] = a
    patched = bytes(patched)
    out_sites = []
    for o, a, note in sites:
        s = {"offset": o}
        if len(a) > 4 and all(32 <= c < 127 for c in a):
            s["after_ascii"] = a.decode()
        else:
            s["after_hex"] = a.hex()
        s.update(window_entry(stock, patched, o, len(a)))
        s["note"] = note
        out_sites.append(s)
    return {"id": fid, "tier": "B", "kind": "bytes", "size": len(stock), "sha256_stock": ep.sha256(stock),
            "sha256_patched": ep.sha256(patched), "why": why, "mark": mark, "sites": out_sites}


def locate_b1(data):
    """The Crashpad URL length: `mov r8d, strlen` right before the `lea` of the minidump URL."""
    pe = PE(data)
    hits = []
    for m in re.finditer(rb"https://sentry\.io/api/[ -~]*?\0", data):
        rva = pe.off2rva(m.start())
        if rva is None:
            continue
        n = m.end() - m.start() - 1
        for o in lea_sites(data, pe, rva):
            if data[o - 6 : o - 4] == b"\x41\xb8" and struct.unpack_from("<I", data, o - 4)[0] == n:
                hits.append(o - 4)
    if len(hits) != 1:
        raise ep.Refuse(f"locate B1: {len(hits)} Crashpad URL length sites, want exactly 1")
    return bytes_entry("B1", "bin64/exefile.exe",
                       "Crashpad upload URL built with length 0 (imm32 of mov r8d,len before lea rdx,[minidump URL]); Crashpad passes no --url and starts no upload thread [I]",
                       "I", data, [(hits[0], b"\0\0\0\0", "minidump URL length")])


def locate_b3(data):
    """The Streamline `Preferences.flags` default: in the function that names
    `slInit`, the one `mov qword [rbp+d], 0x49` followed by `or qword [rbp+d], 0x84`.
    The byte patched is the low byte of the imm32, 0x49 -> 0x01 (clears 0x40 and 0x08 once the 0x84 is or'ed in)."""
    try:
        import capstone  # noqa: PLC0415
    except ImportError:
        raise ep.Refuse("locate B3 needs capstone (tools/analysis.sh has it)")
    pe = PE(data)
    leas = []
    for m in re.finditer(rb"(?<=\0)slInit\0", data):
        rva = pe.off2rva(m.start())
        if rva is not None:
            leas += lea_sites(data, pe, rva)
    if len(leas) != 1:
        raise ep.Refuse(f"locate B3: {len(leas)} references to the slInit name, want exactly 1")
    start = leas[0]
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = False
    movs, ors = [], []
    for i in md.disasm(data[start : start + 0x600], pe.off2rva(start)):
        if i.mnemonic == "mov" and re.fullmatch(r"qword ptr \[rbp - 0x[0-9a-f]+\], 0x49", i.op_str):
            movs.append((i, i.op_str.split("],")[0]))
        elif i.mnemonic == "or" and re.fullmatch(r"qword ptr \[rbp - 0x[0-9a-f]+\], 0x84", i.op_str):
            ors.append((i, i.op_str.split("],")[0]))
    if len(movs) != 1 or len(ors) != 1 or movs[0][1] != ors[0][1] or ors[0][0].address < movs[0][0].address:
        raise ep.Refuse(f"locate B3: found {len(movs)} `mov [rbp-d], 0x49` and {len(ors)} `or [rbp-d], 0x84` after slInit, want one pair on the same slot")
    i = movs[0][0]
    off = pe.rva2off(i.address) + i.size - 4  # low byte of the imm32
    if data[off] != 0x49:
        raise ep.Refuse("locate B3: immediate is not where the encoding says")
    return bytes_entry("B3", "bin64/_trinity_dx12.dll",
                       "NVIDIA Streamline default Preferences.flags 0x49 -> 0x01 (before the or 0x84): no over-the-air updater (nvngx_update.exe) and no downloaded plugins; DLSS itself stays [I: flag names from the SDK header, from memory]",
                       "I", data, [(off, b"\x01", "slInit Preferences.flags default, low byte")])


def locate_b2(data):
    """The six LaunchDarkly hosts: `launchdarkly.com` inside `https://` strings."""
    sites = []
    for m in re.finditer(rb"launchdarkly\.com", data):
        s = data.rfind(b"\0", 0, m.start()) + 1
        if data[s : s + 8] == b"https://" or data[s : m.start()].count(b".") <= 1 and re.fullmatch(rb"[a-z0-9.]*", data[s : m.start()]):
            e = data.find(b"\0", m.end())
            sites.append((m.start(), data[s:e].decode("ascii", "replace")))
    if len(sites) != 6:
        raise ep.Refuse(f"locate B2: {len(sites)} LaunchDarkly host strings, want exactly 6")
    return bytes_entry("B2", "bin64/launchdarkly_stackless_client_sdk.pyd",
                       "LaunchDarkly has no offline switch in this client; rename its six hosts to the same-length reserved name lndarkly.invalid (RFC 6761), which never resolves",
                       "V", data, [(o, b"lndarkly.invalid", t) for o, t in sites])


LOCATE = {
    "C1": ("code.ccp", locate_ccp),
    "B1": ("bin64/exefile.exe", locate_b1),
    "B2": ("bin64/launchdarkly_stackless_client_sdk.pyd", locate_b2),
    "B3": ("bin64/_trinity_dx12.dll", locate_b3),
}
REQUIRES = {"C1": ["B0"], "B1": ["B0"], "B2": ["B0"], "B3": ["B0"]}


def locate(client, only, merge):
    found, rels = {}, {}
    for fid in only:
        rel, fn = LOCATE[fid]
        data, src = stock_bytes(client, rel)
        entry = fn(data)
        found[fid], rels[fid] = entry, rel
        print(f"locate {fid}: {rel} (read from {src}): ok", file=sys.stderr)
    if merge:
        m = json.loads(Path(merge).read_text(encoding="utf-8"))
    else:
        m = {"files": {}, "requires": {}}
    for fid, entry in found.items():
        old = m["files"].get(rels[fid])
        if old and old["id"] != fid:
            raise ep.Refuse(f"{rels[fid]} is {old['id']} in the base manifest, not {fid}")
        m["files"][rels[fid]] = entry
        m.setdefault("requires", {})[fid] = REQUIRES[fid]
    return m


# --------------------------------------------------------------------------
# hosts

TLDS = "com|net|org|io|info|dev|invalid|cn|eu|gg|tv|co|me|us|uk|de|ru|jp|kr|fr|ai|ms|xyz|tech|ca|nl|se|ch|es|au|br"
# A URL of any scheme-://host (any TLD), with an optional `userinfo@` (a Sentry
# DSN is `https://key@host/id`), or a bare dotted name under a known TLD, which
# may follow an `@`.
HOST_RE = re.compile(rb"(?<![A-Za-z0-9._-])(?:[A-Za-z][A-Za-z0-9+.-]*)://(?:[^\s/@:]*(?::[^\s/@]*)?@)?[A-Za-z0-9._-]+(?::\d+)?|(?<![A-Za-z0-9._-])(?:[A-Za-z0-9][A-Za-z0-9-]*\.)+(?:%s)(?![A-Za-z0-9_])" % TLDS.encode())
U16_RE = re.compile(rb"(?:[A-Za-z0-9.:/_-]\x00){6,}")


def host_of(b):
    s = b.decode("ascii", "replace").lower()
    s = re.sub(r"^[a-z][a-z0-9+.-]*://", "", s)
    s = s.rsplit("@", 1)[-1]
    s = s.split(":", 1)[0].strip(".")
    # Protobuf descriptors glue a length byte before `github.com`, and a
    # certificate's OID digits can trail a name: both are noise, not hosts.
    if s.endswith("github.com") and not s.endswith(".github.com"):
        s = "github.com"
    s = re.sub(r"(\.(?:%s))\d+$" % TLDS, r"\1", s)
    return s


def scan_hosts(blob, urls_only=False):
    """Host names in `blob`. `urls_only`: count only `scheme://` matches (for a
    long text blob, where a bare dotted name is a credit line, not a host)."""
    out = set()
    for m in HOST_RE.finditer(blob):
        if urls_only and b"://" not in m.group():
            continue
        h = host_of(m.group())
        if "." in h and len(h) > 3:
            out.add(h)
    return out


def collect_hosts(client):
    """{host: sorted sources} over code.ccp members (inflated) and bin64 files."""
    found = {}

    def add(hs, src):
        for h in hs:
            found.setdefault(h, set()).add(src)

    ccp = Path(client) / "code.ccp"
    if ccp.is_file():
        data, _ = stock_bytes(client, "code.ccp")
        for name, ents in ep.zip_index(data).items():
            for z in ents:
                lo = z["lo"]
                nl, el = struct.unpack_from("<HH", data, lo + 26)
                body = data[lo + 30 + nl + el : lo + 30 + nl + el + z["cs"]]
                try:
                    raw = zlib.decompress(body)
                except zlib.error:
                    add({"unparsed-member:" + name}, "code.ccp:" + name)  # never silent
                    continue
                # Marshal strings are length-prefixed, not delimited, so a scan of the
                # inflated bytes glues neighbours together: walk the strings. A member
                # the walker cannot parse is scanned raw, and noisily.
                try:
                    vals = [v for _, _, v in marshal_strings(raw)]
                except Exception:
                    vals = [raw]
                    add({"unparsed-member:" + name}, "code.ccp:" + name)
                for v in vals:
                    add(scan_hosts(v, urls_only=len(v) >= 512), "code.ccp:" + name)
    root = Path(client) / "bin64"
    for p in sorted(root.rglob("*")):
        if not p.is_file() or p.name.endswith((ep.BACKUP, ep.TMP)):
            continue
        rel = p.relative_to(Path(client)).as_posix()
        bk = trusted_backup(client, rel)
        blob = bk.read_bytes() if bk else p.read_bytes()  # stock where a trusted backup vouches for it
        add(scan_hosts(blob), rel)
        wide = b"".join(m.group().replace(b"\x00", b"") + b"\n" for m in U16_RE.finditer(blob))
        add(scan_hosts(wide), rel)
    return found


def read_known(path):
    out = set()
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            out.add(line.lower())
    return out


def cmd_hosts(client, known_path, write):
    found = collect_hosts(client)
    if write:
        Path(known_path).parent.mkdir(parents=True, exist_ok=True)
        Path(known_path).write_text("# known outbound hosts, one per line; review before trusting\n" + "".join(h + "\n" for h in sorted(found)), encoding="utf-8")
        print(f"hosts: wrote {len(found)} host(s) to {known_path}")
        return 0
    if not Path(known_path).is_file():
        print(f"hosts: no known-hosts list at {known_path}; run with --write-known and review it", file=sys.stderr)
        return 2
    known = read_known(known_path)
    new = sorted(set(found) - known)
    for h in sorted(found):
        print(f"{'NEW  ' if h in new else 'known'} {h}  ({len(found[h])} place(s): {', '.join(sorted(found[h])[:3])})")
    gone = sorted(known - set(found))
    if gone:
        print(f"hosts: {len(gone)} known host(s) no longer present")
    if new:
        print(f"hosts: {len(new)} NEW host(s); review each before this build is trusted", file=sys.stderr)
        return 1
    print(f"hosts: ok, {len(found)} host(s), none new")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("command", choices=["check", "write", "locate", "hosts"])
    ap.add_argument("client")
    ap.add_argument("--manifest", help="check: default tools/patches/<build from start.ini>.json")
    ap.add_argument("--only", help="locate: ids, default every rule (C1,B1,B2,B3)")
    ap.add_argument("--merge", help="locate: base manifest to put the located entries into")
    ap.add_argument("--known", default=str(ep.REPO / "out" / "census" / "known-hosts.txt"), help="hosts: the reviewed list")
    ap.add_argument("--write-known", action="store_true", help="hosts: write the current set as the list")
    a = ap.parse_args(argv)
    try:
        if a.command == "write":
            print("eve_patch_author: write is not done; use `locate --merge`", file=sys.stderr)
            return 2
        if a.command == "hosts":
            return cmd_hosts(a.client, a.known, a.write_known)
        if a.command == "locate":
            only = [x.strip() for x in a.only.split(",")] if a.only else list(LOCATE)
            bad = [x for x in only if x not in LOCATE]
            if bad:
                print(f"eve_patch_author: no rule {', '.join(bad)} (have {', '.join(LOCATE)})", file=sys.stderr)
                return 2
            m = locate(a.client, only, a.merge)
            if a.merge is None:
                m = m["files"]
            print(json.dumps(m, indent=1))
            return 0
        mp = a.manifest
        if mp is None:
            build = ep.read_build(a.client)
            if build is None:
                print("eve_patch_author: no build in start.ini; pass --manifest", file=sys.stderr)
                return 2
            mp = ep.PATCHES / f"{build}.json"
        return check(a.client, mp)
    except ep.Refuse as e:
        print(f"eve_patch_author: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
