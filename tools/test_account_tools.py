"""The account tools around ACC2 (docs/lanes/acc2tools.md): the login file
(tools/login-lib.sh), the bots' accounts (tools/bot-accounts.sh) and the
grant wrapper's lock check (tools/account.sh). Stdlib only; fake binaries
stand in for eve-server and eve-bots, so no Rust and no network.

Run::

    python3 -I tools/test_account_tools.py
"""

from __future__ import annotations

import os
import stat
import subprocess
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LIB = os.path.join(HERE, "login-lib.sh")
BOT_ACCOUNTS = os.path.join(HERE, "bot-accounts.sh")
ACCOUNT = os.path.join(HERE, "account.sh")

# Invented, never real.
USER = "Invented Ann"
PASSWORD = "acc2tools-invented-1"


def bash(script: str, *args: str, cwd: str | None = None, env: dict | None = None):
    return subprocess.run(["bash", "-c", script, "t", *args], capture_output=True,
                          text=True, cwd=cwd, env=env)


def lib(snippet: str, *args: str):
    return bash(f'source "{LIB}"\n{snippet}', *args)


class LoginFile(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name
        self.file = os.path.join(self.dir, "login")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, text: str, mode: int = 0o600) -> None:
        with open(self.file, "w") as f:
            f.write(text)
        os.chmod(self.file, mode)

    def token(self):
        return lib('eve_login_token "$1" "$(id -u)"', self.file)

    def test_a_good_file_gives_user_colon_password(self) -> None:
        self.write(f"# comment\nuser={USER}\npassword={PASSWORD}\n")
        r = self.token()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout, f"{USER}:{PASSWORD}\n")

    def test_crlf_and_no_final_newline_are_read(self) -> None:
        self.write(f"user={USER}\r\npassword={PASSWORD}")
        self.assertEqual(self.token().stdout, f"{USER}:{PASSWORD}\n")

    def test_a_file_others_can_read_is_refused_and_never_printed(self) -> None:
        self.write(f"user={USER}\npassword={PASSWORD}\n", 0o644)
        r = self.token()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("chmod 600", r.stderr)
        self.assertNotIn(PASSWORD, r.stdout + r.stderr)

    def test_a_file_inside_a_git_work_tree_is_refused(self) -> None:
        os.mkdir(os.path.join(self.dir, ".git"))
        sub = os.path.join(self.dir, "a")
        os.mkdir(sub)
        self.file = os.path.join(sub, "login")
        self.write(f"user={USER}\npassword={PASSWORD}\n")
        r = self.token()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("git work tree", r.stderr)

    def test_the_server_rules_are_mirrored(self) -> None:
        bad = [
            ("ab", PASSWORD), ("a:b c", PASSWORD), (" Ann", PASSWORD), ("Ann  B", PASSWORD),
            (USER, "short"), (USER, "has=equals-in-it"), (USER, "has space in it"),
            (USER, USER.upper()), ("Some Bot", "OFFLINE"), ("Some Bot", "probe-123"),
        ]
        for user, pw in bad:
            r = lib('eve_login_check "$1" "$2"', user, pw)
            self.assertNotEqual(r.returncode, 0, (user, pw))
            self.assertNotIn(pw, r.stdout, (user, pw))
        for user, pw in [(USER, PASSWORD), ("Bob's-alt_2", "x" * 8), ("abc", "p:a:s:s:w:d")]:
            r = lib('eve_login_check "$1" "$2"', user, pw)
            self.assertEqual(r.returncode, 0, (user, pw, r.stdout))

    def test_a_missing_password_line_is_refused(self) -> None:
        self.write(f"user={USER}\n")
        self.assertNotEqual(self.token().returncode, 0)

    def test_paths(self) -> None:
        r = lib('eve_login_path /h; eve_login_path /h second; EVE_LOGIN_FILE=/x eve_login_path /h')
        self.assertEqual(r.stdout.split(), ["/h/.config/eve-offline/login",
                                            "/h/.config/eve-offline/login-second", "/x"])

    def test_the_user_name_alone_is_safe_to_print(self) -> None:
        self.write(f"user={USER}\npassword={PASSWORD}\n")
        r = lib('eve_login_user "$1"', self.file)
        self.assertEqual(r.stdout, USER + "\n")


