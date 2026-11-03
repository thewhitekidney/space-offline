"""The friends' tunnel (IP22-a, O18; RR1 H4): tools/tunnel.sh's generated
sshd_config, its key handling and its container flags, and
containers/tunnel.Dockerfile's pin. Stdlib only; no docker, no network, no
ssh binary: `tunnel.sh config` and `add-key` / `remove-key` run without
docker, against a temporary EVE_TUNNEL_DIR.

Every directive is asserted on its own (a subTest each), so dropping or
changing any one line of the config fails this test.

Run::

    python3 -I tools/test_tunnel_config.py
"""

from __future__ import annotations

import base64
import os
import re
import stat
import struct
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TUNNEL = os.path.join(HERE, "tunnel.sh")
DOCKERFILE = os.path.join(HERE, "..", "containers", "tunnel.Dockerfile")

PERMIT = ["127.0.0.1:26001", "127.0.0.1:5223", "127.0.0.1:8443"]

# The whole config, directive by directive. Port and ListenAddress come from
# the arguments the test passes (invented, not tunnel.sh's defaults).
PORT = "47123"
LISTEN = "127.0.0.1"
EXPECTED = {
    "Port": PORT,
    "ListenAddress": LISTEN,
    "HostKey": "/keys/host/ssh_host_ed25519_key",
    "PidFile": "none",
    "AllowUsers": "eve",
    "PermitRootLogin": "no",
    "AuthenticationMethods": "publickey",
    "PubkeyAuthentication": "yes",
    "AuthorizedKeysFile": "/keys/authorized_keys",
    "AuthorizedPrincipalsFile": "none",
    "AuthorizedKeysCommand": "none",
    "PasswordAuthentication": "no",
    "KbdInteractiveAuthentication": "no",
    "PermitEmptyPasswords": "no",
    "HostbasedAuthentication": "no",
    "StrictModes": "yes",
    "PermitTTY": "no",
    "X11Forwarding": "no",
    "AllowAgentForwarding": "no",
    "AllowStreamLocalForwarding": "no",
    "AllowTcpForwarding": "local",
    "PermitOpen": " ".join(PERMIT),
    "PermitListen": "none",
    "GatewayPorts": "no",
    "PermitTunnel": "no",
    "PermitUserEnvironment": "no",
    "PermitUserRC": "no",
    "ForceCommand": "/bin/false",
    "MaxSessions": "0",
    "MaxAuthTries": "3",
    "MaxStartups": "4:50:16",
    "LoginGraceTime": "20",
    "ClientAliveInterval": "30",
    "ClientAliveCountMax": "4",
    "TCPKeepAlive": "no",
    "UseDNS": "no",
    "Compression": "no",
    "LogLevel": "VERBOSE",
}


def run(*args: str, env_dir: str | None = None):
    env = dict(os.environ)
    if env_dir is not None:
        env["EVE_TUNNEL_DIR"] = env_dir
    return subprocess.run(["bash", TUNNEL, *args], capture_output=True, text=True, env=env)


def parse(text: str) -> list[tuple[str, str]]:
    out = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, _, value = line.partition(" ")
        out.append((key, value.strip()))
    return out


def fake_pub(keytype: str = "ssh-ed25519", blob_type: str | None = None, comment: str = "x@y") -> str:
    """An invented public key line: the wire blob is uint32 len + type + 32 bytes."""
    t = (blob_type or keytype).encode()
    blob = struct.pack(">I", len(t)) + t + struct.pack(">I", 32) + bytes(range(32))
    return f"{keytype} {base64.b64encode(blob).decode()} {comment}\n"


class Config(unittest.TestCase):
    def setUp(self) -> None:
        r = run("config", "--port", PORT, "--listen", LISTEN)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.pairs = parse(r.stdout)
        self.conf = dict(self.pairs)

    def test_each_directive(self) -> None:
        for key, want in EXPECTED.items():
            with self.subTest(directive=key):
                self.assertIn(key, self.conf, f"{key} missing from the sshd_config")
                self.assertEqual(self.conf[key], want)

    def test_nothing_else(self) -> None:
        # sshd uses the FIRST occurrence, so a duplicate could shadow a line.
        keys = [k for k, _ in self.pairs]
        self.assertEqual(len(keys), len(set(keys)), "duplicate directive")
        self.assertEqual(set(keys), set(EXPECTED), "a directive the test does not know")
        self.assertNotIn("Subsystem", keys)  # no sftp

    def test_permit_open_is_exactly_the_two_ports(self) -> None:
        self.assertEqual(self.conf["PermitOpen"].split(), PERMIT)
        self.assertNotIn("any", self.conf["PermitOpen"])

    def test_defaults_are_a_high_port_on_all_addresses(self) -> None:
        conf = dict(parse(run("config").stdout))
        self.assertGreaterEqual(int(conf["Port"]), 1024)
        self.assertNotIn(conf["Port"], {"22", "26000", "26001", "5222", "5223"})

    def test_bad_ports_and_addresses_refused(self) -> None:
        for args in (["--port", "22"], ["--port", "26001"], ["--port", "5223"],
                     ["--port", "26000"], ["--port", "5222"], ["--port", "70000"],
                     ["--port", "x"], ["--listen", "evil host"], ["--bogus"]):
            with self.subTest(args=args):
                r = run("config", *args)
                self.assertEqual(r.returncode, 2, r.stdout)
                self.assertEqual(r.stdout, "")


