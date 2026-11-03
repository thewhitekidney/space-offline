"""Tests for the jail's allowlist (packet IP14; rulings 3, 4, 11 and 15; RR1
C3, C4, H1; V6 F1, F2). IP14b added ruling 15: the host socket inventory,
its masking and the connect gate, the /media and /mnt masks, and the
client's own pid namespace and /proc.

Three kinds of check, none of which needs root or touches the host:

* **Static.** Parse `tools/eve-offline-jail.in` and `tools/jail-setup.sh`:
  the helper's bind-back set equals ALLOWLIST below exactly, the masks are
  the five trees, the nft heredocs in the two files agree, 8443 is gone,
  `v_dns` passes `--log`, and the client is dropped with `--clear-groups`
  and `--no-new-privs` only after the view is built and scanned. Adding one
  `bind_back` line, or editing one nft rule in one file, fails a test.
* **The view, for real.** The helper is sourced inside an unprivileged user
  and mount namespace (`unshare -rm`), and `build_view` runs against an
  invented home, prefix, Proton and runtime dir. The test then looks at the
  result from inside: the owner's files are gone, the allowlist is back, and
  `scan_view` passes -- and fails once one stranger socket is added. Then
  `run_gate` runs the real launch gate in a pid namespace of its own, with a
  live listener on an unmasked tmpfs and a stand-in network share under
  /media: the client starts, and each of the three ruling-15 fixes, removed
  in turn, makes the gate refuse (or, for the reaper, lose the game).
* **`jail-verify.sh`, for real.** Run in a user namespace (so `$EUID` is 0)
  with `ip` and `nft` replaced by stubs that never run a probe and never
  send a packet: it passes on a whole jail, and fails with the host table
  deleted, with `policy accept`, or with a second route.

The namespace tests skip, saying why, where `unshare -r` is not permitted.

Run::

    python3 -I tools/test_jail.py
"""

from __future__ import annotations

import os
import re
import shutil
import socket
import stat
import subprocess
import tempfile
import textwrap
import time
import unittest

# The namespace runs below take seconds on a quiet machine and timed out at
# 60 s while seven lanes and 500 bots shared it (gate, 2026-10-28): a bound
# for a hang, not a speed check.
SLOW = 300

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "eve-offline-jail.in")
SETUP = os.path.join(HERE, "jail-setup.sh")
VERIFY = os.path.join(HERE, "jail-verify.sh")

# The allowlist: (mode, owner, kind, label). The helper's `bind_back` lines
# must be exactly this set. Growing it is a reviewed edit in two files.
ALLOWLIST = {
    ("ro", "root", "dir", "udev"),
    ("ro", "root", "file", "resolver"),
    ("rw", "user", "socket", "pulse"),
    ("rw", "user", "socket", "pipewire"),
    ("rw", "user", "socket", "wayland"),
    ("ro", "user", "file", "xauthority"),
    ("rw", "any", "socket", "x11"),
    ("ro", "user", "dir", "proton"),
    ("rw", "user", "dir", "prefix"),
    ("rw", "user", "dir", "game"),
    ("ro", "user", "dir", "resfiles"),
    ("ro", "user", "file", "shim"),
    ("ro", "user", "file", "launch-env"),
    ("ro", "user", "file", "patcher"),
    ("ro", "user", "dir", "patches"),
    # The census (CEN-3): bound only while census0 is in the namespace.
    ("ro", "user", "dir", "census-bin"),
    ("ro", "user", "file", "census-canary"),
    ("ro", "user", "file", "census-wire"),
    ("rw", "user", "file", "census-exec"),
    ("rw", "user", "file", "census-marks"),
}
CENSUS_LABELS = {"census-bin", "census-canary", "census-wire", "census-exec", "census-marks"}
# The trees replaced by an empty tmpfs (plus a fresh /dev/shm). media and mnt:
# ruling 15 (network shares).
MASKS = {"run", "var-run", "tmp", "home", "runtime", "media", "mnt"}

BIND_RE = re.compile(r"^\s*bind_back\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s", re.M)
MASK_RE = re.compile(r"(?:^\s*|\bthen\s+)mask_tree\s+(\S+)\s", re.M)


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def bind_backs(text: str) -> set[tuple[str, str, str, str]]:
    return {m.groups() for m in BIND_RE.finditer(text)}


def function_body(text: str, name: str) -> str:
    """The body of a top-level bash function `name() {` ... `}`."""
    m = re.search(r"^%s\(\) \{\n(.*?)^\}\n" % re.escape(name), text, re.M | re.S)
    if not m:
        raise AssertionError("no function %s" % name)
    return m.group(1)


def nft_tables(text: str) -> dict[str, str]:
    """Every `table inet NAME { ... }` block, whitespace-normalised."""
    out: dict[str, str] = {}
    for m in re.finditer(r"^table inet (\w+) \{\n", text, re.M):
        depth, i = 1, m.end()
        while depth:
            c = text[i]
            depth += c == "{"
            depth -= c == "}"
            i += 1
        body = text[m.start():i]
        out[m.group(1)] = "\n".join(" ".join(l.split()) for l in body.splitlines() if l.strip())
    return out


def userns_ok() -> str | None:
    if not shutil.which("unshare"):
        return "no unshare"
    try:
        r = subprocess.run(["unshare", "--user", "--map-root-user", "--mount", "true"],
                           capture_output=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired) as e:
        return "unshare failed: %s" % e
    return None if r.returncode == 0 else "unprivileged user namespaces are not permitted here"


class StaticHelper(unittest.TestCase):
    def setUp(self) -> None:
        self.h = read(HELPER)

    def test_bind_back_set_is_exactly_the_allowlist(self) -> None:
        self.assertEqual(bind_backs(self.h), ALLOWLIST)

    def test_bind_back_lines_live_only_in_build_view(self) -> None:
        body = function_body(self.h, "build_view")
        self.assertEqual(bind_backs(body), ALLOWLIST)
        outside = self.h.replace(body, "")
        self.assertEqual(bind_backs(outside), set(), "a bind_back outside build_view")

    def test_every_bind_mount_goes_through_the_two_allowlist_functions(self) -> None:
        allowed = function_body(self.h, "bind_back") + function_body(self.h, "place_all")
        masks = function_body(self.h, "mask_sockets")
        for line in self.h.splitlines():
            if re.search(r"\bmount\b.*(--bind|--rbind|\bbind\b)", line) and not line.lstrip().startswith("#"):
                if line in masks:  # ruling 15: only the empty file, only read-only
                    self.assertIn('mount --bind -- "$STAGE/empty" "$p" && mount -o remount,bind,ro,', line)
                    continue
                self.assertIn(line, allowed, "bind mount outside bind_back/place_all/mask_sockets: %s" % line)

    def test_sockets_are_inventoried_first_and_covered_after_the_masks(self) -> None:
        body = function_body(self.h, "build_view")
        order = [body.index(s) for s in (
            "inventory_sockets", "eve-jail-stage", "mask_tree run", "mask_tree media", "mask_tree mnt",
            "place_all", "mask_sockets", 'umount -- "$STAGE"', "eve-jail-shm")]
        self.assertEqual(order, sorted(order))
        inv = function_body(self.h, "inventory_sockets")
        for needle in ('-t "$LOCAL_FS"', "-xdev -type s", "/proc/net/unix", "/proc/1/net/unix",
                       '"${NEVER_VISIBLE[@]}"'):
            self.assertIn(needle, inv)
        self.assertIn("-S $p", function_body(self.h, "mask_sockets"))
        self.assertTrue({"cifs", "nfs", "autofs", "fuse.sshfs"}
                        <= set(re.search(r"(?m)^NETFS='([^']*)'$", self.h).group(1).split()))

    def test_masks_are_the_five_trees_and_shm_is_fresh(self) -> None:
        body = function_body(self.h, "build_view")
        self.assertEqual(set(MASK_RE.findall(body)), MASKS)
        self.assertIn("mount -t tmpfs -o nosuid,nodev,mode=1777 eve-jail-shm /dev/shm", body)
        # Stage, then bind-backs, then masks, then place, then the fresh shm.
        order = [body.index(s) for s in (
            "eve-jail-stage", "bind_back ro root dir", "mask_tree run", "place_all", "eve-jail-shm")]
        self.assertEqual(order, sorted(order))

    def test_client_is_dropped_only_after_view_and_scan(self) -> None:
        body = function_body(self.h, "v_in_jail_client")
        order = [body.index(s) for s in ("assert_in_jail", "build_view", "scan_view ||", 'run_gate "$CLIENT_SHIM"')]
        self.assertEqual(order, sorted(order))
        gate = function_body(self.h, "run_gate")
        lines = gate[gate.index("unshare "):].split("\n")
        self.assertIn("unshare --pid --fork --kill-child --mount-proc --", lines[0])
        for flag in ("--clear-groups", "--no-new-privs", '--reuid="$UID_C"', '--regid="$GID_C"'):
            self.assertIn(flag, lines[1])
        self.assertIn('bash -c "$USER_GATE"', lines[2])
        self.assertIn('4< <(printf', lines[3])
        self.assertIn("export WINEDLLOVERRIDES=$NO_BROWSER", body)
        self.assertIn('run_gate ""', function_body(self.h, "v_in_jail_verify"))
        self.assertRegex(self.h, r"(?m)^NO_BROWSER='winebrowser\.exe=d'$")

    def test_user_gate_refuses_the_root_equivalent_groups(self) -> None:
        m = re.search(r"(?m)^DENIED_GROUPS='([^']*)'$", self.h)
        self.assertIsNotNone(m)
        self.assertTrue({"docker", "libvirt", "sudo"} <= set(m.group(1).split()))
        gate = re.search(r"USER_GATE='(.*?)'\n", self.h, re.S).group(1)
        self.assertIn("id -G", gate)
        self.assertIn("/run/docker.sock", gate)
        # Ruling 15: pid check, network shares, connect gate, and only then the
        # shim -- as a child of pid 1, which stays until the namespace is empty.
        order = [gate.index(s) for s in (
            "id -G", "[[ $$ == 1 && /proc/self -ef /proc/1 ]]", "findmnt", 'python3 -I -c "$connect"',
            '"$shim" || rc=$?', "/proc/[0-9]*")]
        self.assertEqual(order, sorted(order))
        self.assertNotIn('exec "$shim"', gate)
        connect = re.search(r"CONNECT_GATE='(.*?)'\n", self.h, re.S).group(1)
        for needle in ("SOCK_STREAM", "SOCK_DGRAM", "SOCK_SEQPACKET", "os.fdopen(4"):
            self.assertIn(needle, connect)

    def test_v_client_never_execs_the_shim_directly(self) -> None:
        body = function_body(self.h, "v_client")
        self.assertIn('unshare --mount --propagation private -- "$SELF" in-jail-client', body)
        self.assertNotIn('"$CLIENT_SHIM"\n', body.replace('[[ -x $CLIENT_SHIM ]]', ''))
        self.assertNotIn("setpriv", body)

    def test_internal_verbs_check_where_they_run(self) -> None:
        for fn in ("v_in_jail_client", "v_in_jail_verify"):
            body = function_body(self.h, fn)
            self.assertIn("caller_ids", body)
            self.assertIn("assert_in_jail", body)
        self.assertIn("ip netns identify", function_body(self.h, "assert_in_jail"))

    def test_launch_env_is_read_as_the_caller(self) -> None:
        body = function_body(self.h, "read_launch_env")
        self.assertIn('as_caller cat -- "$ENV_FILE"', body)
        line = re.search(r"(?m)^as_caller\(\).*$", self.h).group(0)
        self.assertIn("--clear-groups", line)
        self.assertIn('--reuid="$UID_C"', line)

    def test_dns_writes_the_audit_log(self) -> None:
        body = function_body(self.h, "v_dns")
        self.assertIn('--log "$DNS_LOG"', body)
        self.assertRegex(self.h, r'(?m)^DNS_LOG="\$REPO/logs/eve-dns\.log"$')
        self.assertIn("</dev/null >/dev/null 2>&1 &", body, "root must not open the log")

    def test_verify_checks_tables_route_and_view(self) -> None:
        body = function_body(self.h, "v_verify")
        for needle in ("nft list table inet eve_offline_jail", "policy drop;", "192.0.2.1",
                       '"${HOST_IP%.*}.0/$PREFIX"', "in-jail-verify"):
            self.assertIn(needle, body)
        for stale in ("1.1.1.1", "192.168." "1.151", "192.168." "1.1/"):
            self.assertNotIn(stale, body)
        self.assertIn("scan_view", function_body(self.h, "v_in_jail_verify"))


