"""Tests for `tools/build-delta.py` (lane BA, R-BUILD BA-1).

Every fixture is generated here: two fake installs with a fake `start.ini`,
fake PE files (a hand-built header, invented code bytes and an invented
"certificate"), a fake `code.ccp` of hand-marshalled CPython 2.7 code
objects, fake resfileindex files and a fake patch manifest. No client byte
is read or stored.

Run::

    python3 -I tools/test_build_delta.py
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import struct
import sys
import tempfile
import unittest
import zipfile
import zlib
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last, so nothing in tools/ shadows the stdlib

_spec = importlib.util.spec_from_file_location("build_delta", os.path.join(HERE, "build-delta.py"))
bd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bd)

OLD_BUILD, NEW_BUILD = "1000", "1001"  # invented


# --- fixture builders ----------------------------------------------------------


def pe(body, cert=b"", checksum=0):
    """A minimal PE32+ image: headers, ``body``, then ``cert`` as the
    certificate table named by the security data-directory entry."""
    e_lfanew = 0x40
    head = bytearray(e_lfanew)
    head[0:2] = b"MZ"
    struct.pack_into("<I", head, 0x3C, e_lfanew)
    coff = b"PE\0\0" + struct.pack("<HHIIIHH", 0x8664, 0, 0, 0, 0, 240, 0x22)
    opt = bytearray(240)
    struct.pack_into("<H", opt, 0, 0x20B)
    struct.pack_into("<I", opt, 64, checksum)
    data = bytes(head) + coff + bytes(opt) + body
    if cert:
        sec = 112 + 8 * 4
        off = len(data)
        opt2 = bytearray(opt)
        struct.pack_into("<II", opt2, sec, off, len(cert))
        data = bytes(head) + coff + bytes(opt2) + body + cert
    return data


def w_str(b, tag=b"s"):
    return tag + struct.pack("<i", len(b)) + b


def w_tuple(items):
    return b"(" + struct.pack("<i", len(items)) + b"".join(items)


def w_code(name, code, names=(), consts=()):
    """A marshalled 2.7 code object (enough of one for pyc27)."""
    return (b"c" + struct.pack("<iiii", 0, 0, 1, 0)
            + w_str(code)
            + w_tuple([c if isinstance(c, bytes) and c[:1] == b"c" else w_str(c) for c in consts])
            + w_tuple([w_str(n, b"t") for n in names])
            + w_tuple([]) + w_tuple([]) + w_tuple([])
            + w_str(b"f.py") + w_str(name) + struct.pack("<i", 1) + w_str(b""))


def pyc(body, stamp=0):
    return b"\x03\xf3\r\n" + struct.pack("<I", stamp) + body


def module(funcs, top_names=()):
    """A module whose top level holds one code object per (name, code, names)."""
    kids = [w_code(n, c, ns) for n, c, ns in funcs]
    return w_code(b"<module>", b"d\x00\x00S", top_names, kids)


def ccp(path, mods, stamp):
    with zipfile.ZipFile(path, "w") as z:
        for name, body in mods.items():
            z.writestr(name + ".pyj", zlib.compress(pyc(body, stamp)))


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(data if isinstance(data, bytes) else data.encode())


SIGNED_BODY = b"\x90" * 200 + b"invented code" + b"\xcc" * 50
SITE_OFF = 100
AFTER = b"\x31\xc0\x90\x90"


class Fixture:
    """OLD and NEW installs that differ in exactly the ways a test asks for."""

    def __init__(self, tmp, **change):
        self.old, self.new = os.path.join(tmp, "old"), os.path.join(tmp, "new")
        self.findings = os.path.join(tmp, "findings")
        os.makedirs(self.findings)
        write(os.path.join(self.findings, "client-calls.tsv"),
              "service\tmethod\tcall_site\n" "svcA\tDoThing\tgame/caller.py:12\n")
        for root, build in ((self.old, OLD_BUILD), (self.new, NEW_BUILD)):
            new = root == self.new
            ini = "[main]\r\nbuild = %s\r\nsync = %s\r\ncodename = TEST\r\ncryptoPack = X\r\n" % (build, build)
            if new and change.get("codename"):
                ini = ini.replace("TEST", "TEST2")
            write(os.path.join(root, "start.ini"), ini)
            cert = b"cert-%s" % build.encode() * 20
            write(os.path.join(root, "bin64", "blue.dll"),
                  pe(SIGNED_BODY + (b"!" if new and change.get("blue_code") else b"."), cert, int(build)))
            body = bytearray(SIGNED_BODY)
            if new and change.get("rebuilt"):
                body[0:4] = b"TIME"
            write(os.path.join(root, "bin64", "other.pyd"), pe(bytes(body), cert, int(build)))
            write(os.path.join(root, "bin64", "same.txt"), b"identical")
            write(os.path.join(root, "bin64", "staticdata", "mapObjects.db"),
                  b"map-new" if new and change.get("map_db") else b"map")
            write(os.path.join(root, "bin64", "packages", "deep", "mod.bin"),
                  b"pkg-new" if new and change.get("package") else b"pkg")
            loader = bytearray(b"\x90" * 400)
            if new and change.get("loader_code"):
                loader[0:200] = b"\x11" * 200
            write(os.path.join(root, "bin64", "fooLoader.pyd"), pe(bytes(loader), cert))
            if new and change.get("added_file"):
                write(os.path.join(root, "bin64", "new.dll"), pe(b"x"))
            mods = {
                "game/caller": module([(b"f", b"d\x00\x00S", (b"x",))]),
                "gateway/thing": module([(b"g", b"d\x00\x00S", ())]),
                "game/remote": module([(b"h", b"d\x00\x00S", (b"RemoteSvc",))]),
                "carbon/machoNet/gps": module([(b"k", b"d\x00\x00S", ())]),
            }
            if new:
                if change.get("gateway"):
                    mods["gateway/thing"] = module([(b"g", b"d\x01\x00S", ()),
                                                    (b"g2", b"d\x00\x00S", ())])
                if change.get("caller"):
                    mods["game/caller"] = module([(b"f", b"d\x02\x00S", (b"x",))])
                if change.get("remote"):
                    mods["game/remote"] = module([(b"h", b"d\x03\x00S", (b"RemoteSvc",))])
                if change.get("gps"):
                    mods["carbon/machoNet/gps"] = module([(b"k", b"d\x04\x00S", ())])
            ccp(os.path.join(root, "code.ccp"), mods, int(build))
            idx = ["res:/staticdata/a.fsdbinary,h1", "res:/video/b/c.webm,h2", "res:/ui/x.png,h3"]
            if new and change.get("video"):
                idx[1] = "res:/video/b/c.webm,h2new"
                idx.append("res:/video/b/d.webm,h4")
            if new and change.get("static"):
                idx[0] = "res:/staticdata/a.fsdbinary,h1new"
            write(os.path.join(root, "resfileindex.txt"), "\n".join(idx) + "\n")
            # a patched old install keeps the stock copy beside it
            ld = bytearray(pe(SIGNED_BODY, cert, int(build)))
            if new and change.get("site_moved"):
                ld[SITE_OFF - 4:SITE_OFF + 8] = b"\x77" * 12
            write(os.path.join(root, "bin64", "ld.pyd"), bytes(ld))
            if not new:
                patched = bytearray(ld)
                patched[SITE_OFF:SITE_OFF + 4] = AFTER
                write(os.path.join(root, "bin64", "ld.pyd"), bytes(patched))
                write(os.path.join(root, "bin64", "ld.pyd" + bd.ORIG), bytes(ld))
        stock = pe(SIGNED_BODY, b"cert-%s" % OLD_BUILD.encode() * 20, int(OLD_BUILD))
        w = 16
        win = lambda data: hashlib.sha256(data[SITE_OFF - w:SITE_OFF + 4 + w]).hexdigest()  # noqa: E731
        patched = bytearray(stock)
        patched[SITE_OFF:SITE_OFF + 4] = AFTER
        self.manifest = os.path.join(tmp, "m.json")
        write(self.manifest, json.dumps({"files": {
            "bin64/ld.pyd": {"id": "B9", "kind": "bytes", "size": len(stock),
                             "sha256_stock": hashlib.sha256(stock).hexdigest(),
                             "sites": [{"offset": SITE_OFF, "after_hex": AFTER.hex(), "window": w,
                                        "window_sha256_stock": win(stock),
                                        "window_sha256_patched": win(bytes(patched))}]},
            "start.ini": {"id": "P9", "kind": "ini", "size": 0, "sha256_stock": "-",
                          "set": {"main.cryptoPack": "Placebo"}}}}))

    def run(self, schemas=False):
        return bd.run(self.old, self.new, self.manifest, self.findings, schemas)


def rows(r, layer):
    return {row[1]: row for row in r.rows if row[0] == layer}


def verdict(r):
    return r.rows[-1][2], r.rows[-1][5]


# --- tests -----------------------------------------------------------------------


class PeNormalise(unittest.TestCase):
    def test_signature_only_when_checksum_and_certificate_differ(self):
        a = pe(SIGNED_BODY, b"A" * 64, 1)
        b = pe(SIGNED_BODY, b"B" * 80, 2)
        self.assertNotEqual(a, b)
        self.assertEqual(bd.pe_normalise(a)[0], bd.pe_normalise(b)[0])

    def test_a_code_byte_survives_normalisation(self):
        a = pe(SIGNED_BODY + b".", b"A" * 64)
        b = pe(SIGNED_BODY + b"!", b"A" * 64)
        self.assertNotEqual(bd.pe_normalise(a)[0], bd.pe_normalise(b)[0])

    def test_not_a_pe_is_returned_unchanged(self):
        self.assertEqual(bd.pe_normalise(b"hello"), (b"hello", False))
        self.assertEqual(bd.pe_normalise(b"MZ" + b"\0" * 10), (b"MZ" + b"\0" * 10, False))


class Equivalent(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.tmp = self._t.name

    def tearDown(self):
        self._t.cleanup()

    def test_a_resigned_build_with_gateway_and_video_changes_is_equivalent(self):
        r = Fixture(self.tmp, gateway=True, video=True, rebuilt=True).run()
        files = rows(r, "file")
        self.assertEqual(files["bin64/blue.dll"][2], "signature-only")
        self.assertEqual(files["bin64/same.txt"][2], "identical")
        # subdirectories are walked, not skipped
        self.assertEqual(files["bin64/staticdata/mapObjects.db"][2], "identical")
        self.assertEqual(files["bin64/packages/deep/mod.bin"][2], "identical")
        self.assertEqual(files["bin64/other.pyd"][2], "rebuilt")
        self.assertEqual(files["bin64/ld.pyd"][2], "signature-only")  # our patch not counted
        mods = rows(r, "module")
        self.assertEqual(list(mods), ["gateway/thing"])
        self.assertIn("changed <module>.g", mods["gateway/thing"][5])
        self.assertIn("added <module>.g2", mods["gateway/thing"][5])
        self.assertEqual(rows(r, "callsurface")["gateway/thing"][2], "none")
        self.assertEqual(rows(r, "res")["resfileindex.txt:res:/video/b/*"][5], "+1 -0 ~1")
        self.assertEqual(rows(r, "patch")["B9 bin64/ld.pyd@%d" % SITE_OFF][2], "stock")
        self.assertEqual(rows(r, "patch")["P9 start.ini"][2], "ini keys present")
        ini = rows(r, "ini")
        self.assertEqual(ini["main.build"][3:5], (OLD_BUILD, NEW_BUILD))
        self.assertEqual(verdict(r)[0], "equivalent")

    def test_identical_installs_are_equivalent_with_no_module_rows(self):
        r = Fixture(self.tmp).run()
        self.assertEqual(rows(r, "module"), {})
        self.assertEqual(verdict(r)[0], "equivalent")


class Differs(unittest.TestCase):
    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.tmp = self._t.name

    def tearDown(self):
        self._t.cleanup()

    def check(self, want, **change):
        r = Fixture(self.tmp, **change).run()
        v, why = verdict(r)
        self.assertEqual(v, "differs", why)
        self.assertIn(want, why)
        return r

    def test_a_code_change_in_a_protocol_file(self):
        r = self.check("protocol file bin64/blue.dll", blue_code=True)
        # one byte past the signature: "rebuilt", and still a protocol change
        self.assertEqual(rows(r, "file")["bin64/blue.dll"][2], "rebuilt")

    def test_a_module_with_a_known_call_site(self):
        self.check("call site in findings", caller=True)

    def test_a_module_that_names_remotesvc(self):
        self.check("names RemoteSvc", remote=True)

    def test_a_protocol_module(self):
        self.check("protocol module carbon/machoNet/gps", gps=True)

    def test_a_static_data_change(self):
        self.check("static data changed res:/staticdata/a.fsdbinary", static=True)

    def test_a_changed_map_database_in_a_bin64_subdirectory_is_static_data(self):
        r = self.check("static data bin64/staticdata/mapObjects.db data-differs", map_db=True)
        self.assertEqual(rows(r, "file")["bin64/staticdata/mapObjects.db"][2], "data-differs")

    def test_a_changed_bundled_package_differs(self):
        r = self.check("bin64 subdirectory file bin64/packages/deep/mod.bin", package=True)
        self.assertEqual(rows(r, "file")["bin64/packages/deep/mod.bin"][2], "data-differs")

    def test_a_release_line_change(self):
        self.check("release line: start.ini main.codename", codename=True)

    def test_a_loader_whose_code_changed_needs_its_schema_read(self):
        r = self.check("schema of fooloader not checked".replace("fooloader", "foo"),
                       loader_code=True)
        self.assertEqual(rows(r, "schema")["foo"][2], "skipped")

    def test_a_moved_patch_site_reads_other(self):
        r = Fixture(self.tmp, site_moved=True).run()
        self.assertEqual(rows(r, "patch")["B9 bin64/ld.pyd@%d" % SITE_OFF][2], "other")

    def test_an_added_file_is_listed(self):
        r = Fixture(self.tmp, added_file=True).run()
        self.assertEqual(rows(r, "file")["bin64/new.dll"][2], "added")


class Cli(unittest.TestCase):
    def test_out_then_check_round_trips_and_a_mismatch_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            f = Fixture(tmp, gateway=True)
            out = os.path.join(tmp, "delta.tsv")
            args = [f.old, f.new, "--manifest", f.manifest, "--findings", f.findings, "--no-schemas"]
            err = io.StringIO()
            with redirect_stderr(err):
                self.assertEqual(bd.main(args + ["--out", out]), 0)
                self.assertEqual(bd.main(args + ["--check", out]), 0)
            with open(out, encoding="utf-8") as fh:
                text = fh.read()
            self.assertTrue(text.startswith("# build-delta"))
            self.assertIn("equivalent", err.getvalue())
            with open(out, "a", encoding="utf-8") as fh:
                fh.write("extra\n")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(bd.main(args + ["--check", out]), 1)

    def test_a_differing_build_exits_3_and_not_an_install_exits_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            f = Fixture(tmp, blue_code=True)
            with redirect_stderr(io.StringIO()), redirect_stdout(io.StringIO()):
                self.assertEqual(bd.main([f.old, f.new, "--manifest", f.manifest,
                                          "--findings", f.findings, "--no-schemas"]), 3)
                self.assertEqual(bd.main([tmp, f.new]), 2)

    def test_the_output_carries_no_client_bytes_only_hashes_names_and_counts(self):
        with tempfile.TemporaryDirectory() as tmp:
            f = Fixture(tmp, gateway=True, blue_code=True)
            text = bd.render(f.run())
            self.assertNotIn("invented code", text)
            self.assertNotIn("cert-", text)


if __name__ == "__main__":
    unittest.main()
