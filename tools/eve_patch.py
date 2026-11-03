#!/usr/bin/env python3
"""Take the phone-home paths out of YOUR OWN copy of the EVE client.

Owner decision O11 (docs/port-tracker.md §81): each friend patches the copy
they own, run by our setup scripts; this repository ships recipes, never
client bytes. O17: the recipes are per build, one JSON manifest per build in
`tools/patches/<build>.json`. The research behind every recipe, with its
[V]/[I] marks, is `out/research/R-PATCH.md`; the operator doc is
`docs/guides/patcher.md`.

    python3 -I tools/eve_patch.py status     --client DIR
    python3 -I tools/eve_patch.py verify     --client DIR
    python3 -I tools/eve_patch.py plan       --client DIR
    python3 -I tools/eve_patch.py apply      --client DIR [--tier A,P,B,C] [--only B0,B1]
    python3 -I tools/eve_patch.py revert     --client DIR
    python3 -I tools/eve_patch.py launch-env --client DIR [--server ADDR] [--json]
                                             [--check-args "ARGS"]

`--build N` names the build instead of reading it from `start.ini`.

The model, in one paragraph. A target file is `stock`, `patched` or
`unknown`, decided by its whole-file sha256 against the manifest -- nothing is
located at apply time. Every write command refuses if any target is
`unknown`, so a different build, a half-patched file or a file someone else
edited is never touched. A file is patched by writing all of its recipes into
`<file>.eveoffline-tmp`, checking that the result hashes to `sha256_patched`,
then `os.replace`; it is never half done. The first write of a file leaves a
byte-for-byte backup in `<file>.eveoffline-orig`, which is trusted only while
it hashes to `sha256_stock`. `apply` on a patched file is a no-op.

The tiers. A = the launch env/args (`launch-env`; no file is written).
P = `prefs.ini` keys (a user file the client's manifest does not cover).
B = byte and ini edits on files `manifest.dat` covers; they need B0, the
manifest self-check patch in `blue.dll`, and `requires` enforces that.
C = constants inside `code.ccp` members (kind `ccp`): the member is inflated,
a marshal string replaced, and recompressed into its own slot, so no offset
in the archive moves. It is in the default tiers since the re-pin to 3662168
(docs/guides/patcher.md, "Tier C"); `--tier A,P,B` leaves it out. Compressed bytes differ
between hosts' zlib builds, so a `ccp` entry's `sha256_patched` is the
canonical hash `ccp_canon` computes, not a whole-file hash.

Exit codes: 0 ok; 1 not (fully) applied, or a check failed; 2 refused
(unknown hash, no manifest for the build, bad arguments, a guard), and
nothing was written; 3 `launch-env --check-args` found a forbidden argument.

Stdlib only, run with `python3 -I` so a module planted in the client
directory cannot be imported.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins (the
# convention every tool here follows; `tools/dis.py` has the reason).
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import glob
import hashlib
import json
import os
import re
import struct
import sys
import zlib
from pathlib import Path

SCHEMA = 1
BACKUP = ".eveoffline-orig"
TMP = ".eveoffline-tmp"
PRIOR = ".eveoffline-prior.json"
HERE = Path(__file__).resolve().parent
REPO = HERE.parent
PATCHES = HERE / "patches"
TIERS = ("A", "P", "B", "C")
KINDS = ("bytes", "ini", "ccp")
DEFAULT_TIERS = "A,P,B,C"  # C joined at the 3662168 re-pin, after the boot of a tier C copy (docs/guides/patcher.md)
MAX_AFTER = 64
HEX64 = re.compile(r"^[0-9a-f]{64}$")
ENV_NAME = re.compile(r"^[A-Z_][A-Z0-9_]*$")
PREF_KEY = re.compile(r"^[A-Za-z0-9_.]+$")


class Refuse(Exception):
    """A refusal: nothing was written. Carries the exit code."""

    def __init__(self, msg, code=2):
        super().__init__(msg)
        self.code = code


# --------------------------------------------------------------------------
# hashing and the manifest


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def after_bytes(site):
    if "after_hex" in site:
        return bytes.fromhex(site["after_hex"])
    return site["after_ascii"].encode("ascii")


def _walk_keys(obj):
    if isinstance(obj, dict):
        for k, v in obj.items():
            yield k
            yield from _walk_keys(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from _walk_keys(v)


def validate_manifest(m, name=None):
    """Every rule that keeps a manifest correct and free of client bytes.

    Returns a list of problems (empty when the manifest is good). `name` is
    the file name, whose stem must be the build.
    """
    errs = []

    def err(msg):
        errs.append(msg)

    if not isinstance(m, dict):
        return ["manifest is not an object"]
    if m.get("schema") != SCHEMA:
        err(f"schema must be {SCHEMA}")
    build = m.get("build")
    if not isinstance(build, int) or isinstance(build, bool) or build < 0:
        err("build must be a non-negative integer")
    elif name is not None and Path(name).stem != str(build):
        err(f"file {name} does not match build {build}")
    for k in _walk_keys(m):
        if k == "before" or k.startswith("before_"):
            err(f"key {k!r}: a manifest never holds original bytes")
    ident = m.get("identify", {})
    if not isinstance(ident, dict) or ident.get("start_ini_build") != str(build):
        err("identify.start_ini_build must equal the build, as a string")
    files = m.get("files")
    if not isinstance(files, dict) or not files:
        err("files must be a non-empty object")
        files = {}
    ids = {}
    for rel, e in files.items():
        where = f"files[{rel}]"
        if rel.startswith("/") or ".." in Path(rel).parts or "\\" in rel:
            err(f"{where}: path must be relative, with / separators")
        if not isinstance(e, dict):
            err(f"{where}: not an object")
            continue
        fid = e.get("id")
        if not isinstance(fid, str) or not fid:
            err(f"{where}: id missing")
        elif fid in ids:
            err(f"{where}: id {fid} also used by {ids[fid]}")
        else:
            ids[fid] = rel
        if e.get("tier") not in TIERS or e.get("tier") == "A":
            err(f"{where}: tier must be P, B or C")
        if (e.get("tier") == "C") != (e.get("kind") == "ccp"):
            err(f"{where}: tier C and kind ccp go together")
        size = e.get("size")
        if not isinstance(size, int) or size <= 0:
            err(f"{where}: size must be a positive integer")
            size = 0
        for h in ("sha256_stock", "sha256_patched"):
            if not isinstance(e.get(h), str) or not HEX64.match(e[h]):
                err(f"{where}: {h} must be 64 lowercase hex characters")
        if e.get("sha256_stock") == e.get("sha256_patched"):
            err(f"{where}: stock and patched hashes are equal")
        kind = e.get("kind")
        if kind not in KINDS:
            err(f"{where}: kind must be one of {KINDS}")
        elif kind == "bytes":
            sites = e.get("sites")
            if not isinstance(sites, list) or not sites:
                err(f"{where}: bytes needs a non-empty sites list")
                sites = []
            spans = []
            for i, s in enumerate(sites):
                sw = f"{where}.sites[{i}]"
                if not isinstance(s, dict):
                    err(f"{sw}: not an object")
                    continue
                has = [k for k in ("after_hex", "after_ascii") if k in s]
                if len(has) != 1:
                    err(f"{sw}: exactly one of after_hex/after_ascii")
                    continue
                try:
                    a = after_bytes(s)
                except (ValueError, UnicodeEncodeError):
                    err(f"{sw}: {has[0]} does not decode")
                    continue
                if not 0 < len(a) <= MAX_AFTER:
                    err(f"{sw}: replacement must be 1..{MAX_AFTER} bytes")
                off, w = s.get("offset"), s.get("window")
                if not isinstance(off, int) or off < 0:
                    err(f"{sw}: offset must be a non-negative integer")
                    continue
                if not isinstance(w, int) or not 0 <= w <= 64:
                    err(f"{sw}: window must be an integer 0..64")
                    continue
                if off + len(a) > size:
                    err(f"{sw}: offset + length runs past size")
                if off - w < 0 or off + len(a) + w > size:
                    err(f"{sw}: window runs outside the file")
                for h in ("window_sha256_stock", "window_sha256_patched"):
                    if not isinstance(s.get(h), str) or not HEX64.match(s[h]):
                        err(f"{sw}: {h} must be 64 lowercase hex characters")
                spans.append((off, off + len(a)))
            spans.sort()
            for (a0, a1), (b0, _) in zip(spans, spans[1:]):
                if b0 < a1:
                    err(f"{where}: sites overlap at {b0}")
        elif kind == "ccp":
            ccp_lint(e, where, err)
        elif kind == "ini":
            sets = e.get("set")
            if not isinstance(sets, dict) or not sets:
                err(f"{where}: ini needs a non-empty set object")
            else:
                for k, v in sets.items():
                    if k.count(".") != 1 or not all(k.split(".")):
                        err(f"{where}: ini key {k!r} must be section.key")
                    if not isinstance(v, str) or len(v.encode()) > MAX_AFTER or "\n" in v or "\r" in v:
                        err(f"{where}: ini value for {k!r} must be one short line")
            if e.get("eol", "keep") not in ("keep", "lf", "crlf"):
                err(f"{where}: eol must be keep, lf or crlf")
    req = m.get("requires", {})
    if not isinstance(req, dict):
        err("requires must be an object")
        req = {}
    for rel, e in files.items():
        if isinstance(e, dict) and e.get("kind") == "ccp" and "B0" not in req.get(e.get("id"), []):
            err(f"files[{rel}]: a ccp entry must require B0")
    for k, v in req.items():
        if k not in ids:
            err(f"requires: {k} is not a file id")
        if not isinstance(v, list) or any(x not in ids for x in v):
            err(f"requires[{k}]: every entry must be a file id")
    try:
        order_ids(list(ids), req)
    except ValueError as e:
        err(str(e))
    prefs = m.get("prefs", {})
    if not isinstance(prefs, dict):
        err("prefs must be an object")
        prefs = {}
    for k, v in prefs.get("set", {}).items():
        if not PREF_KEY.match(k):
            err(f"prefs.set: bad key {k!r}")
        if not isinstance(v, str) or "\n" in v or "\r" in v:
            err(f"prefs.set[{k}]: value must be a one-line string")
    for k in prefs.get("forbid", []):
        if not isinstance(k, str) or not PREF_KEY.match(k):
            err(f"prefs.forbid: bad key {k!r}")
        if k in prefs.get("set", {}):
            err(f"prefs: {k} is both set and forbidden")
    launch = m.get("launch", {})
    if not isinstance(launch, dict):
        err("launch must be an object")
        launch = {}
    for k, v in launch.get("env", {}).items():
        if not ENV_NAME.match(k):
            err(f"launch.env: bad name {k!r}")
        if not isinstance(v, str) or not v or any(c in v for c in "\n\r\0 "):
            err(f"launch.env[{k}]: must be set or absent, never empty, no whitespace")
    for key in ("args", "forbid_args"):
        for a in launch.get(key, []):
            if not isinstance(a, str) or not a.startswith("/") or any(c.isspace() for c in a):
                err(f"launch.{key}: {a!r} must be a /flag with no whitespace")
    for a in launch.get("args", []):
        if forbidden_arg(a, launch.get("forbid_args", [])):
            err(f"launch.args: {a} is itself forbidden")
    for r in m.get("resfiles_check", []):
        if not isinstance(r, str) or "/" in r or "\\" in r or r.startswith("."):
            err(f"resfiles_check: {r!r} must be a bare file name")
    return errs


def order_ids(ids, requires):
    """`ids` sorted so that each comes after everything it requires."""
    out, seen, stack = [], set(), set()

    def visit(i):
        if i in seen:
            return
        if i in stack:
            raise ValueError(f"requires has a cycle through {i}")
        stack.add(i)
        for r in requires.get(i, []):
            visit(r)
        stack.discard(i)
        seen.add(i)
        out.append(i)

    for i in ids:
        visit(i)
    return out


def load_manifest(path):
    path = Path(path)
    try:
        m = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise Refuse(f"cannot read manifest {path}: {e}")
    errs = validate_manifest(m, path.name)
    if errs:
        raise Refuse(f"manifest {path} is invalid:\n  " + "\n  ".join(errs))
    return m


def read_build(client):
    """`build` from the [main] section of start.ini, or None."""
    try:
        text = (Path(client) / "start.ini").read_bytes().decode("latin-1")
    except OSError:
        return None
    for line in text.splitlines():
        k, sep, v = line.partition("=")
        if sep and k.strip().lower() == "build":
            return v.strip()
    return None


# --------------------------------------------------------------------------
# the transforms (our bytes only)


def ini_set(data, sets, eol="keep"):
    """Set `section.key = value` lines in an ini file; every key must exist once."""
    want = {k.lower(): (k.split(".", 1)[1], v) for k, v in sets.items()}
    hits = dict.fromkeys(want, 0)
    section = ""
    out = []
    for line in data.decode("latin-1").splitlines(keepends=True):
        body = line.rstrip("\r\n")
        end = line[len(body):]
        s = body.strip()
        if s.startswith("[") and s.endswith("]"):
            section = s[1:-1].strip()
        else:
            k, sep, _ = body.partition("=")
            full = f"{section}.{k.strip()}".lower()
            if sep and full in want:
                key, value = want[full]
                body = f"{key} = {value}"
                hits[full] += 1
        if end and eol == "lf":
            end = "\n"
        elif end and eol == "crlf":
            end = "\r\n"
        out.append(body + end)
    bad = [k for k, n in hits.items() if n != 1]
    if bad:
        raise ValueError(f"ini keys not found exactly once: {', '.join(bad)}")
    return "".join(out).encode("latin-1")


def transform(entry, data):
    """The patched bytes for a STOCK input."""
    if entry["kind"] == "ini":
        return ini_set(data, entry["set"], entry.get("eol", "keep"))
    if entry["kind"] == "ccp":
        return ccp_transform(entry, data)
    out = bytearray(data)
    for s in entry["sites"]:
        a = after_bytes(s)
        out[s["offset"] : s["offset"] + len(a)] = a
    return bytes(out)


def window_state(data, site):
    a = after_bytes(site)
    o, w = site["offset"], site["window"]
    h = sha256(data[o - w : o + len(a) + w]) if o - w >= 0 else None
    if h == site["window_sha256_stock"]:
        return "stock"
    if h == site["window_sha256_patched"]:
        return "patched"
    return "other"


# --------------------------------------------------------------------------
# kind "ccp": marshal strings inside zlib members of `code.ccp`
#
# `code.ccp` is a zip whose members are all method 0 ("stored") and flags 0,
# yet each body is a zlib stream, and the zip CRC32 is the CRC of that body.
# Both size fields equal the body length. So a member whose inflated .pyc has
# a shorter string recompresses into its own slot; the offsets of every other
# member stay where they are, and only that member's own CRC and two sizes
# change, in the local header and in the central directory.

LOCAL_SIG = b"PK\x03\x04"
CENTRAL_SIG = b"PK\x01\x02"
EOCD_SIG = b"PK\x05\x06"
# (level, memLevel) tried in order until the stream fits the slot.
CCP_ATTEMPTS = [(lv, ml) for lv in (9, 8, 7, 6) for ml in (8, 9)]


def ccp_lint(e, where, err):
    members = e.get("members")
    if not isinstance(members, list) or not members:
        err(f"{where}: ccp needs a non-empty members list")
        return
    seen = set()
    for i, mem in enumerate(members):
        mw = f"{where}.members[{i}]"
        if not isinstance(mem, dict):
            err(f"{mw}: not an object")
            continue
        name = mem.get("name")
        if not isinstance(name, str) or not name or name.startswith("/") or ".." in name.split("/") or "\\" in name:
            err(f"{mw}: name must be a relative member name with / separators")
        elif name in seen:
            err(f"{mw}: member {name} listed twice")
        else:
            seen.add(name)
        if not isinstance(mem.get("slot"), int) or isinstance(mem.get("slot"), bool) or mem["slot"] <= 0:
            err(f"{mw}: slot must be a positive integer")
        for h in ("raw_sha256_stock", "raw_sha256_patched"):
            if not isinstance(mem.get(h), str) or not HEX64.match(mem[h]):
                err(f"{mw}: {h} must be 64 lowercase hex characters")
        if mem.get("raw_sha256_stock") == mem.get("raw_sha256_patched"):
            err(f"{mw}: stock and patched inflated hashes are equal")
        sites = mem.get("sites")
        if not isinstance(sites, list) or not sites:
            err(f"{mw}: needs a non-empty sites list")
            continue
        spans = []
        for j, s in enumerate(sites):
            sw = f"{mw}.sites[{j}]"
            if not isinstance(s, dict):
                err(f"{sw}: not an object")
                continue
            if set(s) - {"offset", "old_len", "after_ascii", "note"}:
                err(f"{sw}: only offset, old_len, after_ascii and note")
            a = s.get("after_ascii")
            if not isinstance(a, str) or not a.isascii() or not 0 < len(a) <= MAX_AFTER:
                err(f"{sw}: after_ascii must be 1..{MAX_AFTER} ASCII characters")
                a = ""
            off, ol = s.get("offset"), s.get("old_len")
            if not isinstance(off, int) or isinstance(off, bool) or off < 0:
                err(f"{sw}: offset must be a non-negative integer")
                continue
            if not isinstance(ol, int) or isinstance(ol, bool) or ol <= 0:
                err(f"{sw}: old_len must be a positive integer")
                continue
            spans.append((off, off + 5 + ol))
        spans.sort()
        for (_, a1), (b0, _) in zip(spans, spans[1:]):
            if b0 < a1:
                err(f"{mw}: sites overlap at {b0}")


def zip_index(data):
    """{name: [entries]} of a zip's central directory; entries carry offsets.

    Refuses (Refuse) on anything but the plain layout `code.ccp` has.
    """
    i = data.rfind(EOCD_SIG)
    if i < 0 or len(data) - i < 22:
        raise Refuse("not a zip: no end-of-central-directory record", 1)
    _, _, _, _, n, cdsz, cdoff, clen = struct.unpack_from("<4sHHHHIIH", data, i)
    if clen or cdoff + cdsz > i:
        raise Refuse("zip: unexpected end record", 1)
    out = {}
    p = cdoff
    try:
        for _ in range(n):
            if data[p : p + 4] != CENTRAL_SIG or p + 46 > i:
                raise Refuse("zip: bad central directory entry", 1)
            (_, _, _, flags, meth, _, _, crc, cs, us, nl, el, cl, _, _, _, lo) = struct.unpack_from("<4sHHHHHHIIIHHHHHII", data, p)
            name = data[p + 46 : p + 46 + nl].decode("latin-1")
            out.setdefault(name, []).append({"cd": p, "lo": lo, "meth": meth, "flags": flags, "crc": crc, "cs": cs, "us": us})
            p += 46 + nl + el + cl
    except struct.error as e:
        raise Refuse(f"zip: truncated central directory ({e})", 1)
    if p != cdoff + cdsz:
        raise Refuse("zip: the central directory does not end where the end record says", 1)
    return out


def ccp_member(data, idx, mem):
    """The one entry for `mem`, with `body` (offset) and `lo_name` checked."""
    got = idx.get(mem["name"], [])
    if len(got) != 1:
        raise Refuse(f"code.ccp: {len(got)} members named {mem['name']}, want exactly 1", 1)
    z = dict(got[0])
    lo = z["lo"]
    if data[lo : lo + 4] != LOCAL_SIG or lo + 30 > len(data):
        raise Refuse(f"code.ccp: {mem['name']}: bad local header", 1)
    _, _, _, _, _, _, lcrc, lcs, lus, nl, el = struct.unpack_from("<4sHHHHHIIIHH", data, lo)
    if data[lo + 30 : lo + 30 + nl].decode("latin-1") != mem["name"]:
        raise Refuse(f"code.ccp: {mem['name']}: local header names another member", 1)
    z["body"] = lo + 30 + nl + el
    z["lcrc"], z["lcs"], z["lus"] = lcrc, lcs, lus
    if z["meth"] != 0 or z["flags"] != 0:
        raise Refuse(f"code.ccp: {mem['name']}: not a method 0 member with flags 0", 1)
    if z["body"] + mem["slot"] > len(data):
        raise Refuse(f"code.ccp: {mem['name']}: slot runs past the file", 1)
    return z


def ccp_inflate(data, z, mem):
    """(raw, stream length) of a member body, or Refuse. The stream must end
    inside the slot and the rest of the slot must be zero."""
    if z["cs"] > mem["slot"] or z["cs"] != z["us"] or (z["lcs"], z["lus"]) != (z["cs"], z["us"]):
        raise Refuse(f"code.ccp: {mem['name']}: size fields disagree with the slot", 1)
    body = data[z["body"] : z["body"] + z["cs"]]
    d = zlib.decompressobj()
    try:
        raw = d.decompress(body)
    except zlib.error as e:
        raise Refuse(f"code.ccp: {mem['name']}: not a zlib stream ({e})", 1)
    if not d.eof or d.unused_data:
        raise Refuse(f"code.ccp: {mem['name']}: zlib stream does not end at the recorded size", 1)
    tail = data[z["body"] + z["cs"] : z["body"] + mem["slot"]]
    if tail.strip(b"\0"):
        raise Refuse(f"code.ccp: {mem['name']}: slot tail is not zero", 1)
    return raw, body


def ccp_transform(entry, data):
    """The patched bytes for a STOCK `code.ccp`. Refuse when anything is off."""
    idx = zip_index(data)
    out = bytearray(data)
    for mem in entry["members"]:
        z = ccp_member(data, idx, mem)
        if z["cs"] != mem["slot"]:
            raise Refuse(f"code.ccp: {mem['name']}: size {z['cs']}, manifest says slot {mem['slot']}", 1)
        raw, body = ccp_inflate(data, z, mem)
        if zlib.crc32(body) != z["crc"] or z["crc"] != z["lcrc"]:
            raise Refuse(f"code.ccp: {mem['name']}: CRC does not match its body", 1)
        if sha256(raw) != mem["raw_sha256_stock"]:
            raise Refuse(f"code.ccp: {mem['name']}: inflated member is not this manifest's stock", 1)
        new = bytearray(raw)
        for s in sorted(mem["sites"], key=lambda s: -s["offset"]):
            o, ol, a = s["offset"], s["old_len"], after_bytes(s)
            if bytes(new[o : o + 1]) not in (b"s", b"t") or struct.unpack_from("<I", new, o + 1)[0] != ol:
                raise Refuse(f"code.ccp: {mem['name']}: no marshal string of length {ol} at {o}", 1)
            new[o + 5 : o + 5 + ol] = a
            new[o + 1 : o + 5] = struct.pack("<I", len(a))
        if sha256(bytes(new)) != mem["raw_sha256_patched"]:
            raise Refuse(f"code.ccp: {mem['name']}: patched member does not hash to the manifest's", 1)
        packed = None
        for lv, ml in CCP_ATTEMPTS:
            co = zlib.compressobj(lv, zlib.DEFLATED, 15, ml)
            cand = co.compress(bytes(new)) + co.flush()
            if len(cand) <= mem["slot"]:
                packed = cand
                break
        if packed is None:
            raise Refuse(f"code.ccp: {mem['name']}: patched member does not fit its {mem['slot']}-byte slot at any level; nothing written (--tier A,P,B leaves tier C out)", 1)
        b = z["body"]
        out[b : b + mem["slot"]] = packed + b"\0" * (mem["slot"] - len(packed))
        crc = zlib.crc32(packed)
        struct.pack_into("<III", out, z["lo"] + 14, crc, len(packed), len(packed))
        struct.pack_into("<III", out, z["cd"] + 16, crc, len(packed), len(packed))
    return bytes(out)


def ccp_canon(entry, data):
    """The canonical hash of a PATCHED `code.ccp`, or None when it is not one.

    Compressed bytes depend on the host's zlib, so the hash covers the file
    with every target slot and its CRC and two size fields zeroed, then each
    member's inflated-patched hash, after checking that the member inflates to
    exactly that and that its headers agree with it.
    """
    try:
        idx = zip_index(data)
        buf = bytearray(data)
        tails = []
        for mem in entry["members"]:
            z = ccp_member(data, idx, mem)
            raw, body = ccp_inflate(data, z, mem)
            if sha256(raw) != mem["raw_sha256_patched"] or zlib.crc32(body) != z["crc"] or z["crc"] != z["lcrc"]:
                return None
            buf[z["body"] : z["body"] + mem["slot"]] = b"\0" * mem["slot"]
            for o in (z["lo"] + 14, z["cd"] + 16):
                buf[o : o + 12] = b"\0" * 12
            tails.append(bytes.fromhex(mem["raw_sha256_patched"]))
    except Refuse:
        return None
    h = hashlib.sha256(bytes(buf))
    for t in tails:
        h.update(t)
    return h.hexdigest()


def ccp_stock_members(entry, data):
    """[(name, 'stock'|'other'|'missing')] for each member of a `ccp` entry."""
    out = []
    try:
        idx = zip_index(data)
    except Refuse:
        return [(m["name"], "missing") for m in entry["members"]]
    for mem in entry["members"]:
        if mem["name"] not in idx:
            out.append((mem["name"], "missing"))
            continue
        try:
            z = ccp_member(data, idx, mem)
            raw, _ = ccp_inflate(data, z, mem)
        except Refuse:
            out.append((mem["name"], "other"))
            continue
        out.append((mem["name"], "stock" if sha256(raw) == mem["raw_sha256_stock"] else "other"))
    return out


def is_patched(entry, data):
    """Whether `data` is the entry's patched form (canonical hash for `ccp`)."""
    if entry["kind"] == "ccp":
        return ccp_canon(entry, data) == entry["sha256_patched"]
    return sha256(data) == entry["sha256_patched"]


