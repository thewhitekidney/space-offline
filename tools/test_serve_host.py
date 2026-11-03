"""Tests for the hosting scripts (F-HOST): tools/serve.sh's --bundle-frozen and
owner listener, tools/friend-tunnel.sh --owner, tools/host-bots.sh,
tools/host-backup.sh and tools/host-wipe.sh.

Nothing here builds, uses docker or starts an eve-server: serve.sh runs in a
temporary copy of tools/ beside a stub eve-server that records its arguments,
and every other script runs against a temporary var/ directory. All names,
files and content are invented.

Run::

    tools/analysis.sh python3 -I tools/test_serve_host.py
"""

from __future__ import annotations

import fcntl
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOLS = ("serve.sh", "stop-server.sh")

# A stand-in eve-server: --help lists the flag serve.sh looks for, any other
# run writes its arguments (one per line) to $STUB_OUT and exits 0, which
# serve.sh reads as a clean stop.
STUB_SERVER = r"""#!/usr/bin/env bash
if [[ ${1:-} == --help ]]; then echo "  --world-accounts  --accounts FILE"; exit 0; fi
printf '%s\n' "$@" >"$STUB_OUT"
exit "${STUB_EXIT:-0}"
"""

# A stand-in curl: records its arguments and what it is given on stdin.
STUB_CURL = r"""#!/usr/bin/env bash
printf '%s\n' "$@" >"$CURL_ARGS"
cat >"$CURL_STDIN"
exit 0
"""

# A stand-in eve-bots: records its arguments and exits.
STUB_BOTS = r"""#!/usr/bin/env bash
printf '%s\n' "$@" >"$STUB_OUT"
exit 0
"""


def write(path: str, text: str, mode: int = 0o644) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)


def run(cmd, env=None, cwd=None):
    e = {"PATH": os.environ["PATH"], "HOME": os.environ.get("HOME", "/tmp")}
    e.update(env or {})
    return subprocess.run(
        cmd, env=e, cwd=cwd, capture_output=True, text=True, timeout=60, check=False
    )


