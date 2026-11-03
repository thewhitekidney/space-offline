"""Tests for `tools/import-build.sh`: bring a frozen build from the online
install into the offline prefix (owner decisions O19; ruling 14).

Everything runs against an invented home with an invented Steam library, a
fake online prefix, a fake offline prefix, fake indexes and fake blobs. The
tool is pointed at them through HOME, STEAM_ROOT, EVE_ONLINE_PREFIX,
EVE_OFFLINE_PREFIX and EVE_IMPORT_OUT; no real prefix is read. The property
checked on every run: **the online prefix is only read** -- no path, mode,
size, mtime or content under it changes (a hard link changes a source inode's
nlink and ctime, so those two are the only fields left out).

Run::

    python3 -I tools/test_import_build.py
"""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "import-build.sh")
EVE = os.path.join("pfx", "drive_c", "CCP", "EVE")


def snapshot(root: str) -> dict[str, tuple]:
    out: dict[str, tuple] = {}
    for d, dirs, files in os.walk(root):
        for name in dirs + files:
            p = os.path.join(d, name)
            st = os.lstat(p)
            if os.path.islink(p):
                body = os.readlink(p)
            elif os.path.isfile(p):
                with open(p, "rb") as f:
                    body = hashlib.sha256(f.read()).hexdigest()
            else:
                body = ""
            out[os.path.relpath(p, root)] = (st.st_mode, st.st_size, st.st_mtime_ns, st.st_ino, body)
    return out


def blob(body: bytes, tag: int = 0) -> tuple[str, str]:
    """(relative blob path, md5) for a body; the path follows the store's pattern."""
    md5 = hashlib.md5(body).hexdigest()
    name = "%016x_%s" % (tag, md5)
    return "%s/%s" % (name[:2], name), md5


