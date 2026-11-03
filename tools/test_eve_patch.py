"""Tests for `tools/eve_patch.py` (lane PATCH-1; out/research/R-PATCH.md §4).

Every fixture is GENERATED here: deterministic pseudo-random bytes (a
sha256-counter stream) of a declared size, a fake `start.ini` for the fake
build 0, and a manifest the test builds for them. No client byte is read or
stored. The one exception is the opt-in dry run at the end, which reads a
real install only when `EVE_PATCH_REAL_CLIENT` names one, and writes nothing.

Run::

    python3 -I tools/test_eve_patch.py
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import shutil
import subprocess
import json
import os
import pathlib
import re
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last, so nothing in tools/ shadows the stdlib

import eve_patch as ep  # noqa: E402

H = lambda b: hashlib.sha256(b).hexdigest()  # noqa: E731


def stream(seed, n):
    out = bytearray()
    i = 0
    while len(out) < n:
        out += hashlib.sha256(f"{seed}:{i}".encode()).digest()
        i += 1
    return bytes(out[:n])


START_INI = b"[main]\r\nversion = 0.0\r\nbuild = 0\r\nregion = ccp\r\ncryptoPack = CryptoAPI\r\nport = 26000\r\n\r\n"


def site(data, off, after, w=8):
    patched = bytearray(data)
    patched[off : off + len(after)] = after
    return {"offset": off, "after_hex": after.hex(), "window": w,
            "window_sha256_stock": H(data[off - w : off + len(after) + w]),
            "window_sha256_patched": H(bytes(patched)[off - w : off + len(after) + w]),
            "note": "synthetic"}


def apply_sites(data, sites):
    out = bytearray(data)
    for s in sites:
        a = bytes.fromhex(s["after_hex"])
        out[s["offset"] : s["offset"] + len(a)] = a
    return bytes(out)


A = stream("a", 4096)
B = stream("b", 2048)
SITES_A = [site(A, 1000, bytes.fromhex("4831db90"))]
SITES_B = [site(B, 100, b"\0\0\0\0"), site(B, 1500, b"lndarkly.invalid")]
A_P = apply_sites(A, SITES_A)
B_P = apply_sites(B, SITES_B)
INI_P = START_INI.replace(b"CryptoAPI", b"Placebo")


def manifest():
    """A fresh copy every call: the lint tests mutate it."""
    return json.loads(json.dumps(_manifest()))


def _manifest():
    return {
        "schema": 1, "build": 0, "identify": {"start_ini_build": "0"},
        "files": {
            "bin64/a.dll": {"id": "B0", "tier": "B", "kind": "bytes", "size": len(A),
                            "sha256_stock": H(A), "sha256_patched": H(A_P), "sites": SITES_A},
            "start.ini": {"id": "P0", "tier": "B", "kind": "ini", "size": len(START_INI),
                          "sha256_stock": H(START_INI), "sha256_patched": H(INI_P),
                          "set": {"main.cryptoPack": "Placebo"}, "eol": "keep"},
            "bin64/b.exe": {"id": "B1", "tier": "B", "kind": "bytes", "size": len(B),
                            "sha256_stock": H(B), "sha256_patched": H(B_P), "sites": SITES_B},
        },
        "requires": {"B1": ["B0"], "P0": ["B0"]},
        "prefs": {"set": {"breakpadUpload": "0", "sentry_io_dsn": "http://0@127.0.0.1:9/0"},
                  "forbid": ["networkLogging", "http"]},
        "launch": {"server_default": "10.137.0.1",
                   "env": {"HTTP_PROXY": "http://127.0.0.1:9", "NO_PROXY": "{server},localhost",
                           "EVE_PUBLIC_GATEWAY_ADDRESS": "{server}:8443"},
                   "args": ["/noCrashReportUpload"],
                   "forbid_args": ["/refreshToken", "/py", "/tools="]},
        "resfiles_check": ["resfileindex.txt"],
    }


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        self.patches = root / "patches"
        self.patches.mkdir()
        (self.patches / "0.json").write_text(json.dumps(manifest()))
        # A Wine-shaped tree, so prefs.ini is found the way run-client.sh finds it.
        self.drive_c = root / "pfx" / "drive_c"
        self.client = self.drive_c / "CCP" / "EVE" / "fake-offline"
        (self.client / "bin64").mkdir(parents=True)
        (self.client / "bin64" / "a.dll").write_bytes(A)
        (self.client / "bin64" / "b.exe").write_bytes(B)
        (self.client / "start.ini").write_bytes(START_INI)
        res = self.client.parent / "ResFiles" / "ab"
        res.mkdir(parents=True)
        (res / "x").write_bytes(b"12345")
        (self.client / "resfileindex.txt").write_bytes(b"res:/x.txt,ab/x,0123,5,3\r\n")
        settings = self.drive_c / "users" / "steamuser" / "AppData" / "Local" / "CCP" / "EVE" / "c_ccp_eve_fake-offline_tranquility" / "settings"
        settings.mkdir(parents=True)
        self.prefs = settings / "prefs.ini"
        self.prefs.write_bytes(b"windowed=1\r\nbreakpadUpload=1")  # no final newline, as the client writes it

    def tearDown(self):
        self.tmp.cleanup()

    def run_cmd(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = ep.main(list(argv) + ["--patches-dir", str(self.patches)])
        return code, out.getvalue() + err.getvalue()

    def cmd(self, name, *extra):
        return self.run_cmd(name, "--client", str(self.client), *extra)

    def f(self, rel):
        return (self.client / rel).read_bytes()

    def snapshot(self):
        return {str(p): p.read_bytes() for p in pathlib.Path(self.tmp.name).rglob("*") if p.is_file()}


class RoundTrip(Base):
    def test_apply_verify_revert(self):
        code, out = self.cmd("apply")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("bin64/a.dll"), A_P)
        self.assertEqual(self.f("bin64/b.exe"), B_P)
        self.assertEqual(self.f("start.ini"), INI_P)
        for rel, stock in (("bin64/a.dll", A), ("bin64/b.exe", B), ("start.ini", START_INI)):
            self.assertEqual(self.f(rel + ".eveoffline-orig"), stock)
        self.assertEqual(self.prefs.read_bytes(),
                         b"windowed=1\r\nbreakpadUpload=0\r\nsentry_io_dsn=http://0@127.0.0.1:9/0\r\n")
        self.assertEqual(self.cmd("verify")[0], 0)
        code, out = self.cmd("revert")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("bin64/a.dll"), A)
        self.assertEqual(self.f("bin64/b.exe"), B)
        self.assertEqual(self.f("start.ini"), START_INI)
        # Pinned: revert keeps the backups (they are the user's own bytes).
        self.assertTrue((self.client / "bin64/a.dll.eveoffline-orig").exists())
        self.assertEqual(self.prefs.read_bytes(), b"windowed=1\r\nbreakpadUpload=1\r\n")
        self.assertFalse(self.prefs.with_name("prefs.ini.eveoffline-prior.json").exists())
        self.assertEqual(self.cmd("verify")[0], 1)
        # And again: a reverted install applies cleanly.
        self.assertEqual(self.cmd("apply")[0], 0)

    def test_apply_twice_is_a_noop(self):
        self.assertEqual(self.cmd("apply")[0], 0)
        before = self.snapshot()
        mtimes = {p: os.stat(p).st_mtime_ns for p in before}
        code, out = self.cmd("apply")
        self.assertEqual(code, 0, out)
        self.assertIn("already applied", out)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual({p: os.stat(p).st_mtime_ns for p in before}, mtimes)

    def test_plan_writes_nothing(self):
        before = self.snapshot()
        code, out = self.cmd("plan")
        self.assertEqual(code, 0, out)
        self.assertIn("at 0x3e8: 4 bytes -> 4831db90", out)
        self.assertEqual(self.snapshot(), before)

    def test_status_reports_states(self):
        code, out = self.cmd("status")
        self.assertEqual(code, 0)
        self.assertIn("stock", out)
        self.assertIn("breakpadUpload=1", out)

    def test_existing_stock_backup_is_reused(self):
        (self.client / "bin64/a.dll.eveoffline-orig").write_bytes(A)
        self.assertEqual(self.cmd("apply")[0], 0)
        self.assertEqual(self.f("bin64/a.dll"), A_P)

    def test_tiers(self):
        code, out = self.cmd("apply", "--tier", "P")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("bin64/a.dll"), A)
        self.assertIn(b"breakpadUpload=0", self.prefs.read_bytes())
        code, out = self.cmd("apply", "--tier", "B")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("bin64/a.dll"), A_P)


class Refusals(Base):
    def assert_refused_untouched(self, *argv):
        before = self.snapshot()
        code, out = self.cmd(*argv)
        self.assertEqual(code, 2, out)
        self.assertEqual(self.snapshot(), before)
        return out

    def test_wrong_hash_one_bit(self):
        bad = bytearray(B)
        bad[7] ^= 1
        (self.client / "bin64/b.exe").write_bytes(bytes(bad))
        out = self.assert_refused_untouched("apply")
        self.assertIn("unknown", out)
        self.assertFalse((self.client / "bin64/a.dll.eveoffline-orig").exists())
        self.assertEqual(self.cmd("verify")[0], 2)

    def test_half_patched_file(self):
        (self.client / "bin64/b.exe").write_bytes(apply_sites(B, SITES_B[:1]))
        self.assert_refused_untouched("apply")
        code, out = self.cmd("status")
        self.assertIn("sites: patched,stock", out)

    def test_corrupt_backup_refuses_revert(self):
        self.assertEqual(self.cmd("apply")[0], 0)
        (self.client / "bin64/b.exe.eveoffline-orig").write_bytes(b"x" * len(B))
        self.assert_refused_untouched("revert")

    def test_corrupt_backup_refuses_apply(self):
        (self.client / "bin64/b.exe.eveoffline-orig").write_bytes(b"x")
        self.assert_refused_untouched("apply")

    def test_b1_without_b0(self):
        out = self.assert_refused_untouched("apply", "--only", "B1")
        self.assertIn("requires B0", out)
        self.assertEqual(self.cmd("apply", "--only", "B0")[0], 0)
        self.assertEqual(self.cmd("apply", "--only", "B1")[0], 0)
        self.assertEqual(self.f("bin64/b.exe"), B_P)

    def test_interrupted_write_leaves_original(self):
        real = os.replace

        def boom(src, dst):
            if str(dst).endswith("b.exe"):
                raise OSError("disk full")
            return real(src, dst)

        with mock.patch.object(ep.os, "replace", boom):
            code, out = self.cmd("apply")
        self.assertEqual(code, 1, out)  # reported, not a traceback (review item 3)
        self.assertEqual(self.f("bin64/b.exe"), B)
        self.assertFalse((self.client / "bin64/b.exe.eveoffline-tmp").exists())
        # The next run finishes the job.
        self.assertEqual(self.cmd("apply")[0], 0)

    def test_stale_tmp_is_removed(self):
        (self.client / "bin64/a.dll.eveoffline-tmp").write_bytes(b"junk")
        self.assertEqual(self.cmd("apply")[0], 0)
        self.assertFalse((self.client / "bin64/a.dll.eveoffline-tmp").exists())

    def test_live_install_guard(self):
        live = self.client.parent / "tq"
        self.client.rename(live)
        self.client = live
        out = self.assert_refused_untouched("apply")
        self.assertIn("live install", out)
        self.assertEqual(self.cmd("apply", "--allow-real")[0], 0)

    def test_repo_guard(self):
        with mock.patch.object(ep, "REPO", pathlib.Path(self.tmp.name)):
            self.assert_refused_untouched("apply")

    def test_no_manifest_for_build(self):
        (self.client / "start.ini").write_bytes(START_INI.replace(b"build = 0", b"build = 9"))
        out = self.assert_refused_untouched("apply")
        self.assertIn("no manifest for build 9", out)
        self.assertEqual(self.cmd("verify")[0], 2)

    def test_build_flag_mismatch(self):
        self.assert_refused_untouched("plan", "--build", "5")


class Prefs(Base):
    def test_forbidden_key_fails_verify(self):
        self.assertEqual(self.cmd("apply")[0], 0)
        with open(self.prefs, "ab") as f:
            f.write(b"networkLogging=10.0.0.1:12201\r\n")
        code, out = self.cmd("verify")
        self.assertEqual(code, 1)
        self.assertIn("networkLogging", out)

    def test_created_when_absent(self):
        self.prefs.unlink()
        self.assertEqual(self.cmd("apply")[0], 0, )
        self.assertEqual(self.prefs.read_bytes(),
                         b"breakpadUpload=0\r\nsentry_io_dsn=http://0@127.0.0.1:9/0\r\n")
        self.assertEqual(self.cmd("revert")[0], 0)
        self.assertEqual(self.prefs.read_bytes(), b"")

    def test_user_change_after_apply_survives_revert(self):
        self.assertEqual(self.cmd("apply")[0], 0)
        data = ep.prefs_set(self.prefs.read_bytes(), "breakpadUpload", "1")
        self.prefs.write_bytes(ep.prefs_set(data, "windowed", "0"))
        self.assertEqual(self.cmd("revert")[0], 0)
        got = self.prefs.read_bytes()
        self.assertIn(b"windowed=0", got)
        self.assertIn(b"breakpadUpload=1", got)
        self.assertNotIn(b"sentry_io_dsn", got)

    def test_folder_rule(self):
        drive, folder = ep.prefs_folder("/x/pfx/drive_c/CCP/EVE/Tq Offline")
        self.assertEqual(str(drive), "/x/pfx/drive_c")
        self.assertEqual(folder, "c_ccp_eve_tq_offline")
        self.assertEqual(ep.prefs_folder("/no/wine/here"), (None, None))

    def test_prefs_edits_exact(self):
        self.assertEqual(ep.prefs_set(b"a=1\r\nb=2", "b", "3"), b"a=1\r\nb=3")
        self.assertEqual(ep.prefs_set(b"a=1\r\n", "c", "3"), b"a=1\r\nc=3\r\n")
        self.assertEqual(ep.prefs_set(b"a=1", "c", "3"), b"a=1\r\nc=3\r\n")
        self.assertEqual(ep.prefs_remove(b"a=1\r\nc=3\r\n", "c"), b"a=1\r\n")
        self.assertIsNone(ep.prefs_get(b"httpx=1", "http"))


class Ini(unittest.TestCase):
    def test_crlf_without_final_newline(self):
        src = b"[main]\r\nbuild = 1\r\ncryptoPack = CryptoAPI"
        self.assertEqual(ep.ini_set(src, {"main.cryptoPack": "Placebo"}),
                         b"[main]\r\nbuild = 1\r\ncryptoPack = Placebo")
        self.assertEqual(ep.ini_set(src, {"main.cryptoPack": "Placebo"}, "lf"),
                         b"[main]\nbuild = 1\ncryptoPack = Placebo")

    def test_key_absent_or_twice(self):
        with self.assertRaises(ValueError):
            ep.ini_set(b"[main]\r\nbuild = 1\r\n", {"main.cryptoPack": "Placebo"})
        with self.assertRaises(ValueError):
            ep.ini_set(b"[main]\ncryptoPack=a\ncryptoPack=b\n", {"main.cryptoPack": "Placebo"})

    def test_other_section_untouched(self):
        with self.assertRaises(ValueError):
            ep.ini_set(b"[other]\ncryptoPack = a\n", {"main.cryptoPack": "Placebo"})


class Resfiles(Base):
    def test_missing_resfile_fails_verify(self):
        self.assertEqual(self.cmd("apply")[0], 0)
        (self.client.parent / "ResFiles" / "ab" / "x").write_bytes(b"1")
        code, out = self.cmd("verify")
        self.assertEqual(code, 1)
        self.assertIn("1 wrong size", out)
        self.assertEqual(self.cmd("verify", "--no-resfiles")[0], 0)


class LaunchEnv(Base):
    def test_lines(self):
        code, out = self.cmd("launch-env", "--server", "10.1.2.3")
        self.assertEqual(code, 0)
        self.assertIn("env NO_PROXY=10.1.2.3,localhost\n", out)
        self.assertIn("env EVE_PUBLIC_GATEWAY_ADDRESS=10.1.2.3:8443\n", out)
        self.assertIn("arg /noCrashReportUpload\n", out)
        self.assertIn("forbid-arg /refreshToken\n", out)

    def test_json_by_build_alone(self):
        code, out = self.run_cmd("launch-env", "--build", "0", "--json")
        self.assertEqual(code, 0, out)
        self.assertEqual(json.loads(out)["env"]["NO_PROXY"], "10.137.0.1,localhost")

    def test_check_args(self):
        ok = "/ssoToken=x /fileLogDirectory=C:\\eve-logs /noCrashReportUpload /pyfoo"
        self.assertEqual(self.cmd("launch-env", "--check-args", ok)[0], 0)
        for bad in ("/refreshToken=abc", "/REFRESHTOKEN", "/py", "/tools=x"):
            self.assertEqual(self.cmd("launch-env", "--check-args", ok + " " + bad)[0], 3, bad)

    def test_bad_server(self):
        self.assertEqual(self.cmd("launch-env", "--server", "a b")[0], 2)


class ManifestLint(unittest.TestCase):
    def test_synthetic_manifest_is_valid(self):
        self.assertEqual(ep.validate_manifest(manifest(), "0.json"), [])

    def broken(self, mutate):
        m = manifest()
        mutate(m)
        return ep.validate_manifest(m, "0.json")

    def test_rejections(self):
        f = lambda m: m["files"]["bin64/a.dll"]  # noqa: E731
        cases = {
            "before": lambda m: f(m)["sites"][0].update(before_hex="488b5810"),
            "1..64": lambda m: f(m)["sites"][0].update(after_hex="00" * 65),
            "64 lowercase hex": lambda m: f(m).update(sha256_stock="ABC"),
            "past size": lambda m: f(m)["sites"][0].update(offset=len(A) - 2),
            "cycle": lambda m: m["requires"].update(B0=["B1"]),
            "not a file id": lambda m: m["requires"].update(B9=["B0"]),
            "does not match build": lambda m: m.update(build=1),
            "never empty": lambda m: m["launch"]["env"].update(X=""),
            "itself forbidden": lambda m: m["launch"]["args"].append("/refreshToken=1"),
            "both set and forbidden": lambda m: m["prefs"]["forbid"].append("breakpadUpload"),
            "overlap": lambda m: f(m)["sites"].append(dict(f(m)["sites"][0])),
            "bare file name": lambda m: m["resfiles_check"].append("../x"),
        }
        for want, mutate in cases.items():
            errs = self.broken(mutate)
            self.assertTrue(any(want in e for e in errs), f"{want}: {errs}")

    def test_every_shipped_manifest(self):
        """Every tools/patches/*.json is valid, and its stock hashes agree with
        tools/client-pin.sha256 when the pin is for the same build."""
        pin = {}
        pin_build = None
        pin_path = os.path.join(HERE, "client-pin.sha256")
        if os.path.exists(pin_path):
            for line in pathlib.Path(pin_path).read_text(encoding="utf-8").splitlines():
                if line.startswith("# build:"):
                    pin_build = line.split(":", 1)[1].strip()
                elif line.strip() and not line.startswith("#"):
                    h, p = line.split(None, 1)
                    pin[p.strip().removeprefix("./")] = h
        shipped = sorted(pathlib.Path(HERE, "patches").glob("*.json"))
        self.assertTrue(shipped, "no manifests in tools/patches")
        for path in shipped:
            m = json.loads(path.read_text())
            self.assertEqual(ep.validate_manifest(m, path.name), [], path.name)
            if str(m["build"]) != pin_build:
                continue
            for rel, e in m["files"].items():
                with self.subTest(file=rel):
                    stock = pin.get(rel + ".eveoffline-orig", pin.get(rel))
                    self.assertEqual(stock, e["sha256_stock"])
                    if e["kind"] == "ccp":
                        # A patched code.ccp's bytes depend on the host's zlib;
                        # the manifest holds the canonical hash, and pin-client.sh
                        # compares that. The pin file records the raw one.
                        self.assertIsNotNone(pin.get(rel))
                        if pin[rel] != e["sha256_stock"]:
                            # a non-stock raw row is excused only by the .orig row
                            self.assertEqual(pin.get(rel + ".eveoffline-orig"), e["sha256_stock"])
                        continue
                    self.assertIn(pin.get(rel), (e["sha256_stock"], e["sha256_patched"]))


class JailAllowList(unittest.TestCase):
    """Once tools/jail-client.sh reads `launch-env` (coordinator diff of lane
    PATCH-1), every env key a shipped manifest sets must be in its fixed
    allow-list, or the jail silently drops it."""

    def test_manifest_env_keys_are_allowed(self):
        path = pathlib.Path(HERE, "jail-client.sh")
        text = path.read_text() if path.exists() else ""
        if "eve_patch.py\" launch-env" not in text and "eve_patch.py launch-env" not in text:
            self.skipTest("jail-client.sh does not read eve_patch.py launch-env yet")
        block = text.split("declare -A patch_allowed=(", 1)[1].split(")", 1)[0]
        allowed = set(re.findall(r"\[([A-Z_0-9]+)\]=1", block))
        for path in sorted(pathlib.Path(HERE, "patches").glob("*.json")):
            env = json.loads(path.read_text())["launch"]["env"]
            self.assertLessEqual(set(env), allowed, path.name)


# --------------------------------------------------------------------------
# Tier C: kind "ccp". Every member is generated here; the "strings" are
# invented host names under example.invalid, never a real one.


def marshal_blob(seed, strings, filler=600):
    """A fake .pyc: header, then a tuple of filler text and the given strings
    (`s` type). Returns (raw, {string: offset of its type byte})."""
    out = bytearray(b"\x03\xf3\r\n\0\0\0\0")
    out += b"(" + len(strings).to_bytes(4, "little") + b"\0\0\0"[:0]
    # compressible, seed-dependent filler, as its own marshal string
    text = (f"module {seed} " * 40).encode()[:filler] + stream(seed, 24).hex().encode()
    out = bytearray(b"\x03\xf3\r\n\0\0\0\0") + b"(" + (len(strings) + 1).to_bytes(4, "little")
    out += b"s" + len(text).to_bytes(4, "little") + text
    offs = {}
    for v in strings:
        offs[v] = len(out)
        out += b"s" + len(v).to_bytes(4, "little") + v
    return bytes(out), offs


def make_zip(members, level=6):
    """A code.ccp-shaped zip: method 0, flags 0, bodies are zlib streams, CRC of the body."""
    import struct
    import zlib

    body, cd = bytearray(), bytearray()
    for name, raw in members.items():
        z = zlib.compress(raw, level)
        nb = name.encode()
        crc = zlib.crc32(z)
        lo = len(body)
        body += struct.pack("<4sHHHHHIIIHH", b"PK\x03\x04", 20, 0, 0, 30098, 23901, crc, len(z), len(z), len(nb), 0) + nb + z
        cd += struct.pack("<4sHHHHHHIIIHHHHHII", b"PK\x01\x02", 20, 20, 0, 0, 30098, 23901, crc, len(z), len(z), len(nb), 0, 0, 0, 0, 0, lo) + nb
    eocd = struct.pack("<4sHHHHIIH", b"PK\x05\x06", 0, 0, len(members), len(members), len(cd), len(body), 0)
    return bytes(body + cd + eocd)


OLD1 = b"https://old-one.example.invalid/path/"
OLD2 = b"http://old-two.example.invalid"
RAW1, OFF1 = marshal_blob("one", [OLD1, b"keep me"])
RAW2, OFF2 = marshal_blob("two", [OLD2])
RAW3, _ = marshal_blob("three", [b"unrelated"])
CCP_STOCK = make_zip({"pkg/one.pyj": RAW1, "two.pyj": RAW2, "three.pyj": RAW3})


def ccp_entry(stock=CCP_STOCK, repl1="http://127.0.0.1:9/", repl2="gone.invalid", extra=None):
    import zlib  # noqa: F401

    idx = ep.zip_index(stock)

    def patched_raw(raw, off, old, new):
        out = bytearray(raw)
        out[off + 5 : off + 5 + len(old)] = new.encode()
        out[off + 1 : off + 5] = len(new).to_bytes(4, "little")
        return bytes(out)

    p1 = patched_raw(RAW1, OFF1[OLD1], OLD1, repl1)
    p2 = patched_raw(RAW2, OFF2[OLD2], OLD2, repl2)
    e = {"id": "C1", "tier": "C", "kind": "ccp", "size": len(stock), "sha256_stock": H(stock),
         "sha256_patched": "0" * 64,
         "members": [
             {"name": "pkg/one.pyj", "slot": idx["pkg/one.pyj"][0]["cs"], "raw_sha256_stock": H(RAW1), "raw_sha256_patched": H(p1),
              "sites": [{"offset": OFF1[OLD1], "old_len": len(OLD1), "after_ascii": repl1}]},
             {"name": "two.pyj", "slot": idx["two.pyj"][0]["cs"], "raw_sha256_stock": H(RAW2), "raw_sha256_patched": H(p2),
              "sites": [{"offset": OFF2[OLD2], "old_len": len(OLD2), "after_ascii": repl2}]},
         ]}
    out = ep.ccp_transform(e, stock)
    e["sha256_patched"] = ep.ccp_canon(e, out)
    return e, out


def ccp_manifest(entry):
    m = manifest()
    m["files"]["code.ccp"] = entry
    m["requires"]["C1"] = ["B0"]
    return m


class PinClient(unittest.TestCase):
    """`tools/pin-client.sh`: a pin written from a patched copy accepts the stock
    copy, the patched copy and a differently compressed patched copy."""

    BUILD = "777"

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        tools = self.root / "tools"
        (tools / "patches").mkdir(parents=True)
        for n in ("pin-client.sh", "eve_patch.py"):
            shutil.copy(os.path.join(HERE, n), tools / n)
        self.entry, self.patched = ccp_entry()
        (tools / "patches" / f"{self.BUILD}.json").write_text(json.dumps({"build": int(self.BUILD), "files": {"code.ccp": self.entry}}))
        self.pin = self.root / "pin.sha256"

    def install(self, name, ccp, orig=None):
        d = self.root / name
        d.mkdir()
        (d / "start.ini").write_text(f"[main]\nbuild = {self.BUILD}\n")
        (d / "code.ccp").write_bytes(ccp)
        if orig is not None:
            (d / "code.ccp.eveoffline-orig").write_bytes(orig)
        return d

    def sh(self, *args):
        p = subprocess.run(["bash", str(self.root / "tools" / "pin-client.sh"), *args],
                           capture_output=True, text=True)
        return p.returncode, p.stdout + p.stderr

    def write_pin(self):
        d = self.install("patched", self.patched, orig=CCP_STOCK)
        self.assertEqual(self.sh("--write", "--path", str(d), "--manifest", str(self.pin))[0], 0)
        return d

    def verify(self, d):
        return self.sh("--path", str(d), "--manifest", str(self.pin))

    def test_patched_stock_and_differently_compressed_copies_all_pass(self):
        d = self.write_pin()
        self.assertEqual(self.verify(d)[0], 0)
        stock = self.install("stock", CCP_STOCK)
        code, out = self.verify(stock)
        self.assertEqual(code, 0, out)
        with mock.patch.object(ep, "CCP_ATTEMPTS", ((1, 8),)):
            other = ep.ccp_transform(self.entry, CCP_STOCK)
        self.assertNotEqual(other, self.patched)
        self.assertTrue(ep.is_patched(self.entry, other))
        self.assertEqual(self.verify(self.install("other", other, orig=CCP_STOCK))[0], 0)

    def test_stock_copy_with_its_orig_row_passes_and_so_does_revert(self):
        self.write_pin()
        self.assertEqual(self.verify(self.install("stock", CCP_STOCK, orig=CCP_STOCK))[0], 0)

    def test_a_pin_without_the_orig_row_does_not_excuse_its_raw_code_ccp(self):
        d = self.write_pin()
        lines = [ln for ln in self.pin.read_text().splitlines() if "code.ccp.eveoffline-orig" not in ln]
        self.pin.write_text("\n".join(lines) + "\n")
        self.assertEqual(self.verify(d)[0], 0)  # the patched copy itself still matches
        self.assertEqual(self.verify(self.install("stock", CCP_STOCK))[0], 1)

    def test_an_unknown_code_ccp_fails(self):
        self.write_pin()
        self.assertEqual(self.verify(self.install("bad", CCP_STOCK + b"!", orig=CCP_STOCK))[0], 1)


class _Fat:
    """A stand-in compressor whose output never fits."""

    def compress(self, b):
        return b""

    def flush(self):
        return b"\x78\x9c" + b"\0" * 5000


class Ccp(Base):
    def setUp(self):
        super().setUp()
        self.entry, self.patched = ccp_entry()
        (self.client / "code.ccp").write_bytes(CCP_STOCK)
        (self.patches / "0.json").write_text(json.dumps(ccp_manifest(self.entry)))

    def test_default_tiers_include_c(self):
        self.assertEqual(ep.DEFAULT_TIERS, "A,P,B,C")
        self.assertEqual(self.cmd("apply")[0], 0)
        self.assertEqual(self.f("code.ccp"), self.patched)
        self.assertEqual(self.f("code.ccp.eveoffline-orig"), CCP_STOCK)

    def test_tiers_without_c_leave_code_ccp_alone(self):
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B")[0], 0)
        self.assertEqual(self.f("code.ccp"), CCP_STOCK)
        self.assertFalse((self.client / "code.ccp.eveoffline-orig").exists())
        code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("code.ccp"), self.patched)

    def test_roundtrip_and_rest_of_archive_untouched(self):
        code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertEqual(code, 0, out)
        new = self.f("code.ccp")
        self.assertEqual(len(new), len(CCP_STOCK))
        self.assertEqual(self.f("code.ccp.eveoffline-orig"), CCP_STOCK)
        a, b = ep.zip_index(CCP_STOCK), ep.zip_index(new)
        self.assertEqual(a["three.pyj"], b["three.pyj"])
        self.assertEqual(a["two.pyj"][0]["lo"], b["two.pyj"][0]["lo"])
        self.assertLess(b["two.pyj"][0]["cs"], a["two.pyj"][0]["cs"])
        # the strings really changed, to the replacements
        import zlib

        z = b["pkg/one.pyj"][0]
        lo = z["lo"] + 30 + len("pkg/one.pyj")
        raw = zlib.decompress(new[lo : lo + z["cs"]])
        self.assertIn(b"http://127.0.0.1:9/", raw)
        self.assertNotIn(b"old-one", raw)
        self.assertIn(b"keep me", raw)
        self.assertEqual(self.cmd("verify", "--tier", "A,P,B,C")[0], 0)
        before = self.snapshot()
        code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertIn("already applied", out)
        self.assertEqual(self.snapshot(), before)
        code, out = self.cmd("revert")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.f("code.ccp"), CCP_STOCK)
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B,C")[0], 0)

    def test_status_names_a_wrong_member(self):
        bad = make_zip({"pkg/one.pyj": RAW1 + b"x", "two.pyj": RAW2, "three.pyj": RAW3})
        (self.client / "code.ccp").write_bytes(bad)
        before = self.snapshot()
        code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertEqual(code, 2, out)
        self.assertEqual(self.snapshot(), before)
        self.assertIn("pkg/one.pyj:other", self.cmd("status")[1])

    def test_member_hash_checked_inside_transform(self):
        bad = make_zip({"pkg/one.pyj": RAW1, "two.pyj": RAW2.replace(b"module two", b"module TWO"), "three.pyj": RAW3})
        with self.assertRaises(ep.Refuse):
            ep.ccp_transform(self.entry, bad)

    def test_no_fit_refuses_with_nothing_written(self):
        big = stream("big", 300).hex()  # incompressible, far past any slack in the slot
        e = json.loads(json.dumps(self.entry))
        raw = RAW2[: OFF2[OLD2]] + b"s" + len(big).to_bytes(4, "little") + big.encode() + RAW2[OFF2[OLD2] + 5 + len(OLD2) :]
        e["members"][1]["sites"][0]["after_ascii"] = big[:64]
        e["members"][1]["raw_sha256_patched"] = H(raw)
        with self.assertRaises(ep.Refuse):
            ep.ccp_transform(e, CCP_STOCK)  # hash of the patched member differs first: refused before writing
        # and a replacement that matches its hash but cannot fit:
        e["members"][1]["sites"][0]["after_ascii"] = big[:64]
        small = RAW2[: OFF2[OLD2]] + b"s" + (64).to_bytes(4, "little") + big[:64].encode() + RAW2[OFF2[OLD2] + 5 + len(OLD2) :]
        e["members"][1]["raw_sha256_patched"] = H(small)
        e["members"][1]["slot"] = ep.zip_index(CCP_STOCK)["two.pyj"][0]["cs"]
        with mock.patch.object(ep, "CCP_ATTEMPTS", [(9, 8)]), mock.patch.object(ep.zlib, "compressobj", lambda *a: _Fat()):
            with self.assertRaises(ep.Refuse) as cm:
                ep.ccp_transform(e, CCP_STOCK)
        self.assertIn("slot", str(cm.exception))
        # through the CLI nothing is written either
        (self.client / "code.ccp").write_bytes(CCP_STOCK)
        m = ccp_manifest(e)
        (self.patches / "0.json").write_text(json.dumps(m))
        before = self.snapshot()
        with mock.patch.object(ep.zlib, "compressobj", lambda *a: _Fat()):
            code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertNotEqual(code, 0)
        self.assertEqual(self.f("code.ccp"), CCP_STOCK)
        self.assertEqual({k: v for k, v in self.snapshot().items() if "code.ccp" in k and not k.endswith("orig")}, {k: v for k, v in before.items() if "code.ccp" in k})

    def test_canonical_hash_survives_another_compressor(self):
        import zlib

        class Level1:
            def __init__(self):
                self.buf = b""

            def compress(self, b):
                self.buf += b
                return b""

            def flush(self):
                return zlib.compress(self.buf, 1)

        with mock.patch.object(ep.zlib, "compressobj", lambda *a: Level1()):
            other = ep.ccp_transform(self.entry, CCP_STOCK)
        self.assertNotEqual(H(other), H(self.patched))
        self.assertEqual(ep.ccp_canon(self.entry, other), self.entry["sha256_patched"])
        (self.client / "code.ccp").write_bytes(other)
        self.assertIn("patched", self.cmd("status", "--tier", "C")[1])
        self.assertEqual(self.cmd("verify", "--tier", "C", "--only", "C1")[0], 1)  # C1's backup is absent, the state is patched
        self.assertIn("patched", self.cmd("status")[1])

    def test_canon_rejects_stock_and_tampering(self):
        self.assertIsNone(ep.ccp_canon(self.entry, CCP_STOCK))
        tam = bytearray(self.patched)
        tam[ep.zip_index(self.patched)["three.pyj"][0]["lo"] + 40] ^= 1
        self.assertNotEqual(ep.ccp_canon(self.entry, bytes(tam)), self.entry["sha256_patched"])  # a byte outside the targets counts
        z = ep.zip_index(self.patched)["two.pyj"][0]
        tam = bytearray(self.patched)
        struct_off = z["lo"] + 14
        tam[struct_off] ^= 1  # local CRC no longer the body's
        self.assertIsNone(ep.ccp_canon(self.entry, bytes(tam)))

    def test_unknown_c_file_does_not_block_other_tiers(self):
        (self.client / "code.ccp").write_bytes(CCP_STOCK + b"!")
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B")[0], 0)
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B,C")[0], 2)
        self.assertEqual(self.cmd("apply")[0], 2)  # C is a default tier now

    def test_c_requires_b0(self):
        out = self.cmd("apply", "--tier", "C", "--only", "C1")
        self.assertEqual(out[0], 2, out)
        self.assertIn("requires B0", out[1])
        self.assertEqual(self.f("code.ccp"), CCP_STOCK)
        m = ccp_manifest(self.entry)
        m["requires"].pop("C1")
        self.assertTrue(any("must require B0" in x for x in ep.validate_manifest(m, "0.json")))


class CcpLint(unittest.TestCase):
    def broken(self, mutate):
        e, _ = ccp_entry()
        m = ccp_manifest(e)
        mutate(m["files"]["code.ccp"])
        return ep.validate_manifest(m, "0.json")

    def test_valid(self):
        self.assertEqual(self.broken(lambda e: None), [])

    def test_rejections(self):
        cases = {
            "1..64": lambda e: e["members"][0]["sites"][0].update(after_ascii="a" * 65),
            "never holds original": lambda e: e["members"][0]["sites"][0].update(before="x"),
            "listed twice": lambda e: e["members"].append(dict(e["members"][0])),
            "slot must be": lambda e: e["members"][0].update(slot=0),
            "64 lowercase hex": lambda e: e["members"][0].update(raw_sha256_stock="zz"),
            "overlap": lambda e: e["members"][0]["sites"].append(dict(e["members"][0]["sites"][0])),
            "go together": lambda e: e.update(tier="B"),
            "relative member name": lambda e: e["members"][0].update(name="../x"),
        }
        for want, mutate in cases.items():
            errs = self.broken(mutate)
            self.assertTrue(any(want in x for x in errs), f"{want}: {errs}")


# --------------------------------------------------------------------------
# tools/eve_patch_author.py: locate (C) and hosts


class Author(unittest.TestCase):
    def setUp(self):
        import eve_patch_author as au

        self.au = au
        self.tmp = tempfile.TemporaryDirectory()
        self.client = pathlib.Path(self.tmp.name) / "c"
        (self.client / "bin64").mkdir(parents=True)
        (self.client / "code.ccp").write_bytes(CCP_STOCK)
        self.rules = [("pkg/one.pyj", [(b"old-one.", 1, False)], "http://127.0.0.1:9/"),
                      ("two.pyj", [(b"old-two.example.invalid", 1, False)], "gone.invalid")]

    def tearDown(self):
        self.tmp.cleanup()

    def test_locate_finds_sites_and_matches_the_hand_built_entry(self):
        with mock.patch.object(self.au, "CCP_RULES", self.rules):
            e = self.au.locate_ccp(CCP_STOCK)
        want, patched = ccp_entry()
        self.assertEqual(e["sha256_patched"], want["sha256_patched"])
        self.assertEqual([(m["name"], m["slot"], [(s["offset"], s["old_len"]) for s in m["sites"]]) for m in e["members"]],
                         [(m["name"], m["slot"], [(s["offset"], s["old_len"]) for s in m["sites"]]) for m in want["members"]])
        m = ccp_manifest(e)
        self.assertEqual(ep.validate_manifest(m, "0.json"), [])

    def test_locate_refuses_unless_the_hit_count_is_exact(self):
        bad = [("pkg/one.pyj", [(b"old-one.", 2, False)], "x")]
        with mock.patch.object(self.au, "CCP_RULES", bad):
            with self.assertRaises(ep.Refuse):
                self.au.locate_ccp(CCP_STOCK)
        with mock.patch.object(self.au, "CCP_RULES", [("nope.pyj", [(b"x", 1, False)], "y")]):
            with self.assertRaises(ep.Refuse):
                self.au.locate_ccp(CCP_STOCK)

    def run_hosts(self, known):
        kp = pathlib.Path(self.tmp.name) / "known.txt"
        if known is not None:
            kp.write_text("# reviewed\n" + "".join(h + "\n" for h in known))
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = self.au.cmd_hosts(str(self.client), str(kp), False)
        return code, out.getvalue()

    def test_hosts_alarm_on_a_new_host(self):
        (self.client / "bin64" / "x.dll").write_bytes(b"\0junk\0https://new-one.example.invalid/api\0old-two.example.invalid\0")
        write = io.StringIO()
        kp = pathlib.Path(self.tmp.name) / "known.txt"
        with contextlib.redirect_stdout(write):
            self.assertEqual(self.au.cmd_hosts(str(self.client), str(kp), True), 0)
        self.assertIn("new-one.example.invalid", kp.read_text())
        self.assertEqual(self.run_hosts(read := kp.read_text().splitlines())[0], 0)
        (self.client / "bin64" / "y.dll").write_bytes(b"\0wss://sneaky.example.invalid:443/\0")
        code, out = self.run_hosts(read)
        self.assertEqual(code, 1, out)
        self.assertIn("NEW", out)
        self.assertIn("sneaky.example.invalid", out)

    def test_host_normalisation(self):
        h = self.au.host_of
        self.assertEqual(h(b"https://a.example.invalid:8443/x"), "a.example.invalid")
        self.assertEqual(h(b"xyz.q7github.com"), "github.com")
        self.assertEqual(h(b"ocsp.example.invalid"), "ocsp.example.invalid")
        self.assertEqual(h(b"crl.example.com02"), "crl.example.com")

    def test_hosts_finds_a_dsn_shaped_url_and_unlisted_tlds(self):
        got = self.au.scan_hosts(b"x\0https://abc123@o999.ingest.example.invalid/1\0 telemetry.example.ai \0user@mail.example.ms\0ftp://files.example.invalid\0")
        self.assertEqual(got, {"o999.ingest.example.invalid", "telemetry.example.ai", "mail.example.ms", "files.example.invalid"})

    def test_hosts_walks_subdirectories_and_reports_unparsed_members(self):
        (self.client / "bin64" / "sub").mkdir()
        (self.client / "bin64" / "sub" / "deep.dll").write_bytes(b"\0deep.example.invalid\0")
        got = self.au.collect_hosts(str(self.client))
        self.assertIn("deep.example.invalid", got)

    def test_a_stale_backup_is_not_trusted(self):
        # build 0's manifest names bin/x; a backup that is not its stock is ignored
        client = pathlib.Path(self.tmp.name) / "c"
        (client / "start.ini").write_bytes(START_INI)
        (client / "bin64" / "a.dll").write_bytes(b"\0new-build.example.invalid\0")
        (client / "bin64" / "a.dll.eveoffline-orig").write_bytes(b"\0old-build.example.invalid\0")
        pdir = pathlib.Path(self.tmp.name) / "patches"
        pdir.mkdir()
        (pdir / "0.json").write_text(json.dumps(manifest()))
        with mock.patch.object(ep, "PATCHES", pdir), mock.patch.object(self.au, "_MANIFEST_CACHE", {}):
            self.assertIsNone(self.au.trusted_backup(str(client), "bin64/a.dll"))
            with contextlib.redirect_stderr(io.StringIO()):
                data, name = self.au.stock_bytes(str(client), "bin64/a.dll")
            self.assertIn(b"new-build", data)
            got = self.au.collect_hosts(str(client))
        self.assertIn("new-build.example.invalid", got)
        self.assertNotIn("old-build.example.invalid", got)

    def test_hosts_without_a_list_refuses(self):
        self.assertEqual(self.run_hosts(None)[0], 2)

    def test_hosts_reads_code_ccp_members_and_utf16(self):
        (self.client / "bin64" / "w.dll").write_bytes("\0\0wide.example.invalid\0".encode("utf-16-le"))
        got = self.au.collect_hosts(str(self.client))
        self.assertIn("old-two.example.invalid", got)
        self.assertTrue(any(src.startswith("code.ccp:") for src in got["old-two.example.invalid"]))
        self.assertIn("wide.example.invalid", got)


class ApplyAtomicAcrossFiles(Ccp):
    """Review item 3: a patch that cannot be made refuses BEFORE any file is written."""

    def test_no_fit_on_c1_leaves_the_whole_tree_untouched(self):
        e = json.loads(json.dumps(self.entry))
        big = stream("big", 300).hex()
        raw = RAW2[: OFF2[OLD2]] + b"s" + (64).to_bytes(4, "little") + big[:64].encode() + RAW2[OFF2[OLD2] + 5 + len(OLD2) :]
        e["members"][1]["sites"][0]["after_ascii"] = big[:64]
        e["members"][1]["raw_sha256_patched"] = H(raw)
        (self.patches / "0.json").write_text(json.dumps(ccp_manifest(e)))
        before = self.snapshot()
        with mock.patch.object(ep.zlib, "compressobj", lambda *a: _Fat()):
            code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertEqual(code, 2, out)
        self.assertIn("nothing written", out)
        self.assertEqual(self.snapshot(), before)  # no .orig, no prefs, no patched B files

    def test_a_write_error_midway_is_reported_not_a_traceback(self):
        real = os.replace

        def boom(src, dst):
            if str(dst).endswith("code.ccp"):
                raise PermissionError("locked")
            return real(src, dst)

        with mock.patch.object(ep.os, "replace", boom):
            code, out = self.cmd("apply", "--tier", "A,P,B,C")
        self.assertEqual(code, 1, out)
        self.assertIn("stopped by a write error", out)


class RevertAndVerifyWarnings(Ccp):
    def test_revert_with_an_unknown_unselected_code_ccp_does_not_claim_success(self):
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B")[0], 0)
        (self.client / "code.ccp").write_bytes(CCP_STOCK + b"!")
        code, out = self.cmd("revert", "--tier", "A,P,B")
        self.assertEqual(code, 1, out)
        self.assertIn("code.ccp", out)
        self.assertNotIn("revert: every target is stock", out)

    def test_verify_warns_about_an_unknown_unselected_target(self):
        self.assertEqual(self.cmd("apply", "--tier", "A,P,B")[0], 0)
        (self.client / "code.ccp").write_bytes(CCP_STOCK + b"!")
        code, out = self.cmd("verify", "--tier", "A,P,B")
        self.assertIn("WARN code.ccp", out)


class MalformedZip(unittest.TestCase):
    def test_truncated_and_bent_zips_refuse_instead_of_raising(self):
        e, patched = ccp_entry()
        for bad in (CCP_STOCK[:-30], CCP_STOCK[:-22] + CCP_STOCK[-22:-10] + b"\xff\xff\xff\xff" + CCP_STOCK[-6:]):
            with self.assertRaises(ep.Refuse):
                ep.zip_index(bad)
            self.assertIsNone(ep.ccp_canon(e, bad))
            self.assertEqual([w for _, w in ep.ccp_stock_members(e, bad)], ["missing", "missing"])
        # a central directory entry cut at the end of the file
        cut = bytearray(CCP_STOCK)
        i = bytes(cut).rfind(b"PK\x05\x06")
        cut[i + 16 : i + 20] = (i - 10).to_bytes(4, "little")  # cd offset near the end record
        with self.assertRaises(ep.Refuse):
            ep.zip_index(bytes(cut))


class WindowsPrefs(unittest.TestCase):
    def test_a_windows_install_finds_and_creates_prefs_under_localappdata(self):
        with tempfile.TemporaryDirectory() as t:
            local = pathlib.Path(t)
            settings = local / "CCP" / "EVE" / "c_ccp_eve_tq_tranquility" / "settings"
            with mock.patch.dict(os.environ, {"LOCALAPPDATA": str(local)}):
                found, create = ep.prefs_targets("C:\\CCP\\EVE\\tq", None)
                self.assertEqual(found, [])
                self.assertEqual(create, settings / "prefs.ini")
                settings.mkdir(parents=True)
                (settings / "prefs.ini").write_bytes(b"a=1")
                found, create = ep.prefs_targets("C:\\CCP\\EVE\\tq", None)
                self.assertEqual(found, [settings / "prefs.ini"])
                self.assertIsNone(create)
                found, _ = ep.prefs_targets("D:\\Games\\EVE Online\\tq", None)
                self.assertEqual(found, [])

    def test_without_localappdata_there_is_nothing_to_guess(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("LOCALAPPDATA", None)
            self.assertEqual(ep.prefs_targets("C:\\CCP\\EVE\\tq", None), ([], None))


@unittest.skipUnless(os.environ.get("EVE_PATCH_REAL_CLIENT"), "EVE_PATCH_REAL_CLIENT not set")
class RealInstallDryRun(unittest.TestCase):
    """Reads a real install's hashes; writes nothing; commits nothing."""

    def test_plan_never_unknown(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = ep.main(["plan", "--client", os.environ["EVE_PATCH_REAL_CLIENT"]])
        self.assertEqual(code, 0, out.getvalue())
        self.assertNotIn("unknown", out.getvalue())


if __name__ == "__main__":
    unittest.main()