# eve-server: --grant rewrites the role in the world file, --mark-bot
# appends the bot field to the row (as account_book.rs writes one); otherwise it
# serves until killed. eve-bots: "signs up" every --name by appending its
# account row (as store.rs writes one) unless FAKE_REFUSE names it.
FAKE_SERVER = r"""#!/usr/bin/env bash
world='' grant=() mark=''
while [[ $# -gt 0 ]]; do
    case $1 in
        --world) world=$2; shift ;;
        --grant) grant=("$2" "$3"); shift 2 ;;
        --mark-bot) mark=$2; shift ;;
        --bind) echo "$2" >>"$FAKE_LOG.binds" ; shift ;;
        *) ;;
    esac
    shift
done
echo "$world" >"$FAKE_LOG.world"
if ((${#grant[@]})); then
    echo "grant ${grant[*]}" >>"$FAKE_LOG"
    awk -F'\t' -v OFS='\t' -v n="${grant[0]}" -v r="${grant[1]}" \
        '$1=="delta" && $2=="account" && tolower($5)==tolower(n) {$6=r} {print}' "$world" >"$world.t"
    mv "$world.t" "$world"
    exit 0
fi
if [[ -n $mark ]]; then
    echo "mark-bot $mark" >>"$FAKE_LOG"
    awk -F'\t' -v OFS='\t' -v n="$mark" \
        '$1=="delta" && $2=="account" && tolower($5)==tolower(n) && NF==11 {$12="bot"} {print}' "$world" >"$world.t"
    mv "$world.t" "$world"
    exit 0
fi
echo "serve" >>"$FAKE_LOG"
exec sleep 60
"""

FAKE_BOTS = r"""#!/usr/bin/env bash
names=() tokens=''
while [[ $# -gt 0 ]]; do
    case $1 in
        --name) names+=("$2"); shift ;;
        --tokens) tokens=$2; shift ;;
        *) ;;
    esac
    shift
done
world=$(cat "$FAKE_LOG.world")
echo "bots ${names[*]} tokens=$(stat -c %a "$tokens")" >>"$FAKE_LOG"
uid=$((70000000 + RANDOM))
for n in "${names[@]}"; do
    [[ " ${FAKE_REFUSE:-} " == *" $n "* ]] && continue
    printf 'delta\taccount\t1\t%s\t%s\tplayer\t0\tpbkdf2-sha256\t2\tsalt\thash\n' "$uid" "$n" >>"$world"
    uid=$((uid + 1))
done
exec sleep 60
"""