class Lock:
    """Holds <world>.lock the way a running eve-server does."""

    def __init__(self, path: str) -> None:
        self.f = open(path, "w")
        fcntl.flock(self.f, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def close(self) -> None:
        self.f.close()


class ServeTree(unittest.TestCase):
    """serve.sh in a copy of the tools directory with a stub server."""

    def setUp(self) -> None:
        self.dir = tempfile.mkdtemp(prefix="eve-serve-host-")
        self.addCleanup(shutil.rmtree, self.dir, ignore_errors=True)
        os.makedirs(os.path.join(self.dir, "tools"))
        for t in TOOLS:
            shutil.copy(os.path.join(HERE, t), os.path.join(self.dir, "tools", t))
        write(
            os.path.join(self.dir, "server/target/release/eve-offline-server"),
            STUB_SERVER,
            0o755,
        )
        self.out = os.path.join(self.dir, "stub.out")

    def serve(self, *args, manifest=True):
        if manifest:
            write(
                os.path.join(self.dir, "out/content/MANIFEST.tsv"),
                "format\t1.4\nclient_build\t1234567\ndata_hash\tabcd1234\n",
            )
        return run(
            ["bash", os.path.join(self.dir, "tools/serve.sh"), "--no-build", *args],
            env={"STUB_OUT": self.out, "EVE_WEBHOOK_FILE": "/nonexistent"},
        )

    def binds(self):
        with open(self.out) as f:
            a = f.read().split("\n")
        return [a[i + 1] for i, x in enumerate(a) if x == "--bind"]

    def test_frozen_bundle_prints_build_and_hash_and_skips_the_check(self) -> None:
        r = self.serve("--bundle-frozen")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("frozen: build 1234567, data_hash abcd1234", r.stdout)
        self.assertNotIn("content.py --check", r.stdout)
        self.assertNotIn("extracting", r.stdout)

    def test_frozen_bundle_needs_a_manifest(self) -> None:
        r = self.serve("--bundle-frozen", manifest=False)
        self.assertEqual(r.returncode, 1)
        self.assertIn("MANIFEST.tsv", r.stderr)
        self.assertFalse(os.path.exists(self.out), "the server must not start")

    def test_friends_brings_a_third_owner_listener(self) -> None:
        r = self.serve("--bundle-frozen", "--friends")
        self.assertEqual(r.returncode, 0, r.stderr)
        b = self.binds()
        self.assertEqual(len(b), 3, b)
        self.assertTrue(b[0].startswith("127.0.0.1:26000"))
        self.assertTrue(b[1].startswith("127.0.0.1:26001,ceiling=player"))
        self.assertIn("signup=on", b[1])
        owner = b[2].split(",")
        self.assertEqual(owner[0], "127.0.0.1:26002")
        self.assertIn("ceiling=gm", owner)
        self.assertIn("cap=2", owner)
        self.assertIn("chat=127.0.0.1:5224", owner)
        self.assertNotIn("signup=on", owner)

    def test_signup_is_on_the_owner_listener_too(self) -> None:
        r = self.serve("--bundle-frozen", "--friends", "--signup")
        self.assertEqual(r.returncode, 0, r.stderr)
        b = self.binds()
        self.assertIn("signup=on", b[0].split(","))
        self.assertIn("signup=on", b[2].split(","))

    def test_the_webhook_url_is_never_in_curls_argv_and_a_failure_pings(self) -> None:
        url = "https://example.invalid/hook/invented-secret"
        hook = os.path.join(self.dir, "webhook")
        write(hook, url + "\n", 0o600)
        write(os.path.join(self.dir, "fakebin/curl"), STUB_CURL, 0o755)
        args, stdin = os.path.join(self.dir, "curl.args"), os.path.join(self.dir, "curl.stdin")
        write(
            os.path.join(self.dir, "out/content/MANIFEST.tsv"),
            "format\t1.4\nclient_build\t1234567\ndata_hash\tabcd1234\n",
        )
        r = run(
            ["bash", os.path.join(self.dir, "tools/serve.sh"), "--no-build", "--bundle-frozen"],
            env={
                "STUB_OUT": self.out,
                "STUB_EXIT": "3",
                "EVE_WEBHOOK_FILE": hook,
                "CURL_ARGS": args,
                "CURL_STDIN": stdin,
                "PATH": os.path.join(self.dir, "fakebin") + ":" + os.environ["PATH"],
            },
        )
        self.assertEqual(r.returncode, 1, r.stderr)
        with open(args) as f:
            self.assertNotIn("invented-secret", f.read())
        with open(stdin) as f:
            self.assertIn(url, f.read())

    def test_a_second_serve_is_refused_and_serve_lock_keeps_its_mtime(self) -> None:
        lock = os.path.join(self.dir, "var/serve.lock")
        os.makedirs(os.path.dirname(lock))
        write(lock, "")
        os.utime(lock, (1000, 1000))
        held = open(lock, "a")
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.addCleanup(held.close)
        r = self.serve("--bundle-frozen")
        self.assertEqual(r.returncode, 1)
        self.assertIn("serve.lock", r.stderr)
        self.assertEqual(int(os.stat(lock).st_mtime), 1000)

    def test_owner_listener_can_be_left_out_and_is_not_there_without_friends(self) -> None:
        self.serve("--bundle-frozen", "--friends", "--no-owner-listener")
        self.assertEqual(len(self.binds()), 2)
        os.remove(self.out)
        self.serve("--bundle-frozen")
        self.assertEqual(self.binds(), ["127.0.0.1:26000"])

    def test_the_tunnel_never_forwards_the_owner_listener(self) -> None:
        with open(os.path.join(HERE, "tunnel.sh")) as f:
            text = f.read()
        self.assertIn("PERMIT_OPEN=(127.0.0.1:26001 127.0.0.1:5223 127.0.0.1:8443)", text)
        for port in ("26002", "5224"):
            self.assertNotIn(port, text.replace("never 26000", ""))


class FriendTunnelOwner(unittest.TestCase):
    def dry(self, *args):
        d = tempfile.mkdtemp(prefix="eve-ft-")
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        os.chmod(d, 0o700)
        r = run(
            ["bash", os.path.join(HERE, "friend-tunnel.sh"), "h.invalid", "22", *args, "--dry-run"],
            env={"XDG_RUNTIME_DIR": d, "EVE_OWNER_SSH_USER": "invowner"},
        )
        return r

    def test_owner_maps_the_owner_ports_and_logs_in_as_the_owner(self) -> None:
        r = self.dry("--owner")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("eve-game.sock:127.0.0.1:26002", r.stdout)
        self.assertIn("eve-chat.sock:127.0.0.1:5224", r.stdout)
        self.assertIn("eve-gw.sock:127.0.0.1:8443", r.stdout)
        self.assertIn("invowner@h.invalid", r.stdout)
        self.assertNotIn("26001", r.stdout)

    def test_a_friend_run_is_unchanged(self) -> None:
        r = self.dry()
        self.assertIn("eve-game.sock:127.0.0.1:26001", r.stdout)
        self.assertIn("eve@h.invalid", r.stdout)
        self.assertNotIn("26002", r.stdout)

    def test_a_bad_owner_account_name_is_refused(self) -> None:
        d = tempfile.mkdtemp(prefix="eve-ft-")
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        os.chmod(d, 0o700)
        r = run(
            ["bash", os.path.join(HERE, "friend-tunnel.sh"), "h.invalid", "22", "--owner", "--dry-run"],
            env={"XDG_RUNTIME_DIR": d, "EVE_OWNER_SSH_USER": "x -o ProxyCommand=y"},
        )
        self.assertEqual(r.returncode, 2)


class VarTree(unittest.TestCase):
    """host-bots, host-backup and host-wipe against a temporary var/."""

    def setUp(self) -> None:
        self.dir = tempfile.mkdtemp(prefix="eve-host-")
        self.addCleanup(shutil.rmtree, self.dir, ignore_errors=True)
        self.var = os.path.join(self.dir, "var")
        self.tokens = os.path.join(self.dir, "cfg/bots.tsv")
        self.logs = os.path.join(self.dir, "logs")
        os.makedirs(self.var)
        self.env = {
            "EVE_VAR_DIR": self.var,
            "EVE_BOT_TOKENS": self.tokens,
            "EVE_LOGS_DIR": self.logs,
            "EVE_BIN_DIR": os.path.join(self.dir, "bin"),
            "EVE_BACKUPS_KEPT": "3",
            "STUB_OUT": os.path.join(self.dir, "stub.out"),
        }
        self.lock = None

    def tearDown(self) -> None:
        if self.lock:
            self.lock.close()

    def world(self, text="world-one") -> None:
        write(os.path.join(self.var, "world.dat"), text)
        write(os.path.join(self.var, "world.dat.ledger/2000-01-01.tsv"), "ledger-" + text)
        write(os.path.join(self.var, "bots/state/Miner1"), "mind-" + text)
        write(os.path.join(self.var, "tunnel/keys/host/k"), "hostkey")
        write(self.tokens, "Miner1\tMiner1:pw\n", 0o600)

    def script(self, name, *args, env=None):
        e = dict(self.env)
        e.update(env or {})
        return run(["bash", os.path.join(HERE, name), *args], env=e)

    def hold(self) -> None:
        self.lock = Lock(os.path.join(self.var, "world.dat.lock"))

    # ---- host-backup

    def backups(self):
        r = self.script("host-backup.sh", "list")
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout.split()

    def test_backup_copies_everything_while_the_server_runs(self) -> None:
        self.world()
        self.hold()
        r = self.script("host-backup.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        (b,) = self.backups()
        for rel, text in (
            ("world.dat", "world-one"),
            ("world.dat.ledger/2000-01-01.tsv", "ledger-world-one"),
            ("bots/state/Miner1", "mind-world-one"),
            ("tunnel/keys/host/k", "hostkey"),
            ("bots.tsv", "Miner1\tMiner1:pw\n"),
        ):
            with open(os.path.join(b, rel)) as f:
                self.assertEqual(f.read(), text, rel)
        self.assertEqual(stat.S_IMODE(os.stat(b).st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(os.stat(os.path.join(b, "bots.tsv")).st_mode), 0o600)

    def test_backup_keeps_only_the_newest_n(self) -> None:
        self.world()
        for i in range(5):
            # Distinct names without sleeping: the script adds its pid on a clash.
            self.assertEqual(self.script("host-backup.sh").returncode, 0)
        self.assertEqual(len(self.backups()), 3)

    def test_backup_without_a_world_fails(self) -> None:
        r = self.script("host-backup.sh")
        self.assertEqual(r.returncode, 1)

    def test_rsync_failure_exits_non_zero(self) -> None:
        self.world()
        r = self.script("host-backup.sh", env={"EVE_BACKUP_RSYNC": os.path.join(self.dir, "no/such/dir/")})
        if shutil.which("rsync"):
            self.assertEqual(r.returncode, 1)
            self.assertEqual(len(self.backups()), 1, "the local backup stays")
        else:
            self.assertEqual(r.returncode, 1)

    def test_restore_puts_the_world_back_and_keeps_what_it_replaced(self) -> None:
        self.world("world-one")
        self.assertEqual(self.script("host-backup.sh").returncode, 0)
        (b,) = self.backups()
        self.world("world-two")
        r = self.script("host-backup.sh", "restore", b)
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(self.var, "world.dat")) as f:
            self.assertEqual(f.read(), "world-one")
        with open(os.path.join(self.var, "world.dat.ledger/2000-01-01.tsv")) as f:
            self.assertEqual(f.read(), "ledger-world-one")
        with open(os.path.join(self.var, "bots/state/Miner1")) as f:
            self.assertEqual(f.read(), "mind-world-one")
        kept = os.listdir(os.path.join(self.var, "wiped"))
        self.assertEqual(len(kept), 1)
        with open(os.path.join(self.var, "wiped", kept[0], "world.dat")) as f:
            self.assertEqual(f.read(), "world-two")

    def test_restore_refuses_while_a_server_holds_the_lock_or_on_a_non_backup(self) -> None:
        self.world()
        self.assertEqual(self.script("host-backup.sh").returncode, 0)
        (b,) = self.backups()
        self.hold()
        self.assertEqual(self.script("host-backup.sh", "restore", b).returncode, 1)
        self.assertEqual(self.script("host-backup.sh", "restore", self.dir).returncode, 2)

    # ---- host-wipe

    def test_wipe_says_what_it_would_do_and_moves_nothing_without_yes(self) -> None:
        self.world()
        r = self.script("host-wipe.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("world.dat", r.stdout)
        self.assertTrue(os.path.exists(os.path.join(self.var, "world.dat")))

    def test_wipe_moves_the_world_and_leaves_the_lock_backups_and_tunnel(self) -> None:
        self.world()
        write(os.path.join(self.var, "world.dat.lock"), "")
        write(os.path.join(self.var, "backups/a.dat"), "old")
        r = self.script("host-wipe.sh", "--yes")
        self.assertEqual(r.returncode, 0, r.stderr)
        for gone in ("world.dat", "world.dat.ledger", "bots/state"):
            self.assertFalse(os.path.exists(os.path.join(self.var, gone)), gone)
        self.assertFalse(os.path.exists(self.tokens))
        for stays in ("world.dat.lock", "backups/a.dat", "tunnel/keys/host/k"):
            self.assertTrue(os.path.exists(os.path.join(self.var, stays)), stays)
        (stamp,) = os.listdir(os.path.join(self.var, "wiped"))
        moved = os.path.join(self.var, "wiped", stamp)
        for rel in ("world.dat", "world.dat.ledger/2000-01-01.tsv", "state/Miner1", "bots.tsv"):
            self.assertTrue(os.path.exists(os.path.join(moved, rel)), rel)
        self.assertIn("--grant 'YourName' gm", r.stdout)
        self.assertNotIn("account.sh grant", r.stdout)

    def test_wipe_refuses_while_a_server_holds_the_lock(self) -> None:
        self.world()
        self.hold()
        r = self.script("host-wipe.sh", "--yes")
        self.assertEqual(r.returncode, 1)
        self.assertTrue(os.path.exists(os.path.join(self.var, "world.dat")))

    def test_wipe_and_restore_refuse_under_a_serve_sh_or_bots_lock(self) -> None:
        self.world()
        self.assertEqual(self.script("host-backup.sh").returncode, 0)
        (b,) = self.backups()
        for name in ("serve.lock", "bots.lock"):
            f = open(os.path.join(self.var, name), "a")
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                self.assertEqual(self.script("host-wipe.sh", "--yes").returncode, 1, name)
                self.assertEqual(self.script("host-backup.sh", "restore", b).returncode, 1, name)
            finally:
                f.close()
            self.assertTrue(os.path.exists(os.path.join(self.var, "world.dat")), name)

    def test_a_second_bots_run_or_accounts_is_refused(self) -> None:
        self.bots_setup()
        f = open(os.path.join(self.var, "bots.lock"), "a")
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            self.hold()
            r = self.script("host-bots.sh", "run", "--no-build", "--miners", "2", "--escorts", "1")
            self.assertEqual(r.returncode, 1)
            self.assertIn("bots.lock", r.stderr)
        finally:
            f.close()

    def test_rsync_goes_into_its_own_subdirectory_and_never_the_bare_target(self) -> None:
        if not shutil.which("rsync"):
            self.skipTest("no rsync")
        self.world()
        target = os.path.join(self.dir, "elsewhere")
        os.makedirs(target)
        write(os.path.join(target, "precious"), "keep me")
        r = self.script("host-backup.sh", env={"EVE_BACKUP_RSYNC": target + "/"})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(os.path.exists(os.path.join(target, "precious")))
        (b,) = self.backups()
        self.assertTrue(
            os.path.exists(os.path.join(target, "eve-host-backups", os.path.basename(b), "world.dat"))
        )

    def test_prune_ignores_foreign_names_and_survives_spaces(self) -> None:
        self.world()
        os.makedirs(os.path.join(self.var, "host-backups/not a backup"))
        os.makedirs(os.path.join(self.var, "host-backups/20200101T000000Z old"))
        for _ in range(5):
            self.assertEqual(self.script("host-backup.sh").returncode, 0)
        self.assertTrue(os.path.isdir(os.path.join(self.var, "host-backups/not a backup")))
        self.assertEqual(len(self.backups()), 3)

    def test_restore_warns_when_the_bots_passwords_differ(self) -> None:
        self.world("world-one")
        self.assertEqual(self.script("host-backup.sh").returncode, 0)
        (b,) = self.backups()
        write(self.tokens, "Miner1\tMiner1:other\n", 0o600)
        r = self.script("host-backup.sh", "restore", b)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("differ", r.stderr)

    def test_a_failed_backup_leaves_no_temp_directory(self) -> None:
        self.world()
        os.chmod(os.path.join(self.var, "bots/state/Miner1"), 0)
        try:
            r = self.script("host-backup.sh")
        finally:
            os.chmod(os.path.join(self.var, "bots/state/Miner1"), 0o644)
        if os.geteuid() != 0:
            self.assertNotEqual(r.returncode, 0)
        out = os.path.join(self.var, "host-backups")
        self.assertEqual([n for n in os.listdir(out) if n.startswith(".tmp-")], [])

    # ---- host-bots

    def test_roster_names_the_bots_the_soak_way(self) -> None:
        r = self.script("host-bots.sh", "roster", "--miners", "2", "--escorts", "1", "--ratters", "1")
        self.assertEqual(r.stdout.split(), ["Miner1", "Miner2", "Escort1", "Ratter1"])

    def bots_setup(self) -> None:
        write(os.path.join(self.dir, "content/MANIFEST.tsv"), "format\t1.4\n")
        write(os.path.join(self.dir, "bin/eve-bots"), STUB_BOTS, 0o755)
        self.env["EVE_CONTENT"] = os.path.join(self.dir, "content")
        write(self.tokens, "Miner1\tMiner1:pw\nMiner2\tMiner2:pw\nEscort1\tEscort1:pw\n", 0o600)

    def test_run_refuses_without_a_server_and_without_a_password(self) -> None:
        self.bots_setup()
        r = self.script("host-bots.sh", "run", "--no-build", "--miners", "2", "--escorts", "1")
        self.assertEqual(r.returncode, 1)
        self.assertIn("serve.sh", r.stderr)
        write(os.path.join(self.var, "world.dat.lock"), "")
        self.hold()
        r = self.script("host-bots.sh", "run", "--no-build", "--miners", "3", "--escorts", "1")
        self.assertEqual(r.returncode, 1)
        self.assertIn("Miner3", r.stderr)

    def test_run_starts_the_society_bots_on_the_owner_listener_niced(self) -> None:
        self.bots_setup()
        self.hold()
        r = self.script(
            "host-bots.sh", "run", "--no-build", "--miners", "2", "--escorts", "1", "--difficulty", "hard"
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.env["STUB_OUT"]) as f:
            a = f.read().split("\n")
        self.assertEqual(a[:2], ["--connect", "127.0.0.1:26000"])
        for flag in ("--society", "--closed", "--tokens", "--content", "--difficulty"):
            self.assertIn(flag, a)
        self.assertEqual(a[a.index("--state") + 1], os.path.join(self.var, "bots/state"))
        self.assertEqual(a[a.index("--sessions") + 1], "always")
        self.assertEqual(a[a.index("--difficulty") + 1], "hard")
        self.assertIn("Miner", a)
        self.assertEqual(a[a.index("--tokens") + 1], self.tokens)
        self.assertEqual(len(os.listdir(self.logs)), 1)

    def test_accounts_refuses_while_the_server_is_up(self) -> None:
        self.bots_setup()
        self.hold()
        r = self.script("host-bots.sh", "accounts", "--no-build", "--miners", "1")
        self.assertEqual(r.returncode, 1)
        self.assertIn("stop serve.sh", r.stderr)

    def test_bad_arguments_are_refused(self) -> None:
        self.assertEqual(self.script("host-bots.sh", "run", "--miners", "x").returncode, 2)
        self.assertEqual(self.script("host-bots.sh", "fly").returncode, 2)
        self.assertEqual(self.script("host-bots.sh", "roster", "--miners", "0", "--escorts", "0").returncode, 2)


if __name__ == "__main__":
    unittest.main()
