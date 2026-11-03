"""The gate's hook for tools/jail-relay.py's own self-test (IP22-b).

The relay carries a friend's game and chat bytes between the rootless
jail's 10.137.0.1 and the SSH tunnel's unix sockets. Its self-test (two
socketpairs, bytes both ways, EOF passed through) lives in the script, so a
friend's kit can run it too; this file only makes the gate run it.
"""

import pathlib
import subprocess
import sys
import unittest

RELAY = pathlib.Path(__file__).resolve().parent / "jail-relay.py"


class JailRelaySelftest(unittest.TestCase):
    def test_the_relays_own_selftest_passes(self) -> None:
        r = subprocess.run(
            [sys.executable, "-I", str(RELAY), "selftest"],
            capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


if __name__ == "__main__":
    unittest.main()
