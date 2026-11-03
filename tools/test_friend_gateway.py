"""The friends' gateway path (lane friend-gateway, NJ-4): the tunnel forwards
127.0.0.1:8443 and no more, the friend's end makes a third unix socket for it,
and `serve.sh --friends` starts the gateway. Stdlib only; no docker, no ssh
connection: `friend-tunnel.sh --dry-run` prints the ssh line.

Run::

    python3 -I tools/test_friend_gateway.py
"""

from __future__ import annotations

import os
import re
import shlex
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TUNNEL = os.path.join(HERE, "tunnel.sh")
FRIEND = os.path.join(HERE, "friend-tunnel.sh")
SERVE = os.path.join(HERE, "serve.sh")


def read(p: str) -> str:
    with open(p) as f:
        return f.read()


class TunnelPermitsTheGateway(unittest.TestCase):
    def test_permit_open_is_exactly_game_chat_gateway_on_loopback(self) -> None:
        m = re.search(r"^PERMIT_OPEN=\((.*)\)$", read(TUNNEL), re.M)
        self.assertIsNotNone(m)
        self.assertEqual(m.group(1).split(),
                         ["127.0.0.1:26001", "127.0.0.1:5223", "127.0.0.1:8443"])


class FriendEnd(unittest.TestCase):
    def dry(self) -> list[str]:
        with tempfile.TemporaryDirectory() as rt, tempfile.TemporaryDirectory() as home:
            os.chmod(rt, 0o700)
            env = {"PATH": os.environ["PATH"], "HOME": home, "XDG_RUNTIME_DIR": rt}
            r = subprocess.run(["bash", FRIEND, "owner.example", "26022", "--dry-run"],
                               env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            return [*shlex.split(r.stdout), rt]

    def test_three_unix_socket_forwards_and_no_tcp_listener(self) -> None:
        argv = self.dry()
        rt = argv[-1]
        fwd = [argv[i + 1] for i, a in enumerate(argv) if a == "-L"]
        self.assertEqual(fwd, [f"{rt}/eve-game.sock:127.0.0.1:26001",
                               f"{rt}/eve-chat.sock:127.0.0.1:5223",
                               f"{rt}/eve-gw.sock:127.0.0.1:8443"])
        for f in fwd:
            self.assertTrue(f.startswith("/"), f"a TCP listener on the friend's machine: {f}")
        self.assertNotIn("-R", argv)
        self.assertNotIn("-D", argv)

    def test_security_options_are_unchanged(self) -> None:
        argv = self.dry()
        for o in ("StrictHostKeyChecking=yes", "GatewayPorts=no", "ExitOnForwardFailure=yes",
                  "ClearAllForwardings=no", "ForwardAgent=no", "UpdateHostKeys=no",
                  "GlobalKnownHostsFile=/dev/null", "PasswordAuthentication=no"):
            self.assertIn(o, argv)
        self.assertEqual(argv[0:2], ["ssh", "-F"])
        self.assertEqual(argv[2], "none")

    def test_cleanup_removes_all_three_sockets(self) -> None:
        m = re.search(r"^cleanup\(\) \{ rm -f (.*); \}", read(FRIEND), re.M)
        self.assertIsNotNone(m)
        self.assertEqual(m.group(1).split(), ['"$GAME_SOCK"', '"$CHAT_SOCK"', '"$GW_SOCK"'])


class ServeStartsTheGateway(unittest.TestCase):
    def test_friends_adds_gateway_and_only_then(self) -> None:
        self.assertRegex(read(SERVE), r"\(\( FRIENDS \)\) && ARGS\+=\(--gateway\)")
        self.assertEqual(read(SERVE).count("ARGS+=(--gateway)"), 1)

    def test_advertise_defaults_to_the_jail_host_and_is_overridable(self) -> None:
        s = read(SERVE)
        self.assertIn("FRIEND_ADVERTISE=${EVE_FRIEND_ADVERTISE:-$HOST_IP}", s)
        self.assertIn("advertise=$FRIEND_ADVERTISE", s)
        self.assertNotIn("advertise=$HOST_IP", s)

    def test_the_friends_listener_itself_is_still_loopback(self) -> None:
        self.assertIn('FRIENDS_LISTENER="127.0.0.1:26001,ceiling=player,cap=16,chat=127.0.0.1:5223,',
                      read(SERVE))


if __name__ == "__main__":
    unittest.main()