class StaticNft(unittest.TestCase):
    def test_the_nft_heredocs_agree(self) -> None:
        a, b = nft_tables(read(HELPER)), nft_tables(read(SETUP))
        self.assertEqual(set(a), {"eve_offline_jail", "eve_offline_hostin", "eve_offline_out", "eve_census"})
        a.pop("eve_census")  # the census is the helper's and the rootless jail's only
        self.assertEqual(a, b)

    def test_no_log_rule_that_cannot_fire(self) -> None:
        # nf_log is suppressed outside the init netns (nf_log_all_netns=0):
        # a `log` rule in the jail's chains never wrote a line (R-CENSUS §0).
        for path in (HELPER, SETUP):
            out = nft_tables(read(path))["eve_offline_out"]
            self.assertNotIn(" log ", out, path)
            self.assertEqual(out.count("counter comment"), 2, path)

    def test_both_jail_chains_default_to_drop(self) -> None:
        out = nft_tables(read(HELPER))["eve_offline_out"]
        self.assertEqual(out.count("policy drop;"), 2)
        self.assertNotIn("policy accept", out)

    def test_ports_agree(self) -> None:
        # 8443 is back (UI-LAG): eve-server --gateway answers the client's
        # public gRPC gateway there; like the others it is on HOST_IP only.
        h = re.search(r"(?m)^GAME_PORTS='([^']*)'$", read(HELPER)).group(1)
        s = re.search(r"(?m)^GAME_PORTS=\$\{EVE_JAIL_PORTS:-([^}]*)\}$", read(SETUP)).group(1)
        self.assertEqual(h, s)
        self.assertEqual(set(h.split(",")), {"26000", "5222", "8443"})

    def test_run_client_direct_branch_is_the_helpers_view(self) -> None:
        rc = read(os.path.join(HERE, "run-client.sh"))  # IP15: no allowlist of its own, no sudo -u
        self.assertTrue(bind_backs(rc) == set() and 'source "$1" && v_in_jail_client' in rc and not re.search(r"(?m)^[^#]*sudo -u", rc))

    def test_scripts_parse(self) -> None:
        for path in (HELPER, SETUP, VERIFY):
            r = subprocess.run(["bash", "-n", path], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)


VIEW_SCRIPT = r"""
source "$GEN"
as_caller() { "$@"; }
caller_home() { printf '%s' "$TEST_HOME"; }
UID_C=0 GID_C=0
# A user namespace cannot setgroups, so --clear-groups becomes --keep-groups
# here (and only here); every other flag of the launch reaches the real
# unshare and setpriv.
drop_flags=""
unshare() {
    local a=() x
    for x; do
        if [[ $x == --clear-groups ]]; then a+=(--keep-groups)
        elif [[ $drop_flags != *" $x "* ]]; then a+=("$x"); fi
    done
    command unshare "${a[@]}"
}
# The host, as ruling 15 found it: a live socket outside every masked tree,
# and a network share under /media (ramfs stands in for cifs here).
mount -t tmpfs eve-test-vartmp /var/tmp
python3 -I -c 'import socket, sys, time; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.listen(); time.sleep(120)' /var/tmp/listener.sock &
listener=$!
for _ in $(seq 1 100); do [[ -S /var/tmp/listener.sock ]] && break; sleep 0.05; done
mount -t tmpfs eve-test-media /media
mkdir /media/nas && mount -t ramfs eve-test-nas /media/nas
NETFS="$NETFS ramfs"
# Each falsifier removes one fix.
case $VARIANT in
    nomask)  mask_sockets() { :; } ;;
    nomedia) eval "real_$(declare -f mask_tree)"
             mask_tree() { [[ $1 == media ]] || real_mask_tree "$@"; } ;;
    nopid)   drop_flags=" --pid --fork --kill-child --mount-proc " ;;
    noreap)  USER_GATE=${USER_GATE/'"$shim" || rc=$?'/'exec "$shim"'}
             [[ $USER_GATE == *'exec "$shim"'* ]] || echo "VARIANT-NOOP" ;;
    census)  census_here() { true; } ;;   # as if census-up had made census0
esac
build_view >/dev/null
printf 'LABELS %s\n' "${B_LABEL[*]}"
t() { if "${@:2}"; then echo "YES $1"; else echo "NO $1"; fi; }
t secret       test -e "$TEST_HOME/.ssh/id_test"
t prefix       test -f "$PREFIX_DIR/pfx/system.reg"
t game         test -f "$GAME_DIR/bin64/exefile.exe"
t prefix-rw    touch "$PREFIX_DIR/written-in-jail"
t resfiles     test -d "$GAME_DIR/../ResFiles"
t resfiles-rw  touch "$GAME_DIR/../ResFiles/written-in-jail"
t proton       test -x "$PROTON_DIR/proton"
t proton-rw    touch "$PROTON_DIR/written-in-jail"
t pulse        test -S "$RUNTIME/pulse/native"
t pipewire     test -S "$RUNTIME/pipewire-0"
t wayland      test -S "$RUNTIME/wayland-0"
t bus          test -e "$RUNTIME/bus"
t agent        test -e "$RUNTIME/openssh_agent"
t xauth        test -f "$RUNTIME/xauth"
t xauth-rw     touch "$RUNTIME/xauth"
t shim         test -x "$REPO/tools/jail-client.sh"
t shim-rw      touch "$REPO/tools/jail-client.sh"
t repo-other   test -e "$REPO/tools/up.sh"
t launch-env   test -f "$REPO/out/client-ci/launch-env"
t census-exec-rw  touch "$REPO/out/census/live/exec.log"
t census-marks-rw touch "$REPO/out/census/live/canary.jsonl"
t census-tun      test -e "$REPO/out/census/live/tun.jsonl"
t census-shim     test -x "$REPO/tools/census/bin/xdg-open"
t census-bin-rw   touch "$REPO/tools/census/written-in-jail"
t census-canary   test -f "$REPO/tools/census_canary.py"
t shm-empty    test -z "$(ls -A /dev/shm)"
t tmp-stranger test -e "$TMP_STRANGER"
t run-stranger test -e /run/dbus/system_bus_socket
if scan_view >/dev/null; then echo "SCAN0 pass"; else echo "SCAN0 fail"; fi
t listener     test -S /var/tmp/listener.sock
t nas          test -e /media/nas
if run_gate "$CLIENT_SHIM" 2>&1; then echo "GATE pass"; else echo "GATE fail"; fi
python3 -I -c 'import socket, sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' /tmp/stranger.sock
if scan_view >/dev/null; then echo "SCAN1 pass"; else echo "SCAN1 fail"; fi
kill "$listener"
"""

