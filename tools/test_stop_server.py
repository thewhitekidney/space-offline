"""Tests for `tools/stop-server.sh`, the clean stop the launch scripts use
(lane shutdown): touch eve-server's stop file, wait, SIGTERM only as a
fallback.

The "server" here is a stub bash loop, never eve-server: one that exits 0
when its stop file appears (what eve-server does after writing its world),
and one that ignores it (what a wedged server would do).

Run::

    python3 -I tools/test_stop_server.py
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "stop-server.sh")

# $1 = the stop file, $2 = obey (1) or ignore (0). The parent script starts
# it in the background, as up.sh, serve.sh and soak.sh start eve-server.
DRIVER = r"""
set -u
source "$HELPER"
file=$1 obey=$2
if (( obey )); then
    ( while [[ ! -e $file ]]; do sleep 0.05; done; exit 0 ) &
else
    ( trap 'exit 143' TERM; while :; do sleep 0.05; done ) &
fi
pid=$!
if stop_server "$pid" "$file" 2; then how=exit; else how=killed; fi
wait "$pid"; echo "$how $?"
"""


def drive(stop_file: str, obey: bool) -> str:
    r = subprocess.run(
        ["bash", "-c", DRIVER, "driver", stop_file, "1" if obey else "0"],
        capture_output=True, text=True, timeout=30,
        env={"HELPER": HELPER, "PATH": os.environ.get("PATH", "/usr/bin:/bin")},
    )
    if r.returncode != 0:
        raise AssertionError(r.stderr)
    return r.stdout.strip()


class StopServer(unittest.TestCase):
    def test_it_parses(self) -> None:
        r = subprocess.run(["bash", "-n", HELPER], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_a_server_that_obeys_its_stop_file_exits_zero(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            f = os.path.join(d, "world.dat.stop")
            self.assertEqual(drive(f, True), "exit 0")
            self.assertTrue(os.path.exists(f), "the helper touched it")

    def test_one_that_ignores_it_gets_sigterm_after_the_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            f = os.path.join(d, "world.dat.stop")
            self.assertEqual(drive(f, False), "killed 143")

    def test_the_launchers_use_it(self) -> None:
        for name in ("up.sh", "serve.sh", "soak.sh"):
            with open(os.path.join(HERE, name)) as fh:
                text = fh.read()
            self.assertIn("stop-server.sh", text, name)
            self.assertIn("stop_server ", text, name)


if __name__ == "__main__":
    unittest.main()