# --------------------------------------------------------------------------
# prefs.ini (CRLF, key=value, a final newline may be missing)


def _prefs_lines(data):
    parts = data.split(b"\n")
    lines = []
    for i, p in enumerate(parts):
        last = i == len(parts) - 1
        if last:
            if p:
                lines.append([p, b""])
        elif p.endswith(b"\r"):
            lines.append([p[:-1], b"\r\n"])
        else:
            lines.append([p, b"\n"])
    return lines


def _pkey(content):
    k, sep, _ = content.partition(b"=")
    return k.strip().decode("latin-1") if sep else None


def prefs_get(data, key):
    for content, _ in _prefs_lines(data):
        if _pkey(content) == key:
            return content.partition(b"=")[2].decode("latin-1")
    return None


def prefs_keys(data):
    return [k for k in (_pkey(c) for c, _ in _prefs_lines(data)) if k is not None]


def prefs_set(data, key, value):
    """`key=value` replaced in place, or appended with CRLF."""
    lines = _prefs_lines(data)
    new = f"{key}={value}".encode("latin-1")
    found = False
    for ln in lines:
        if _pkey(ln[0]) == key:
            ln[0] = new
            found = True
    if not found:
        if lines and lines[-1][1] == b"":
            lines[-1][1] = b"\r\n"
        lines.append([new, b"\r\n"])
    return b"".join(c + e for c, e in lines)