# The client shim, as the gate starts it: it reports what it sees, and leaves
# one process behind that writes into the prefix a moment after it exits.
SHIM = r"""#!/bin/bash
echo "SHIM ran"
echo "SHIM-INIT $(tr '\0' ' ' < /proc/1/cmdline)"
echo "SHIM-NPROC $(ls -d /proc/[0-9]* | wc -l)"
echo "SHIM-HOSTPID $([[ -e /proc/$HOST_PID_PROBE ]] && echo visible || echo gone)"
(sleep 1; echo late > "%s/late") </dev/null >/dev/null 2>&1 &
"""


class View(unittest.TestCase):
    """build_view in an unprivileged user + mount namespace. Invented paths."""

    @classmethod
    def setUpClass(cls) -> None:
        why = userns_ok()
        if why:
            raise unittest.SkipTest(why)

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-jail-")
        self.socks: list[socket.socket] = []

    def tearDown(self) -> None:
        for s in self.socks:
            s.close()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def sock(self, path: str) -> None:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        s = socket.socket(socket.AF_UNIX)
        s.bind(path)
        self.socks.append(s)

    def file(self, path: str, mode: int = 0o644) -> None:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write("x\n")
        os.chmod(path, mode)

    # One namespace run per (with_wayland, variant): each costs seconds (the
    # inventory walks every local filesystem, and pid 1 waits for the shim's
    # last process), and three tests read the default one.
    _runs: dict[tuple[bool, str], dict[str, str]] = {}

    def run_view(self, with_wayland: bool = True, variant: str = "") -> dict[str, str]:
        key = (with_wayland, variant)
        if key not in View._runs:
            View._runs[key] = self._run_view(with_wayland, variant)
        return View._runs[key]

    def _run_view(self, with_wayland: bool, variant: str) -> dict[str, str]:
        home = os.path.join(self.tmp, "home")
        repo = os.path.join(home, "code", "repo")
        steam = os.path.join(home, ".local", "share", "Steam")
        proton_dir = os.path.join(steam, "compatibilitytools.d", "Proton-Test")
        prefix = os.path.join(steam, "steamapps", "compatdata", "4242")
        game = os.path.join(prefix, "pfx", "drive_c", "CCP", "EVE", "copy")
        runtime = os.path.join(self.tmp, "runtime")
        os.makedirs(runtime, mode=0o700)
        self.file(os.path.join(home, ".ssh", "id_test"), 0o600)
        self.file(os.path.join(repo, "tools", "jail-client.sh"), 0o755)
        with open(os.path.join(repo, "tools", "jail-client.sh"), "w") as f:
            f.write(SHIM % prefix)
        self.file(os.path.join(repo, "tools", "up.sh"), 0o755)
        # The census files (CEN-3), present on disk in every run: only the
        # census variant may bind them back.
        self.file(os.path.join(repo, "tools", "census", "browser-shim"), 0o755)
        os.makedirs(os.path.join(repo, "tools", "census", "bin"))
        os.symlink("../browser-shim", os.path.join(repo, "tools", "census", "bin", "xdg-open"))
        for name in ("census_canary.py", "census_wire.py"):
            self.file(os.path.join(repo, "tools", name))
        for name in ("exec.log", "canary.jsonl", "tun.jsonl"):
            self.file(os.path.join(repo, "out", "census", "live", name), 0o600)
        self.file(os.path.join(proton_dir, "proton"), 0o755)
        self.file(os.path.join(prefix, "pfx", "system.reg"))
        self.file(os.path.join(game, "bin64", "exefile.exe"))
        os.makedirs(os.path.join(game, "..", "ResFiles"))
        self.file(os.path.join(runtime, "xauth"), 0o600)
        for name in ("pulse/native", "pipewire-0", "wayland-0", "bus", "openssh_agent"):
            self.sock(os.path.join(runtime, name))
        stranger = os.path.join(self.tmp, "stranger-on-host.sock")
        self.sock(stranger)
        env_lines = [
            "XDG_RUNTIME_DIR=" + runtime,
            "WAYLAND_DISPLAY=" + ("wayland-0" if with_wayland else ""),
            "DISPLAY=:987654",  # no such X socket: the x11 entry is skipped
            "XAUTHORITY=" + os.path.join(runtime, "xauth"),
            "STEAM_COMPAT_DATA_PATH=" + prefix,
            "EVE_CLIENT=" + game,
            "PROTON=" + os.path.join(proton_dir, "proton"),
            "LD_PRELOAD=/nonexistent/evil.so",
        ]
        self.file(os.path.join(repo, "out", "client-ci", "launch-env"))
        with open(os.path.join(repo, "out", "client-ci", "launch-env"), "w") as f:
            f.write("\n".join(env_lines) + "\n")
        gen = os.path.join(self.tmp, "eve-offline-jail")
        text = read(HELPER)
        for k, v in (("@REPO@", repo), ("@NS@", "eveoffline-test"),
                     ("@HOST_IP@", "10.137.0.1"), ("@NS_IP@", "10.137.0.2")):
            text = text.replace(k, v)
        with open(gen, "w") as f:
            f.write(text)
        env = dict(os.environ, GEN=gen, TEST_HOME=home, REPO=repo, PREFIX_DIR=prefix,
                   GAME_DIR=game, PROTON_DIR=proton_dir, RUNTIME=runtime, TMP_STRANGER=stranger,
                   VARIANT=variant, HOST_PID_PROBE=str(os.getpid()))
        # --net: the client runs in the jail's netns, where the host's abstract
        # socket names do not exist; the gate still probes each one.
        r = subprocess.run(["unshare", "--user", "--map-root-user", "--mount", "--net",
                            "--propagation", "private", "bash", "-c", VIEW_SCRIPT],
                           capture_output=True, text=True, env=env, timeout=SLOW)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        out: dict[str, str] = {"_stdout": r.stdout}
        for line in r.stdout.splitlines():
            word, _, rest = line.partition(" ")
            if word in ("YES", "NO"):
                out[rest] = word
            elif word.startswith(("SCAN", "GATE", "SHIM", "VARIANT")) or word == "LABELS":
                out[word] = rest
        # The left-behind process outlives the shim only if pid 1 waits for it.
        for _ in range(40):
            if os.path.exists(os.path.join(prefix, "late")):
                break
            time.sleep(0.1)
        out["late"] = "YES" if os.path.exists(os.path.join(prefix, "late")) else "NO"
        # Nothing written inside the jail's read-only entries reached the host.
        self.assertFalse(os.path.exists(os.path.join(proton_dir, "written-in-jail")))
        self.assertFalse(os.path.exists(os.path.join(game, "..", "ResFiles", "written-in-jail")))
        self.assertTrue(os.path.exists(os.path.join(prefix, "written-in-jail")))
        return out

    def test_the_view_is_the_allowlist_and_nothing_else(self) -> None:
        out = self.run_view()
        self.assertEqual(set(out["LABELS"].split()),
                         {"pulse", "pipewire", "wayland", "xauthority", "proton",
                          "prefix", "resfiles", "shim", "launch-env"})  # game: inside prefix
        want = {
            "secret": "NO", "prefix": "YES", "game": "YES", "prefix-rw": "YES",
            # O19: read-only on top of the read-write prefix.
            "resfiles": "YES", "resfiles-rw": "NO",
            "proton": "YES", "proton-rw": "NO", "pulse": "YES", "pipewire": "YES",
            "wayland": "YES", "bus": "NO", "agent": "NO", "xauth": "YES",
            "xauth-rw": "NO", "shim": "YES", "shim-rw": "NO", "repo-other": "NO",
            "launch-env": "YES", "shm-empty": "YES", "tmp-stranger": "NO",
            "run-stranger": "NO",
        }
        self.assertEqual({k: out.get(k) for k in want}, want)

    def test_no_census_file_is_bound_outside_a_census(self) -> None:
        out = self.run_view()
        self.assertFalse(CENSUS_LABELS & set(out["LABELS"].split()))
        self.assertEqual({k: out.get(k) for k in ("census-exec-rw", "census-shim", "census-canary")},
                         {"census-exec-rw": "NO", "census-shim": "NO", "census-canary": "NO"})

    def test_a_census_binds_its_logs_writable_and_its_tools_read_only(self) -> None:
        out = self.run_view(variant="census")
        self.assertTrue(CENSUS_LABELS <= set(out["LABELS"].split()), out["LABELS"])
        want = {"census-exec-rw": "YES", "census-marks-rw": "YES", "census-shim": "YES",
                "census-canary": "YES", "census-bin-rw": "NO",
                # the sink's own log stays out of the client's reach
                "census-tun": "NO", "secret": "NO"}
        self.assertEqual({k: out.get(k) for k in want}, want)
        self.assertEqual(out["SCAN0"], "pass")

    def test_scan_passes_on_the_view_and_fails_on_one_stranger(self) -> None:
        out = self.run_view()
        self.assertEqual(out["SCAN0"], "pass")
        self.assertEqual(out["SCAN1"], "fail")

    def test_an_unset_value_binds_nothing(self) -> None:
        out = self.run_view(with_wayland=False)
        self.assertNotIn("wayland", out["LABELS"].split())
        self.assertEqual(out["wayland"], "NO")

    # Ruling 15 (IP14b). The gate passes on the whole view and the client
    # starts as a child of pid 1, alone in its pid namespace; each fix removed
    # in turn makes the gate refuse, and the shim never runs.
    def test_the_gate_launches_the_client_alone_in_its_pid_namespace(self) -> None:
        out = self.run_view()
        self.assertEqual(out.get("GATE"), "pass", out["_stdout"])
        self.assertEqual(out.get("SHIM"), "ran")
        self.assertTrue(out["SHIM-INIT"].startswith("bash -c shim=$1 "))  # pid 1 is the gate
        self.assertEqual(out["SHIM-HOSTPID"], "gone")
        self.assertLess(int(out["SHIM-NPROC"]), 6)
        self.assertEqual((out["listener"], out["nas"]), ("NO", "NO"))  # covered by an empty file; masked
        self.assertIn("ok    failed to connect to every one of", out["_stdout"])
        self.assertEqual(out["late"], "YES", "pid 1 left before the client's last process")

    def test_an_uncovered_host_socket_refuses_the_launch(self) -> None:
        out = self.run_view(variant="nomask")
        self.assertEqual(out.get("GATE"), "fail", out["_stdout"])
        self.assertIn("LEAK  connected to /var/tmp/listener.sock", out["_stdout"])
        self.assertNotIn("SHIM", out)

    def test_a_visible_network_share_refuses_the_launch(self) -> None:
        out = self.run_view(variant="nomedia")
        self.assertEqual(out.get("GATE"), "fail", out["_stdout"])
        self.assertIn("network filesystem ramfs visible at /media/nas", out["_stdout"])
        self.assertNotIn("SHIM", out)

    def test_no_pid_namespace_refuses_the_launch(self) -> None:
        out = self.run_view(variant="nopid")
        self.assertEqual(out.get("GATE"), "fail", out["_stdout"])
        self.assertIn("not pid 1 of a pid namespace with its own /proc", out["_stdout"])
        self.assertNotIn("SHIM", out)

    def test_without_the_reaper_the_game_dies_with_the_shim(self) -> None:
        out = self.run_view(variant="noreap")
        self.assertNotIn("VARIANT-NOOP", out)
        self.assertEqual(out.get("SHIM"), "ran", out["_stdout"])
        self.assertEqual(out["late"], "NO")