class Container(unittest.TestCase):
    def setUp(self) -> None:
        with open(TUNNEL) as f:
            self.script = f.read()
        with open(DOCKERFILE) as f:
            self.dockerfile = f.read()

    def test_base_image_is_pinned_by_digest(self) -> None:
        froms = re.findall(r"^FROM\s+(\S+)", self.dockerfile, re.M)
        self.assertEqual(len(froms), 1)
        self.assertRegex(froms[0], r"^[a-z0-9./-]+@sha256:[0-9a-f]{64}$")

    def test_image_installs_sshd_only(self) -> None:
        adds = re.findall(r"apk add\s+--no-cache\s+([^\\\n&]+)", self.dockerfile)
        self.assertEqual([a.split() for a in adds], [["openssh-server"]])

    def test_image_runs_as_the_unprivileged_user(self) -> None:
        users = re.findall(r"^USER\s+(\S+)", self.dockerfile, re.M)
        self.assertEqual(users, ["eve"])
        self.assertIn('test "$UID" != 0', self.dockerfile)

    def test_run_flags(self) -> None:
        body = self.script[self.script.index('docker run -d --name "$NAME"'):]
        body = body[:body.index("|| die")]
        for flag in ("--network host", "--cap-drop ALL", "--security-opt no-new-privileges",
                     "--read-only", '--user "$(id -u):$(id -g)"',
                     "sshd_config:/etc/eve-tunnel/sshd_config:ro", '"$DIR/keys:/keys:ro"',
                     "/usr/sbin/sshd -D -e -f /etc/eve-tunnel/sshd_config"):
            with self.subTest(flag=flag):
                self.assertIn(flag, body)

    def test_never_published_privileged_or_gateway(self) -> None:
        runs = re.findall(r"docker run (.*?)\|\| die", self.script, re.S)
        self.assertGreaterEqual(len(runs), 3)
        for bad in (" -p ", "--publish", "-P ", "--privileged", "--cap-add"):
            for i, run_ in enumerate(runs):
                with self.subTest(bad=bad, run=i):
                    self.assertFalse(bad in run_, f"{bad!r} in a docker run")
        for bad in ("GatewayPorts yes", "GatewayPorts clientspecified", "PermitOpen any"):
            with self.subTest(bad=bad):
                self.assertFalse(bad in self.script, bad)


class Keys(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = os.path.join(self.tmp.name, "tunnel")
        self.pub = os.path.join(self.tmp.name, "ann.pub")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, text: str) -> str:
        with open(self.pub, "w") as f:
            f.write(text)
        return self.pub

    def ak(self) -> str:
        with open(os.path.join(self.dir, "keys", "authorized_keys")) as f:
            return f.read()

    def test_add_key_restricts_the_line(self) -> None:
        r = run("add-key", self.write(fake_pub()), "ann", env_dir=self.dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        lines = self.ak().splitlines()
        self.assertEqual(len(lines), 1)
        opts, keytype, _blob, comment = lines[0].split(" ")
        self.assertEqual(opts, 'restrict,port-forwarding,'
                               'permitopen="127.0.0.1:26001",permitopen="127.0.0.1:5223",'
                               'permitopen="127.0.0.1:8443"')
        self.assertEqual(keytype, "ssh-ed25519")
        self.assertEqual(comment, "eve-offline:ann")
        mode = lambda p: stat.S_IMODE(os.stat(p).st_mode)  # noqa: E731
        self.assertEqual(mode(self.dir), 0o700)
        self.assertEqual(mode(os.path.join(self.dir, "keys", "authorized_keys")), 0o600)

    def test_remove_key(self) -> None:
        run("add-key", self.write(fake_pub()), "ann", env_dir=self.dir)
        run("add-key", self.write(fake_pub("ecdsa-sha2-nistp256")), "bob", env_dir=self.dir)
        r = run("remove-key", "ann", env_dir=self.dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("eve-offline:ann", self.ak())
        self.assertIn("eve-offline:bob", self.ak())
        self.assertNotEqual(run("remove-key", "ann", env_dir=self.dir).returncode, 0)

    def test_refusals(self) -> None:
        cases = {
            "private key": "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n",
            "options prefix": 'command="/bin/sh" ' + fake_pub(),
            "permitopen prefix": 'permitopen="127.0.0.1:22" ' + fake_pub(),
            "two keys": fake_pub() + fake_pub(),
            "empty": "",
            "dsa": fake_pub("ssh-dss"),
            "blob disagrees": fake_pub("ssh-ed25519", blob_type="ssh-rsa"),
        }
        for what, text in cases.items():
            with self.subTest(what=what):
                r = run("add-key", self.write(text), "ann", env_dir=self.dir)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                path = os.path.join(self.dir, "keys", "authorized_keys")
                self.assertTrue(not os.path.exists(path) or self.ak() == "")

    def test_duplicate_and_bad_names_refused(self) -> None:
        self.assertEqual(run("add-key", self.write(fake_pub()), "ann", env_dir=self.dir).returncode, 0)
        self.assertEqual(run("add-key", self.pub, "ann2", env_dir=self.dir).returncode, 2)
        other = fake_pub("ecdsa-sha2-nistp256")
        self.assertEqual(run("add-key", self.write(other), "ann", env_dir=self.dir).returncode, 2)
        self.assertEqual(run("add-key", self.pub, "a b", env_dir=self.dir).returncode, 2)
        self.assertEqual(run("add-key", self.pub, 'x"y', env_dir=self.dir).returncode, 2)
        self.assertEqual(len(self.ak().splitlines()), 1)


class Script(unittest.TestCase):
    def test_parses(self) -> None:
        self.assertEqual(subprocess.run(["bash", "-n", TUNNEL]).returncode, 0)

    def test_unknown_verb_refused(self) -> None:
        self.assertEqual(run("open-everything").returncode, 2)


if __name__ == "__main__":
    unittest.main()
