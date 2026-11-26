#!/usr/bin/env python3
r"""Launch the native Windows EVE client against a space-offline server (stdlib only).

For the same-PC setup in docs/guides/windows.md: the server runs in WSL2 and
the native Windows client reaches it over `127.0.0.1`.

The launcher writes no client file itself. What it changes and checks goes
through `tools/eve_patch.py`, the shared patcher, so the Windows launch gets
the same closures as the Linux one:

  * `eve_patch.py launch-env --client ROOT --server S --json` gives the
    environment (the dead loopback proxy with NO_PROXY, the gateway address,
    a loopback Sentry DSN), the arguments the client must get
    (`/noCrashReportUpload`, `/resfileserver=...`) and the ones it must never
    get. It refuses a build with no manifest. tools/jail-client.sh reads the
    same layer, through the same allow-list of variables.
  * `eve_patch.py apply --tier P` sets the prefs.ini keys (chat without TLS,
    no crash upload, a dead Sentry DSN). It is given the prefs.ini files of
    this install, found as `<folder>_*\settings*\prefs.ini`, and the one the
    client uses for this server, which does not exist before the first launch.
  * `eve_patch.py verify --no-resfiles` must pass, so an unpatched copy is
    never launched.

Then it runs `bin64\exefile.exe` with the server, port and sso token, with
its working directory at the install root:

    py -3 -I tools\windows_launch.py --root "C:\Fenris Creations\EVE\tq-offline" --token me:mypass
    py -3 -I tools\windows_launch.py --root "C:\Fenris Creations\EVE\tq-offline" --token me:mypass --dry-run

It refuses an install named `tq` (the one you play on) unless you pass
`--allow-real`, and a `common.ini` whose `cryptoPack` is not `Placebo` (it
would win over the patcher's `start.ini`). `--dry-run` asks the patcher for the launch layer, writes
nothing and launches nothing, and prints what it would run with the password
masked.

This is the Windows counterpart of tools/run-client.sh. It is NOT a jail: a
native Windows client has no network isolation. Read the no-jail section of
docs/guides/windows.md before you run a server you do not own.

Exit codes: 0 ok; 1 the copy is not (fully) patched; 2 refused (no client,
the live install, no manifest for the build, a patcher refusal); 3 a
forbidden client argument. The patcher's own code is passed through.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import glob
import json
import ntpath
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PATCHER = os.path.join(HERE, "eve_patch.py")

# The variables taken from the launch layer: the same fixed list
# tools/jail-client.sh honours. Anything else in the layer's output is ignored.
ENV_ALLOWED = (
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "EVE_PUBLIC_GATEWAY_ADDRESS",
    "EVE_CLIENT_SENTRY_DSN",
)


class Refused(Exception):
    """A refusal before anything was launched. Carries the exit code."""

    def __init__(self, msg: str, code: int = 2) -> None:
        super().__init__(msg)
        self.code = code


def is_live_install(root: str) -> bool:
    """True for an install directory named `tq`, the one the official launcher
    keeps up to date (eve_patch.py refuses it for the same reason)."""
    parts = [p for p in re.split(r"[\\/]+", root) if p]
    return bool(parts) and parts[-1].lower() == "tq"


def install_folder_name(root: str) -> str:
    r"""The install part of the client's per-install, per-server folder under
    `%LOCALAPPDATA%\CCP\EVE`, the way the client builds it
    [V utillib.GetClientUniqueFolderName, build 3724492]: the install path,
    normalised and lower-cased, with `/`, `\` and spaces turned into `_` and
    `:` dropped. `C:\Fenris Creations\EVE\tq-offline` gives
    `c_fenris_creations_eve_tq-offline`.
    """
    path = ntpath.normpath(root).lower()
    for sep in ("/", "\\", " "):
        path = path.replace(sep, "_")
    return path.replace(":", "")


def server_folder_part(server: str) -> str:
    """The server part of that folder: the name lower-cased, spaces turned into
    `_`; a `*.servers.eveonline.com` name keeps only its first label."""
    name = server.lower().replace(" ", "_")
    if name.endswith(".servers.eveonline.com"):
        name = name.split(".")[0]
    return name


def settings_folder_name(root: str, server: str = "127.0.0.1") -> str:
    r"""The whole folder name: `C:\Fenris Creations\EVE\tq-offline` against
    127.0.0.1 gives `c_fenris_creations_eve_tq-offline_127.0.0.1`."""
    return install_folder_name(root) + "_" + server_folder_part(server)


def ccp_eve_dir() -> str:
    local = os.environ.get("LOCALAPPDATA") or os.path.join(os.path.expanduser("~"), "AppData", "Local")
    return os.path.join(local, "CCP", "EVE")


def _abs_root(root: str) -> str:
    return os.path.abspath(root) if os.name == "nt" else root


def prefs_targets(root: str, server: str = "127.0.0.1") -> list[str]:
    r"""The prefs.ini files the patcher is given before a launch.

    Every `<folder>_*\settings*\prefs.ini` of this install, as eve_patch.py
    and run-client.sh find them, then the file the client reads for this
    server (`<folder>_<server>\settings\prefs.ini`) if it is not among them.
    That last one does not exist before the first launch; the patcher creates
    it with its keys.
    """
    base = ccp_eve_dir()
    folder = install_folder_name(_abs_root(root))
    pattern = os.path.join(base, glob.escape(folder) + "_*", "settings*", "prefs.ini")
    found = sorted(glob.glob(pattern))
    want = os.path.join(base, folder + "_" + server_folder_part(server), "settings", "prefs.ini")
    if os.path.normcase(want) not in {os.path.normcase(p) for p in found}:
        found.append(want)
    return found


def mask_token(token: str) -> str:
    """`user:password` with the password hidden, for anything printed."""
    user, sep, _ = token.partition(":")
    return user + ":****" if sep else "****"


def patcher_argv(command: str, root: str, *extra: str) -> list[str]:
    """The isolated-interpreter command line for one eve_patch.py command."""
    return [sys.executable, "-I", PATCHER, command, "--client", root, *extra]


def launch_block(root: str, server: str, check_args: list[str], run=subprocess.run) -> dict:
    """The launch layer for this install and server, from
    `eve_patch.py launch-env --json`. Its `--check-args` refuses a forbidden
    argument (exit 3); a build with no manifest is refused (exit 2)."""
    cmd = patcher_argv("launch-env", root, "--server", server, "--json",
                       "--check-args", " ".join(check_args))
    res = run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        msg = (res.stderr or res.stdout or "").strip() or "eve_patch.py launch-env failed"
        raise Refused(msg, res.returncode)
    try:
        block = json.loads(res.stdout)
    except ValueError:
        raise Refused("eve_patch.py launch-env printed no JSON") from None
    for key in ("env", "args", "forbid_args"):
        if key not in block:
            raise Refused("eve_patch.py launch-env: no %r in its output" % key)
    return block


def build_command(
    root: str, server: str, port: int, token: str, block: dict, gateway: str | None = None
) -> tuple[list[str], dict[str, str]]:
    """The (argv, env) for the client. Pure; launches nothing.

    The env is this process's own, with the launch layer's allowed variables
    set over it; `gateway`, when given, replaces the layer's gateway address.
    The layer's arguments are added unless one is already there.
    """
    exe = os.path.join(root, "bin64", "exefile.exe")
    argv = [exe, "/server:%s" % server, "/port:%d" % int(port), "/ssoToken=%s" % token]
    have = {a.lower() for a in argv[1:]}
    for a in block.get("args", []):
        if a.lower() not in have:
            argv.append(a)
            have.add(a.lower())
    env = dict(os.environ)
    for k, v in block.get("env", {}).items():
        if k in ENV_ALLOWED and v:
            env[k] = v
    if gateway:
        env["EVE_PUBLIC_GATEWAY_ADDRESS"] = gateway
    return argv, env


def printable(argv: list[str], token: str) -> str:
    """The command line as text (Windows quoting), the password masked."""
    secret = "/ssoToken=" + token
    return subprocess.list2cmdline(["/ssoToken=" + mask_token(token) if a == secret else a for a in argv])


def common_ini_crypto(root: str) -> str | None:
    """The `cryptoPack` value a `common.ini` at the install root sets, or None.

    The client reads `common.ini` over `start.ini` [V: builds 3707965 and
    3724492 logged in with a stock start.ini and only common.ini setting
    Placebo; docs/guides/windows.md, "What is verified"], so a value here
    would win over the patcher's start.ini edit (P0).
    Every `cryptoPack = <value>` line counts, in any section; comments and
    other keys are ignored. The last one is returned.
    """
    path = os.path.join(root, "common.ini")
    try:
        with open(path, "rb") as fh:
            text = fh.read().decode("latin-1")
    except OSError:
        return None
    value = None
    for line in text.splitlines():
        s = line.strip()
        if not s or s[0] in ";#[":
            continue
        k, sep, v = s.partition("=")
        if sep and k.strip().lower() == "cryptopack":
            value = v.strip()
    return value


def parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(description="Launch the Windows EVE client against space-offline.")
    ap.add_argument("--root", required=True, help=r"the offline client install root (holds bin64\exefile.exe)")
    ap.add_argument("--token", required=True, help="ssoToken as user:password (a signup=on listener makes the account)")
    ap.add_argument("--server", default="127.0.0.1", help="server address (default %(default)s; also names the prefs folder)")
    ap.add_argument("--port", type=int, default=26000, help="server port (default %(default)s)")
    ap.add_argument("--gateway", help="EVE_PUBLIC_GATEWAY_ADDRESS (default: the launch layer's, <server>:8443)")
    ap.add_argument("--prefs", action="append", help="a prefs.ini path for the patcher (repeatable; default: found from --root)")
    ap.add_argument("--tier", help="tiers verify checks (default: the patcher's)")
    ap.add_argument("--allow-real", action="store_true", help="allow an install named tq (the one you play on)")
    ap.add_argument("--dry-run", action="store_true", help="write nothing, launch nothing; print what would run")
    return ap


def main(argv: list[str] | None = None, run=subprocess.run, popen=subprocess.Popen) -> int:
    args = parser().parse_args(argv)

    root = args.root
    exe = os.path.join(root, "bin64", "exefile.exe")
    try:
        # Checked before --dry-run returns too, so a wrong --root shows up on
        # the dry run rather than on launch day.
        if not os.path.exists(exe):
            raise Refused("no exefile.exe at %s -- is --root the install root?" % exe)
        if is_live_install(root) and not args.allow_real:
            raise Refused("%s looks like the install you play on; launch a copy, or pass --allow-real" % root)
        crypto = common_ini_crypto(root)
        if crypto is not None and crypto.lower() != "placebo":
            raise Refused("%s sets cryptoPack = %s, which overrides start.ini; make it Placebo or remove it"
                          % (os.path.join(root, "common.ini"), crypto))
        own = ["/server:%s" % args.server, "/port:%d" % args.port, "/ssoToken=" + mask_token(args.token)]
        block = launch_block(root, args.server, own, run=run)
    except Refused as e:
        print("windows_launch: %s" % e, file=sys.stderr)
        return e.code

    cmd, env = build_command(root, args.server, args.port, args.token, block, args.gateway)
    prefs = args.prefs or prefs_targets(root, args.server)
    prefs_args = [x for p in prefs for x in ("--prefs", p)]
    apply_cmd = patcher_argv("apply", root, "--tier", "P", "--no-resfiles", *prefs_args)
    if args.allow_real:
        apply_cmd.append("--allow-real")
    verify_cmd = patcher_argv("verify", root, "--no-resfiles", *prefs_args)
    if args.tier:
        verify_cmd += ["--tier", args.tier]

    if args.dry_run:
        print("would run:", subprocess.list2cmdline(apply_cmd))
        print("would run:", subprocess.list2cmdline(verify_cmd))
        print("would launch:", printable(cmd, args.token))
        print("cwd:", root)
        for k in ENV_ALLOWED:
            print("env %s=%s" % (k, env.get(k, "(unset)")))
        for p in prefs:
            print("prefs.ini:", p, "" if os.path.exists(p) else "(apply creates it)")
        return 0

    for step in (apply_cmd, verify_cmd):
        rc = run(step).returncode
        if rc != 0:
            print("windows_launch: %s %s exited %d; not launching (see docs/guides/windows.md, step 4)"
                  % (os.path.basename(PATCHER), step[3], rc), file=sys.stderr)
            return rc

    proc = popen(cmd, cwd=root, env=env)
    print("launched pid", proc.pid)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