ROOTLESS = os.path.join(HERE, "jail-rootless.sh")
SHIM_PATH = os.path.join(HERE, "jail-client.sh")
LIB = os.path.join(HERE, "prefix-lib.sh")


def loose_function_body(text: str, name: str) -> str:
    """As function_body, allowing a comment after `name() {`."""
    m = re.search(r"^%s\(\) \{[^\n]*\n(.*?)^\}\n" % re.escape(name), text, re.M | re.S)
    if not m:
        raise AssertionError("no function %s" % name)
    return m.group(1)


class OnlinePrefixStatic(unittest.TestCase):
    """O19 (ruling 14): no jail binds the online prefix (Steam appid 8500)."""

    def test_the_four_copies_name_one_id(self) -> None:
        for path, pat in ((HELPER, r"(?m)^ONLINE_PREFIX_ID=(\d+)$"),
                          (ROOTLESS, r"(?m)^ONLINE_PREFIX_ID=(\d+)$"),
                          (SHIM_PATH, r"(?m)^ONLINE_PREFIX_ID=(\d+)$"),
                          (LIB, r"(?m)^EVE_ONLINE_PREFIX_ID=(\d+)$")):
            m = re.search(pat, read(path))
            self.assertIsNotNone(m, path)
            self.assertEqual(m.group(1), "8500", path)

    def test_every_bind_back_is_checked_before_it_mounts(self) -> None:
        for path in (HELPER, ROOTLESS):
            text = read(path)
            body = loose_function_body(text, "bind_back")
            self.assertLess(body.index('online_overlap "$src"'), body.index("mount --bind"), path)
            self.assertLess(body.index("src=$VETTED"), body.index('online_overlap "$src"'), path)
            view = function_body(text, "build_view" if path == HELPER else "v_view")
            self.assertLess(view.index("ONLINE_R=$(realpath -m"), view.index("bind_back "), path)
            ov = loose_function_body(text, "online_overlap")
            for needle in ('compatdata/"$ONLINE_PREFIX_ID"/*', '$ONLINE_R == "$p"/*'):
                self.assertIn(needle, ov, path)

    def test_no_launcher_defaults_to_the_online_prefix(self) -> None:
        for name in ("run-client.sh", "client-ci.sh", "client-headless.sh", "up.sh",
                     "client2-setup.sh"):
            code = "\n".join(l for l in read(os.path.join(HERE, name)).splitlines()
                             if not l.lstrip().startswith("#"))
            self.assertNotIn("compatdata/8500", code, name)
            self.assertNotRegex(code, r"EVE_PREFIX_ID:=8500", name)
        for name in ("run-client.sh", "client-ci.sh", "client-headless.sh"):
            text = read(os.path.join(HERE, name))
            self.assertIn('source "$REPO/tools/prefix-lib.sh"', text, name)
            self.assertIn("eve_refuse_online", text, name)


# Sourced helper, everything stubbed but the bind-backs: build_view must die
# before it binds anything when one entry touches the online prefix.
ONLINE_SCRIPT = r"""
source "$GEN"
as_caller() { "$@"; }
caller_home() { printf '%s' "$TEST_HOME"; }
inventory_sockets() { SOCKETS=(); }
UID_C=0 GID_C=0
build_view
echo BUILT
"""


class OnlinePrefixView(unittest.TestCase):
    """build_view, in a user + mount namespace, with a launch-env that names
    the online prefix in each way it can: as the prefix, by holding it, and
    through a ResFiles symlink (a second client built before O19)."""

    @classmethod
    def setUpClass(cls) -> None:
        why = userns_ok()
        if why:
            raise unittest.SkipTest(why)

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-jail-o19-")
        self.home = os.path.join(self.tmp, "home")
        self.steam = os.path.join(self.home, ".local", "share", "Steam")
        self.online = os.path.join(self.steam, "steamapps", "compatdata", "8500")
        self.offline = os.path.join(self.home, ".local", "share", "eve-offline", "prefix")
        self.proton = os.path.join(self.steam, "compatibilitytools.d", "Proton-Test", "proton")
        self.repo = os.path.join(self.home, "code", "repo")
        for prefix in (self.online, self.offline):
            eve = os.path.join(prefix, "pfx", "drive_c", "CCP", "EVE")
            os.makedirs(os.path.join(eve, "tq-offline", "bin64"))
            os.makedirs(os.path.join(eve, "ResFiles"))
        os.makedirs(os.path.dirname(self.proton))
        with open(self.proton, "w") as f:
            f.write("#!/bin/sh\n")
        os.chmod(self.proton, 0o755)
        os.makedirs(os.path.join(self.repo, "tools"))
        os.makedirs(os.path.join(self.repo, "out", "client-ci"))
        with open(os.path.join(self.repo, "tools", "jail-client.sh"), "w") as f:
            f.write("#!/bin/sh\n")
        self.gen = os.path.join(self.tmp, "eve-offline-jail")
        text = read(HELPER)
        for k, v in (("@REPO@", self.repo), ("@NS@", "eveoffline-test"),
                     ("@HOST_IP@", "10.137.0.1"), ("@NS_IP@", "10.137.0.2")):
            text = text.replace(k, v)
        with open(self.gen, "w") as f:
            f.write(text)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def client(self, prefix: str) -> str:
        return os.path.join(prefix, "pfx", "drive_c", "CCP", "EVE", "tq-offline")

    def build(self, prefix: str, client: str, proton: str | None = None) -> subprocess.CompletedProcess[str]:
        with open(os.path.join(self.repo, "out", "client-ci", "launch-env"), "w") as f:
            f.write("STEAM_COMPAT_DATA_PATH=%s\nEVE_CLIENT=%s\nPROTON=%s\n"
                    % (prefix, client, proton or self.proton))
        env = dict(os.environ, GEN=self.gen, TEST_HOME=self.home)
        return subprocess.run(["unshare", "--user", "--map-root-user", "--mount",
                               "--propagation", "private", "bash", "-c", ONLINE_SCRIPT],
                              capture_output=True, text=True, env=env, timeout=SLOW)

    def assertRefused(self, r: subprocess.CompletedProcess[str], label: str) -> None:
        self.assertNotEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotIn("BUILT", r.stdout)
        self.assertIn("online EVE prefix (compatdata/8500)", r.stderr)
        self.assertIn(label + " (", r.stderr)

    def test_the_offline_prefix_builds(self) -> None:
        r = self.build(self.offline, self.client(self.offline))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("BUILT", r.stdout)
        self.assertIn("prefix", r.stdout)

    def test_the_online_prefix_is_refused(self) -> None:
        self.assertRefused(self.build(self.online, self.client(self.online)), "prefix")

    def test_a_prefix_that_holds_the_online_prefix_is_refused(self) -> None:
        self.assertRefused(self.build(self.steam, self.client(self.offline)), "prefix")

    def test_a_proton_dir_that_holds_it_is_refused(self) -> None:
        proton = os.path.join(self.steam, "proton")
        os.symlink(self.proton, proton)  # dirname is the Steam root itself
        self.assertRefused(self.build(self.offline, self.client(self.offline), proton), "proton")

    def test_a_resfiles_symlink_into_it_is_refused(self) -> None:
        second = os.path.join(self.tmp, "second")
        eve = os.path.join(second, "pfx", "drive_c", "CCP", "EVE")
        os.makedirs(os.path.join(eve, "tq-offline-2", "bin64"))
        online_res = os.path.join(self.online, "pfx", "drive_c", "CCP", "EVE", "ResFiles")
        os.symlink(online_res, os.path.join(eve, "ResFiles"))
        self.assertRefused(self.build(second, os.path.join(eve, "tq-offline-2")), "resfiles")


