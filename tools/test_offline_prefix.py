"""Tests for the offline client's own Wine prefix (owner decision O19;
ruling 14): `tools/offline-prefix.sh`, which makes it once as a copy of the
online prefix, and `tools/prefix-lib.sh`, which the launchers read to find it
and to refuse the online prefix.

Everything runs against an invented home with an invented Steam library; no
client data, no Proton, no root. The property that matters most is checked
on every run of the script: **the online prefix is only read** -- every
path, mode, size, mtime and inode under it is the same afterwards.

Run::

    python3 -I tools/test_offline_prefix.py
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "offline-prefix.sh")
LIB = os.path.join(HERE, "prefix-lib.sh")
EVE = os.path.join("pfx", "drive_c", "CCP", "EVE")


def snapshot(root: str) -> dict[str, tuple]:
    """Every entry under root: kind, mode, size, mtime, inode, content hash."""
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
    st = os.lstat(root)
    out["."] = (st.st_mode, st.st_mtime_ns, st.st_ino)
    return out


@unittest.skipIf(os.geteuid() == 0, "offline-prefix.sh refuses root, by design")
class OfflinePrefix(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-offline-prefix-")
        self.home = os.path.join(self.tmp, "home")
        self.steam = os.path.join(self.home, ".local", "share", "Steam")
        self.src = os.path.join(self.steam, "steamapps", "compatdata", "8500")
        self.dst = os.path.join(self.home, ".local", "share", "eve-offline", "prefix")
        self.write("version", "GE-Proton10-34\n")
        self.write("pfx/system.reg", "WINE REGISTRY Version 2\n")
        self.write("pfx/user.reg", "WINE REGISTRY Version 2\n")
        self.write("pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE/c_ccp_eve_tq-offline_tranquility/settings/prefs.ini", "x=1\r\n")
        self.write(os.path.join(EVE, "tq-offline", "bin64", "exefile.exe"), "MZ")
        self.write(os.path.join(EVE, "tq-offline", "start.ini"), "[main]\ncryptoPack = Placebo\n")
        self.write(os.path.join(EVE, "tq", "bin64", "exefile.exe"), "MZ online")
        self.write(os.path.join(EVE, "sisi", "bin64", "exefile.exe"), "MZ sisi")
        self.write(os.path.join(EVE, "ResFiles", "ab", "ab12_cafe"), "resource")
        self.write(os.path.join(EVE, "index_tranquility.txt"), "res:/x,ab/ab12_cafe,0,8\n")
        os.makedirs(os.path.join(self.src, "pfx", "dosdevices"))
        os.symlink("../drive_c", os.path.join(self.src, "pfx", "dosdevices", "c:"))
        os.symlink("/", os.path.join(self.src, "pfx", "dosdevices", "z:"))

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def write(self, rel: str, text: str) -> None:
        p = os.path.join(self.src, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def run_script(self, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        # An empty fake /proc: a real client running on this box must not matter.
        proc = os.path.join(self.tmp, "proc")
        os.makedirs(proc, exist_ok=True)
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.home,
               "EVE_PROC_DIR": proc, **extra}
        return subprocess.run(["bash", SCRIPT, *args], capture_output=True, text=True,
                              env=env, timeout=120)

    def made(self, *args: str, **extra: str) -> subprocess.CompletedProcess[str]:
        before = snapshot(self.src)
        r = self.run_script(*args, **extra)
        self.assertEqual(snapshot(self.src), before, "the online prefix changed")
        return r

    def test_the_copy_is_whole_and_the_online_prefix_is_only_read(self) -> None:
        r = self.made()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for rel in ("version", "pfx/system.reg",
                    os.path.join(EVE, "tq-offline", "bin64", "exefile.exe"),
                    os.path.join(EVE, "ResFiles", "ab", "ab12_cafe"),
                    "pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE/c_ccp_eve_tq-offline_tranquility/settings/prefs.ini",
                    "eve-offline-prefix.txt"):
            self.assertTrue(os.path.isfile(os.path.join(self.dst, rel)), rel)
        # The online installs are left out.
        for rel in ("tq", "sisi"):
            self.assertFalse(os.path.exists(os.path.join(self.dst, EVE, rel)), rel)
        # Relative links stay relative, so they lead into the copy.
        self.assertEqual(os.readlink(os.path.join(self.dst, "pfx", "dosdevices", "c:")), "../drive_c")
        # A real copy: its own inode.
        a = os.stat(os.path.join(self.src, EVE, "ResFiles", "ab", "ab12_cafe"))
        b = os.stat(os.path.join(self.dst, EVE, "ResFiles", "ab", "ab12_cafe"))
        self.assertNotEqual((a.st_dev, a.st_ino), (b.st_dev, b.st_ino))
        self.assertFalse(os.path.exists(self.dst + ".partial"))
        self.assertIn("ready", self.run_script("--check").stdout)
        self.assertIn("its own copy", r.stdout)

    def test_the_launcher_login_state_is_not_copied(self) -> None:
        """F3: a real CCP session's cookies must not reach the jailed client's prefix."""
        users = "pfx/drive_c/users"
        for user in ("steamuser", "other"):
            self.write(f"{users}/{user}/AppData/Roaming/EVE Online/Network/Cookies", "invented")
            self.write(f"{users}/{user}/AppData/Roaming/EVE Online/state.json", "{}")
            self.write(f"{users}/{user}/AppData/Local/CCP/EVE/Launcher/cache", "invented")
        r = self.made()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for user in ("steamuser", "other"):
            base = os.path.join(self.dst, users, user, "AppData")
            self.assertFalse(os.path.exists(os.path.join(base, "Roaming", "EVE Online")), user)
            self.assertFalse(os.path.exists(os.path.join(base, "Local", "CCP", "EVE", "Launcher")), user)
        # Its sibling, the client's own settings, is still there.
        self.assertTrue(os.path.isfile(os.path.join(
            self.dst, users, "steamuser/AppData/Local/CCP/EVE/c_ccp_eve_tq-offline_tranquility/settings/prefs.ini")))

    def test_every_login_name_is_left_out_in_any_case(self) -> None:
        """F-KIT-FIX 2 and 4: the full list, mixed case, plus Wine's old directory names."""
        u = "pfx/drive_c/users/steamuser"
        secret = [
            f"{u}/AppData/LOCAL/ccp/Launcher/Cookies",
            f"{u}/AppData/Local/CCP/EVE/Launcher/cache",
            f"{u}/AppData/Local/CCP/c_ccp_eve_tq_tranquility/prefs.ini",
            f"{u}/.ccp/.eve-online/token",
            f"{u}/.CCP/x",
            f"{u}/AppData/Local/Temp/leftover.tmp",
            f"{u}/AppData/Roaming/eve online/Local Storage/leveldb/1.ldb",
            f"{u}/Application Data/EVE ONLINE/Network/Cookies",
            f"{u}/Local Settings/Application Data/ccp/Launcher/x",
            f"{u}/Local Settings/Temp/x",
            f"pfx/drive_c/CCP/EVE/settings.dat",
            f"pfx/drive_c/CCP/EVE/History.txt",
            f"pfx/drive_c/CCP/EVE/Downloads/x",
            f"pfx/drive_c/CCP/EVE/METADATA/x",
            f"pfx/drive_c/CCP/EVE/c_ccp_eve_tq_tranquility/cache/cef/abc/Network/Cookies",
            f"pfx/drive_c/CCP/EVE/c_ccp_eve_tq_tranquility/cache/cef/abc/Local Storage/x",
            f"pfx/drive_c/CCP/EVE/c_ccp_eve_tq_tranquility/cache/cef/abc/Session Storage/x",
            ".ccp/x",
        ]
        for rel in secret:
            self.write(rel, "invented")
        keep = f"{u}/AppData/Local/CCP/EVE/C_CCP_EVE_TQ-OFFLINE_tranquility/settings/core_user_1.dat"
        self.write(keep, "x")
        r = self.made()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for rel in secret:
            self.assertFalse(os.path.lexists(os.path.join(self.dst, rel)), rel)
        self.assertTrue(os.path.isfile(os.path.join(self.dst, keep)))
        self.assertIn("ready", self.run_script("--check").stdout)

    def test_a_secret_name_that_slips_through_refuses_and_check_fails(self) -> None:
        # Not at any excluded path: under an ordinary directory.
        self.write("pfx/drive_c/Program Files/App/Login Data", "invented")
        r = self.made()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("look like a login session", r.stderr)
        self.assertFalse(os.path.exists(self.dst))
        # And a copy that has one fails --check.
        os.remove(os.path.join(self.src, "pfx/drive_c/Program Files/App/Login Data"))
        self.assertEqual(self.made("--force").returncode, 0)
        for name in ("user_information.json", "Session Storage"):
            p = os.path.join(self.dst, "pfx", "drive_c", name)
            with open(p, "w") as f:
                f.write("x")
            r = self.run_script("--check")
            self.assertNotEqual(r.returncode, 0, name)
            self.assertIn("login state", r.stdout)
            os.remove(p)

    def test_a_symlinked_appdata_cannot_hide_a_session(self) -> None:
        users = os.path.join(self.src, "pfx/drive_c/users/steamuser")
        self.write("pfx/drive_c/users/steamuser/Real/Roaming/EVE Online/Cookies", "invented")
        shutil.rmtree(os.path.join(users, "AppData"))
        os.symlink("Real", os.path.join(users, "AppData"))
        r = self.made()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("look like a login session", r.stderr)

    def test_an_existing_copy_is_not_replaced_without_force(self) -> None:
        self.assertEqual(self.made().returncode, 0)
        planted = os.path.join(self.dst, "pfx", "drive_c", "planted.dll")
        with open(planted, "w") as f:
            f.write("x")
        r = self.made()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("already exists", r.stderr)
        self.assertTrue(os.path.exists(planted))
        r = self.made("--force")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertFalse(os.path.exists(planted))

    def test_force_will_not_remove_a_directory_it_did_not_make(self) -> None:
        os.makedirs(os.path.join(self.dst, "mine"))
        r = self.made("--force")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not made by this script", r.stderr)
        self.assertTrue(os.path.isdir(os.path.join(self.dst, "mine")))

    def test_link_resfiles_shares_inodes_and_nothing_else(self) -> None:
        r = self.made("--link-resfiles")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        a = os.stat(os.path.join(self.src, EVE, "ResFiles", "ab", "ab12_cafe"))
        b = os.stat(os.path.join(self.dst, EVE, "ResFiles", "ab", "ab12_cafe"))
        self.assertEqual((a.st_dev, a.st_ino), (b.st_dev, b.st_ino))
        a = os.stat(os.path.join(self.src, EVE, "tq-offline", "start.ini"))
        b = os.stat(os.path.join(self.dst, EVE, "tq-offline", "start.ini"))
        self.assertNotEqual(a.st_ino, b.st_ino)
        self.assertIn("hard-linked", self.run_script("--check").stdout)

    def test_a_symlink_back_into_the_online_prefix_refuses(self) -> None:
        os.symlink(os.path.join(self.src, "pfx", "system.reg"),
                   os.path.join(self.src, "pfx", "drive_c", "back"))
        r = self.made()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("symlinks into the online prefix", r.stderr)
        self.assertFalse(os.path.exists(self.dst))

    def test_a_destination_inside_the_online_prefix_refuses(self) -> None:
        r = self.made(EVE_OFFLINE_PREFIX=os.path.join(self.src, "copy"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("overlap", r.stderr)
        r = self.made(EVE_OFFLINE_PREFIX=os.path.join(self.tmp, "lib", "steamapps", "compatdata", "8500"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("inside an online prefix", r.stderr)

    def test_no_client_copy_refuses(self) -> None:
        shutil.rmtree(os.path.join(self.src, EVE, "tq-offline"))
        r = self.made()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("no client copy", r.stderr)
        self.assertFalse(os.path.exists(self.dst))

    def test_check_on_nothing_fails(self) -> None:
        r = self.run_script("--check")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("no prefix here", r.stdout)


def pinned_build() -> str:
    with open(os.path.join(HERE, "builds.tsv")) as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            if len(p) == 5 and p[2] == "pinned":
                return p[0]
    raise AssertionError("no pinned build")


@unittest.skipIf(os.geteuid() == 0, "friend-setup.sh refuses root, by design")
class FriendSetup(unittest.TestCase):
    """tools/friend-setup.sh, on an invented install (issue #140)."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-friend-setup-")
        self.home = os.path.join(self.tmp, "home")
        self.steam = os.path.join(self.home, ".local", "share", "Steam")
        self.pfx = os.path.join(self.steam, "steamapps", "compatdata", "8500")
        self.eve = os.path.join(self.pfx, EVE)
        self.off = os.path.join(self.home, ".local", "share", "eve-offline", "prefix", EVE)
        self.proc = os.path.join(self.tmp, "proc")
        os.makedirs(self.proc)
        self.pin = pinned_build()
        self.path = os.environ.get("PATH", "/usr/bin:/bin")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def put(self, rel: str, text: str) -> None:
        p = os.path.join(self.eve, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def good_install(self, build: str | None = None) -> None:
        """A complete install: both indices, every file at its size."""
        self.put("tq/start.ini", "[main]\nbuild = %s\n" % (build or self.pin))
        self.put("tq/bin64/exefile.exe", "MZ")
        row = "res:/a,ab/ab12_cafe,x,8,4\n"
        self.put("tq/resfileindex.txt", row)
        self.put("tq/resfileindex_Windows.txt", row)
        self.put("ResFiles/ab/ab12_cafe", "12345678")

    def run_step(self, *args: str, path: str | None = None, **extra: str) -> subprocess.CompletedProcess[str]:
        env = {"PATH": path or self.path, "HOME": self.home, "EVE_PROC_DIR": self.proc, **extra}
        return subprocess.run(["bash", os.path.join(HERE, "friend-setup.sh"), *args],
                              capture_output=True, text=True, env=env, timeout=120)

    def check(self) -> subprocess.CompletedProcess[str]:
        return self.run_step("check")

    def test_a_moved_build_says_you_cannot_play_on_it(self) -> None:
        self.good_install("100")
        r = self.check()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("You cannot play on this copy", r.stdout)

    def test_a_moved_build_with_a_good_tq_offline_is_not_a_failure(self) -> None:
        self.good_install("100")
        self.put("tq-offline/start.ini", "[main]\nbuild = %s\n" % self.pin)
        r = self.check()
        self.assertNotIn("You cannot play on this copy", r.stdout)
        self.assertIn("play on that one", r.stdout)

    def test_a_stale_tq_offline_gets_a_way_forward(self) -> None:
        self.good_install()
        self.put("tq-offline/start.ini", "[main]\nbuild = 100\n")
        r = self.check()
        self.assertIn("not the pinned", r.stdout)
        self.assertIn("rm -rf", r.stdout)
        self.assertIn("offline-prefix.sh --force", r.stdout)

    def test_missing_resfiles_are_counted(self) -> None:
        self.good_install()
        self.put("tq/resfileindex.txt", "res:/a,ab/ab12_cafe,x,8,4\nres:/b,cd/cd34_beef,x,1,1\n")
        r = self.check()
        self.assertIn("ResFiles incomplete: checked", r.stdout)
        self.assertIn("missing 1", r.stdout)

    def test_a_wrong_size_and_an_empty_or_absent_index_fail(self) -> None:
        self.good_install()
        self.put("ResFiles/ab/ab12_cafe", "123")
        self.assertIn("wrong-size 2", self.check().stdout)
        self.good_install()
        self.put("tq/resfileindex_Windows.txt", "")
        r = self.check()
        self.assertIn("ResFiles incomplete", r.stdout)
        self.assertIn("has no rows", r.stdout)
        os.remove(os.path.join(self.eve, "tq", "resfileindex_Windows.txt"))
        self.assertIn("ResFiles incomplete", self.check().stdout)

    def test_a_complete_install_is_ok(self) -> None:
        self.good_install()
        self.assertIn("ok    ResFiles complete", self.check().stdout)

    def test_no_install_names_steam_root(self) -> None:
        r = self.check()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("STEAM_ROOT", r.stdout)

    def test_copy_makes_both_and_a_second_run_changes_nothing(self) -> None:
        self.good_install()
        r = self.run_step("copy")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.eve, "tq-offline", "bin64", "exefile.exe")))
        self.assertTrue(os.path.isfile(os.path.join(self.off, "tq-offline", "bin64", "exefile.exe")))
        r = self.run_step("copy")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("not copying it again", r.stdout)

    def test_a_leftover_half_copy_is_removed_not_nested(self) -> None:
        self.good_install()
        self.put("tq-offline.partial/junk", "half")
        r = self.run_step("copy")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline", "tq")))
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline", "junk")))
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline.partial")))

    def test_a_copy_that_fails_leaves_no_partial_and_no_tq_offline(self) -> None:
        self.good_install()
        stub = os.path.join(self.tmp, "stub")
        os.makedirs(stub)
        with open(os.path.join(stub, "cp"), "w") as f:
            f.write('#!/bin/sh\nfor a; do last=$a; done\nmkdir -p "$last"\necho half > "$last/x"\nexit 1\n')
        os.chmod(os.path.join(stub, "cp"), 0o755)
        r = self.run_step("copy", path=stub + ":" + self.path)
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline.partial")))
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline")))
        # And the next run is clean.
        self.assertEqual(self.run_step("copy").returncode, 0)

    def test_copy_refuses_a_moved_build_an_incomplete_install_and_a_busy_prefix(self) -> None:
        self.good_install("100")
        r = self.run_step("copy")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not the pinned", r.stderr)
        self.good_install()
        self.put("ResFiles/ab/ab12_cafe", "1")
        r = self.run_step("copy")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("ResFiles incomplete", r.stdout)
        self.good_install()
        d = os.path.join(self.proc, "77")
        os.makedirs(d)
        with open(os.path.join(d, "environ"), "wb") as f:
            f.write(b"WINEPREFIX=" + os.path.join(self.pfx, "pfx").encode() + b"\0")
        with open(os.path.join(d, "comm"), "w") as f:
            f.write("wineserver\n")
        r = self.run_step("copy")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("Wine is running", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.eve, "tq-offline")))

    def test_a_stale_tq_offline_stops_copy(self) -> None:
        self.good_install()
        self.put("tq-offline/bin64/exefile.exe", "MZ")
        self.put("tq-offline/start.ini", "[main]\nbuild = 100\n")
        r = self.run_step("copy")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("rm -rf", r.stdout)

    def test_copy_rerun_with_a_broken_own_prefix_explains_and_changes_nothing(self) -> None:
        self.good_install()
        self.assertEqual(self.run_step("copy").returncode, 0)
        os.remove(os.path.join(self.off, "tq-offline", "bin64", "exefile.exe"))
        r = self.run_step("copy")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("offline-prefix.sh --force", r.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.off, "tq-offline", "bin64", "exefile.exe")))
        self.assertFalse(os.path.exists(self.off + ".partial"))

    def test_full_makes_a_real_copy_and_a_half_made_prefix_is_redone(self) -> None:
        self.good_install()
        os.makedirs(os.path.join(self.home, ".local", "share", "eve-offline", "prefix.partial", "junk"))
        r = self.run_step("copy", "--full")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        a = os.stat(os.path.join(self.eve, "ResFiles", "ab", "ab12_cafe"))
        b = os.stat(os.path.join(self.off, "ResFiles", "ab", "ab12_cafe"))
        self.assertNotEqual(a.st_ino, b.st_ino)
        self.assertFalse(os.path.exists(self.off.replace("/pfx/drive_c/CCP/EVE", "") + ".partial"))

    def test_patch_fails_when_the_pin_check_fails(self) -> None:
        os.makedirs(os.path.join(self.off, "tq-offline"))
        with open(os.path.join(self.off, "tq-offline", "start.ini"), "w") as f:
            f.write("[main]\nbuild = %s\n" % self.pin)
        r = self.run_step("patch")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("does not match the pinned build", r.stderr)
        self.assertNotIn("== plan", r.stdout)


class PrefixLib(unittest.TestCase):
    def lib(self, code: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(["bash", "-c", 'source "$1"; ' + code, "_", LIB],
                              capture_output=True, text=True, timeout=30)

    def test_the_online_prefix_is_recognised_in_any_library(self) -> None:
        for p in ("/home/alice/.local/share/Steam/steamapps/compatdata/8500",
                  "/home/alice/.local/share/Steam/steamapps/compatdata/8500/",
                  "/home/alice/.local/share/Steam/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE/tq-offline",
                  "/srv/lib/SteamApps/compatdata/8500/pfx"):
            self.assertEqual(self.lib('eve_is_online_path "%s"' % p).returncode, 0, p)
        for p in ("/home/alice/.local/share/eve-offline/prefix",
                  "/home/alice/.local/share/Steam/steamapps/compatdata/8501",
                  "/home/alice/.local/share/Steam/steamapps/compatdata/85000",
                  "/home/alice/notsteamapps/compatdata/8500x"):
            self.assertNotEqual(self.lib('eve_is_online_path "%s"' % p).returncode, 0, p)

    def test_a_symlink_into_it_is_recognised(self) -> None:
        tmp = tempfile.mkdtemp(prefix="test-prefix-lib-")
        try:
            target = os.path.join(tmp, "steamapps", "compatdata", "8500", "pfx")
            os.makedirs(target)
            os.symlink(target, os.path.join(tmp, "innocent"))
            self.assertEqual(self.lib('eve_is_online_path "%s/innocent/x"' % tmp).returncode, 0)
        finally:
            shutil.rmtree(tmp)

    def test_a_busy_prefix_is_found_through_links_cwd_and_the_game_name(self) -> None:
        """F-KIT-FIX 7, on a fake /proc."""
        tmp = tempfile.mkdtemp(prefix="test-busy-")
        try:
            pre = os.path.join(tmp, "lib", "compatdata", "8500")
            os.makedirs(os.path.join(pre, "pfx", "sub"))
            os.symlink(pre, os.path.join(tmp, "alias"))
            proc = os.path.join(tmp, "proc")

            def fake(pid: str, comm: str = "bash", env: str = "", cwd: str | None = None) -> None:
                d = os.path.join(proc, pid)
                os.makedirs(d)
                with open(os.path.join(d, "comm"), "w") as f:
                    f.write(comm + "\n")
                with open(os.path.join(d, "environ"), "wb") as f:
                    f.write(env.encode())
                if cwd:
                    os.symlink(cwd, os.path.join(d, "cwd"))

            def busy() -> tuple[int, str]:
                r = subprocess.run(["bash", "-c", 'source "$1"; eve_prefix_busy "$2"', "_", LIB, pre],
                                   capture_output=True, text=True, timeout=30, env={"EVE_PROC_DIR": proc,
                                   "PATH": os.environ.get("PATH", "/usr/bin:/bin")})
                return r.returncode, r.stdout.strip()

            os.makedirs(proc)
            fake("11", env="HOME=/h\0WINEPREFIX=/elsewhere/pfx\0")
            self.assertEqual(busy()[0], 1)
            fake("12", env="STEAM_COMPAT_DATA_PATH=%s/alias/./\0" % tmp)  # a link, a dot, a slash
            self.assertEqual(busy(), (0, "12"))
            shutil.rmtree(os.path.join(proc, "12"))
            fake("13", env="WINEPREFIX=%s/pfx\0" % os.path.join(tmp, "alias"))
            self.assertEqual(busy(), (0, "13"))
            shutil.rmtree(os.path.join(proc, "13"))
            fake("14", cwd=os.path.join(pre, "pfx", "sub"))
            self.assertEqual(busy(), (0, "14"))
            shutil.rmtree(os.path.join(proc, "14"))
            fake("15", comm="eve-online.exe")
            self.assertEqual(busy(), (0, "15"))
        finally:
            shutil.rmtree(tmp)

    def test_prefix_ids(self) -> None:
        r = self.lib("eve_prefix_of offline /h /s; echo; eve_prefix_of 8501 /h /s; echo")
        self.assertEqual(r.stdout.split(), ["/h/.local/share/eve-offline/prefix",
                                            "/s/steamapps/compatdata/8501"])
        self.assertNotEqual(self.lib("eve_prefix_of ../8500 /h /s").returncode, 0)

    def test_refusal_names_what_and_exits(self) -> None:
        r = self.lib('eve_refuse_online me "EVE_CLIENT=/h/.local/share/eve-offline/prefix/c" '
                     '"EVE_PREFIX=/s/steamapps/compatdata/8500"; echo NOT-REACHED')
        self.assertEqual(r.returncode, 1)
        self.assertNotIn("NOT-REACHED", r.stdout)
        self.assertIn("me: EVE_PREFIX is inside the online EVE prefix", r.stderr)
        r = self.lib('eve_refuse_online me "EVE_PREFIX=/h/.local/share/eve-offline/prefix" "X="; echo OK')
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "OK"))


if __name__ == "__main__":
    unittest.main()