def prefs_remove(data, key):
    return b"".join(c + e for c, e in _prefs_lines(data) if _pkey(c) != key)


WIN_DRIVE = re.compile(r"^[A-Za-z]:[\\/]")


def prefs_windows(client):
    """(CCP/EVE profile root, folder) for an install on a Windows drive, else None.

    `%LOCALAPPDATA%\\CCP\\EVE\\<drive letter>_<install path, lowercased, \\ and spaces -> _>_<server>`:
    the same name Wine's `c_` folders carry, one drive letter in front.
    """
    text = str(client)
    if not (WIN_DRIVE.match(text) or (os.name == "nt" and Path(text).drive)):
        return None
    from pathlib import PureWindowsPath

    pw = PureWindowsPath(text if WIN_DRIVE.match(text) else os.path.abspath(text))
    letter = pw.drive[:1].lower()
    rel = "_".join(pw.parts[1:]).lower().replace(" ", "_").replace(":", "")
    local = os.environ.get("LOCALAPPDATA")
    if not letter or not local:
        return None
    return Path(local) / "CCP" / "EVE", f"{letter}_{rel}"


def prefs_folder(client):
    """`c_<install path below drive_c, lowercased, / and spaces -> _>`."""
    parts = Path(os.path.abspath(client)).parts
    low = [p.lower() for p in parts]
    if "drive_c" not in low:
        return None, None
    i = len(low) - 1 - low[::-1].index("drive_c")
    rel = "/".join(parts[i + 1 :])
    folder = "c_" + rel.lower().replace("/", "_").replace(" ", "_").replace(":", "")
    return Path(*parts[: i + 1]), folder