class ShimOnlinePrefix(unittest.TestCase):
    """jail-client.sh's own O19 check, run as a function (no namespace)."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-shim-o19-")
        body = loose_function_body(read(SHIM_PATH), "refuse_online_prefix")
        self.script = os.path.join(self.tmp, "check.sh")
        with open(self.script, "w") as f:
            f.write('gate() { echo "REFUSED $*" >&2; exit 3; }\nONLINE_PREFIX_ID=8500\n'
                    "refuse_online_prefix() {\n%s}\nrefuse_online_prefix \"$@\"\necho PASSED\n" % body)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_check(self, home: str, steam: str, prefix: str, client: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(["bash", self.script, home, steam, prefix, client],
                              capture_output=True, text=True, timeout=30)

    def test_the_offline_prefix_passes(self) -> None:
        home = os.path.join(self.tmp, "home")
        prefix = os.path.join(home, ".local", "share", "eve-offline", "prefix")
        r = self.run_check(home, os.path.join(home, ".local", "share", "Steam"), prefix,
                           os.path.join(prefix, "pfx", "drive_c", "CCP", "EVE", "tq-offline"))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("PASSED", r.stdout)

    def test_a_launch_in_the_online_prefix_is_refused(self) -> None:
        prefix = "/home/alice/.local/share/Steam/steamapps/compatdata/8500"
        r = self.run_check("", "", prefix, prefix + "/pfx/drive_c/CCP/EVE/tq-offline")
        self.assertEqual(r.returncode, 3)
        self.assertIn("online EVE prefix", r.stderr)

    def test_a_client_whose_resfiles_lead_there_is_refused(self) -> None:
        client = os.path.join(self.tmp, "p", "CCP", "EVE", "tq-offline-2")
        os.makedirs(client)
        os.symlink("/srv/SteamLibrary/steamapps/compatdata/8500/pfx/drive_c/CCP/EVE/ResFiles",
                   os.path.join(client, "..", "ResFiles"))
        r = self.run_check("", "", os.path.join(self.tmp, "p"), client)
        self.assertEqual(r.returncode, 3)
        self.assertIn("ResFiles is in the online EVE prefix", r.stderr)

    def test_a_visible_online_prefix_is_refused(self) -> None:
        home = os.path.join(self.tmp, "home")
        os.makedirs(os.path.join(home, ".local", "share", "Steam", "steamapps", "compatdata", "8500"))
        r = self.run_check(home, "", os.path.join(self.tmp, "p"), os.path.join(self.tmp, "p", "c"))
        self.assertEqual(r.returncode, 3)
        self.assertIn("is visible inside the jail", r.stderr)

    def test_a_writable_resfiles_is_refused(self) -> None:
        client = os.path.join(self.tmp, "p", "CCP", "EVE", "tq-offline")
        os.makedirs(os.path.join(client, "..", "ResFiles"))
        r = self.run_check("", "", os.path.join(self.tmp, "p"), client)
        self.assertEqual(r.returncode, 3)
        self.assertIn("ResFiles is writable", r.stderr)


STUB_IP = r"""#!/bin/bash
# Stub `ip` for test_jail.py. It never runs a probe and never sends a packet:
# every command it is asked to run inside the namespace "fails" (blocked),
# except the read-only `ip` and `nft` listings, which come from fixtures.
if [[ $1 == netns && $2 == exec ]]; then
    shift 3
    case $1 in
        nft) STUB_IN_NS=1 exec nft "${@:2}" ;;
        ip)  STUB_IN_NS=1 exec "$0" "${@:2}" ;;
        *)   exit 1 ;;
    esac
fi
args="$*"
if [[ -n ${STUB_IN_NS:-} ]]; then
    case $args in
        "-4 route show") cat "$STUB_ROUTES" ;;
        *) ;;   # no default route, no IPv6, nothing else
    esac
    exit 0
fi
case $args in
    "-4 route show default") echo "default via 192.0.2.254 dev stub0" ;;
    "-4 route get 192.0.2.1") echo "192.0.2.1 via 192.0.2.254 dev stub0 src 192.0.2.10 uid 0" ;;
esac
exit 0
"""

STUB_NFT = r"""#!/bin/bash
args="$*"
if [[ -n ${STUB_IN_NS:-} ]]; then
    case $args in
        "list chain inet eve_offline_out output"|"list chain inet eve_offline_out input")
            [[ -n $STUB_OUT_TABLE ]] || exit 1
            printf 'table inet eve_offline_out {\n\tchain x {\n\t\ttype filter hook x priority filter; policy %s;\n\t}\n}\n' "$STUB_POLICY"
            exit 0 ;;
    esac
    exit 1
fi
case $args in
    "list table inet eve_offline_jail") [[ -n $STUB_HOST_TABLE ]] ;;
    *) exit 1 ;;
esac
"""


class VerifyScript(unittest.TestCase):
    """tools/jail-verify.sh with stubbed ip/nft, in a user namespace."""

    @classmethod
    def setUpClass(cls) -> None:
        why = userns_ok()
        if why:
            raise unittest.SkipTest(why)

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-jail-verify-")
        for name, body in (("ip", STUB_IP), ("nft", STUB_NFT)):
            p = os.path.join(self.tmp, name)
            with open(p, "w") as f:
                f.write(body)
            os.chmod(p, stat.S_IRWXU)
        self.routes = os.path.join(self.tmp, "routes")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_verify(self, routes: str = "10.137.0.0/30 dev eve-ns proto kernel scope link src 10.137.0.2\n",
                   host_table: bool = True, out_table: bool = True,
                   policy: str = "drop") -> subprocess.CompletedProcess[str]:
        with open(self.routes, "w") as f:
            f.write(routes)
        env = {"PATH": self.tmp + ":/usr/bin:/bin", "STUB_ROUTES": self.routes,
               "STUB_HOST_TABLE": "1" if host_table else "", "STUB_OUT_TABLE": "1" if out_table else "",
               "STUB_POLICY": policy, "EVE_JAIL_HOST_IP": "10.137.0.1"}
        return subprocess.run(["unshare", "--user", "--map-root-user", "bash", VERIFY],
                              capture_output=True, text=True, env=env, timeout=SLOW)

    def test_a_whole_jail_passes(self) -> None:
        r = self.run_verify()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("JAIL OK", r.stdout)

    def test_fails_with_the_host_table_deleted(self) -> None:
        r = self.run_verify(host_table=False)
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertIn("eve_offline_jail", r.stdout)

    def test_fails_without_the_jail_table(self) -> None:
        self.assertNotEqual(self.run_verify(out_table=False).returncode, 0)

    def test_fails_on_policy_accept(self) -> None:
        self.assertNotEqual(self.run_verify(policy="accept").returncode, 0)

    def test_fails_on_a_second_route(self) -> None:
        r = self.run_verify(routes="0.0.0.0/1 via 10.137.0.1 dev eve-ns\n"
                                   "10.137.0.0/30 dev eve-ns proto kernel scope link src 10.137.0.2\n")
        self.assertNotEqual(r.returncode, 0, r.stdout)

    def test_no_third_party_address(self) -> None:
        code = "\n".join(l for l in read(VERIFY).splitlines() if not l.lstrip().startswith("#"))
        for addr in ("1.1.1.1", "8.8.8.8", "sentry.io/"):
            self.assertNotIn(addr, code)
        self.assertIn("route get 192.0.2.1", code)



def census_nft_text(path: str) -> str:
    m = re.search(r"^census_nft\(\) \{\n    cat <<'NFT'\n(.*?)^NFT\n\}\n", read(path), re.M | re.S)
    if not m:
        raise AssertionError("no census_nft in %s" % path)
    return m.group(1)


class CensusHelper(unittest.TestCase):
    """CEN-2: the helper's census verbs, read statically (no root, no TUN)."""

    VERBS = ("census-up", "census-down", "census-dump", "census-verify")

    def setUp(self) -> None:
        self.h = read(HELPER)

    def test_the_verbs_dispatch_and_take_no_arguments(self) -> None:
        for verb in self.VERBS:
            fn = "v_" + verb.replace("-", "_")
            self.assertRegex(self.h, r"(?m)^    %s\)\s+%s ;;$" % (re.escape(verb), fn))
            function_body(self.h, fn)
            self.assertIn(verb, re.search(r'die "usage: ([^"]*)"', self.h).group(1))
        # The installed header names them too, for the reader of the diff.
        self.assertIn("census-up | census-down | census-dump | census-verify", self.h)

    def test_the_two_census_tables_are_one(self) -> None:
        self.assertEqual(census_nft_text(HELPER), census_nft_text(ROOTLESS))
        t = census_nft_text(HELPER)
        self.assertIn("priority -10; policy accept;", t)
        self.assertNotIn("drop", t)
        for name in ("tcp4", "udp4", "tcp6", "udp6", "oth4", "oth6"):
            self.assertIn("set %s {" % name, t)
            self.assertIn("update @%s {" % name, t)

    def test_the_census_table_parses(self) -> None:
        if not shutil.which("nft"):
            self.skipTest("no nft here")
        why = userns_ok()
        if why:
            self.skipTest(why)
        # `nft -c` in a throwaway user + net namespace: checks, applies nothing.
        r = subprocess.run(["unshare", "--user", "--map-root-user", "--net", "nft", "-c", "-f", "-"],
                           input=census_nft_text(HELPER), capture_output=True, text=True, timeout=30)
        if r.returncode != 0 and "Operation not permitted" in r.stderr:
            self.skipTest("nft is not permitted in a user namespace here")
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_recorders_run_as_the_caller_never_as_root(self) -> None:
        start = loose_function_body(self.h, "census_start")
        for flag in ('--reuid="$UID_C"', '--regid="$GID_C"', "--clear-groups", "--no-new-privs"):
            self.assertIn(flag, start)
        self.assertIn('ip netns exec "$NS" setsid setpriv', start)
        self.assertIn('2>"$CENSUS_STATE/$name.err"', start)
        self.assertRegex(self.h, r"(?m)^CENSUS_STATE=/run/")
        up = function_body(self.h, "v_census_up")
        self.assertIn("caller_ids", up)
        self.assertIn('as_caller cat -- "$CENSUS_LIVE/mode"', up)
        self.assertIn("-O $1", up)  # the live directory is the caller's, checked as the caller
        build = function_body(self.h, "census_build")
        self.assertIn('tuntap add dev "$CENSUS_DEV" mode tun user "$UID_C" group "$GID_C"', build)
        self.assertLess(build.index("nft -c -f -"), build.index("census_nft | in_ns nft -f -"))

    def test_census_down_undoes_census_up(self) -> None:
        build = function_body(self.h, "census_build")
        down = function_body(self.h, "census_teardown")
        pairs = (('tuntap add dev "$CENSUS_DEV"', 'tuntap del dev "$CENSUS_DEV"'),
                 ('route add default dev "$CENSUS_DEV"', 'route del default dev "$CENSUS_DEV"'),
                 ('-6 route add default dev "$CENSUS_DEV"', '-6 route del default dev "$CENSUS_DEV"'),
                 ("census_nft | in_ns nft -f -", "nft delete table inet eve_census"),
                 ("nft insert rule inet eve_offline_out output", "nft delete rule inet eve_offline_out output handle"),
                 ("ip_unprivileged_port_start=$CENSUS_PROXY_PORT", "ip_unprivileged_port_start=$UNPRIV_PORT_DEFAULT"),
                 ("ping_group_range=0 2147483647", "ping_group_range=$PING_GROUP_DEFAULT"),
                 ("census_start sink", "census_stop_recorders"))
        for made, undone in pairs:
            self.assertIn(made, build)
            self.assertIn(undone, down)
        # Stop first (each recorder writes its stop line), then take apart.
        self.assertTrue(down.lstrip().startswith("local h\n    census_stop_recorders"))
        self.assertRegex(self.h, r"(?m)^UNPRIV_PORT_DEFAULT=1024$")
        self.assertRegex(self.h, r"(?m)^PING_GROUP_DEFAULT='1 0'$")
        stop = function_body(self.h, "census_stop_recorders")
        self.assertLess(stop.index("kill -TERM"), stop.index("kill -KILL"))
        # A failed census-up rolls back and refuses.
        up = function_body(self.h, "v_census_up")
        self.assertIn('if ! census_build "$mode"; then\n        census_teardown', up)
        self.assertIn("census_stop_recorders", function_body(self.h, "v_down"))

    def test_verify_still_means_no_route(self) -> None:
        # `verify` is unchanged: it fails while the census is up, by design.
        body = function_body(self.h, "v_verify")
        self.assertIn('"${HOST_IP%.*}.0/$PREFIX"', body)
        self.assertNotIn("census", body)
        cv = function_body(self.h, "v_census_verify")
        for needle in ("tun_flags", "0x1001", "/owner", "default dev $CENSUS_DEV", "/dev/net/tun",
                       "eve_offline_jail", "policy drop;", "127.0.0.1:$CENSUS_PROXY_PORT"):
            self.assertIn(needle, cv)


