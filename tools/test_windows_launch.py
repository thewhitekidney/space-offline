#!/usr/bin/env python3
r"""Tests for tools/windows_launch.py (the Windows client launcher).

**Synthetic only.** Every path here is a temp directory and every byte is
invented; nothing reads a client and nothing launches one. The patcher is
replaced by a recorder, except in the launch-layer tests, which run the real
`tools/eve_patch.py launch-env` against a made-up install whose `start.ini`
names the pinned build of `tools/builds.tsv` (launch-env reads only that
line and the build's manifest). The real launch (subprocess.Popen) is never
reached, so these pass in the Linux gate container too.

Run::

    python3 -I tools/test_windows_launch.py
    tools/analysis.sh python3 -I /work/tools/test_windows_launch.py
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import windows_launch as wl  # noqa: E402

REPO = os.path.dirname(HERE)
GUIDE = os.path.join(REPO, "docs", "guides", "windows.md")

# An invented launch layer, the shape `eve_patch.py launch-env --json` prints.
BLOCK = {
    "build": 1,
    "server": "127.0.0.1",
    "env": {
        "HTTP_PROXY": "http://127.0.0.1:9",
        "HTTPS_PROXY": "http://127.0.0.1:9",
        "NO_PROXY": "127.0.0.1,localhost,127.0.0.1",
        "EVE_PUBLIC_GATEWAY_ADDRESS": "127.0.0.1:8443",
        "EVE_CLIENT_SENTRY_DSN": "http://0@127.0.0.1:9/0",
        "NOT_ON_THE_LIST": "x",
    },
    "args": ["/noCrashReportUpload", "/resfileserver=http://127.0.0.1:9/"],
    "forbid_args": ["/refreshToken", "/py", "/tools"],
}


def pinned_build() -> str:
    with open(os.path.join(HERE, "builds.tsv"), encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if len(f) > 2 and f[2] == "pinned":
                return f[0]
    raise AssertionError("tools/builds.tsv has no pinned row")


def make_install(parent: str, name: str = "tq-offline", build: str = "1") -> str:
    root = os.path.join(parent, name)
    os.makedirs(os.path.join(root, "bin64"))
    open(os.path.join(root, "bin64", "exefile.exe"), "wb").close()
    with open(os.path.join(root, "start.ini"), "w", encoding="ascii") as fh:
        fh.write("[main]\nbuild = %s\n" % build)
    return root


class Recorder:
    """Stands in for subprocess.run and subprocess.Popen; records every call."""

    def __init__(self, codes: dict[str, int] | None = None, block: dict | None = None) -> None:
        self.calls: list[list[str]] = []
        self.launched: list[tuple[list[str], dict]] = []
        self.codes = codes or {}
        self.block = BLOCK if block is None else block

    def run(self, cmd, **kw):
        self.calls.append(list(cmd))
        command = cmd[3]
        rc = self.codes.get(command, 0)
        out = json.dumps(self.block) if command == "launch-env" and rc == 0 else ""
        return subprocess.CompletedProcess(cmd, rc, out, "refused by the recorder" if rc else "")

    def popen(self, cmd, **kw):
        self.launched.append((list(cmd), kw))
        return mock.Mock(pid=4242)

    def commands(self) -> list[str]:
        return [c[3] for c in self.calls]


def quiet(fn, *a, **kw):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = fn(*a, **kw)
    return rc, out.getvalue(), err.getvalue()


class FolderName(unittest.TestCase):
    def test_the_folder_name_is_the_clients_rule(self) -> None:
        # [V utillib.GetClientUniqueFolderName]; both names seen on real machines
        self.assertEqual(wl.settings_folder_name(r"E:\dl\eve-emu\client-tq\EVE\tq-offline"),
                         "e_dl_eve-emu_client-tq_eve_tq-offline_127.0.0.1")
        self.assertEqual(wl.settings_folder_name(r"C:\Fenris Creations\EVE\tq-offline"),
                         "c_fenris_creations_eve_tq-offline_127.0.0.1")

    def test_the_server_part_follows_the_server(self) -> None:
        self.assertTrue(wl.settings_folder_name(r"C:\EVE\tq", "Tranquility").endswith("_tranquility"))
        self.assertTrue(wl.settings_folder_name(r"C:\EVE\tq", "x.servers.eveonline.com").endswith("_x"))


class PrefsTargets(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        patcher = mock.patch.dict(os.environ, {"LOCALAPPDATA": self.tmp.name})
        patcher.start()
        self.addCleanup(patcher.stop)  # LOCALAPPDATA is restored after each test
        self.base = os.path.join(self.tmp.name, "CCP", "EVE")
        self.root = r"C:\Fenris Creations\EVE\tq-offline"

    def touch(self, *parts: str) -> str:
        p = os.path.join(self.base, *parts)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "wb").close()
        return p

    def test_before_the_first_launch_the_target_is_the_servers_folder(self) -> None:
        self.assertEqual(wl.prefs_targets(self.root), [os.path.join(
            self.base, "c_fenris_creations_eve_tq-offline_127.0.0.1", "settings", "prefs.ini")])

    def test_every_folder_of_the_install_is_found_and_no_other(self) -> None:
        mine = self.touch("c_fenris_creations_eve_tq-offline_127.0.0.1", "settings", "prefs.ini")
        other = self.touch("c_fenris_creations_eve_tq-offline_10.0.0.5", "settings_default", "prefs.ini")
        self.touch("c_fenris_creations_eve_tq_tranquility", "settings", "prefs.ini")  # the live install
        self.touch("c_fenris_creations_eve_tq-offline2_127.0.0.1", "settings", "prefs.ini")  # another copy
        got = wl.prefs_targets(self.root)
        self.assertEqual(sorted(got), sorted([mine, other]))  # the server's own is not listed twice

    def test_the_servers_folder_is_added_when_only_another_server_has_one(self) -> None:
        other = self.touch("c_fenris_creations_eve_tq-offline_10.0.0.5", "settings", "prefs.ini")
        got = wl.prefs_targets(self.root)
        self.assertEqual(got[0], other)
        self.assertTrue(got[1].endswith(os.path.join("_127.0.0.1", "settings", "prefs.ini")))


class CommonIni(unittest.TestCase):
    def write(self, d: str, text: bytes) -> None:
        with open(os.path.join(d, "common.ini"), "wb") as fh:
            fh.write(text)

    def test_no_common_ini_sets_nothing(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(wl.common_ini_crypto(d))

    def test_the_value_is_parsed_not_searched_for(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            self.write(d, b"[main]\r\ncryptoPack = Placebo\r\n")
            self.assertEqual(wl.common_ini_crypto(d), "Placebo")
            # both words present, but the value is not Placebo
            self.write(d, b"; Placebo was here\r\n[main]\r\ncryptoPack = CryptoAPI\r\n")
            self.assertEqual(wl.common_ini_crypto(d), "CryptoAPI")
            self.write(d, b"[main]\r\nsomethingElse = Placebo\r\n")
            self.assertIsNone(wl.common_ini_crypto(d))

    def test_a_common_ini_that_is_not_placebo_refuses_the_launch(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            root = make_install(d)
            self.write(root, b"[main]\r\ncryptoPack = CryptoAPI\r\n")
            rec = Recorder()
            rc, _, err = quiet(wl.main, ["--root", root, "--token", "a:b"], run=rec.run, popen=rec.popen)
            self.assertEqual(rc, 2)
            self.assertIn("cryptoPack = CryptoAPI", err)
            self.assertEqual(rec.calls, [])
            self.assertEqual(rec.launched, [])


class Command(unittest.TestCase):
    def test_the_argv_takes_the_launch_layers_args_once(self) -> None:
        argv, _ = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", BLOCK)
        self.assertTrue(argv[0].endswith(os.path.join("bin64", "exefile.exe")))
        self.assertEqual(argv[1:4], ["/server:127.0.0.1", "/port:26000", "/ssoToken=me:pw"])
        self.assertEqual(argv[4:], ["/noCrashReportUpload", "/resfileserver=http://127.0.0.1:9/"])
        block = dict(BLOCK, args=["/NOCRASHREPORTUPLOAD", "/noCrashReportUpload"])
        argv, _ = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", block)
        self.assertEqual(len(argv), 5)

    def test_the_env_takes_only_the_allowed_variables(self) -> None:
        _, env = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", BLOCK)
        for k in wl.ENV_ALLOWED:
            self.assertEqual(env[k], BLOCK["env"][k])
        self.assertNotIn("NOT_ON_THE_LIST", env)

    def test_gateway_replaces_the_layers_address(self) -> None:
        _, env = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", BLOCK, "10.0.0.5:8444")
        self.assertEqual(env["EVE_PUBLIC_GATEWAY_ADDRESS"], "10.0.0.5:8444")

    def test_the_password_is_masked(self) -> None:
        self.assertEqual(wl.mask_token("me:secret"), "me:****")
        self.assertEqual(wl.mask_token("secret"), "****")
        argv, _ = wl.build_command("ROOT", "127.0.0.1", 26000, "me:secret", BLOCK)
        self.assertNotIn("secret", wl.printable(argv, "me:secret"))


class Launch(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        patcher = mock.patch.dict(os.environ, {"LOCALAPPDATA": os.path.join(self.tmp.name, "local")})
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_a_root_without_the_client_fails_even_on_a_dry_run(self) -> None:
        rec = Recorder()
        rc, _, err = quiet(wl.main, ["--root", os.path.join(self.tmp.name, "nope"), "--token", "a:b", "--dry-run"],
                           run=rec.run, popen=rec.popen)
        self.assertEqual(rc, 2)
        self.assertIn("exefile.exe", err)
        self.assertEqual(rec.calls, [])

    def test_the_live_install_is_refused_without_allow_real(self) -> None:
        root = make_install(self.tmp.name, "TQ")
        rec = Recorder()
        rc, _, err = quiet(wl.main, ["--root", root, "--token", "a:b"], run=rec.run, popen=rec.popen)
        self.assertEqual(rc, 2)
        self.assertIn("--allow-real", err)
        self.assertEqual(rec.calls, [])
        rc, _, _ = quiet(wl.main, ["--root", root, "--token", "a:b", "--allow-real"], run=rec.run, popen=rec.popen)
        self.assertEqual(rc, 0)
        apply = next(c for c in rec.calls if c[3] == "apply")
        self.assertIn("--allow-real", apply)  # the patcher's own guard is lifted too

    def test_a_launch_runs_the_patcher_then_the_client(self) -> None:
        root = make_install(self.tmp.name)
        rec = Recorder()
        rc, out, _ = quiet(wl.main, ["--root", root, "--token", "me:pw"], run=rec.run, popen=rec.popen)
        self.assertEqual(rc, 0)
        self.assertEqual(rec.commands(), ["launch-env", "apply", "verify"])
        for c in rec.calls:
            self.assertEqual(c[1:3], ["-I", wl.PATCHER])  # isolated interpreter
        env_call, apply, verify = rec.calls
        self.assertEqual(env_call[env_call.index("--server") + 1], "127.0.0.1")
        self.assertNotIn("me:pw", " ".join(env_call))  # the password is not passed to --check-args
        self.assertEqual(apply[apply.index("--tier") + 1], "P")
        self.assertIn("--prefs", apply)
        self.assertIn("--no-resfiles", verify)
        (argv, kw), = rec.launched
        self.assertEqual(kw["cwd"], root)
        self.assertIn("/resfileserver=http://127.0.0.1:9/", argv)
        self.assertEqual(kw["env"]["HTTPS_PROXY"], "http://127.0.0.1:9")
        self.assertIn("launched pid 4242", out)

    def test_a_patcher_refusal_stops_the_launch(self) -> None:
        root = make_install(self.tmp.name)
        for command, code in (("launch-env", 2), ("launch-env", 3), ("apply", 2), ("verify", 1)):
            rec = Recorder(codes={command: code})
            rc, _, _ = quiet(wl.main, ["--root", root, "--token", "a:b"], run=rec.run, popen=rec.popen)
            self.assertEqual(rc, code, command)
            self.assertEqual(rec.launched, [], command)

    def test_a_dry_run_writes_and_launches_nothing_and_masks_the_password(self) -> None:
        root = make_install(self.tmp.name)
        rec = Recorder()
        rc, out, _ = quiet(wl.main, ["--root", root, "--token", "me:secret", "--dry-run"], run=rec.run, popen=rec.popen)
        self.assertEqual(rc, 0)
        self.assertEqual(rec.commands(), ["launch-env"])  # read-only
        self.assertEqual(rec.launched, [])
        self.assertNotIn("secret", out)
        self.assertIn("/ssoToken=me:****", out)
        self.assertIn("(apply creates it)", out)


class RealLaunchLayer(unittest.TestCase):
    """The real `eve_patch.py launch-env`, on an invented install (start.ini only)."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        patcher = mock.patch.dict(os.environ, {"LOCALAPPDATA": os.path.join(self.tmp.name, "local")})
        patcher.start()
        self.addCleanup(patcher.stop)

    def dry_run(self, root: str, *extra: str) -> tuple[int, str, str]:
        return quiet(wl.main, ["--root", root, "--token", "me:secret", "--dry-run", *extra])

    def test_the_gateway_follows_the_server(self) -> None:
        root = make_install(self.tmp.name, build=pinned_build())
        rc, out, err = self.dry_run(root, "--server", "10.0.0.5")
        self.assertEqual(rc, 0, err)
        self.assertIn("env EVE_PUBLIC_GATEWAY_ADDRESS=10.0.0.5:8443", out)
        self.assertRegex(out, r"env NO_PROXY=10\.0\.0\.5,")
        self.assertRegex(out, r"env HTTPS?_PROXY=http://127\.0\.0\.1:9")
        self.assertIn("/noCrashReportUpload", out)
        self.assertIn("/resfileserver=", out)
        self.assertNotIn("secret", out)

    def test_a_build_with_no_manifest_is_refused(self) -> None:
        root = make_install(self.tmp.name, build="1")
        rc, _, err = self.dry_run(root)
        self.assertEqual(rc, 2)
        self.assertIn("no manifest", err)


class Guide(unittest.TestCase):
    def test_every_flag_is_in_the_guides_table(self) -> None:
        with open(GUIDE, encoding="utf-8") as fh:
            guide = fh.read()
        table = set(re.findall(r"^\| `(--[a-z-]+)", guide, re.M))
        flags = {o for a in wl.parser()._actions for o in a.option_strings if o.startswith("--")}
        flags.discard("--help")
        self.assertTrue(flags)
        self.assertEqual(table, flags)


if __name__ == "__main__":
    unittest.main()
