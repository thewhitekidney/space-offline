#!/usr/bin/env python3
"""Several client-CI runs at once (lane ci-parallel, issue #139).

Nothing here starts a client or a server. It proves, with the real scripts and
stubs for the parts that need a machine:

  - two slots share none of what a run writes (slot-lib.sh, jail-rootless.sh, the
    client-ci.sh preflight), and no slot's client can be mistaken for another's;
  - the prefix a slot gets is a copy that leaves the offline prefix alone;
  - the runner keeps at most --jobs rows going, one per slot, runs the `--serial`
    rows alone afterwards, records rows as they finish, restores the settings
    sweep and stops what it started, however it ends; and --jobs 1 is the old
    runner.
"""
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
CI = os.path.join(HERE, "client-ci")
SLOW = 120


def rd(path):
    with open(path) as f:
        return f.read()


def sh(script, **kw):
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=SLOW, **kw)


def write(path, text, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if mode:
        os.chmod(path, mode)


PGREP_STUB = r"""#!/bin/sh
# A pgrep that finds a "client" only for the patterns listed in $EVE_TEST_LIVE, so the tests do
# not depend on (or trip over) a real client running on this host. It answers with a live pid: its
# caller's parent (the caller is a command substitution's subshell, gone by the time the pid is used).
[ "$1" = -f ] && pat=$2 || pat=$1
[ -f "$EVE_TEST_LIVE" ] || exit 1
live=$(sed 's/^.*) [A-Za-z] \([0-9]*\) .*/\1/' "/proc/$PPID/stat")
while IFS= read -r l; do [ "$pat" = "$l" ] && { echo "$live"; exit 0; }; done < "$EVE_TEST_LIVE"
exit 1
"""


def stub_pgrep(tmp):
    """-> (directory to put first on PATH, file listing the client patterns that are 'running')."""
    d = os.path.join(tmp, "pgrep-bin")
    write(os.path.join(d, "pgrep"), PGREP_STUB, 0o755)
    return d, os.path.join(tmp, "live-clients")


def dump(repo, home, slot, src="tq-offline"):
    r = sh('source "%s/slot-lib.sh"; ci_slot_dump "%s" "%s" "%s" "%s"' % (CI, repo, home, slot, src))
    assert r.returncode == 0, r.stderr
    return dict(line.split("=", 1) for line in r.stdout.splitlines())


class SlotsShareNothing(unittest.TestCase):
    repo, home = "/r/eve", "/h/user"

    def test_no_value_is_shared_between_two_slots(self):
        slots = {n: dump(self.repo, self.home, n) for n in (1, 2, 3, 11, 111)}
        for key in slots[1]:
            vals = [slots[n][key] for n in slots]
            self.assertEqual(len(vals), len(set(vals)), "%s is shared between slots: %s" % (key, vals))

    def test_every_path_is_under_the_slots_own_directory(self):
        for n in (1, 2, 10):
            d = dump(self.repo, self.home, n)
            own = ("/slots/%d/" % n, "/slots/%d" % n)
            for key, val in d.items():
                if key in ("SLOT_ACCOUNT_PORT", "SLOT_CLIENT_DIR", "SLOT_CLIENT_DIR2"):
                    continue
                self.assertTrue(val.endswith("-s%d.tsv" % n) or any(o in val + "/" for o in own),
                                "%s=%s is not under slot %d" % (key, val, n))

    def test_a_slot_never_uses_what_the_lone_run_uses(self):
        alone = {self.repo + "/out/client-ci/rootless", self.repo + "/out/client-ci/launch-env",
                 self.repo + "/out/client-ci/dev-world", self.repo + "/out/client-ci",
                 self.home + "/.config/eve-offline/client-ci-pilot.tsv",
                 self.home + "/.local/share/eve-offline/prefix",
                 self.home + "/.local/share/Steam/steamapps/compatdata/8501", "26093"}
        for n in (1, 2, 3):
            for key, val in dump(self.repo, self.home, n).items():
                self.assertNotIn(val, alone, key)
                self.assertNotIn("compatdata", val, key)

    def test_the_pilot_ports_differ_and_leave_the_lone_run_alone(self):
        ports = {int(dump(self.repo, self.home, n)["SLOT_ACCOUNT_PORT"]) for n in (1, 2, 3)}
        self.assertEqual(len(ports), 3)
        self.assertNotIn(26093, ports)

    def test_no_clients_process_pattern_matches_another(self):
        """client-ci.sh finds and stops a client by `<dir>.bin64.exefile` (pgrep -f)."""
        names = ["tq-offline", "tq-offline-2"]
        for n in (1, 2, 3, 11, 111):
            d = dump(self.repo, self.home, n)
            names += [d["SLOT_CLIENT_DIR"], d["SLOT_CLIENT_DIR2"]]
        self.assertEqual(len(names), len(set(names)))
        for mine in names:
            pat = re.compile(re.escape(mine).replace(r"\.", ".") + ".bin64.exefile")
            for other in names:
                cmd = "C:\\CCP\\EVE\\%s\\bin64\\exefile.exe /ssoToken=x" % other
                self.assertEqual(bool(pat.search(cmd)), mine == other, "%s vs %s" % (mine, other))

    def test_a_bad_slot_is_refused(self):
        for bad in ("0", "-1", "x", "1000", "1/../2", ""):
            r = sh('source "%s/slot-lib.sh"; ci_slot_vars /r /h "%s"' % (CI, bad))
            self.assertNotEqual(r.returncode, 0, bad)


class RootlessJailPaths(unittest.TestCase):
    JAIL = os.path.join(HERE, "jail-rootless.sh")

    def paths(self, slot=None):
        env = dict(os.environ)
        env.pop("EVE_RL_SLOT", None)
        if slot is not None:
            env["EVE_RL_SLOT"] = str(slot)
        return subprocess.run(["bash", self.JAIL, "paths"], capture_output=True, text=True, env=env, timeout=SLOW)

    def test_the_jail_state_follows_the_slot_and_matches_the_lib(self):
        repo = os.path.dirname(HERE)
        seen = set()
        for n in (1, 2, 3):
            r = self.paths(n)
            self.assertEqual(r.returncode, 0, r.stderr)
            got = dict(line.split("=", 1) for line in r.stdout.splitlines())
            want = dump(repo, "/h", n)
            self.assertEqual(got["STATE"], want["SLOT_RL_STATE"])
            self.assertEqual(got["ENV_FILE"], want["SLOT_LAUNCH_ENV"])
            for k in ("STATE", "HOLDER_PID", "INVENTORY", "RESOLV", "ENV_FILE"):
                self.assertNotIn(got[k], seen, k)
                seen.add(got[k])

    def test_without_a_slot_the_paths_are_the_old_ones(self):
        got = dict(line.split("=", 1) for line in self.paths().stdout.splitlines())
        repo = os.path.dirname(HERE)
        self.assertEqual(got["STATE"], repo + "/out/client-ci/rootless")
        self.assertEqual(got["ENV_FILE"], repo + "/out/client-ci/launch-env")

    def test_a_slot_is_a_number_and_nothing_else_comes_in(self):
        for bad in ("../x", "0", "1 2", "/tmp/x", "1000"):
            r = self.paths(bad)
            self.assertEqual(r.returncode, 3, bad)
            self.assertIn("EVE_RL_SLOT", r.stderr)

    def test_the_view_binds_the_slots_launch_env_at_the_path_the_shim_reads(self):
        text = rd(self.JAIL)
        self.assertIn('bind_back ro user file   launch-env "$ENV_FILE" "$CANON_ENV_FILE"', text)
        self.assertIn("CANON_ENV_FILE=$REPO/out/client-ci/launch-env", text)
        shim = rd(os.path.join(HERE, "jail-client.sh"))
        self.assertIn("ENV_FILE=$REPO/out/client-ci/launch-env", shim)

    def test_the_gate_and_the_online_prefix_check_do_not_depend_on_the_slot(self):
        text = rd(self.JAIL)
        gate = text[text.index("v_gate() {"):text.index("# --- a friend's client")]
        for needle in ("SLOT", "STATE", "EVE_RL_SLOT"):
            self.assertNotIn(needle, gate)
        self.assertIn('online_overlap "$src"', text)


# ---------------------------------------------------------------- the slot prefix

def fake_offline_prefix(home):
    """A small stand-in for ~/.local/share/eve-offline/prefix, invented content."""
    pfx = os.path.join(home, ".local", "share", "eve-offline", "prefix")
    eve = os.path.join(pfx, "pfx", "drive_c", "CCP", "EVE")
    write(os.path.join(pfx, "pfx", "user.reg"), "WINE REGISTRY\n")
    write(os.path.join(eve, "ResFiles", "ab", "abcdef"), "resource\n")
    write(os.path.join(eve, "tq-offline", "bin64", "exefile.exe"), "exe v1\n")
    write(os.path.join(eve, "tq-offline", "start.ini"), "build = 1\n")
    write(os.path.join(eve, "tq-other", "bin64", "exefile.exe"), "another install\n")
    users = os.path.join(pfx, "pfx", "drive_c", "users", "steamuser", "AppData", "Local", "CCP", "EVE")
    write(os.path.join(users, "c_ccp_eve_tq-offline_tranquility", "settings", "prefs.ini"), "chatserver_disable_tls=1\r\n")
    write(os.path.join(users, "c_ccp_eve_tq-offline_tranquility", "settings", "core_public__.yaml"), "owner: [1, 1]\n")
    write(os.path.join(users, "c_ccp_eve_tq_tranquility", "cache", "big"), "another install's cache\n")
    write(os.path.join(users, "downloads", "patch.bin"), "launcher download\n")
    write(os.path.join(users, "c_program_files_x_shared_cache", "cache", "big"), "shared cache of another install\n")
    write(os.path.join(users, "reports", "r1"), "kept\n")
    write(os.path.join(pfx, "pfx", "drive_c", "eve-logs", "client-old.log"), "old\n")
    return pfx


def tree(root):
    out = {}
    for d, dirs, files in os.walk(root):
        for f in files + dirs:
            p = os.path.join(d, f)
            st = os.lstat(p)
            out[os.path.relpath(p, root)] = (st.st_mode, st.st_size, st.st_mtime_ns)
    return out


class SlotPrefix(unittest.TestCase):
    SCRIPT = os.path.join(CI, "slot-prefix.sh")

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-slotprefix-")
        self.pgrep_dir, self.live = stub_pgrep(self.tmp)
        self.home = os.path.join(self.tmp, "home")
        self.src = fake_offline_prefix(self.home)
        self.lock = os.path.join(self.tmp, "runner.lock")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_s(self, *args, src=None, env=None):
        base = {k: v for k, v in os.environ.items() if k not in ("EVE_CLIENT_DIR", "EVE_CI_RUNNER_LOCK_HELD")}
        env = dict(base, EVE_SLOT_HOME=self.home, EVE_SLOT_SRC_PREFIX=src or self.src,
                   EVE_CI_RUNNER_LOCK=self.lock, EVE_TEST_LIVE=self.live, **(env or {}))
        env["PATH"] = self.pgrep_dir + ":" + env.get("PATH", os.environ["PATH"])
        return subprocess.run(["bash", self.SCRIPT, *args], capture_output=True, text=True, env=env, timeout=SLOW)

    def slot_eve(self, n, second=False):
        return os.path.join(self.home, ".local", "share", "eve-offline", "slots", str(n),
                            "prefix2" if second else "prefix", "pfx", "drive_c", "CCP", "EVE")

    def test_ensure_copies_the_prefix_renames_the_client_and_links_resfiles(self):
        r = self.run_s("ensure", "1")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        eve = self.slot_eve(1)
        self.assertEqual(rd(os.path.join(eve, "tq-offline-s1", "bin64", "exefile.exe")), "exe v1\n")
        self.assertFalse(os.path.exists(os.path.join(eve, "tq-offline")), "the unrenamed client is in the slot")
        self.assertFalse(os.path.exists(os.path.join(eve, "tq-other")), "another install was copied")
        res = os.path.join(eve, "ResFiles")
        self.assertTrue(os.path.islink(res))
        self.assertEqual(os.readlink(res), os.path.join(self.src, "pfx", "drive_c", "CCP", "EVE", "ResFiles"))
        slot = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(eve))))
        users = os.path.join(slot, "pfx", "drive_c", "users", "steamuser", "AppData", "Local", "CCP", "EVE")
        self.assertEqual(sorted(os.listdir(users)), ["c_ccp_eve_tq-offline-s1_tranquility", "reports"])
        self.assertTrue(os.path.exists(os.path.join(users, "c_ccp_eve_tq-offline-s1_tranquility", "settings", "prefs.ini")))
        self.assertEqual(os.listdir(os.path.join(slot, "pfx", "drive_c", "eve-logs")), [], "old logs came along")
        self.assertEqual(rd(os.path.join(slot, "pfx", "user.reg")), "WINE REGISTRY\n")
        self.assertEqual(self.run_s("check", "1").returncode, 0)

    def test_the_launcher_downloads_and_shared_caches_stay_behind_but_reports_come(self):
        self.assertEqual(self.run_s("ensure", "1").returncode, 0)
        users = os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix/pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE")
        self.assertEqual(sorted(os.listdir(users)), ["c_ccp_eve_tq-offline-s1_tranquility", "reports"])

    def test_a_copy_that_dies_half_way_is_never_taken_as_done(self):
        bin_ = os.path.join(self.tmp, "bin")
        write(os.path.join(bin_, "tar"), "#!/bin/sh\nexit 1\n", 0o755)
        slots = os.path.join(self.home, ".local/share/eve-offline/slots/1")
        r = self.run_s("ensure", "1", env={"PATH": bin_ + ":" + os.environ["PATH"]})   # a tar that fails
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(slots, "prefix")), "a failed copy left a prefix")
        self.assertFalse(os.path.exists(os.path.join(slots, "prefix.tmp")))
        # a copy killed outright leaves prefix.tmp: the next ensure starts over and ends whole
        write(os.path.join(slots, "prefix.tmp", "pfx", "half"), "partial\n")
        self.assertEqual(self.run_s("ensure", "1").returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(slots, "prefix.tmp")))
        self.assertFalse(os.path.exists(os.path.join(slots, "prefix", "pfx", "half")))
        self.assertEqual(self.run_s("check", "1").returncode, 0)

    def test_ensure_refuses_while_a_client_runs_from_the_source(self):
        write(self.live, "tq-offline.bin64.exefile\n")
        r = self.run_s("ensure", "1")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("a client runs from tq-offline", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix")))
        # another slot's client (tq-offline-s1) is not the source's
        write(self.live, "tq-offline-s1.bin64.exefile\n")
        self.assertEqual(self.run_s("ensure", "2").returncode, 0)

    def test_ensure_and_remove_refuse_while_a_runner_holds_the_slots(self):
        import fcntl
        self.assertEqual(self.run_s("ensure", "1").returncode, 0)
        with open(self.lock, "w") as f:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for args in (("ensure", "1"), ("remove", "1"), ("remove", "all")):
                r = self.run_s(*args)
                self.assertNotEqual(r.returncode, 0, args)
                self.assertIn("holds", r.stderr)
            self.assertTrue(os.path.isdir(os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix")))
            # the runner's own calls say it holds the lock
            self.assertEqual(self.run_s("ensure", "1", env={"EVE_CI_RUNNER_LOCK_HELD": "1"}).returncode, 0)
        self.assertEqual(self.run_s("remove", "1").returncode, 0)

    def test_remove_all_takes_every_slot(self):
        for n in ("1", "2"):
            self.run_s("ensure", n)
        r = self.run_s("remove", "all")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(os.listdir(os.path.join(self.home, ".local/share/eve-offline/slots")), [])
        self.assertTrue(os.path.exists(os.path.join(self.src, "pfx/drive_c/CCP/EVE/ResFiles/ab/abcdef")))

    def test_the_offline_prefix_is_not_touched(self):
        before = tree(self.src)
        for args in (("ensure", "1"), ("ensure", "2"), ("ensure", "1", "--second"), ("ensure", "1"), ("check", "1")):
            self.assertEqual(self.run_s(*args).returncode, 0, args)
        self.assertEqual(tree(self.src), before)

    def test_two_slots_share_no_file_with_each_other_or_the_offline_prefix(self):
        self.run_s("ensure", "1")
        self.run_s("ensure", "2")
        inodes = {}
        for n in (1, 2):
            base = os.path.join(self.home, ".local", "share", "eve-offline", "slots", str(n))
            for d, _dirs, files in os.walk(base):
                for f in files:
                    p = os.path.join(d, f)
                    if os.path.islink(p):
                        continue
                    st = os.stat(p)
                    key = (st.st_dev, st.st_ino)
                    self.assertNotIn(key, inodes, "%s shares an inode with %s" % (p, inodes.get(key)))
                    inodes[key] = p
        for d, _dirs, files in os.walk(self.src):
            for f in files:
                p = os.path.join(d, f)
                if "ResFiles" in p:
                    continue
                st = os.stat(p)
                self.assertNotIn((st.st_dev, st.st_ino), inodes, p)

    def test_forcing_one_slots_settings_leaves_the_others_and_the_owners_alone(self):
        qs = os.path.join(CI, "quiet-settings.py")
        self.run_s("ensure", "1")
        self.run_s("ensure", "2")
        owner_dc = os.path.join(self.src, "pfx", "drive_c")
        s1 = os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix/pfx/drive_c")
        s2 = os.path.join(self.home, ".local/share/eve-offline/slots/2/prefix/pfx/drive_c")
        subprocess.run([sys.executable, "-I", qs, "apply", s1, "tq-offline-s1"], check=True, capture_output=True, timeout=SLOW)

        def settings(dc, client):
            base = os.path.join(dc, "users", "steamuser", "AppData", "Local", "CCP", "EVE", "c_ccp_eve_%s_tranquility" % client, "settings")
            return rd(os.path.join(base, "core_public__.yaml"))

        self.assertIn("audioEnabled", settings(s1, "tq-offline-s1"))
        self.assertEqual(settings(s2, "tq-offline-s2"), "owner: [1, 1]\n")
        self.assertEqual(settings(owner_dc, "tq-offline"), "owner: [1, 1]\n")
        # the run died: the next apply, or the runner's sweep, puts the kept file back
        subprocess.run([sys.executable, "-I", qs, "restore", s1, "tq-offline-s1"], check=True, capture_output=True, timeout=SLOW)
        self.assertEqual(settings(s1, "tq-offline-s1"), "owner: [1, 1]\n")

    def test_the_second_client_has_a_prefix_and_a_name_of_its_own(self):
        self.assertEqual(self.run_s("ensure", "1").returncode, 0)
        self.assertEqual(self.run_s("ensure", "1", "--second").returncode, 0)
        self.assertTrue(os.path.exists(os.path.join(self.slot_eve(1, True), "tq-offline-s1-2", "bin64", "exefile.exe")))
        self.assertFalse(os.path.exists(os.path.join(self.slot_eve(1, True), "tq-offline-s1")))
        self.assertEqual(self.run_s("check", "1", "--second").returncode, 0)

    def test_ensure_again_brings_in_the_current_build_and_keeps_the_slots_settings(self):
        self.run_s("ensure", "1")
        write(os.path.join(self.src, "pfx/drive_c/CCP/EVE/tq-offline/bin64/exefile.exe"), "exe v2\n")
        users = os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix/pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE")
        mine = os.path.join(users, "c_ccp_eve_tq-offline-s1_tranquility", "settings", "slot-only")
        write(mine, "kept\n")
        self.assertEqual(self.run_s("ensure", "1").returncode, 0)
        self.assertEqual(rd(os.path.join(self.slot_eve(1), "tq-offline-s1", "bin64", "exefile.exe")), "exe v2\n")
        self.assertEqual(rd(mine), "kept\n")

    def test_another_client_directory(self):
        write(os.path.join(self.src, "pfx/drive_c/CCP/EVE/tq-9/bin64/exefile.exe"), "x\n")
        write(os.path.join(self.src, "pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE/c_ccp_eve_tq-9_tranquility/settings/prefs.ini"), "p\n")
        self.assertEqual(self.run_s("ensure", "3", "--client", "tq-9").returncode, 0)
        self.assertTrue(os.path.exists(os.path.join(self.slot_eve(3), "tq-9-s3", "bin64", "exefile.exe")))

    def test_the_online_prefix_is_refused_as_source_and_slot_numbers_are_checked(self):
        online = os.path.join(self.tmp, "Steam", "steamapps", "compatdata", "8500")
        os.makedirs(os.path.join(online, "pfx"))
        r = self.run_s("ensure", "1", src=online)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("online EVE prefix", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.home, ".local/share/eve-offline/slots")))
        for bad in ("0", "x", "1000", "../1"):
            self.assertNotEqual(self.run_s("ensure", bad).returncode, 0, bad)

    def test_remove_deletes_the_slot_but_never_follows_the_resfiles_link(self):
        self.run_s("ensure", "1")
        self.assertEqual(self.run_s("remove", "1").returncode, 0)
        self.assertFalse(os.path.exists(os.path.join(self.home, ".local/share/eve-offline/slots/1")))
        self.assertTrue(os.path.exists(os.path.join(self.src, "pfx/drive_c/CCP/EVE/ResFiles/ab/abcdef")))


# ---------------------------------------------------------------- client-ci.sh, preflight

class ClientCiSlotPreflight(unittest.TestCase):
    """`client-ci.sh run X --check --slot N`: the preflight only, in a throwaway repo
    with stub tools (the pattern of test_jail.ClientCiCensusCheck)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-ci-slot-")
        self.home = os.path.join(self.tmp, "home")
        repo = self.repo = os.path.join(self.tmp, "repo")
        tools = os.path.join(repo, "tools")
        os.makedirs(os.path.join(tools, "client-ci"))
        for name in ("client-ci.sh", "prefix-lib.sh", "content-lib.sh"):
            shutil.copy(os.path.join(HERE, name), tools)
        shutil.copy(os.path.join(CI, "slot-lib.sh"), os.path.join(tools, "client-ci"))
        for name, body in (("install-jail-helper.sh", "exit 4"), ("jail-rootless.sh", "exit 0")):
            write(os.path.join(tools, name), "#!/bin/sh\n%s\n" % body, 0o755)
        write(os.path.join(repo, "out", "content", "MANIFEST.tsv"), "x\n")
        self.scen = os.path.join(self.tmp, "scenarios")
        write(os.path.join(self.scen, "slot_x.py"), "# invented\n")
        self.bin = os.path.join(self.tmp, "bin")
        write(os.path.join(self.bin, "sudo"), "#!/bin/sh\nexit 1\n", 0o755)
        # This preflight must not see a real client: a wave may be running.
        write(os.path.join(self.bin, "pgrep"), "#!/bin/sh\nexit 1\n", 0o755)
        for n in (1, 2):
            d = os.path.join(self.home, ".local/share/eve-offline/slots", str(n), "prefix", "pfx", "drive_c")
            write(os.path.join(d, "CCP/EVE/tq-offline-s%d/bin64/exefile.exe" % n), "x\n")
            os.makedirs(os.path.join(d, "eve-logs"))
        d = os.path.join(self.home, ".local/share/eve-offline/slots/1/prefix2/pfx/drive_c")
        write(os.path.join(d, "CCP/EVE/tq-offline-s1-2/bin64/exefile.exe"), "x\n")
        os.makedirs(os.path.join(d, "eve-logs"))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def check(self, *extra, env=None):
        e = {"PATH": self.bin + ":/usr/bin:/bin", "HOME": self.home, "DISPLAY": ":987654",
             "EVE_CI_SCENARIOS": self.scen}
        e.update(env or {})
        return subprocess.run(["bash", os.path.join(self.repo, "tools", "client-ci.sh"), "run", "slot_x",
                               "--check", "--inherit-display", "--no-scenario-flag", *extra],
                              capture_output=True, text=True, env=e, timeout=SLOW)

    def test_a_slot_checks_its_own_prefix_and_client(self):
        out = {}
        for n in (1, 2):
            r = self.check("--rootless", "--slot", str(n))
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            out[n] = r.stdout
            self.assertIn("prefix:    %s/.local/share/eve-offline/slots/%d/prefix" % (self.home, n), r.stdout)
            self.assertIn("slot:      %d (own prefix, client tq-offline-s%d," % (n, n), r.stdout)
            self.assertIn("out/client-ci/slots/%d/rootless" % n, r.stdout)
            self.assertIn("out/client-ci/slots/%d/runs" % n, r.stdout)
        self.assertNotEqual(out[1], out[2])

    def test_the_slot_may_come_from_the_environment(self):
        r = self.check("--rootless", env={"EVE_CI_SLOT": "2"})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("slots/2/prefix", r.stdout)

    def test_a_slot_without_a_prefix_is_told_how_to_make_one(self):
        r = self.check("--rootless", "--slot", "3")
        self.assertEqual(r.returncode, 3)
        self.assertIn("tools/client-ci/slot-prefix.sh ensure 3", r.stderr)

    def test_a_slot_needs_the_rootless_jail_and_refuses_the_census(self):
        r = self.check("--slot", "1")
        self.assertEqual(r.returncode, 3)
        self.assertIn("needs --rootless", r.stderr)
        r = self.check("--rootless", "--slot", "1", "--census", os.path.join(self.tmp, "run"))
        self.assertEqual(r.returncode, 3)
        self.assertIn("--slot and --census", r.stderr)
        r = self.check("--rootless", "--slot", "1", env={"EVE_PREFIX": os.path.join(self.tmp, "p")})
        self.assertEqual(r.returncode, 3)
        self.assertIn("makes its own prefix", r.stderr)
        r = self.check("--rootless", "--slot", "zero")
        self.assertEqual(r.returncode, 3)
        self.assertIn("number from 1 to 999", r.stderr)

    def test_a_set_eve_client_is_refused_for_a_slot_as_eve_prefix_is(self):
        r = self.check("--rootless", "--slot", "1", env={"EVE_CLIENT": os.path.join(self.tmp, "c")})
        self.assertEqual(r.returncode, 3)
        self.assertIn("names its own client; EVE_CLIENT is set", r.stderr)

    def test_a_lone_run_does_not_inherit_a_slot_for_its_jail_calls(self):
        text = rd(os.path.join(HERE, "client-ci.sh"))
        self.assertIn("[[ $SLOT != 0 ]] || unset EVE_RL_SLOT", text)
        self.assertLess(text.index("[[ $SLOT != 0 ]] || unset EVE_RL_SLOT"), text.index('jail_check=(timeout 120 "$REPO/tools/jail-rootless.sh" check)'))

    def test_a_slots_second_client_is_the_slots_own(self):
        r = self.check("--rootless", "--slot", "1", "--with-second")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("slots/1/prefix2 (slot 1, tq-offline-s1-2)", r.stdout)
        r = self.check("--rootless", "--slot", "2", "--with-second")
        self.assertEqual(r.returncode, 3)
        self.assertIn("slot-prefix.sh ensure 2 --second", r.stderr)

    def test_the_jail_check_runs_for_a_slot_and_its_refusal_refuses_the_run(self):
        text = rd(os.path.join(HERE, "client-ci.sh"))
        self.assertIn('jail_check=(timeout 120 "$REPO/tools/jail-rootless.sh" check)', text)
        self.assertIn("export EVE_RL_SLOT=$SLOT", text)
        i = text.index('if ! "${jail_check[@]}"')
        self.assertIn("exit $EX_REFUSED", text[i:i + 400])
        # the slot's jail is the one checked, and the check comes before any deploy
        self.assertLess(text.index("export EVE_RL_SLOT=$SLOT"), text.index('if ! "${jail_check[@]}"'))
        self.assertLess(text.index('if ! "${jail_check[@]}"'), text.index('say "deploying $NAME"'))

    def test_a_lone_run_keeps_its_paths(self):
        write(os.path.join(self.tmp, "p", "pfx", "drive_c", "eve-logs", "x"), "")
        r = self.check("--rootless", env={"EVE_PREFIX": os.path.join(self.tmp, "p")})
        if r.returncode != 0 and "a client from tq-offline is already running" in r.stdout + r.stderr:
            self.skipTest("a real client is running on this host")   # a slot's checks above ignore it
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotIn("slot:", r.stdout)


# ---------------------------------------------------------------- the runner

STUB_CLIENT_CI = r'''#!/usr/bin/env bash
# A stand-in for client-ci.sh: records when it started and stopped, and what slot it had.
log=$EVE_TEST_LOG
name=$2
trap 'echo "TERM $name" >> "$log"; exit 143' TERM
echo "START $name slot=${EVE_CI_SLOT:-none} args=$*" >> "$log"
sleep "${EVE_TEST_SLEEP:-0.5}" & wait $!
rc=0
[[ " ${EVE_TEST_FAIL:-} " == *" $name "* ]] && rc=1
echo "END $name" >> "$log"
exit $rc
'''

LIST = """# a list
row_a
row_b -- --dev
row_c --with-second -- --character Alice --character Bob
row_d
row_e
row_f
row_s1 --serial
row_s2 --serial -- --dev
"""


class Runner(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-regress-")
        self.home = os.path.join(self.tmp, "home")
        os.makedirs(self.home)
        repo = self.repo = os.path.join(self.tmp, "repo")
        tools = os.path.join(repo, "tools")
        os.makedirs(os.path.join(tools, "client-ci"))
        for name in ("client-ci-regress.sh", "prefix-lib.sh"):
            shutil.copy(os.path.join(HERE, name), tools)
        shutil.copy(os.path.join(CI, "slot-lib.sh"), os.path.join(tools, "client-ci"))
        self.log = os.path.join(self.tmp, "events.log")
        open(self.log, "w").close()
        stubs = os.path.join(self.tmp, "stubs")
        write(os.path.join(stubs, "client-ci.sh"), STUB_CLIENT_CI, 0o755)
        write(os.path.join(stubs, "slot-prefix.sh"),
              '#!/usr/bin/env bash\necho "PREFIX $*" >> "$EVE_TEST_LOG"\n', 0o755)
        write(os.path.join(stubs, "headless.sh"),
              '#!/usr/bin/env bash\necho "DISPLAY $*" >> "$EVE_TEST_LOG"\n'
              '[[ $1 != status ]] || [[ -n ${EVE_TEST_DISPLAY_UP:-} ]]\n', 0o755)
        write(os.path.join(stubs, "quiet.py"),
              'import os, sys\nopen(os.environ["EVE_TEST_LOG"], "a").write("SWEEP %s %s\\n" % (sys.argv[2], sys.argv[3]))\n')
        self.scen = os.path.join(self.tmp, "scenarios")
        for n in ("row_a", "row_b", "row_c", "row_d", "row_e", "row_f", "row_s1", "row_s2"):
            write(os.path.join(self.scen, n + ".py"), "# invented\n")
        self.list = os.path.join(self.tmp, "regress.list")
        write(self.list, LIST)
        self.out = os.path.join(self.tmp, "out")
        self.env = dict(os.environ, HOME=self.home, EVE_TEST_LOG=self.log,
                        EVE_CI_CLIENT_CI=os.path.join(stubs, "client-ci.sh"),
                        EVE_CI_SLOT_PREFIX=os.path.join(stubs, "slot-prefix.sh"),
                        EVE_CI_HEADLESS=os.path.join(stubs, "headless.sh"),
                        EVE_CI_QUIET=os.path.join(stubs, "quiet.py"),
                        EVE_CI_SCENARIOS=self.scen,
                        EVE_CI_RUNNER_LOCK=os.path.join(self.tmp, "runner.lock"))
        pgrep_dir, self.live = stub_pgrep(self.tmp)
        self.env["PATH"] = pgrep_dir + ":" + os.environ["PATH"]
        self.env["EVE_TEST_LIVE"] = self.live
        self.env.pop("EVE_CI_JOBS", None)
        self.env.pop("EVE_CLIENT_DIR", None)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def start(self, *args, env=None, build=False):
        e = dict(self.env, **(env or {}))
        return subprocess.Popen(["bash", os.path.join(self.repo, "tools", "client-ci-regress.sh"),
                                 "--list", self.list, "--out", self.out, *([] if build else ["--no-build"]), *args],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=e)

    def run_r(self, *args, env=None):
        p = self.start(*args, env=env)
        out, _ = p.communicate(timeout=SLOW)
        return p.returncode, out

    def events(self):
        return [l.split() for l in rd(self.log).splitlines()]

    def tsv(self):
        return [l.split("\t") for l in rd(os.path.join(self.out, "regress.tsv")).splitlines()]

    # -- the schedule

    def intervals(self):
        """name -> (index of its START event, index of its END event, slot)."""
        ev = self.events()
        res = {}
        for i, e in enumerate(ev):
            if e[0] == "START":
                res[e[1]] = [i, None, e[2].split("=")[1]]
            elif e[0] == "END":
                res[e[1]][1] = i
        return res

    def clients_now(self, jobs, par):
        """Replay the event log: the clients running (a --with-second row is 2) after each event."""
        cur = peak = 0
        cost = {}
        for e in self.events():
            if e[0] == "START" and e[1] in par:
                cost[e[1]] = 2 if "--with-second" in e else 1
                cur += cost[e[1]]
            elif e[0] == "END" and e[1] in par:
                cur -= cost[e[1]]
            self.assertLessEqual(cur, jobs, "over the budget after %s" % e)
            peak = max(peak, cur)
        return peak

    def test_three_clients_at_a_time_a_pair_costs_two_and_the_serial_rows_run_alone_at_the_end(self):
        rc, out = self.run_r("--jobs", "3")
        self.assertEqual(rc, 0, out)
        iv = self.intervals()
        self.assertEqual(sorted(iv), sorted(["row_a", "row_b", "row_c", "row_d", "row_e", "row_f", "row_s1", "row_s2"]))
        par = ["row_a", "row_b", "row_c", "row_d", "row_e", "row_f"]
        self.assertEqual(self.clients_now(3, par), 3)
        # row_c (--with-second) never starts with more than one other client running
        ev = self.events()
        run = set()
        for e in ev:
            if e[0] == "START" and e[1] in par:
                if e[1] == "row_c":
                    self.assertLessEqual(len(run), 1, "the pair started with %s running" % run)
                run.add(e[1])
            elif e[0] == "END":
                run.discard(e[1])
        # no two overlapping rows share a slot, and every parallel row has one of 1..3
        for a in par:
            self.assertIn(iv[a][2], ("1", "2", "3"), a)
            for b in par:
                if a < b and iv[a][2] == iv[b][2]:
                    self.assertTrue(iv[a][1] < iv[b][0] or iv[b][1] < iv[a][0], "%s and %s overlap in slot %s" % (a, b, iv[a][2]))
        # the serial rows: no slot (the owner's prefix), after every parallel END, one after the other
        last_par_end = max(iv[a][1] for a in par)
        for s in ("row_s1", "row_s2"):
            self.assertEqual(iv[s][2], "none")
            self.assertGreater(iv[s][0], last_par_end)
        self.assertLess(iv["row_s1"][1], iv["row_s2"][0])

    def test_a_pair_waits_at_the_head_for_two_free_and_a_budget_of_two_runs_it_alone(self):
        rc, out = self.run_r("--jobs", "2")
        self.assertEqual(rc, 0, out)
        par = ["row_a", "row_b", "row_c", "row_d", "row_e", "row_f"]
        self.assertEqual(self.clients_now(2, par), 2)
        ev = self.events()
        run = set()
        for e in ev:
            if e[0] == "START" and e[1] in par:
                if e[1] == "row_c":
                    self.assertEqual(run, set(), "the pair did not run alone at --jobs 2: %s" % run)
                else:
                    self.assertNotIn("row_c", run, "%s started beside the pair at --jobs 2" % e[1])
                run.add(e[1])
            elif e[0] == "END":
                run.discard(e[1])

    def test_a_row_that_costs_more_than_the_budget_runs_alone(self):
        write(self.list, "row_a\nrow_c --with-second\nrow_d\n")
        rc, out = self.run_r("--jobs", "2")   # a cost of 2 against 2 is a whole budget; ...
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.clients_now(2, ["row_a", "row_c", "row_d"]), 2)

    def test_the_serial_flag_never_reaches_client_ci_and_the_rest_does(self):
        self.run_r("--jobs", "3")
        starts = {e[1]: " ".join(e[3:]) for e in self.events() if e[0] == "START"}
        for name, args in starts.items():
            self.assertNotIn("--serial", args, name)
            self.assertIn("--rootless", args)
            self.assertIn("--no-build", args)
        self.assertIn("-- --gateway --dev", starts["row_b"])
        self.assertIn("--with-second -- --gateway --character Alice --character Bob", starts["row_c"])

    def test_the_prefixes_and_the_display_are_made_before_and_stopped_after(self):
        rc, out = self.run_r("--jobs", "3")
        self.assertEqual(rc, 0, out)
        ev = [" ".join(e) for e in self.events()]
        first_start = next(i for i, e in enumerate(ev) if e.startswith("START"))
        for n in (1, 2, 3):
            self.assertIn("PREFIX ensure %d --client tq-offline" % n, ev[:first_start])
            self.assertIn("PREFIX ensure %d --second --client tq-offline" % n, ev[:first_start])
        self.assertIn("DISPLAY start", ev[:first_start])
        self.assertEqual(ev[-1].split()[0:2], ["DISPLAY", "stop"] if ev[-1].startswith("DISPLAY") else ev[-1].split()[0:2])
        self.assertEqual(len([e for e in ev if e == "DISPLAY stop"]), 1)
        self.assertGreater(max(i for i, e in enumerate(ev) if e == "DISPLAY stop"), max(i for i, e in enumerate(ev) if e.startswith("END")))

    def test_a_display_that_was_already_up_is_left_alone(self):
        rc, out = self.run_r("--jobs", "3", env={"EVE_TEST_DISPLAY_UP": "1"})
        self.assertEqual(rc, 0, out)
        ev = [" ".join(e) for e in self.events()]
        self.assertIn("DISPLAY status", ev)
        self.assertNotIn("DISPLAY start", ev)
        self.assertNotIn("DISPLAY stop", ev)

    def test_the_table_has_every_row_as_it_finished_and_ends_finished(self):
        rc, out = self.run_r("--jobs", "3", env={"EVE_TEST_FAIL": "row_d row_s1"})
        self.assertEqual(rc, 1, out)
        rows = self.tsv()
        self.assertEqual(rows[-1], ["finished"])
        got = {r[0]: r[1] for r in rows[:-1]}
        self.assertEqual(set(got), {"row_a", "row_b", "row_c", "row_d", "row_e", "row_f", "row_s1", "row_s2"})
        self.assertEqual({k for k, v in got.items() if v != "0"}, {"row_d", "row_s1"})
        self.assertIn("8 ran, 2 not green", out)
        clock = [l.split("\t") for l in rd(os.path.join(self.out, "clock.tsv")).splitlines()]
        self.assertEqual(len(clock), 9)
        self.assertEqual({r[1] for r in clock[1:]}, {"0", "1", "2", "3"})

    def test_rows_finish_out_of_order_and_fails_reruns_only_the_lost_ones(self):
        # row_a is slow, the rest fast: it is recorded after the rows that started later
        write(os.path.join(self.tmp, "stubs", "client-ci.sh"),
              STUB_CLIENT_CI.replace('sleep "${EVE_TEST_SLEEP:-0.5}"', '[[ $name == row_a ]] && s=1.5 || s=0.2; sleep $s'), 0o755)
        rc, out = self.run_r("--jobs", "3", env={"EVE_TEST_FAIL": "row_e"})
        names = [r[0] for r in self.tsv()[:-1]]
        self.assertLess(names.index("row_b"), names.index("row_a"))
        open(self.log, "w").close()
        rc, out = self.run_r("--jobs", "3", "--fails")
        self.assertEqual(rc, 0, out)
        self.assertEqual([e[1] for e in self.events() if e[0] == "START"], ["row_e"])
        self.assertEqual(self.tsv(), [["row_e", "0"], ["finished"]])

    def test_a_name_on_the_command_line_selects_rows(self):
        rc, out = self.run_r("--jobs", "2", "row_a", "row_s1")
        self.assertEqual(rc, 0, out)
        self.assertEqual(sorted(e[1] for e in self.events() if e[0] == "START"), ["row_a", "row_s1"])

    def test_jobs_one_is_the_old_runner_list_order_no_slots_no_prefixes(self):
        rc, out = self.run_r("--jobs", "1")
        self.assertEqual(rc, 0, out)
        ev = self.events()
        starts = [e for e in ev if e[0] == "START"]
        self.assertEqual([e[1] for e in starts], ["row_a", "row_b", "row_c", "row_d", "row_e", "row_f", "row_s1", "row_s2"])
        self.assertTrue(all(e[2] == "slot=none" for e in starts))
        self.assertFalse([e for e in ev if e[0] in ("PREFIX", "DISPLAY")])
        # one at a time: every START follows the previous END
        for i, e in enumerate(ev):
            if e[0] == "START" and i:
                self.assertEqual(ev[i - 1][0], "END")
        self.assertNotIn("--serial", " ".join(" ".join(e) for e in starts))

    def test_dry_run_shows_the_plan_and_starts_nothing(self):
        rc, out = self.run_r("--jobs", "3", "--dry-run")
        self.assertEqual(rc, 0, out)
        self.assertEqual(rd(self.log), "")
        self.assertFalse(os.path.exists(os.path.join(self.out, "regress.tsv")))
        self.assertIn("plan: --jobs 3: a budget of 3 client(s), a --with-second row costs 2; 6 row(s) in parallel on at most 3 slot(s)", out)
        self.assertEqual(len(re.findall(r"^\[slot [123] cost [12]\] EVE_CI_SLOT=[123] tools/client-ci.sh run ", out, re.M)), 6)
        self.assertEqual(len(re.findall(r"^\[slot [123] cost 2\] .* run row_c ", out, re.M)), 1)
        self.assertEqual(len(re.findall(r"^\[slot [123] cost 1\] ", out, re.M)), 5)
        self.assertEqual(len(re.findall(r"^\[serial\] tools/client-ci.sh run row_s", out, re.M)), 2)
        self.assertNotIn("--serial", re.sub(r"^plan:.*$", "", out, flags=re.M).replace("[serial]", ""))

    def test_dry_run_plan_gives_the_slots_nothing_in_common(self):
        rc, out = self.run_r("--jobs", "3", "--dry-run")
        per = {}
        for m in re.finditer(r"^slot (\d+): (\w+)=(.*)$", out, re.M):
            per.setdefault(m.group(2), []).append(m.group(3))
        self.assertTrue(per["SLOT_PREFIX"])
        for key, vals in per.items():
            self.assertEqual(len(vals), 3, key)
            self.assertEqual(len(set(vals)), 3, "%s is shared: %s" % (key, vals))

    def test_jobs_needs_the_rootless_jail(self):
        rc, out = self.run_r("--jobs", "2", "--no-rootless", "--dry-run")
        self.assertEqual(rc, 2)
        self.assertIn("needs the rootless jail", out)
        rc, out = self.run_r("--jobs", "0")
        self.assertEqual(rc, 2)

    def test_a_missing_scenario_is_a_failed_row_not_a_hole(self):
        os.remove(os.path.join(self.scen, "row_d.py"))
        rc, out = self.run_r("--jobs", "3")
        self.assertEqual(rc, 1)
        self.assertIn(["row_d", "missing"], self.tsv())
        self.assertNotIn("row_d", [e[1] for e in self.events() if e[0] == "START"])

    def wait_starts(self, n):
        for _ in range(100):
            if sum(1 for e in self.events() if e[0] == "START") >= n:
                break
            time.sleep(0.1)
        self.assertEqual(sum(1 for e in self.events() if e[0] == "START"), n)

    def test_a_signal_stops_every_running_row_records_143_and_sweeps_only_what_it_touched(self):
        # a, b run (2 clients); c is a pair and waits for 2 free, so the rest never start
        p = self.start("--jobs", "3", env={"EVE_TEST_SLEEP": "30"})
        self.wait_starts(2)
        time.sleep(0.5)
        p.send_signal(signal.SIGTERM)
        out, _ = p.communicate(timeout=60)
        self.assertEqual(p.returncode, 143, out)
        ev = self.events()
        self.assertEqual(len([e for e in ev if e[0] == "TERM"]), 2)
        self.assertEqual(len([e for e in ev if e[0] == "START"]), 2, "a row started after the signal")
        sweeps = {e[1] for e in ev if e[0] == "SWEEP"}
        # no serial row has started: the owner's prefix is none of the sweep's business
        self.assertFalse(any(s.endswith("/eve-offline/prefix/pfx/drive_c") for s in sweeps), sweeps)
        for n in (1, 2, 3):
            self.assertIn(os.path.join(self.home, ".local/share/eve-offline/slots/%d/prefix/pfx/drive_c" % n), sweeps)
        self.assertEqual(len([e for e in ev if e[:2] == ["DISPLAY", "stop"]]), 1)
        # every row has a status: the stopped ones and the ones that never started are 143
        got = {r[0]: r[1] for r in self.tsv() if len(r) == 2}
        self.assertEqual(set(got), {"row_a", "row_b", "row_c", "row_d", "row_e", "row_f", "row_s1", "row_s2"})
        self.assertEqual(set(got.values()), {"143"})
        open(self.log, "w").close()
        rc, out = self.run_r("--jobs", "3", "--fails")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len([e for e in self.events() if e[0] == "START"]), 8)

    def test_a_signal_during_a_serial_row_stops_that_client_too(self):
        for jobs in ("3", "1"):
            open(self.log, "w").close()
            p = self.start("--jobs", jobs, "row_s1", "row_s2", env={"EVE_TEST_SLEEP": "30"})
            self.wait_starts(1)
            p.send_signal(signal.SIGTERM)
            out, _ = p.communicate(timeout=60)
            self.assertEqual(p.returncode, 143, out)
            ev = self.events()
            self.assertEqual([e[1] for e in ev if e[0] == "TERM"], ["row_s1"], "--jobs " + jobs)
            self.assertEqual(self.tsv(), [["row_s1", "143"], ["row_s2", "143"]])
            sweeps = {e[1] for e in ev if e[0] == "SWEEP"}
            self.assertTrue(any(s.endswith("/eve-offline/prefix/pfx/drive_c") for s in sweeps), sweeps)

    def test_the_owners_settings_are_not_swept_under_a_running_client(self):
        write(self.live, "tq-offline.bin64.exefile\n")
        p = self.start("--jobs", "3", "row_s1", env={"EVE_TEST_SLEEP": "30"})
        self.wait_starts(1)
        p.send_signal(signal.SIGTERM)
        p.communicate(timeout=60)
        sweeps = {e[1] for e in self.events() if e[0] == "SWEEP"}
        self.assertFalse(any(s.endswith("/eve-offline/prefix/pfx/drive_c") for s in sweeps), sweeps)

    def test_a_second_runner_is_refused_while_one_holds_the_slots(self):
        import fcntl
        with open(os.path.join(self.tmp, "runner.lock"), "w") as f:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            rc, out = self.run_r("--jobs", "3")
            self.assertEqual(rc, 2)
            self.assertIn("holds the slots", out)
            self.assertEqual(self.events(), [])
            # --jobs 1 uses no slot and needs no lock
            rc, out = self.run_r("--jobs", "1", "row_a")
            self.assertEqual(rc, 0, out)

    def test_clean_removes_every_slot_through_the_tool(self):
        rc, out = self.run_r("--clean")
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.events(), [["PREFIX", "remove", "all"]])

    def test_a_row_starts_with_the_stale_runs_of_its_name_gone_from_idle_slots(self):
        base = os.path.join(self.repo, "out", "client-ci", "slots")
        for n in (1, 2, 3, 4):
            write(os.path.join(base, str(n), "runs", "row_a", "up.log"), "stale\n")
            write(os.path.join(base, str(n), "runs", "row_b", "up.log"), "other row\n")
        rc, out = self.run_r("--jobs", "3", "row_a")
        self.assertEqual(rc, 0, out)
        for n in (2, 3, 4):   # slot 1 ran it (the stub leaves its dir alone; client-ci.sh wipes its own)
            self.assertFalse(os.path.exists(os.path.join(base, str(n), "runs", "row_a")), n)
            self.assertTrue(os.path.exists(os.path.join(base, str(n), "runs", "row_b")), n)

    def test_the_build_goes_to_the_first_row_that_runs_not_the_first_in_the_list(self):
        os.remove(os.path.join(self.scen, "row_a.py"))
        p = self.start("--jobs", "1", "row_a", "row_b", "row_d", build=True)
        out, _ = p.communicate(timeout=SLOW)
        starts = {e[1]: " ".join(e[3:]) for e in self.events() if e[0] == "START"}
        self.assertNotIn("--no-build", starts["row_b"], out)
        self.assertIn("--no-build", starts["row_d"])

    def test_the_real_list_is_a_plan_with_the_marked_rows_last(self):
        real = os.path.join(CI, "regress.list")
        e = dict(self.env)
        e.pop("EVE_CI_SCENARIOS")
        p = subprocess.run(["bash", os.path.join(HERE, "client-ci-regress.sh"), "--list", real, "--out", self.out,
                            "--jobs", "3", "--dry-run"], capture_output=True, text=True, env=e, timeout=SLOW)
        self.assertEqual(p.returncode, 0, p.stderr)
        lines = p.stdout.splitlines()
        kinds = [("slot" if l.startswith("[slot") else "serial") for l in lines if l.startswith(("[slot", "[serial"))]
        self.assertIn("serial", kinds)
        self.assertEqual(kinds, sorted(kinds, key=lambda k: k == "serial"), "a serial row before a parallel one")
        serial = [l for l in lines if l.startswith("[serial]")]
        self.assertTrue(all("--with-bots" in l or re.search(r"run (warp_blob|throttle_and_warp|undock_distance|park_census|fci_\w+)", l) for l in serial))
        # every row of the list is planned exactly once
        listed = [l.split("#")[0].split()[0] for l in rd(real).splitlines() if l.split("#")[0].split()]
        planned = [re.search(r"client-ci.sh run (\S+)", l).group(1) for l in lines if "client-ci.sh run" in l]
        self.assertEqual(sorted(planned), sorted(k.split("@")[0] for k in listed))
        for l in lines:
            if "--with-bots" in l:
                self.assertTrue(l.startswith("[serial]"), l)


if __name__ == "__main__":
    unittest.main()