SHIM_CENSUS = r"""
gate() { echo "REFUSED $*" >&2; exit 3; }
CENSUS_DEV=census0
CENSUS_SYS=$1/sys
CENSUS_PROC=$1/proc
PATH=/usr/bin:/bin
census_setup() {
%s}
census_setup "$2" "$3" "$1/repo" "${4:-}"
echo "PASSED census=$CENSUS control=$CENSUS_CONTROL noenv=$CENSUS_NOENV"
printf 'ENV %%s\n' "${CENSUS_ENV[@]}"
"""

ROUTE_HEAD = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n"


class ShimCensus(unittest.TestCase):
    """jail-client.sh's census_setup against a fake /sys and /proc (CEN-3).
    EVE_CENSUS_CONTROL weakens a launch, so its refusals are pinned here."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-shim-census-")
        self.script = os.path.join(self.tmp, "check.sh")
        with open(self.script, "w") as f:
            f.write(SHIM_CENSUS % loose_function_body(read(SHIM_PATH), "census_setup"))
        os.makedirs(os.path.join(self.tmp, "proc"))
        repo = os.path.join(self.tmp, "repo")
        os.makedirs(os.path.join(repo, "tools", "census", "bin"))
        shim = os.path.join(repo, "tools", "census", "browser-shim")
        with open(shim, "w") as f:
            f.write("#!/bin/sh\nexit 1\n")
        os.chmod(shim, 0o755)
        os.symlink("../browser-shim", os.path.join(repo, "tools", "census", "bin", "xdg-open"))
        os.makedirs(os.path.join(repo, "out", "census", "live"))
        for name in ("exec.log", "canary.jsonl"):
            open(os.path.join(repo, "out", "census", "live", name), "w").close()

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def view(self, tun: bool = True, v4: tuple = ("census0",), v6: tuple = ("census0",),
             flags: str = "0x1001") -> None:
        dev = os.path.join(self.tmp, "sys", "census0")
        if tun:
            os.makedirs(dev, exist_ok=True)
            with open(os.path.join(dev, "tun_flags"), "w") as f:
                f.write(flags + "\n")
        with open(os.path.join(self.tmp, "proc", "route"), "w") as f:
            f.write(ROUTE_HEAD)
            f.write("eve-ns\t00008900\t00000000\t0001\t0\t0\t0\tFCFFFFFF\t0\t0\t0\n")
            for ifc in v4:
                f.write("%s\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\n" % ifc)
            f.write("lo\t00000000\t00000000\t0201\t0\t0\t0\t00000000\t0\t0\t0\n")  # a reject route
        with open(os.path.join(self.tmp, "proc", "ipv6_route"), "w") as f:
            zero = "0" * 32
            for ifc in v6:
                f.write("%s 00 %s 00 %s 00000400 00000001 00000000 00000001 %8s\n" % (zero, zero, zero, ifc))
            f.write("%s 00 %s 00 %s ffffffff 00000001 00000000 00200200       lo\n" % (zero, zero, zero))

    def run_setup(self, census: str, control: str, noenv: str = "") -> subprocess.CompletedProcess[str]:
        return subprocess.run(["bash", self.script, self.tmp, census, control, noenv],
                              capture_output=True, text=True, timeout=30)

    def assertRefused(self, r: subprocess.CompletedProcess[str], why: str) -> None:
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn(why, r.stderr)
        self.assertNotIn("PASSED", r.stdout)

    def test_no_census_is_an_ordinary_launch(self) -> None:
        r = self.run_setup("", "")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("PASSED census=0 control=0", r.stdout)

    def test_the_control_without_census_is_refused(self) -> None:
        self.view()
        self.assertRefused(self.run_setup("", "1"), "EVE_CENSUS_CONTROL=1 without EVE_CENSUS=1")

    def test_the_control_without_census0_is_refused(self) -> None:
        self.view(tun=False, v4=(), v6=())
        self.assertRefused(self.run_setup("1", "1"), "no TUN device census0")

    def test_a_persistent_tun_passes_and_a_wrong_one_does_not(self) -> None:
        # `ip tuntap add` makes census0 persistent, and the kernel shows
        # IFF_PERSIST (0x800): 0x1801. The rootless census was refused on it.
        self.view(flags="0x1801")
        r = self.run_setup("1", "")
        self.assertEqual(r.returncode, 0, r.stderr)
        for bad in ("0x1002", "0x801", "0x1", ""):  # a tap, no NO_PI, bare tun, empty
            self.view(flags=bad)
            self.assertRefused(self.run_setup("1", ""), "no TUN device census0")

    def test_census0_without_its_routes_is_refused(self) -> None:
        self.view(v4=("eth0",))
        self.assertRefused(self.run_setup("1", "1"), "not exactly census0")
        self.view(v4=("census0", "eth0"))
        self.assertRefused(self.run_setup("1", ""), "not exactly census0")
        self.view(v6=())
        self.assertRefused(self.run_setup("1", ""), "v6 'none'")

    def test_only_the_value_1_counts(self) -> None:
        self.view()
        self.assertRefused(self.run_setup("yes", ""), "EVE_CENSUS='yes'")
        self.assertRefused(self.run_setup("1", "true"), "EVE_CENSUS_CONTROL='true'")

    def test_a_real_census_passes_and_reaches_the_client(self) -> None:
        self.view()
        r = self.run_setup("1", "1")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("PASSED census=1 control=1", r.stdout)
        env = [l[4:] for l in r.stdout.splitlines() if l.startswith("ENV ")]
        repo = os.path.join(self.tmp, "repo")
        self.assertIn("PATH=%s/tools/census/bin:/usr/bin:/bin" % repo, env)
        self.assertIn("CENSUS_EXEC_LOG=%s/out/census/live/exec.log" % repo, env)
        self.assertIn("EVE_CENSUS=1", env)
        self.assertIn("EVE_CENSUS_CONTROL=1", env)
        r = self.run_setup("1", "")
        self.assertNotIn("EVE_CENSUS_CONTROL=1", r.stdout)

    def test_the_noenv_run_is_a_census_run_or_nothing(self) -> None:
        self.view()
        self.assertRefused(self.run_setup("", "", "1"), "EVE_CENSUS_NOENV=1 without EVE_CENSUS=1")
        self.assertRefused(self.run_setup("1", "", "yes"), "EVE_CENSUS_NOENV='yes'")
        self.assertRefused(self.run_setup("1", "1", "1"), "the control is already unpatched")
        r = self.run_setup("1", "", "1")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("noenv=1", r.stdout)
        # the client sees it, so each scenario's begin stage can record it
        self.assertIn("ENV EVE_CENSUS_NOENV=1", r.stdout)
        self.assertNotIn("EVE_CENSUS_NOENV", self.run_setup("1", "").stdout.replace("noenv=", ""))
        self.assertIn("noenv=0", self.run_setup("1", "").stdout)

    def test_unwritable_census_logs_are_refused(self) -> None:
        self.view()
        os.remove(os.path.join(self.tmp, "repo", "out", "census", "live", "exec.log"))
        self.assertRefused(self.run_setup("1", ""), "exec.log is not writable")


class ShimCensusStatic(unittest.TestCase):
    """What the control drops, and what census mode adds, in jail-client.sh."""

    def setUp(self) -> None:
        self.s = read(SHIM_PATH)

    def test_the_noenv_run_drops_the_env_and_keeps_the_args(self) -> None:
        self.assertIn("[EVE_CENSUS_NOENV]=1", self.s)
        self.assertIn("(( CENSUS_NOENV )) && patch_env=()", self.s)
        # after the loop that reads both env and arg lines, so args are kept
        self.assertLess(self.s.index('done <<< "$lenv"'), self.s.index("(( CENSUS_NOENV )) && patch_env=()"))

    def test_the_control_drops_exactly_the_patch(self) -> None:
        self.assertIn("[EVE_CENSUS]=1 [EVE_CENSUS_CONTROL]=1", self.s)
        self.assertIn("(( CENSUS_CONTROL )) && lenv=''", self.s)
        # the patcher still runs (it checks the args) before its lines are dropped
        self.assertLess(self.s.index("eve_patch.py\" launch-env"), self.s.index("lenv=''"))
        self.assertIn("/nocrashreportupload ]] || kept+=", self.s)
        self.assertIn("env_unset=(-u WINEDLLOVERRIDES)", self.s)
        self.assertIn('env_args+=("WINEDLLOVERRIDES=winebrowser.exe=d")', self.s)
        # census_setup runs (and refuses) before anything is launched
        self.assertLess(self.s.index('census_setup "${cfg[EVE_CENSUS]:-}"'), self.s.index('PROTON]}" run "$exe"'))

    def test_canaries_fire_before_the_client(self) -> None:
        body = self.s[self.s.index("if (( CENSUS )); then\n    # The canaries"):]
        order = [body.index(n) for n in ("census_canary.py", "run --out", "mark start wine",
                                         "wine_canary icmp ping.exe -n 1 -w 2000 192.0.2.7",
                                         "wine_canary dns ping.exe -n 1 -w 1000 canary-wine.census.invalid",
                                         "mark end wine", 'run "$exe"')]
        self.assertEqual(order, sorted(order))


class RootlessCensusStatic(unittest.TestCase):
    """jail-rootless.sh --census: the holder builds the census, the gate
    expects it, and nothing probes the network while it records."""

    def setUp(self) -> None:
        self.r = read(ROOTLESS)

    def test_the_holder_builds_it_first_and_stops_it_last(self) -> None:
        holder = loose_function_body(self.r, "v_holder")
        self.assertIn("trap 'census_stop; exit 0' TERM INT", holder)
        self.assertLess(holder.index("census_build"), holder.index('"$DNS_BIN"'))
        start = loose_function_body(self.r, "census_start")
        for flag in ("--inh-caps=-all", "--ambient-caps=-all", "--bounding-set=-all", "--no-new-privs"):
            self.assertIn(flag, start)

    def test_the_gate_skips_probes_in_census_mode(self) -> None:
        gate = loose_function_body(self.r, "v_gate")
        census = gate[gate.index('if [[ ${EVE_RL_CENSUS:-0} == 1 ]]; then'):gate.index("    else\n")]
        self.assertNotIn("/dev/tcp", census)
        self.assertNotIn("getent", census)
        self.assertIn("dev $CENSUS_DEV", census)
        self.assertIn("egress probes and lookups skipped", gate)

    def test_the_gate_reads_tun_flags_where_the_shim_does(self) -> None:
        gate = loose_function_body(self.r, "v_gate")
        self.assertIn('/sys/class/net/$CENSUS_DEV/tun_flags', gate)
        self.assertIn("~0x800", gate)

    def test_the_view_mounts_its_own_sysfs_for_a_census(self) -> None:
        # The view inherits the HOST's /sys, which has no census0; census_setup
        # reads /sys/class/net/census0, so a census view mounts a fresh sysfs.
        view = loose_function_body(self.r, "v_view")
        self.assertIn("mount -t sysfs -o ro,nosuid,nodev,noexec", view)
        self.assertLess(view.index("if (( census )); then\n        mount -t sysfs"),
                        view.index("setpriv"))

    def test_the_view_binds_the_census_as_the_helper_does(self) -> None:
        mine = {b for b in bind_backs(loose_function_body(self.r, "v_view")) if b[3].startswith("census-")}
        helper = {b for b in bind_backs(read(HELPER)) if b[3].startswith("census-")}
        self.assertEqual(mine, helper)
        self.assertEqual({b[3] for b in helper}, CENSUS_LABELS)



class RelayKeepsOnlyARegularLog(unittest.TestCase):
    """F-KIT-FIX 1: the relay copies the client's log on the host, from a
    directory the (server-driven) client writes. A symlink, a FIFO and a huge
    file must not leak, hang or fill the disk."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-keeplog-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.prefix = os.path.join(self.tmp, "prefix")
        self.logs = os.path.join(self.prefix, "pfx", "drive_c", "eve-logs")
        self.rundir = os.path.join(self.tmp, "run")
        os.makedirs(self.logs)
        os.makedirs(self.rundir)
        self.up = os.path.join(self.rundir, "up.log")
        with open(self.up, "w") as f:
            f.write("up\n")
        os.utime(self.up, (1000, 1000))
        text = read(ROOTLESS)
        m = re.search(r"^RELAY_LOG_MAX=.*?^relay_keep_client_log\(\) \{.*?^\}\n", text, re.M | re.S)
        assert m
        self.code = m.group(0)

    def keep(self, cap: int | None = None) -> subprocess.CompletedProcess[str]:
        pre = 'say() { printf "%s\\n" "$*"; }; eve_offline_prefix() { echo /nonexistent; }\n'
        if cap is not None:
            self.code = self.code.replace("RELAY_LOG_MAX=$((8 * 1024 * 1024))", "RELAY_LOG_MAX=%d" % cap)
        return subprocess.run(["bash", "-c", pre + self.code + '\nrelay_keep_client_log "$1"', "_", self.rundir],
                              capture_output=True, text=True, timeout=20,
                              env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.tmp,
                                   "EVE_PREFIX": self.prefix})

    def log(self, name: str, body: str, mtime: int = 2000) -> str:
        p = os.path.join(self.logs, name)
        with open(p, "w") as f:
            f.write(body)
        os.utime(p, (mtime, mtime))
        return p

    def kept(self) -> str | None:
        p = os.path.join(self.rundir, "client.log")
        if not os.path.exists(p):
            return None
        with open(p) as f:
            return f.read()

    def test_the_newest_regular_log_is_copied(self) -> None:
        self.log("client-a.log", "old", 1500)
        self.log("client-b.log", "new", 2500)
        self.log("client-c.log", "before the run", 500)
        r = self.keep()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.kept(), "new")
        self.assertEqual(os.stat(os.path.join(self.rundir, "client.log")).st_mode & 0o777, 0o600)

    def test_a_symlink_is_not_followed(self) -> None:
        secret = os.path.join(self.tmp, "secret")
        with open(secret, "w") as f:
            f.write("PRIVATE KEY")
        link = os.path.join(self.logs, "client-x.log")
        os.symlink(secret, link)
        r = self.keep()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIsNone(self.kept())
        self.assertIn("no client log", r.stdout)

    def test_a_fifo_does_not_hang_it(self) -> None:
        os.mkfifo(os.path.join(self.logs, "client-x.log"))
        r = self.keep()  # the 20 s timeout is the failure
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIsNone(self.kept())

    def test_an_oversized_log_is_cut(self) -> None:
        self.log("client-x.log", "A" * 5000)
        r = self.keep(cap=100)
        self.assertEqual(r.returncode, 0, r.stderr)
        body = self.kept() or ""
        self.assertTrue(body.startswith("A" * 100))
        self.assertLess(len(body), 200)
        self.assertIn("truncated", body)

    def test_an_existing_destination_is_not_overwritten_or_followed(self) -> None:
        self.log("client-x.log", "new")
        victim = os.path.join(self.tmp, "victim")
        with open(victim, "w") as f:
            f.write("keep")
        os.symlink(victim, os.path.join(self.rundir, "client.log"))
        self.keep()
        with open(victim) as f:
            self.assertEqual(f.read(), "keep")

    def test_it_runs_after_the_namespace_is_gone(self) -> None:
        body = loose_function_body(read(ROOTLESS), "v_relay")
        self.assertLess(body.index('wait "$up" 2>/dev/null || true'), body.index("relay_keep_client_log"))