def prefs_targets(client, explicit):
    """(existing prefs.ini paths, a path to create when there are none)."""
    if explicit:
        return [Path(p) for p in explicit if Path(p).exists()], next(
            (Path(p) for p in explicit if not Path(p).exists()), None
        )
    win = prefs_windows(client)
    if win is not None:
        root, folder = win
        found = sorted(Path(p) for p in glob.glob(str(root / (glob.escape(folder) + "_*") / "settings*" / "prefs.ini")))
        create = None if found else root / f"{folder}_tranquility" / "settings" / "prefs.ini"
        return found, create
    drive_c, folder = prefs_folder(client)
    if drive_c is None:
        return [], None
    pat = str(drive_c / "users" / "*" / "AppData" / "Local" / "CCP" / "EVE" / f"{glob.escape(folder)}_*" / "settings*" / "prefs.ini")
    found = sorted(Path(p) for p in glob.glob(pat))
    create = None
    if not found and (drive_c / "users" / "steamuser").is_dir():
        create = drive_c / "users" / "steamuser" / "AppData" / "Local" / "CCP" / "EVE" / f"{folder}_tranquility" / "settings" / "prefs.ini"
    return found, create


# --------------------------------------------------------------------------
# file state


def backup_of(path):
    return path.with_name(path.name + BACKUP)