class BotAccounts(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        d = self.tmp.name
        self.bin = os.path.join(d, "bin")
        os.mkdir(self.bin)
        for name, body in (("eve-offline-server", FAKE_SERVER), ("eve-bots", FAKE_BOTS)):
            p = os.path.join(self.bin, name)
            with open(p, "w") as f:
                f.write(body)
            os.chmod(p, 0o755)
        self.world = os.path.join(d, "world.dat")
        self.tokens = os.path.join(d, "cfg", "bots.tsv")
        self.log = os.path.join(d, "fake.log")
        self.env = dict(os.environ, FAKE_LOG=self.log)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_it(self, *names: str, refuse: str = ""):
        env = dict(self.env, FAKE_REFUSE=refuse)
        return subprocess.run(
            ["bash", BOT_ACCOUNTS, "--bin", self.bin, "--world", self.world, "--tokens",
             self.tokens, "--logs", self.tmp.name, "--wait", "6", *names, "--", "--content", "X"],
            capture_output=True, text=True, env=env, timeout=60)

    def rows(self) -> dict:
        out = {}
        with open(self.world) as f:
            for line in f:
                p = line.rstrip("\n").split("\t")
                if p[:2] == ["delta", "account"]:
                    out[p[4]] = p[5]
        return out

    def marks(self) -> dict:
        out = {}
        with open(self.world) as f:
            for line in f:
                p = line.rstrip("\n").split("\t")
                if p[:2] == ["delta", "account"]:
                    out[p[4]] = p[11:] == ["bot"]
        return out

    def log_lines(self) -> list:
        with open(self.log) as f:
            return f.read().splitlines()

    def test_new_bots_sign_up_then_are_granted_gm(self) -> None:
        r = self.run_it("Miner1", "Escort1")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.rows(), {"Miner1": "gm", "Escort1": "gm"})
        log = self.log_lines()
        self.assertEqual(log[0], "serve")
        self.assertEqual(log[1], "bots Miner1 Escort1 tokens=600")
        self.assertEqual(log[2:4], ["grant Miner1 gm", "grant Escort1 gm"])
        self.assertEqual(log[4:], ["mark-bot Miner1", "mark-bot Escort1"])
        self.assertEqual(self.marks(), {"Miner1": True, "Escort1": True})
        with open(self.log + ".binds") as f:
            self.assertEqual(f.read().split(), ["127.0.0.1:26090,ceiling=player,signup=on,botnames=on,cap=4096"])
        # The tokens file: mode 600, NAME<TAB>NAME:32 hex, never the name alone.
        self.assertEqual(stat.S_IMODE(os.stat(self.tokens).st_mode), 0o600)
        with open(self.tokens) as f:
            lines = f.read().splitlines()
        self.assertEqual([l.split("\t")[0] for l in lines], ["Miner1", "Escort1"])
        for l in lines:
            name, tok = l.split("\t")
            user, pw = tok.split(":")
            self.assertEqual(user, name)
            self.assertRegex(pw, r"^[0-9a-f]{32}$")

    def test_a_second_run_keeps_passwords_and_starts_no_server(self) -> None:
        self.assertEqual(self.run_it("Miner1").returncode, 0)
        with open(self.tokens) as f:
            first = f.read()
        os.remove(self.log)
        r = self.run_it("Miner1")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.tokens) as f:
            self.assertEqual(f.read(), first)
        self.assertFalse(os.path.exists(self.log), "nothing to sign up or grant")

    def test_a_player_bot_is_only_granted(self) -> None:
        with open(self.world, "w") as f:
            f.write("delta\taccount\t1\t7\tminer1\tplayer\t0\tpbkdf2-sha256\t2\ts\th\n")
        r = self.run_it("Miner1")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.log_lines(), ["grant Miner1 gm", "mark-bot Miner1"])

    def test_a_gm_bot_without_the_mark_is_only_marked(self) -> None:
        with open(self.world, "w") as f:
            f.write("delta\taccount\t1\t7\tminer1\tgm\t0\tpbkdf2-sha256\t2\ts\th\n")
        r = self.run_it("Miner1")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.log_lines(), ["mark-bot Miner1"])
        self.assertEqual(self.marks(), {"miner1": True})

    def test_a_refused_sign_up_fails_and_grants_nothing(self) -> None:
        r = self.run_it("Miner1", "Miner2", refuse="Miner2")
        self.assertEqual(r.returncode, 1)
        self.assertIn("Miner2", r.stderr)
        self.assertFalse(any(l.startswith(("grant", "mark-bot")) for l in self.log_lines()))


class GrantLock(unittest.TestCase):
    def test_grant_is_refused_while_a_server_holds_the_lock(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            world = os.path.join(d, "world.dat")
            open(world, "w").close()
            lock = world + ".lock"
            holder = subprocess.Popen(["flock", lock, "sleep", "30"])
            try:
                for _ in range(50):
                    if subprocess.run(["flock", "-n", lock, "true"]).returncode != 0:
                        break
                    subprocess.run(["sleep", "0.1"])
                r = subprocess.run(["bash", ACCOUNT, "grant", "Someone", "gm", "--world", world,
                                    "--no-build"], capture_output=True, text=True)
            finally:
                holder.kill()
                holder.wait()
            self.assertEqual(r.returncode, 1, r.stderr)
            self.assertIn("holds", r.stderr)
            self.assertIn("/grant", r.stderr)


class Scripts(unittest.TestCase):
    def test_they_parse(self) -> None:
        for name in ("login-lib.sh", "bot-accounts.sh", "account.sh", "up.sh", "serve.sh",
                     "bots.sh", "soak.sh", "run-client.sh", "jail-client.sh", "client-ci.sh"):
            r = subprocess.run(["bash", "-n", os.path.join(HERE, name)], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, name + r.stderr)

    def test_no_launcher_puts_the_token_in_the_split_args(self) -> None:
        with open(os.path.join(HERE, "run-client.sh")) as f:
            rc = f.read()
        self.assertNotIn('"/ssoToken=', rc)
        self.assertIn("EVE_SSO_TOKEN=%s", rc)
        self.assertIn("scrub_token", rc)
        with open(os.path.join(HERE, "jail-client.sh")) as f:
            jc = f.read()
        self.assertIn("[EVE_SSO_TOKEN]=1", jc)
        # the token joins the args only after the patcher's --check-args.
        self.assertLess(jc.index("--check-args"), jc.index('"/ssoToken=${cfg[EVE_SSO_TOKEN]}"'))


if __name__ == "__main__":
    unittest.main()