@unittest.skipIf(os.geteuid() == 0, "import-build.sh refuses root, by design")
class ImportBuild(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-import-build-")
        self.home = os.path.join(self.tmp, "home")
        self.steam = os.path.join(self.home, ".local", "share", "Steam")
        self.on = os.path.join(self.steam, "steamapps", "compatdata", "8500")
        self.off = os.path.join(self.home, ".local", "share", "eve-offline", "prefix")
        self.out = os.path.join(self.tmp, "out")
        self.on_eve = os.path.join(self.on, EVE)
        self.off_eve = os.path.join(self.off, EVE)
        # Three invented blobs; one is already in the offline store.
        self.bodies = [b"alpha resource", b"bravo resource body", b"charlie"]
        self.rels = [blob(b, i + 1) for i, b in enumerate(self.bodies)]
        for rel, md5 in self.rels:
            self.put(self.on_eve, "ResFiles/" + rel, self.bodies[self.rels.index((rel, md5))])
        self.put(self.off_eve, "ResFiles/" + self.rels[0][0], self.bodies[0])
        self.put(self.off, "eve-offline-prefix.txt", "made by offline-prefix.sh\n")
        self.write_install("tq", "3619879")
        self.put(self.off_eve, "tq-offline/start.ini", "[main]\nbuild = 3619879\n")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    # --- building the trees -----------------------------------------------
    def put(self, root: str, rel: str, data) -> str:
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode())
        return p

    def index_row(self, i: int, body_override: bytes | None = None) -> str:
        rel, md5 = self.rels[i]
        size = len(self.bodies[i])
        return "res:/x/%d.red,%s,%s,%d,%d\n" % (i, rel, md5, size, size)

    def write_install(self, name: str, build: str, rows: list[int] | None = None) -> str:
        root = os.path.join(self.on_eve, name)
        rows = [0, 1, 2] if rows is None else rows
        self.put(root, "start.ini", "[main]\nbuild = %s\ncodename = V99.9\n" % build)
        self.put(root, "bin64/exefile.exe", "MZ")
        self.put(root, "resfileindex.txt", "".join(self.index_row(i) for i in rows))
        self.put(root, "resfileindex_Windows.txt", self.index_row(0))
        return root

    def run_tool(self, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.home,
               "STEAM_ROOT": self.steam, "EVE_ONLINE_PREFIX": self.on,
               "EVE_OFFLINE_PREFIX": self.off, "EVE_IMPORT_OUT": self.out, **extra}
        return subprocess.run(["bash", SCRIPT, *args], capture_output=True, text=True,
                              env=env, timeout=120)

    def read_only(self, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        """Run the tool and check the online prefix is untouched (links aside)."""
        before = snapshot(self.on)
        ino = {r: os.stat(os.path.join(self.on_eve, "ResFiles", r)).st_ino for r, _ in self.rels
               if os.path.exists(os.path.join(self.on_eve, "ResFiles", r))}
        r = self.run_tool(*args, **extra)
        after = snapshot(self.on)
        self.assertEqual(after, before, "the online prefix changed")
        for rel, i in ino.items():
            if os.path.exists(os.path.join(self.on_eve, "ResFiles", rel)):
                self.assertEqual(os.stat(os.path.join(self.on_eve, "ResFiles", rel)).st_ino, i)
        return r

    def off_blob(self, i: int) -> str:
        return os.path.join(self.off_eve, "ResFiles", self.rels[i][0])

    def dst(self, name: str = "tq-9999") -> str:
        return os.path.join(self.off_eve, name)

    # --- the happy path ----------------------------------------------------
    def test_import_copies_links_checks_and_pins(self) -> None:
        r = self.read_only("tq-9999")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.dst(), "bin64", "exefile.exe")))
        self.assertFalse(os.path.exists(self.dst() + ".partial"))
        # a real copy: its own inode, nothing shared with the online install
        a = os.stat(os.path.join(self.on_eve, "tq", "start.ini"))
        b = os.stat(os.path.join(self.dst(), "start.ini"))
        self.assertNotEqual((a.st_dev, a.st_ino), (b.st_dev, b.st_ino))
        self.assertEqual(b.st_nlink, 1)
        # the missing blobs are the online store's own inodes
        for i in (1, 2):
            a = os.stat(os.path.join(self.on_eve, "ResFiles", self.rels[i][0]))
            b = os.stat(self.off_blob(i))
            self.assertEqual((a.st_dev, a.st_ino), (b.st_dev, b.st_ino), i)
        self.assertIn("complete.", r.stdout)
        imp = os.path.join(self.out, "3619879", "import.txt")
        with open(imp) as f:
            text = f.read()
        self.assertIn("build:       3619879", text)
        self.assertIn("2 linked", text)
        self.assertIn("check-resources exit: 0", text)
        pin = os.path.join(self.out, "3619879", "pin.sha256")
        with open(pin) as f:
            self.assertIn("# build:   3619879", f.read())
        # provenance is not written into the install
        self.assertFalse(os.path.exists(os.path.join(self.dst(), "import.txt")))

    def test_an_existing_blob_is_never_overwritten(self) -> None:
        # The offline store holds a different file under blob 0's name.
        self.put(self.off_eve, "ResFiles/" + self.rels[0][0], b"mine, not the index's")
        r = self.read_only("tq-9999")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        with open(self.off_blob(0), "rb") as f:
            self.assertEqual(f.read(), b"mine, not the index's")

    # --- --check -----------------------------------------------------------
    def test_check_is_a_dry_run(self) -> None:
        before = snapshot(self.off)
        r = self.read_only("tq-9999", "--check")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("would link", r.stdout)
        self.assertEqual(snapshot(self.off), before, "--check wrote into the offline prefix")
        self.assertFalse(os.path.exists(self.out))

    def test_check_reports_missing_and_fails(self) -> None:
        os.remove(os.path.join(self.on_eve, "ResFiles", self.rels[2][0]))
        r = self.read_only("tq-9999", "--check")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("MISSING " + self.rels[2][0], r.stdout)

    # --- blobs -------------------------------------------------------------
    def test_missing_blob_is_listed_exit_nonzero_and_nothing_is_created(self) -> None:
        os.remove(os.path.join(self.on_eve, "ResFiles", self.rels[2][0]))
        r = self.read_only("tq-9999")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("MISSING " + self.rels[2][0], r.stdout)
        self.assertIn("download all resources", r.stdout)
        self.assertFalse(os.path.exists(self.dst()))
        self.assertFalse(os.path.exists(self.dst() + ".partial"))
        self.assertFalse(os.path.exists(os.path.join(self.out, "3619879", "import.txt")))

    def test_md5_mismatch_is_not_linked(self) -> None:
        # same size, other content: the launcher rewrote the blob in place
        path = os.path.join(self.on_eve, "ResFiles", self.rels[1][0])
        with open(path, "wb") as f:
            f.write(b"X" * len(self.bodies[1]))
        r = self.read_only("tq-9999")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("MISMATCH " + self.rels[1][0], r.stdout)
        self.assertIn("md5", r.stdout)
        self.assertFalse(os.path.exists(self.off_blob(1)))
        self.assertFalse(os.path.exists(self.dst()))

    def test_size_mismatch_is_not_linked(self) -> None:
        path = os.path.join(self.on_eve, "ResFiles", self.rels[1][0])
        with open(path, "ab") as f:
            f.write(b"more")
        r = self.read_only("tq-9999")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("size", r.stdout)
        self.assertFalse(os.path.exists(self.off_blob(1)))

    def test_hostile_index_names_create_nothing_outside_resfiles(self) -> None:
        root = os.path.join(self.on_eve, "tq")
        self.put(root, "resfileindex_prefetch.txt",
                 "res:/a,../../escape,%s,3,3\n" % ("0" * 32)
                 + "res:/b,/etc/passwd,%s,3,3\n" % ("0" * 32)
                 + "res:/c,ab/../../x_%s,%s,3,3\n" % ("0" * 32, "0" * 32))
        r = self.read_only("tq-9999")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("BAD-NAME", r.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.off_eve, "escape")))
        self.assertFalse(os.path.exists(os.path.join(self.off, "x_" + "0" * 32)))
        self.assertFalse(os.path.exists(self.dst()))

    # --- refusals ----------------------------------------------------------
    def refused(self, *args: str, why: str, **extra: str) -> None:
        before = snapshot(self.off)
        r = self.read_only(*args, **extra)
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertIn(why, r.stderr)
        self.assertEqual(snapshot(self.off), before, "a refused run wrote into the offline prefix")

    def test_refuses_the_online_names(self) -> None:
        self.refused("tq", why="online install name")
        self.refused("sisi", why="online install name")

    def test_refuses_bad_and_reserved_names(self) -> None:
        for n in ("../x", "a/b", "x.partial", "x.eveoffline-orig", ".hidden"):
            self.refused(n, why="")

    def test_refuses_an_existing_destination(self) -> None:
        self.refused("tq-offline", why="already exists")
        os.makedirs(self.dst() + ".partial")
        self.refused("tq-9999", why="already exists")

    def test_refuses_a_symlink_destination(self) -> None:
        os.symlink(self.tmp, self.dst())
        self.refused("tq-9999", why="already exists")

    def test_refuses_a_symlink_in_the_source(self) -> None:
        os.symlink("/etc/hostname", os.path.join(self.on_eve, "tq", "bin64", "link"))
        self.refused("tq-9999", why="symlink")
        self.assertFalse(os.path.exists(self.dst()))

    def test_refuses_our_patch_files_in_the_source(self) -> None:
        self.put(os.path.join(self.on_eve, "tq"), "start.ini.eveoffline-orig", "x")
        self.refused("tq-9999", why="not stock")

    def test_refuses_a_destination_that_is_not_the_offline_prefix(self) -> None:
        self.refused("tq-9999", why="inside an online prefix",
                     EVE_OFFLINE_PREFIX=os.path.join(self.steam, "steamapps", "compatdata", "8500", "copy"))
        os.remove(os.path.join(self.off, "eve-offline-prefix.txt"))
        self.refused("tq-9999", why="no offline prefix")

    def test_refuses_an_offline_prefix_that_is_a_symlink_into_the_online_one(self) -> None:
        link = os.path.join(self.tmp, "innocent")
        os.symlink(self.on, link)
        self.refused("tq-9999", why="inside an online prefix", EVE_OFFLINE_PREFIX=link)

    def test_refuses_a_source_outside_the_online_prefix(self) -> None:
        elsewhere = os.path.join(self.tmp, "elsewhere")
        shutil.copytree(self.on, elsewhere, symlinks=True)
        self.refused("tq-9999", why="not inside the online prefix", EVE_ONLINE_PREFIX=elsewhere)

    def test_refuses_while_a_process_uses_the_online_prefix(self) -> None:
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
               "WINEPREFIX": os.path.join(self.on, "pfx")}
        p = subprocess.Popen(["sleep", "60"], env=env)
        try:
            time.sleep(0.3)
            self.refused("tq-9999", why="processes are using the online prefix")
        finally:
            p.kill()
            p.wait()
        r = self.run_tool("tq-9999", "--check")
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_refuses_a_missing_install(self) -> None:
        shutil.rmtree(os.path.join(self.on_eve, "tq"))
        self.refused("tq-9999", why="no install")

    def test_refuses_resfiles_that_is_a_symlink(self) -> None:
        shutil.rmtree(os.path.join(self.off_eve, "ResFiles"))
        os.symlink(os.path.join(self.on_eve, "ResFiles"), os.path.join(self.off_eve, "ResFiles"))
        self.refused("tq-9999", why="ResFiles is not a plain directory")

    def test_refuses_root_and_conflicting_flags(self) -> None:
        r = self.run_tool("tq-9999", "--check", "--sweep")
        self.assertEqual(r.returncode, 2)
        r = self.run_tool()
        self.assertEqual(r.returncode, 2)

    # --- atomicity ---------------------------------------------------------
    def test_a_cut_off_run_leaves_no_destination(self) -> None:
        # A failing blob pass after the copy: .partial is removed, no tq-9999.
        os.remove(os.path.join(self.on_eve, "ResFiles", self.rels[1][0]))
        r = self.read_only("tq-9999")
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(self.dst()))
        self.assertFalse(os.path.exists(self.dst() + ".partial"))
        # restoring the blob lets the same name import: nothing was half-made
        self.put(self.on_eve, "ResFiles/" + self.rels[1][0], self.bodies[1])
        r = self.read_only("tq-9999")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    # --- --sweep -----------------------------------------------------------
    def test_sweep_is_clean_then_catches_a_rewritten_blob(self) -> None:
        self.assertEqual(self.read_only("tq-9999").returncode, 0)
        r = self.run_tool("tq-9999", "--sweep")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("sweep: clean", r.stdout)
        # the launcher rewrites a shared inode in place
        with open(self.off_blob(1), "wb") as f:
            f.write(b"Y" * len(self.bodies[1]))
        r = self.run_tool("tq-9999", "--sweep")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("MISMATCH " + self.rels[1][0], r.stdout)
        os.remove(self.off_blob(2))
        r = self.run_tool("tq-9999", "--sweep")
        self.assertIn("ABSENT " + self.rels[2][0], r.stdout)

    def test_sweep_does_not_need_or_touch_the_online_prefix(self) -> None:
        self.assertEqual(self.read_only("tq-9999").returncode, 0)
        shutil.rmtree(self.on)
        r = self.run_tool("tq-9999", "--sweep")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_sweep_of_nothing_fails(self) -> None:
        r = self.run_tool("tq-nope", "--sweep")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("no install", r.stderr)


class Client2Setup(unittest.TestCase):
    def test_install_dirs_are_settable_and_default_unchanged(self) -> None:
        with open(os.path.join(HERE, "client2-setup.sh")) as f:
            text = f.read()
        self.assertIn('${EVE_SECOND_SRC_DIR:-tq-offline}', text)
        self.assertIn('${EVE_SECOND_CLIENT_DIR:-tq-offline-2}', text)


if __name__ == "__main__":
    unittest.main()