def tmp_of(path):
    return path.with_name(path.name + TMP)


def file_state(client, rel, entry):
    p = Path(client) / rel
    st = {"rel": rel, "path": p, "id": entry["id"], "tier": entry["tier"]}
    if not p.is_file():
        st["state"], st["hash"] = "unknown", None
        st["why"] = "missing"
    else:
        size = p.stat().st_size
        h = sha256_file(p)
        st["hash"] = h
        if h == entry["sha256_stock"]:
            st["state"] = "stock"
        elif (entry["kind"] != "ccp" and h == entry["sha256_patched"]) or (entry["kind"] == "ccp" and is_patched(entry, p.read_bytes())):
            st["state"] = "patched"
        else:
            st["state"] = "unknown"
            st["why"] = f"size {size}, sha256 {h[:16]}..." if size == entry["size"] else f"size {size}, manifest says {entry['size']}"
            if entry["kind"] == "bytes" and size == entry["size"]:
                data = p.read_bytes()
                st["sites"] = [window_state(data, s) for s in entry["sites"]]
            if entry["kind"] == "ccp":
                st["sites"] = [f"{n}:{w}" for n, w in ccp_stock_members(entry, p.read_bytes())]
    b = backup_of(p)
    if not b.exists():
        st["backup"] = "absent"
    else:
        st["backup"] = "ok" if sha256_file(b) == entry["sha256_stock"] else "corrupt"
    return st