class RootlessProbe(unittest.TestCase):
    """F2/F7 (issue #140): `probe` does what the jail does and names AppArmor;
    a dead relay prints its own log. `unshare` is a stub, so this runs anywhere."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-probe-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.fn = "return 0"

    def stub(self, body: str) -> None:
        # The script pins PATH, so the stub is an exported function, which
        # bash prefers to any command of that name.
        self.fn = body

    def probe(self, sysctl: str | None) -> subprocess.CompletedProcess[str]:
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
               "HOME": self.tmp, "XDG_RUNTIME_DIR": self.tmp}
        if sysctl is not None:
            path = os.path.join(self.tmp, "sysctl")
            with open(path, "w") as f:
                f.write(sysctl + "\n")
            env["EVE_RL_APPARMOR_SYSCTL"] = path
        else:
            env["EVE_RL_APPARMOR_SYSCTL"] = os.path.join(self.tmp, "absent")
        script = "unshare() { %s\n}; export -f unshare; exec bash \"$0\" probe" % self.fn
        return subprocess.run(["bash", "-c", script, ROOTLESS], capture_output=True, text=True,
                              env=env, timeout=30)

    def test_it_runs_the_loopback_and_tmpfs_steps_in_the_namespace(self) -> None:
        self.stub('echo "$@" > "%s/args"; return 0' % self.tmp)
        r = self.probe("0")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(self.tmp, "args")) as f:
            args = f.read()
        self.assertIn("ip link set lo up", args)
        self.assertIn("mount -t tmpfs", args)

    def test_failure_with_the_apparmor_sysctl_on_names_apparmor(self) -> None:
        self.stub("return 1")
        r = self.probe("1")
        self.assertEqual(r.returncode, 3)
        self.assertIn("AppArmor", r.stderr)
        self.assertIn("apparmor_restrict_unprivileged_userns", r.stderr)
        # Not settled advice: says it lasts until reboot, lowers protection, owner undecided.
        self.assertIn("until reboot", r.stderr)
        self.assertIn("lowers protection", r.stderr)
        self.assertIn("unshare only", r.stderr)

    def test_failure_without_it_does_not_blame_apparmor(self) -> None:
        self.stub("return 1")
        r = self.probe("0")
        self.assertEqual(r.returncode, 3)
        self.assertNotIn("AppArmor", r.stderr)
        self.assertIn("unavailable", r.stderr)

    def test_a_namespace_that_forms_but_cannot_set_up_says_so(self) -> None:
        # Fails when the command is `bash -c ...`, succeeds for `true`.
        self.stub('for a; do [ "$a" = true ] && return 0; done; return 1')
        r = self.probe("0")
        self.assertEqual(r.returncode, 3)
        self.assertIn("loopback or a tmpfs mount failed", r.stderr)

    def test_the_holder_tails_the_log_it_wrote_and_survives_a_missing_one(self) -> None:
        holder = loose_function_body(read(ROOTLESS), "v_holder")
        self.assertIn('tail -n 5 "$rundir/${EVE_RL_CMD_LOG:-eve-server.log}" >&2 || true', holder)
        self.assertNotIn('"$rundir/eve-server.log" >&2;', holder)


CLIENT_CI = os.path.join(HERE, "client-ci.sh")


class ClientCiCensusCheck(unittest.TestCase):
    """`client-ci.sh run X --check --census RUN`: the preflight only, in a
    throwaway repo with stub tools. No root, no sudo (a stub that refuses is
    first on PATH), nothing started."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="test-ci-census-")
        repo = self.repo = os.path.join(self.tmp, "repo")
        tools = os.path.join(repo, "tools")
        os.makedirs(os.path.join(tools, "census", "bin"))
        shutil.copy(CLIENT_CI, tools)
        shutil.copy(LIB, tools)
        shutil.copy(os.path.join(HERE, "content-lib.sh"), tools)
        stubs = {"install-jail-helper.sh": "exit 4", "jail-rootless.sh": "exit 0",
                 "census/browser-shim": "exit 1"}
        for name in ("census_sink.py", "census_proxy.py", "census_canary.py", "census_wire.py",
                     "census_report.py"):
            stubs[name] = ""
        for name, body in stubs.items():
            p = os.path.join(tools, name)
            with open(p, "w") as f:
                f.write("#!/bin/sh\n%s\n" % body)
            os.chmod(p, 0o755)
        os.symlink("../browser-shim", os.path.join(tools, "census", "bin", "xdg-open"))
        os.makedirs(os.path.join(repo, "out", "content"))
        with open(os.path.join(repo, "out", "content", "MANIFEST.tsv"), "w") as f:
            f.write("x\n")
        self.bin = os.path.join(self.tmp, "bin")
        os.makedirs(self.bin)
        with open(os.path.join(self.bin, "sudo"), "w") as f:
            f.write("#!/bin/sh\nexit 1\n")
        os.chmod(os.path.join(self.bin, "sudo"), 0o755)
        self.scen = os.path.join(self.tmp, "scenarios")
        os.makedirs(self.scen)
        with open(os.path.join(self.scen, "census_x.py"), "w") as f:
            f.write("# invented\n")
        self.prefix = os.path.join(self.tmp, "prefix")
        os.makedirs(os.path.join(self.prefix, "pfx", "drive_c", "eve-logs"))

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def check(self, *extra: str) -> subprocess.CompletedProcess[str]:
        env = {"PATH": self.bin + ":/usr/bin:/bin", "HOME": self.tmp, "DISPLAY": ":987654",
               "EVE_CI_SCENARIOS": self.scen, "EVE_PREFIX": self.prefix}
        return subprocess.run(["bash", os.path.join(self.repo, "tools", "client-ci.sh"), "run", "census_x",
                               "--check", "--inherit-display", "--no-scenario-flag", *extra],
                              capture_output=True, text=True, env=env, timeout=SLOW)

    def test_a_rootless_census_passes_the_preflight(self) -> None:
        r = self.check("--rootless", "--census", os.path.join(self.tmp, "run"), "--census-control")
        # The preflight rightly refuses while a real client runs on this host
        # (client-CI runs from main's tree now, 2026-10-27): that is the host's
        # state, not this checkout's, so the check cannot say anything then.
        if r.returncode != 0 and "a client from tq-offline is already running" in r.stdout + r.stderr:
            self.skipTest("a real client is running on this host")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("census:    %s (control, sink observe)" % os.path.join(self.tmp, "run"), r.stdout)
        self.assertIn("--check: stopping here, nothing started.", r.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "run")), "a check made the run directory")

    def test_without_the_helper_a_census_is_refused(self) -> None:
        r = self.check("--census", os.path.join(self.tmp, "run"))
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--census needs the jail helper", r.stderr)

    def test_the_control_outside_a_census_is_refused(self) -> None:
        r = self.check("--rootless", "--census-control")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--census-control without --census", r.stderr)

    def test_census_refuses_keep_second_and_a_bad_mode(self) -> None:
        r = self.check("--rootless", "--census", os.path.join(self.tmp, "run"), "--keep",
                       "--census-mode", "loud")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        for why in ("--census and --keep", "observe or handshake, not 'loud'"):
            self.assertIn(why, r.stderr)

    def test_missing_census_tools_are_named(self) -> None:
        os.remove(os.path.join(self.repo, "tools", "census_sink.py"))
        r = self.check("--rootless", "--census", os.path.join(self.tmp, "run"))
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("no tools/census_sink.py", r.stderr)