def atomic_write(path, data, expect=None, entry=None):
    """Write via `<path>.eveoffline-tmp` + os.replace; check first.

    `expect` is a whole-file sha256. With `entry` the check is `is_patched`
    instead (a `ccp` file has a canonical hash, not a whole-file one).
    """
    if entry is not None:
        if not is_patched(entry, data):
            raise Refuse(f"{path}: result is not the manifest's patched form; not written", 1)
    elif expect is not None and sha256(data) != expect:
        raise Refuse(f"{path}: result does not hash to {expect[:16]}...; not written", 1)
    tmp = tmp_of(path)
    try:
        with open(tmp, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise


# --------------------------------------------------------------------------
# the context: which client, which build, which manifest


class Ctx:
    pass


def open_ctx(args, need_client=True):
    c = Ctx()
    c.args = args
    c.client = Path(args.client).resolve() if args.client else None
    if need_client and c.client is None:
        raise Refuse("--client DIR is required")
    if c.client is not None and not c.client.is_dir():
        raise Refuse(f"no client directory at {c.client}")
    found = read_build(c.client) if c.client else None
    if args.build is not None and found is not None and str(args.build) != found:
        raise Refuse(f"--build {args.build} but start.ini says build = {found}")
    c.build = str(args.build) if args.build is not None else found
    if c.build is None:
        raise Refuse("cannot read the build from start.ini; pass --build N")
    mpath = Path(args.patches_dir) / f"{c.build}.json"
    if not mpath.is_file():
        raise Refuse(f"no manifest for build {c.build} in {args.patches_dir}; run tools/eve_patch_author.py (docs/guides/patcher.md)")
    c.manifest_path = mpath
    c.m = load_manifest(mpath)
    if found is not None and found != c.m["identify"]["start_ini_build"]:
        raise Refuse(f"start.ini build {found} does not identify manifest {mpath.name}")
    req = c.m.get("requires", {})
    by_id = {e["id"]: rel for rel, e in c.m["files"].items()}
    c.order = [by_id[i] for i in order_ids(list(by_id), req)]
    c.by_id = by_id
    c.tiers = set(t.strip().upper() for t in (args.tier or DEFAULT_TIERS).split(",") if t.strip())
    if not c.tiers <= set(TIERS):
        raise Refuse(f"--tier takes {','.join(TIERS)}")
    c.only = set(x.strip() for x in args.only.split(",") if x.strip()) if args.only else None
    if c.only and not c.only <= set(by_id):
        raise Refuse(f"--only: unknown id(s) {', '.join(sorted(c.only - set(by_id)))}")
    return c


def write_guards(c):
    if c.client.name.lower() == "tq" and not c.args.allow_real:
        raise Refuse(f"{c.client} looks like the live install; patch a copy, or pass --allow-real")
    try:
        c.client.relative_to(REPO)
        raise Refuse(f"{c.client} is inside the repository; client bytes never live there")
    except ValueError:
        pass


def selected(c, rel):
    e = c.m["files"][rel]
    if e["tier"] not in c.tiers:
        return False
    return c.only is None or e["id"] in c.only


def states(c):
    return {rel: file_state(c.client, rel, c.m["files"][rel]) for rel in c.order}


# --------------------------------------------------------------------------
# planning


def make_plan(c, st):
    """(actions, refusals). Actions: ('backup'|'patch', rel) and ('prefs', path, changes, create)."""
    actions, refusals = [], []
    for rel in c.order:
        s = st[rel]
        if s["state"] == "unknown" and (s["tier"] != "C" or "C" in c.tiers):
            refusals.append(f"{rel}: unknown ({s.get('why')}); not build {c.build} stock or patched")
    if refusals:
        return actions, refusals
    planned = set()
    for rel in c.order:
        s, e = st[rel], c.m["files"][rel]
        if not selected(c, rel) or s["state"] == "patched":
            continue
        if s["state"] == "unknown":
            continue  # an unselected tier C file we do not understand: left alone
        for r in c.m.get("requires", {}).get(e["id"], []):
            rrel = c.by_id[r]
            if st[rrel]["state"] != "patched" and rrel not in planned:
                refusals.append(f"{rel}: {e['id']} requires {r} ({rrel}), which is not patched and not selected")
        if s["backup"] == "corrupt":
            refusals.append(f"{rel}: {BACKUP} exists but does not hash to stock; move it aside first")
            continue
        if s["backup"] == "absent":
            actions.append(("backup", rel))
        actions.append(("patch", rel))
        planned.add(rel)
    if "P" in c.tiers and c.m.get("prefs", {}).get("set"):
        found, create = prefs_targets(c.client, c.args.prefs)
        for p in found:
            data = p.read_bytes()
            ch = {k: v for k, v in c.m["prefs"]["set"].items() if prefs_get(data, k) != v}
            if ch:
                actions.append(("prefs", p, ch, False))
        if not found and create is not None:
            actions.append(("prefs", create, dict(c.m["prefs"]["set"]), True))
    return actions, refusals


def describe(c, a):
    if a[0] == "backup":
        e = c.m["files"][a[1]]
        return f"backup  {a[1]} -> {a[1]}{BACKUP}  (sha256 {e['sha256_stock'][:16]}..., {e['size']} bytes)"
    if a[0] == "patch":
        e = c.m["files"][a[1]]
        lines = [f"patch   {a[1]}  [{e['id']}, tier {e['tier']}]  {e['sha256_stock'][:16]}... -> {e['sha256_patched'][:16]}..."]
        if e["kind"] == "bytes":
            for s in e["sites"]:
                lines.append(f"          at {s['offset']:#x}: {len(after_bytes(s))} bytes -> {after_bytes(s).hex()}  ({s.get('note', '')})")
        elif e["kind"] == "ccp":
            for m in e["members"]:
                for s in m["sites"]:
                    lines.append(f"          {m['name']} +{s['offset']:#x}: string of {s['old_len']} bytes -> {s['after_ascii']!r}  ({s.get('note', '')})")
        else:
            for k, v in e["set"].items():
                lines.append(f"          {k} = {v}  (eol {e.get('eol', 'keep')})")
        return "\n".join(lines)
    _, p, ch, create = a
    head = f"prefs   {'create ' if create else ''}{p}"
    return head + "".join(f"\n          {k}={v}" for k, v in ch.items())


# --------------------------------------------------------------------------
# checks


def check_prefs(c):
    """(ok, lines) for the P tier."""
    if not c.m.get("prefs"):
        return True, []
    found, _ = prefs_targets(c.client, c.args.prefs)
    if not found:
        return False, ["prefs: no prefs.ini found for this install (pass --prefs PATH, or run apply)"]
    ok, lines = True, []
    for p in found:
        data = p.read_bytes()
        keys = set(prefs_keys(data))
        for k, v in c.m["prefs"].get("set", {}).items():
            got = prefs_get(data, k)
            if got != v:
                ok = False
                lines.append(f"prefs: {p}: {k} is {got!r}, want {v!r}")
        bad = [k for k in c.m["prefs"].get("forbid", []) if k in keys]
        if bad:
            ok = False
            lines.append(f"prefs: {p}: forbidden key(s) present: {', '.join(bad)} (remove them by hand)")
        if ok:
            lines.append(f"prefs: {p}: ok")
    return ok, lines


def check_resfiles(c):
    root = Path(c.args.resfiles) if c.args.resfiles else c.client.parent / "ResFiles"
    names = c.m.get("resfiles_check", [])
    if not names:
        return True, []
    if not root.is_dir():
        return False, [f"resfiles: no ResFiles directory at {root} (pass --resfiles DIR)"]
    ok, lines = True, []
    for n in names:
        idx = c.client / n
        if not idx.is_file():
            ok = False
            lines.append(f"resfiles: {n} missing")
            continue
        rows = missing = wrong = 0
        first = []
        for line in idx.read_bytes().decode("utf-8", "replace").splitlines():
            f = line.split(",")
            if len(f) < 4:
                continue
            rows += 1
            p = root / f[1]
            try:
                size = p.stat().st_size
            except OSError:
                missing += 1
                first.append(f"missing {f[1]}")
                continue
            if f[3].isdigit() and size != int(f[3]):
                wrong += 1
                first.append(f"size {f[1]}")
        if missing or wrong:
            ok = False
            lines.append(f"resfiles: {n}: {rows} rows, {missing} missing, {wrong} wrong size (a miss is a CDN download): {', '.join(first[:5])}")
        else:
            lines.append(f"resfiles: {n}: all {rows} rows present")
    return ok, lines


def do_verify(c, st=None, out=print):
    st = st or states(c)
    code = 0
    for rel in c.order:
        s = st[rel]
        if not selected(c, rel):
            continue
        if s["state"] == "unknown":
            out(f"FAIL {rel}: unknown ({s.get('why')})")
            code = 2
        elif s["state"] == "stock":
            out(f"FAIL {rel}: stock, not patched")
            code = max(code, 1)
        elif s["backup"] != "ok":
            out(f"FAIL {rel}: patched, but the backup is {s['backup']}; revert is impossible")
            code = max(code, 1)
        else:
            out(f"ok   {rel}: patched [{s['id']}], backup ok")
    for rel in c.order:
        if not selected(c, rel) and st[rel]["state"] == "unknown":
            out(f"WARN {rel}: unknown and its tier is not selected, so it was NOT checked ({st[rel].get('why')})")
    if "P" in c.tiers:
        ok, lines = check_prefs(c)
        for ln in lines:
            out(("ok   " if ln.endswith(": ok") else "FAIL ") + ln)
        if not ok:
            code = max(code, 1)
    if not c.args.no_resfiles:
        ok, lines = check_resfiles(c)
        for ln in lines:
            out(("ok   " if ln.endswith("present") else "FAIL ") + ln)
        if not ok:
            code = max(code, 1)
    out(f"verify: {['ok', 'NOT APPLIED', 'UNKNOWN FILES'][code]} (build {c.build})")
    return code


# --------------------------------------------------------------------------
# commands


def clean_tmp(c):
    for rel in c.order:
        t = tmp_of(c.client / rel)
        if t.exists():
            t.unlink()
            print(f"removed stale {t.name}")


def cmd_status(c):
    print(f"client:   {c.client}")
    print(f"build:    {c.build}  (manifest {c.manifest_path})")
    st = states(c)
    for rel in c.order:
        s = st[rel]
        extra = f" ({s['why']})" if s["state"] == "unknown" else ""
        if s.get("sites"):
            extra += " sites: " + ",".join(s["sites"])
        print(f"{s['id']:<3} {s['tier']}  {s['state']:<8} backup {s['backup']:<7} {rel}{extra}")
        if tmp_of(s["path"]).exists():
            print(f"         stale {TMP} present (removed by the next apply/revert)")
    found, create = prefs_targets(c.client, c.args.prefs)
    if not found:
        print(f"prefs:    none found{f' (apply creates {create})' if create else ''}")
    for p in found:
        data = p.read_bytes()
        have = {k: prefs_get(data, k) for k in c.m.get("prefs", {}).get("set", {})}
        forb = [k for k in c.m.get("prefs", {}).get("forbid", []) if k in prefs_keys(data)]
        print(f"prefs:    {p}")
        print("          " + ", ".join(f"{k}={v}" for k, v in have.items()) + (f"; FORBIDDEN: {', '.join(forb)}" if forb else ""))
        if p.with_name(p.name + PRIOR).exists():
            print(f"          {PRIOR} present (revert restores the prior values)")
    return 0


def cmd_plan(c):
    st = states(c)
    actions, refusals = make_plan(c, st)
    for rel in c.order:
        print(f"{st[rel]['id']:<3} {st[rel]['state']:<8} {rel}")
    for r in refusals:
        print(f"REFUSE {r}")
    if refusals:
        print("plan: apply would refuse; nothing would be written")
        return 2
    for a in actions:
        print(describe(c, a))
    print(f"plan: {len(actions)} write(s)" if actions else "plan: nothing to do")
    return 0


def cmd_apply(c):
    write_guards(c)
    clean_tmp(c)
    st = states(c)
    actions, refusals = make_plan(c, st)
    if refusals:
        for r in refusals:
            print(f"REFUSE {r}", file=sys.stderr)
        print("apply: refused; nothing written", file=sys.stderr)
        return 2
    # Every transform is computed in memory first, so a patch that cannot be
    # made (a member that does not fit its slot on this host's zlib, a changed
    # file) refuses before the first write. Only then are files written.
    made = {}
    try:
        for a in actions:
            if a[0] != "patch":
                continue
            e = c.m["files"][a[1]]
            p = c.client / a[1]
            data = p.read_bytes()
            if sha256(data) != e["sha256_stock"]:
                raise Refuse(f"{a[1]} changed under us", 2)
            if sha256_file(backup_of(p)) != e["sha256_stock"] if backup_of(p).exists() else False:
                raise Refuse(f"{a[1]}: backup is not stock", 2)
            out = transform(e, data)
            if not is_patched(e, out):
                raise Refuse(f"{a[1]}: result is not the manifest's patched form", 2)
            made[a[1]] = out
    except (Refuse, ValueError) as err:
        print(f"REFUSE {err}", file=sys.stderr)
        print("apply: refused; nothing written", file=sys.stderr)
        return 2
    try:
        for a in actions:
            print(describe(c, a))
            if a[0] == "backup":
                src = c.client / a[1]
                data = src.read_bytes()
                atomic_write(backup_of(src), data, c.m["files"][a[1]]["sha256_stock"])
            elif a[0] == "patch":
                e = c.m["files"][a[1]]
                p = c.client / a[1]
                if sha256_file(backup_of(p)) != e["sha256_stock"]:
                    raise Refuse(f"{a[1]}: backup is not stock; not writing")
                if e["kind"] == "ccp":
                    atomic_write(p, made[a[1]], entry=e)
                else:
                    atomic_write(p, made[a[1]], e["sha256_patched"])
            else:
                _, p, ch, create = a
                if create:
                    p.parent.mkdir(parents=True, exist_ok=True)
                    data = b""
                else:
                    data = p.read_bytes()
                prior = p.with_name(p.name + PRIOR)
                if not prior.exists():
                    rec = {k: prefs_get(data, k) for k in c.m["prefs"]["set"]}
                    atomic_write(prior, json.dumps({"schema": 1, "created": create, "prior": rec}, indent=1).encode())
                for k, v in ch.items():
                    data = prefs_set(data, k, v)
                atomic_write(p, data)
    except OSError as err:
        # A locked file (Windows) or a full disk: each file is whole, but the
        # list is not done. Say so; `apply` again finishes it, `revert` undoes it.
        print(f"apply: stopped by a write error ({err}); files written so far are complete; run apply again or revert", file=sys.stderr)
        return 1
    if not actions:
        print("apply: already applied; nothing written")
    return do_verify(c)


def cmd_revert(c):
    write_guards(c)
    clean_tmp(c)
    st = states(c)
    refusals = []
    skipped = set()
    for rel in c.order:
        s = st[rel]
        if s["state"] == "unknown" and s["tier"] == "C" and "C" not in c.tiers:
            skipped.add(rel)
            print(f"{rel}: unknown, tier C is not selected; left as is")
        elif s["state"] == "unknown":
            refusals.append(f"{rel}: unknown ({s.get('why')}); will not guess what to restore")
        elif s["state"] == "patched" and s["backup"] != "ok":
            refusals.append(f"{rel}: patched but the backup is {s['backup']}; cannot restore")
    if refusals:
        for r in refusals:
            print(f"REFUSE {r}", file=sys.stderr)
        print("revert: refused; nothing written", file=sys.stderr)
        return 2
    for rel in reversed(c.order):
        if st[rel]["state"] != "patched":
            continue
        e = c.m["files"][rel]
        p = c.client / rel
        atomic_write(p, backup_of(p).read_bytes(), e["sha256_stock"])
        print(f"restored {rel} from {rel}{BACKUP}")
    found, _ = prefs_targets(c.client, c.args.prefs)
    ours = c.m.get("prefs", {}).get("set", {})
    for p in found:
        prior = p.with_name(p.name + PRIOR)
        if not prior.exists():
            print(f"prefs: {p}: no {PRIOR}; left as is")
            continue
        rec = json.loads(prior.read_text())["prior"]
        data = p.read_bytes()
        for k, old in rec.items():
            if prefs_get(data, k) != ours.get(k):
                continue  # changed since apply: the user's value wins
            data = prefs_remove(data, k) if old is None else prefs_set(data, k, old)
        atomic_write(p, data)
        prior.unlink()
        print(f"prefs: {p}: prior values restored")
    bad = [rel for rel, s in states(c).items() if s["state"] != "stock" and rel not in skipped]
    if bad:
        print(f"revert: not stock afterwards: {', '.join(bad)}", file=sys.stderr)
        return 1
    if skipped:
        print(f"revert: NOT every target is stock: {', '.join(sorted(skipped))} is unknown and was left alone (pass --tier A,P,B,C to refuse instead of skip)", file=sys.stderr)
        return 1
    print("revert: every target is stock")
    return 0


def forbidden_arg(arg, forbid):
    """True when `arg` matches a forbid entry (case-insensitive).

    An entry ending in `=` or `:` is a prefix; otherwise it matches the
    argument's name, the part before any `=` or `:`.
    """
    a = arg.lower()
    for f in forbid:
        f = f.lower()
        if f.endswith(("=", ":")):
            if a.startswith(f):
                return True
        elif re.split(r"[=:]", a, maxsplit=1)[0] == f:
            return True
    return False


def launch_block(m, server):
    if not re.match(r"^[A-Za-z0-9.\-]+$", server):
        raise Refuse(f"--server {server!r}: a host name or IPv4 address")
    launch = m["launch"]
    env = {k: v.replace("{server}", server) for k, v in launch.get("env", {}).items()}
    return {"build": m["build"], "server": server, "env": env,
            "args": list(launch.get("args", [])), "forbid_args": list(launch.get("forbid_args", []))}


def cmd_launch_env(c):
    m = c.m
    server = c.args.server or m["launch"].get("server_default", "127.0.0.1")
    blk = launch_block(m, server)
    if c.args.check_args is not None:
        bad = [a for a in c.args.check_args.split() if forbidden_arg(a, blk["forbid_args"])]
        if bad:
            print(f"launch-env: forbidden client argument(s): {' '.join(bad)}", file=sys.stderr)
            return 3
    if c.args.json:
        print(json.dumps(blk, indent=1))
        return 0
    print(f"# eve_patch launch-env: build {blk['build']}, server {server}")
    print("# 'env K=V' must be set exactly; 'arg X' must be passed; 'forbid-arg X' must never be.")
    for k, v in blk["env"].items():
        print(f"env {k}={v}")
    for a in blk["args"]:
        print(f"arg {a}")
    for a in blk["forbid_args"]:
        print(f"forbid-arg {a}")
    return 0


COMMANDS = {
    "status": cmd_status,
    "verify": do_verify,
    "plan": cmd_plan,
    "apply": cmd_apply,
    "revert": cmd_revert,
    "launch-env": cmd_launch_env,
}


def parser():
    ap = argparse.ArgumentParser(prog="eve_patch.py", description=__doc__.splitlines()[0])
    ap.add_argument("command", choices=sorted(COMMANDS))
    ap.add_argument("--client", help="the install directory (holds start.ini and bin64/)")
    ap.add_argument("--build", help="the build, instead of reading start.ini")
    ap.add_argument("--patches-dir", default=str(PATCHES), help=argparse.SUPPRESS)
    ap.add_argument("--tier", default=DEFAULT_TIERS, help="tiers to apply/verify (default A,P,B,C; C is the code.ccp constants)")
    ap.add_argument("--only", help="file ids to apply/verify, e.g. B0,B1")
    ap.add_argument("--prefs", action="append", help="a prefs.ini path (repeatable); default: found from --client")
    ap.add_argument("--resfiles", help="the ResFiles directory (default: ../ResFiles beside the install)")
    ap.add_argument("--no-resfiles", action="store_true", help="skip the ResFiles completeness check")
    ap.add_argument("--allow-real", action="store_true", help="allow writing an install named tq")
    ap.add_argument("--server", help="launch-env: our server address (default from the manifest)")
    ap.add_argument("--json", action="store_true", help="launch-env: JSON instead of lines")
    ap.add_argument("--check-args", help="launch-env: exit 3 if this argument string holds a forbidden arg")
    return ap


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        c = open_ctx(args, need_client=args.command != "launch-env" or args.build is None)
        return COMMANDS[args.command](c)
    except Refuse as e:
        print(f"eve_patch: {e}", file=sys.stderr)
        return e.code


if __name__ == "__main__":
    sys.exit(main())