CENSUS_SH = os.path.join(HERE, "census.sh")


class CensusOrchestrator(unittest.TestCase):
    """tools/census.sh: the order of copies and the report calls (static),
    plus usage refusals that start nothing."""

    def test_usage_refusals(self) -> None:
        for args, why in ((["--copies", "a:control,b:control"], "one control"),
                          (["--copies", "../x:subject"], "bad copy"),
                          (["--mode", "loud"], "--mode is observe or handshake")):
            r = subprocess.run(["bash", CENSUS_SH, *args, "--check"], capture_output=True, text=True,
                               timeout=30, env={"PATH": "/usr/bin:/bin"})
            self.assertEqual(r.returncode, 4, r.stdout + r.stderr)
            self.assertIn(why, r.stderr)

    def test_the_control_is_judged_first_and_subjects_against_it(self) -> None:
        s = read(CENSUS_SH)
        self.assertIn("CDIR=(\"$d\" ${CDIR[@]+", s)  # the control goes to the front
        self.assertIn('rargs+=(--control "$CONTROL_REPORT")', s)
        self.assertIn('CONTROL_REPORT=$run/report.json', s)
        self.assertIn("census_report.py", s)
        self.assertIn("EOCI_CENSUS_TIMEOUT_S", s)
        self.assertIn("--census-control", s)
        self.assertIn("faults.txt", s)

    def test_a_run_with_no_client_says_so_and_reports_nothing(self) -> None:
        s = read(CENSUS_SH)
        i = s.index("NO CLIENT RAN")
        self.assertLess(i, s.index('python3 -I "$REPO/tools/census_report.py"'))
        self.assertIn("noclient.txt", s)
        ci = read(CLIENT_CI)
        end = loose_function_body(ci, "census_end")
        self.assertIn("noclient.txt", end)
        # R3's end is read before census-down and the verify after it, whose
        # sentry.io lookups are the jail's own, not a client's.
        self.assertLess(end.index("dns_end=$(stat"), end.index("if (( ROOTLESS )); then"))
        self.assertNotIn('end=$(stat -c %s "$dns")', end)

    def test_it_parses(self) -> None:
        r = subprocess.run(["bash", "-n", CENSUS_SH], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)


class PipefailGrep(unittest.TestCase):
    """No `cmd | grep -q` in a jail script that runs under `pipefail`.

    `grep -q` exits at the first match; a writer still writing then gets
    SIGPIPE, and under `pipefail` the whole pipeline counts as failed. So a
    check flips with timing. It reported a policy-drop chain as missing
    (failing closed, 2026-10-26), and the same shape in the default-route and
    IPv6 checks would have reported a real route as absent (failing open).
    The fix is `grep -q PAT <<<"$(cmd)"`: the output is captured first.
    """

    SCRIPTS = ("eve-offline-jail.in", "jail-verify.sh", "jail-setup.sh",
               "jail-client.sh", "offline-prefix.sh", "jail-drops.sh", "census.sh",
               "friend-setup.sh")

    def test_no_pipe_into_grep_q_under_pipefail(self) -> None:
        for name in self.SCRIPTS:
            path = os.path.join(HERE, name)
            if not os.path.exists(path):
                continue
            text = open(path, encoding="utf-8").read()
            if "pipefail" not in text:
                continue
            for n, line in enumerate(text.splitlines(), 1):
                code = line.split("#", 1)[0]
                self.assertNotRegex(
                    code, r"\|\s*grep\s+-q",
                    f"{name}:{n}: pipe into grep -q under pipefail: {line.strip()}")

if __name__ == "__main__":
    unittest.main()
