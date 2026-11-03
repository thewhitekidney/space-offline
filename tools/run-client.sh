#!/usr/bin/env bash
# Launch the client inside the jail, under Proton.
#
#   tools/run-client.sh [extra client args...]
#   tools/run-client.sh --catch-all         # log in as `offline`, for a server
#                                           # without accounts (up.sh --catch-all)
#   tools/run-client.sh --second            # the second player's client, in
#                                           # its own prefix (client2-setup.sh)
#
# Refuses to start unless jail-verify.sh passes, so the client can never be
# launched with a live network path.
#
# **Two launch paths, one view (IP15; rulings 3 and 4).** Through the
# root-owned helper's `client` verb when it is installed, current, and
# installed for THIS checkout (`install-jail-helper.sh --check`); otherwise
# through `sudo` directly, which asks for your password. Both build the same
# mount allowlist -- the direct branch sources tools/eve-offline-jail.in, as
# generated for this checkout, and runs its `v_in_jail_client` -- so on both
# the client sees an empty /run, /tmp, /dev/shm and $HOME with only display,
# audio, Proton, the prefix and the game bound back, and both drop to you with
# `setpriv --clear-groups`, never `sudo -u` (which keeps the docker group).
set -euo pipefail

NS=${EVE_JAIL_NS:-eveoffline}

# Who to run the client as, and with whose HOME.
#
# `id -un` is wrong the moment this is invoked under sudo: it returns root,
# and $HOME becomes /root, so the client gets no Wine prefix and no X
# authority. SUDO_USER names the human who actually invoked sudo, so prefer
# it and fall back to id -un when there is no sudo in the picture.
RUN_AS=${SUDO_USER:-$(id -un)}
[[ "$RUN_AS" == "root" ]] && { echo "run-client.sh: refusing to launch the client as root" >&2; exit 1; }
RUN_HOME=$(getent passwd "$RUN_AS" | cut -d: -f6)
[[ -n "$RUN_HOME" ]] || { echo "run-client.sh: cannot resolve a home directory for $RUN_AS" >&2; exit 1; }
RUN_UID=$(id -u "$RUN_AS")
RUN_GID=$(id -g "$RUN_AS")
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# Which Proton prefix, and so which client copy.
#
# **Never the online prefix (O19, ruling 14).** The first client runs in its
# own prefix, ~/.local/share/eve-offline/prefix, a one-time copy of the online
# prefix 8500 that `tools/offline-prefix.sh` makes; `tools/prefix-lib.sh`
# decides the path. 8500 is the prefix the owner's online, Steam-launched EVE
# uses, and the jail binds the client's prefix read-write, so a jailed client
# running there could plant a DLL the online client loads later. A prefix or
# client inside compatdata/8500 is refused here, and again by the jail.
#
# Two clients cannot share one prefix: they would share a wineserver, a
# registry and the install directory's own `cache:`. So a second player means
# a second prefix, and `tools/client2-setup.sh` builds one -- 403M of client
# copy plus a symlink to the 48G `ResFiles` of the offline prefix, which is
# read-only in the jail and safely shared. `--second` is the shorthand for
# pointing at it.
#
# The prefix also decides where the client logs, which is why it is one
# variable rather than three literals: this script had `compatdata/8500`
# written into it in three places.
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
# shellcheck source=tools/login-lib.sh
source "$REPO/tools/login-lib.sh"
: "${EVE_PREFIX_ID:=offline}"
: "${EVE_CLIENT_DIR:=tq-offline}"
LOGIN_WHICH=''
for a in "$@"; do
    case "$a" in
        --second)
            EVE_PREFIX_ID=8501
            EVE_CLIENT_DIR=tq-offline-2
            LOGIN_WHICH=second
            ;;
        --catch-all)
            EVE_SSO_TOKEN=offline
            ;;
    esac
done
# `--second` is ours, not the client's: strip it before the client sees it.
args_in=()
for a in "$@"; do [[ "$a" == "--second" || "$a" == "--catch-all" ]] || args_in+=("$a"); done
set -- ${args_in[@]+"${args_in[@]}"}

# **`$RUN_HOME`, not `$HOME`.** Under sudo `$HOME` is `/root`, so defaults
# built from it point at a Steam install that does not exist and the script
# dies with "proton not found at /root/...". `RUN_HOME` is the home of the
# human who invoked sudo, which is whose Steam library and Wine prefix we
# actually want -- the same reason `RUN_AS` exists ten lines up. This only
# ever worked because `up.sh` exports both variables before calling.
: "${STEAM_ROOT:=$RUN_HOME/.local/share/Steam}"
if [[ -z ${EVE_PREFIX:-} ]]; then
    EVE_PREFIX=$(eve_prefix_of "$EVE_PREFIX_ID" "$RUN_HOME" "$STEAM_ROOT") \
        || { echo "run-client.sh: EVE_PREFIX_ID must be 'offline' or a number, not '$EVE_PREFIX_ID'" >&2; exit 1; }
fi
: "${EVE_CLIENT:=$EVE_PREFIX/pfx/drive_c/CCP/EVE/$EVE_CLIENT_DIR}"
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"

# **The login (ACC2).** The client logs in with `/ssoToken=TOKEN`. Against a
# `--world-accounts` server (tools/up.sh and tools/serve.sh by default) the
# token is `USER:PASSWORD`, read from your login file
# (~/.config/eve-offline/login, or login-second for --second; mode 600,
# tools/login-lib.sh), never from a command line. EVE_SSO_TOKEN, when set,
# wins: `up.sh --catch-all` and client-ci set it to `offline`, which a
# catch-all server accepts from anyone. No file and a terminal: it asks once
# and writes the file. The token is only ever in a variable here.
if [[ -n ${EVE_SSO_TOKEN+set} ]]; then
    SSO_TOKEN=$EVE_SSO_TOKEN
else
    LOGIN_FILE=$(eve_login_path "$RUN_HOME" "$LOGIN_WHICH")
    if [[ ! -e $LOGIN_FILE && $EUID -ne 0 && -t 0 ]]; then
        eve_login_prompt "$LOGIN_FILE" || exit 1
    fi
    SSO_TOKEN=$(eve_login_token "$LOGIN_FILE" "$RUN_UID") || {
        echo "run-client.sh: no usable login. Make it, as yourself, with:" >&2
        echo "    tools/account.sh login${LOGIN_WHICH:+ --second}" >&2
        echo "  (or pass --catch-all against a server without accounts: up.sh --catch-all)" >&2
        exit 1
    }
    echo "== login: $(eve_login_user "$LOGIN_FILE") (from $LOGIN_FILE)"
fi

eve_refuse_online run-client.sh "EVE_PREFIX=$EVE_PREFIX" "EVE_CLIENT=$EVE_CLIENT" \
    "the client's ResFiles=$EVE_CLIENT/../ResFiles"
[[ -d $EVE_PREFIX/pfx ]] || {
    echo "run-client.sh: no Wine prefix at $EVE_PREFIX" >&2
    if [[ $EVE_PREFIX_ID == offline ]]; then
        echo "  The offline client has a prefix of its own (O19). Make it once:" >&2
        echo "      tools/offline-prefix.sh" >&2
    else
        echo "  The second client's prefix is made by tools/client2-setup.sh." >&2
    fi
    exit 1
}
[[ -x "$PROTON" ]] || { echo "run-client.sh: proton not found at $PROTON" >&2; exit 1; }
[[ -f "$EVE_CLIENT/bin64/exefile.exe" ]] || { echo "run-client.sh: client not found at $EVE_CLIENT" >&2; exit 1; }

# We run a COPY, never the installation the owner plays on.
#
# Two reasons, and the second is the one that matters. First, we modify the
# client (cryptoPack, and later possibly loose .py overrides or a patched
# blue.dll); doing that to a live install means Steam can revalidate and
# silently revert it mid-debug. Second, and more important: if the jail ever
# leaked, whatever phoned home would be the client sitting next to the owner's
# real EVE state. A copy keeps the two apart.
#
# The copy lives at `CCP/EVE/tq-offline` inside the offline prefix, beside
# that prefix's own `ResFiles`, so `../ResFiles` resolves exactly as the
# client expects and no drive mapping changes. The prefix itself is a copy
# too (O19, above), so nothing the client writes reaches the online install.
case "$EVE_CLIENT" in
    */tq)
        echo "run-client.sh: EVE_CLIENT points at the real install ($EVE_CLIENT)." >&2
        echo "  Run the copy instead, or set EVE_OFFLINE_ALLOW_REAL_CLIENT=1 to override." >&2
        [[ "${EVE_OFFLINE_ALLOW_REAL_CLIENT:-0}" == "1" ]] || exit 1
        echo "  override set; continuing against the real install" >&2
        ;;
esac

pack=$(sed -n 's/^[[:space:]]*cryptoPack[[:space:]]*=[[:space:]]*//p' "$EVE_CLIENT/start.ini" | tr -d '\r')
if [[ "$pack" != "Placebo" ]]; then
    # EVE_OFFLINE_STOCK_CRYPTO=1 (up.sh --stock-crypto) launches a client
    # whose files all match manifest.dat, so that a startup failure can be
    # attributed to the launch method rather than to our one-line edit.
    # The login cannot succeed in this mode; that is not what it is for.
    [[ "${EVE_OFFLINE_STOCK_CRYPTO:-0}" == "1" ]] || {
        echo "run-client.sh: $EVE_CLIENT/start.ini has cryptoPack = ${pack:-<unset>}, need Placebo" >&2
        exit 1
    }
    echo "run-client.sh: cryptoPack = $pack (diagnostic run; login will not succeed)" >&2
fi

# The client's own command line is what identifies it: the Windows path
# carries the client directory name, so this matches our copy and not the real
# install, and not somebody's other EVE.
CLIENT_PAT="$(basename "$EVE_CLIENT")"'.bin64.exefile'

# **One source of truth, read as a value, never as a pipeline.**
#
# This was `if pgrep -f ...; then echo headline; pgrep -af ... | sed; echo
# explanation; exit 1; fi` and it had two bugs that fired together on
# 2026-10-24 and cost a whole two-player run:
#
#   1. `pgrep -f` can match a process that is *in the act of exiting*, and
#      the `pgrep -af` a line later then finds nothing. Refusing on that is
#      refusing for no reason: the thing we are protecting against is gone.
#   2. The empty `pgrep -af` exits 1, so under `set -o pipefail` the `| sed`
#      took the script with it -- before the explanation, before the `exit
#      1`. The log read exactly one line, "a client from tq-offline is
#      already running:", with no process list and no advice, and the harness
#      reported "the client never started".
#
# **Third site of that same `set -e`-kills-the-diagnosis mistake.** `up.sh`
# had `sudo -n cat ... | sed` and it deleted the useful half of a resolver
# diagnosis the same way. The rule the three of them add up to: a diagnostic
# must not be able to fail, so compute it into a variable with `|| true` and
# print the variable.
#
# **And the process it was finding was another `pgrep`.** With the evidence
# restored, the refusal named its culprit: `42107 pgrep -f
# tq-offline.bin64.exefile`. `pgrep -f` matches any command line *containing*
# the pattern, it excludes only itself, and `client-ci.sh` staggers its two
# clients by polling for the first one with this exact pattern -- so the
# harness's poll and this check saw each other, every `--with-second` run,
# with a probability that depended on the interleaving. The runs that passed
# before got lucky. Deciding by the process's own `comm` is what makes a
# match mean "a client": a pgrep is not a client.
# **An allow-list, because the deny-list was unbounded.** This filtered out
# `pgrep` and `pkill` and treated everything else as a client, which is the
# wrong way round: on 2026-10-24 a diagnostic `bash -c` whose command line
# merely *mentioned* the pattern was reported as a live client, and the
# launch refused to start. Naming the things that are not clients can never
# be finished; naming the things that are, can.
#
# What a real client looks like, read off a live session (pids 406235/58/332):
#
#     comm=python3     argv0=python3                 <- Proton's own script
#     comm=steam.exe   argv0=c:\windows\...\steam.exe
#     comm=exefile.exe argv0=C:\CCP\EVE\...\exefile.exe
#
# So: any Windows-side process (`comm` ends in `.exe`), plus Proton's
# interpreter, which is recognised by its *second* argument rather than its
# name because argv0 is the bare word `python3`. The interpreter is included
# deliberately -- it exists for a moment before `steam.exe` does, and the
# wait loop below must not read that moment as "the client exited".
client_procs() {
    local pid comm argv1
    for pid in $(pgrep -f "$CLIENT_PAT" 2>/dev/null || true); do
        comm=$(cat "/proc/$pid/comm" 2>/dev/null || true)
        argv1=$(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | sed -n 2p || true)
        case $comm in
            *.exe) ;;
            python | python3) [[ $argv1 == */proton ]] || continue ;;
            *) continue ;;
        esac
        printf '%s %s\n' "$pid" "$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    done
}

running=$(client_procs)
if [[ -n $running ]]; then
    # Wait out a client that is already exiting -- and only that. Five
    # seconds is longer than a clean teardown and far shorter than a run.
    for _ in 1 2 3 4 5; do
        sleep 1
        running=$(client_procs)
        [[ -z $running ]] && break
    done
fi
if [[ -n $running ]]; then
    {
        echo "run-client.sh: a client from $(basename "$EVE_CLIENT") is already running:"
        echo "$running" | sed 's/^/  /'
        echo "  It did not exit within 5s, so this is a live client and not a"
        echo "  teardown. Close it first: two clients from one directory would"
        echo "  share one jail and one session."
    } >&2
    exit 1
fi

# **The jail helper, detected here rather than further down.** The check
# immediately below is the one check this script treats as non-negotiable, and
# it needs root -- so it has to know about the helper before it runs, not
# after. It used to call `sudo tools/jail-verify.sh` unconditionally, which on
# a machine with the helper and no cached timestamp cannot run at all: the
# launch was refused for want of a password rather than for want of a sealed
# jail, and the client never started. Third site of that same mistake; the
# other two were in up.sh.
#
# **Only a current helper, installed for this checkout** (V4 C1, C2). A stale
# one predates the allowlist; a foreign one launches ANOTHER tree's shim with
# that tree's launch-env. Either way the direct branch, which builds the view
# from this checkout's template, is the safe one -- it just needs a password.
JAIL_HELPER=/usr/local/sbin/eve-offline-jail
USE_HELPER=0
if [[ -x $JAIL_HELPER ]] && sudo -n "$JAIL_HELPER" status >/dev/null 2>&1; then
    if helper_state=$("$REPO/tools/install-jail-helper.sh" --check 2>&1); then
        USE_HELPER=1
    else
        {
            echo "run-client.sh: not using the installed helper:"
            printf '%s\n' "$helper_state" | sed 's/^/  /'
            echo "  Launching through sudo instead (same allowlist; asks for a password)."
        } >&2
    fi
fi

echo "== verifying the jail before launch"
if [[ ${EVE_CENSUS:-} == 1 ]]; then
    # A census (CEN-3, tools/client-ci.sh --census): `verify` fails by design
    # while census0 holds the default routes; census-verify checks that shape.
    (( USE_HELPER )) || { echo "run-client.sh: EVE_CENSUS=1 needs the jail helper's census verbs" >&2; exit 1; }
    jail_check=(sudo -n "$JAIL_HELPER" census-verify)
elif (( USE_HELPER )); then
    jail_check=(sudo -n "$JAIL_HELPER" verify)
else
    jail_check=(sudo "$REPO/tools/jail-verify.sh")
fi
"${jail_check[@]}" >/dev/null || {
    echo "run-client.sh: jail verification FAILED - refusing to launch" >&2
    echo "  ran: ${jail_check[*]}" >&2
    exit 1
}
echo "   jail ok"

# Where the client writes its own log, including full Python tracebacks.
#
# This is the difference between debugging with evidence and debugging by
# inference. The client reports its exceptions to the server's `alert` service
# as an opaque stack hash, and its Gamelog file carries only user-facing
# messages, so a client-side traceback was invisible to us. `blue` has a
# native file-logging sink and `carbon/common/lib/autoexec_common` turns it on
# from the command line, very early in startup:
#
#     fileLoggingThreshold = 0
#     for arg in blue.pyos.GetArg():
#         if arg.startswith('/fileLogDirectory'):
#             fileLoggingDirectory = arg.split('=')[1]
#     ...
#     blue.EnableFileLogging(os.path.normpath(fileLoggingDirectory),
#                            boot.role, fileLoggingThreshold)
#
# Note the threshold. The command-line path leaves it at 0 -- every severity,
# info included -- while the prefs.ini route defaults it to 1 and silently
# drops info. That alone is a reason to pass the switch rather than edit a
# config file, quite apart from it touching nothing on disk.
#
# `logmodule.LogException` and `LogTraceback` both default `toLogServer=1` and
# write the whole formatted traceback, with locals, as a single record, so one
# exception is one entry rather than something to reassemble.
#
# The path is a Windows path because the client is a Windows process; it lands
# in the Proton prefix, where it is an ordinary directory on this machine.
EVE_LOG_WINDOWS='C:\eve-logs'
EVE_LOG_HOST="$EVE_PREFIX/pfx/drive_c/eve-logs"
mkdir -p "$EVE_LOG_HOST" 2>/dev/null || true
chown "$RUN_AS" "$EVE_LOG_HOST" 2>/dev/null || true

# /ssoToken= is a recognised client argument (parsed in gameui and loginII),
# which is how we skip the launcher entirely. It is NOT in this list: it goes
# to jail-client.sh as its own launch-env key, EVE_SSO_TOKEN (below), so a
# user name with a space survives (EVE_CLIENT_ARGS is split on spaces) and
# the token is never an argument of the patcher's --check-args.
args=(
    "/fileLogDirectory=$EVE_LOG_WINDOWS"
    # **Belt as well as braces on phoning home.** The jail already makes
    # egress impossible, so a crash report cannot leave this machine either
    # way -- but not attempting it is better than relying on the block, and
    # the client ships a SECOND program that would try: `eve_crashmon.exe`,
    # which is in our own bin64 alongside exefile. The flag string is present
    # in OUR build's `bin64/exefile.exe` (3619879) [V]; that it suppresses the
    # upload is [I], inherited from a third-party offline setup on build
    # 3396210 [V2] and not yet confirmed against a forced crash here. A
    # falsification is cheap: crash the client in the jail and look for a
    # connect attempt in tools/jail-drops.sh.
    "/noCrashReportUpload"
    "$@"
)
# EVE_CLIENT_FILE_LOG=off drops the file log for play. Its threshold is 0, so
# info is open, and with info open michelle dumps its whole destiny history on
# every server update (`michelle.py:1115` DumpHistory): with 50 bots in space
# that is thousands of lines a second, the client's main thread pinned and
# its log at 200 MB in ten minutes. Client CI keeps the default (on).
if [[ ${EVE_CLIENT_FILE_LOG:-on} == off ]]; then
    args=("${args[@]:1}")
fi

# The client must start with its CWD set to the installation root.
#
# blue resolves its virtual path roots -- `bin:/`, `res:/`, `script:/` -- from
# the working directory, so a client started from anywhere else cannot find
# its own files. The first symptom is a "Verification Failure" dialog saying
#   VerifyManifestFile: Couldn't open manifest file bin:/manifest.dat
# which reads like file corruption and is nothing of the sort: manifest.dat is
# sitting in the install root, and the client is looking for it relative to
# wherever you happened to run this script from. Steam sets this implicitly,
# which is why it never shows up when launching normally.
cd "$EVE_CLIENT"

# **Why the Vulkan and X variables are forwarded.** `sudo` scrubs the
# environment, so anything not named in the launch-env below is gone by the
# time the client starts -- and three of them decide whether it draws at all when there is no
# monitor (`docs/lanes/ci-display.md`):
#
#   - `VK_DRIVER_FILES` / `VK_ICD_FILENAMES` pick the Vulkan driver. On a
#     virtual display there is no DRI3, so RADV cannot present
#     (`MESA: info: vulkan: No DRI3 support detected`) -- and the client does
#     **not** fall back by itself: with both ICDs visible it leaves three
#     `exefile.exe` processes alive, never makes a window, and drops its X
#     connection. Lavapipe has to be forced, and forcing it means getting the
#     variable through this `env`.
#   - `XAUTHORITY` because a cookie-protected X server is unreachable without
#     it, and `sudo`'s `env_reset` drops it.
#   - `LP_NUM_THREADS` caps lavapipe's rasteriser pool. lavapipe renders in
#     SOFTWARE, so without this it takes a thread per core and the client alone
#     saturates the machine -- which on a 32-thread box is every core, for the
#     whole length of a run. Set by client-ci.sh from EVE_MAX_JOBS.
#   - `PROTON_USE_XALIA` because Xalia's accessibility layer calls SDL's
#     windowing init before the game gets anywhere, and on a screen it does
#     not like that is `System.PlatformNotSupportedException: Video driver
#     not supported` -- observed in a real `client-ci.sh` run. Setting it to
#     `0` is how the headless path avoids the whole component.
#
# All three are `:-` so they are empty when unset, which is every ordinary
# human launch. The launch-env is the one place that decides what the client
# can see, and a variable missing from it fails as "the client did nothing",
# which is the hardest symptom in this project to diagnose.
# **In-game chat needs one line in the client's prefs.ini**, and this is the
# only place that can write it (docs/lanes/chat.md §1).
#
# The client dials our chat server on 5222 and, unless prefs.ini has
# `chatserver_disable_tls`, wraps the socket in TLS that checks the chain
# against its own certifi bundle -- which is listed in the signed
# manifest.dat, so "add our CA" is not available. Our server speaks plain
# XMPP. prefs.ini is NOT in the manifest, is user-writable, and is read once
# at startup (`eveprefs.prefs`), so the line has to be there before launch.
# Without it a friend's client puts up a modal "RetryChatServerConnection"
# dialog, and our server logs "client attempted TLS".
#
# Which prefs.ini: `%LOCALAPPDATA%\CCP\EVE\<folder>_<server>\settings*\`,
# where <folder> is the client's install path lowercased with `\`, `/` and
# spaces turned into `_` and `:` dropped -- `C:\CCP\EVE\tq-offline` is
# `c_ccp_eve_tq-offline` [V]. **Only our copy's folder is touched**: the
# owner's real install (`c_ccp_eve_tq_tranquility`) has a folder here too
# (in the offline prefix, a copy of it since O19), and a glob over every
# `*_tranquility` would edit settings that are not ours. The file is CRLF, `key=value`, and the client keeps lines
# it loaded when it rewrites it. `EVE_CHAT_VERBOSE=1` also sets
# `xmppChatVerbose=1`, which logs every stanza in the client log.
#
# Written as $RUN_AS, never as root: `sed -i` replaces the file, and a
# root-owned prefs.ini is one the client can no longer save. Dropped with
# setpriv and no supplementary groups, like the client (ruling 3).
as_user() {
    if (( EUID == 0 )); then
        setpriv --reuid="$RUN_UID" --regid="$RUN_GID" --clear-groups -- "$@"
    else
        "$@"
    fi
}
set_pref() {
    local file=$1 key=$2 value=$3
    if grep -qs "^$key=" "$file"; then
        as_user sed -i "s/^$key=[^\r]*/$key=$value/" "$file"
    else
        as_user bash -c '
            f=$1; line=$2
            # The client writes no final newline; add one before appending.
            if [[ -s $f && -n $(tail -c1 "$f" | tr -d "\n") ]]; then printf "\r\n" >>"$f"; fi
            printf "%s\r\n" "$line" >>"$f"
        ' _ "$file" "$key=$value"
    fi
}
ensure_chat_prefs() {
    local rel folder base f found=0
    rel=${EVE_CLIENT#"$EVE_PREFIX/pfx/drive_c/"}
    folder="c_$(printf '%s' "$rel" | tr '[:upper:]/ ' '[:lower:]__' | tr -d ':')"
    base="$EVE_PREFIX/pfx/drive_c/users"
    for f in "$base"/*/AppData/Local/CCP/EVE/"$folder"_*/settings*/prefs.ini; do
        [[ -f $f ]] || continue
        found=1
        set_pref "$f" chatserver_disable_tls 1
        [[ ${EVE_CHAT_VERBOSE:-0} == 1 ]] && set_pref "$f" xmppChatVerbose 1
        echo "   chat: chatserver_disable_tls=1 in ${f#"$base"/}"
    done
    if (( ! found )); then
        # A fresh copy that has never run: create the default profile's file.
        [[ -d $base/steamuser ]] || { echo "   chat: no Wine user directory yet; prefs.ini not written" >&2; return 0; }
        f="$base/steamuser/AppData/Local/CCP/EVE/${folder}_tranquility/settings/prefs.ini"
        as_user mkdir -p "$(dirname "$f")"
        set_pref "$f" chatserver_disable_tls 1
        [[ ${EVE_CHAT_VERBOSE:-0} == 1 ]] && set_pref "$f" xmppChatVerbose 1
        echo "   chat: created ${f#"$base"/}"
    fi
    return 0
}
echo "== chat: plain-TCP XMPP needs chatserver_disable_tls in prefs.ini"
ensure_chat_prefs || echo "run-client.sh: could not write prefs.ini; in-game chat will ask to retry" >&2

echo "== launching (namespace $NS, cwd $PWD)"
echo "   client log: $EVE_LOG_HOST  (tail -f \"$EVE_LOG_HOST\"/client-*.log)"

# **The launch-env, for both branches.** Both end in `tools/jail-client.sh`,
# which reads its settings from this file **as you** and honours an
# allow-list of keys rather than sourcing it: sudo scrubs the environment, and
# the helper's `build_view` reads the same file (as you) to know which
# display, audio, Proton, prefix and game paths to bind back.
launch_env=$REPO/out/client-ci/launch-env
as_user mkdir -p "$(dirname "$launch_env")"
# **Mode 600, and the token line is removed once the client has started**
# (after the launch below): it holds your password while the launch reads it.
# GNU sed -i keeps the mode.
as_user bash -c 'umask 077 && touch "$1" && chmod 600 "$1"' _ "$launch_env"
{
    printf 'HOME=%s\n' "$RUN_HOME"
    printf 'DISPLAY=%s\n' "${DISPLAY:-}"
    printf 'WAYLAND_DISPLAY=%s\n' "${WAYLAND_DISPLAY:-}"
    printf 'XDG_RUNTIME_DIR=%s\n' "${XDG_RUNTIME_DIR:-}"
    printf 'XAUTHORITY=%s\n' "${XAUTHORITY:-}"
    printf 'PROTON_USE_XALIA=%s\n' "${PROTON_USE_XALIA:-}"
    printf 'VK_DRIVER_FILES=%s\n' "${VK_DRIVER_FILES:-}"
    printf 'VK_ICD_FILENAMES=%s\n' "${VK_ICD_FILENAMES:-}"
    printf 'LP_NUM_THREADS=%s\n' "${LP_NUM_THREADS:-}"
    # Diagnostics, so a launch that dies inside Proton can be asked why.
    printf 'PROTON_LOG=%s\n' "${PROTON_LOG:-}"
    printf 'PROTON_LOG_DIR=%s\n' "${PROTON_LOG_DIR:-}"
    printf 'WINEDEBUG=%s\n' "${WINEDEBUG:-}"
    printf 'STEAM_COMPAT_DATA_PATH=%s\n' "$EVE_PREFIX"
    printf 'STEAM_COMPAT_CLIENT_INSTALL_PATH=%s\n' "$STEAM_ROOT"
    printf 'EVE_CLIENT=%s\n' "$EVE_CLIENT"
    printf 'PROTON=%s\n' "$PROTON"
    printf 'EVE_CLIENT_ARGS=%s\n' "${args[*]}"
    printf 'EVE_SSO_TOKEN=%s\n' "$SSO_TOKEN"
    # The census (CEN-3): jail-client.sh honours these only if census0 is
    # really in the namespace with both default routes.
    printf 'EVE_CENSUS=%s\n' "${EVE_CENSUS:-}"
    printf 'EVE_CENSUS_CONTROL=%s\n' "${EVE_CENSUS_CONTROL:-}"
    printf 'EVE_CENSUS_NOENV=%s\n' "${EVE_CENSUS_NOENV:-}"
} | as_user tee "$launch_env" >/dev/null
scrub_token() { as_user sed -i '/^EVE_SSO_TOKEN=/d' "$launch_env" 2>/dev/null || true; }
trap scrub_token EXIT

# **The helper path, when it is current.**
#
# `sudo ip netns exec … <anything>` is the widest of the privileged
# operations -- it is literally "run an arbitrary command as root" -- so when
# the root-owned helper is present we go through its `client` verb instead.
# That verb enters the namespace, builds the allowlisted view, and `setpriv`s
# down to us *before* exec'ing `tools/jail-client.sh`, so the thing that
# actually launches the client is unprivileged and this repo stays unable to
# make root run anything.
if (( USE_HELPER )); then
    sudo -n "$JAIL_HELPER" client || true
else
    # **The direct branch: the helper's own view code, from this checkout.**
    # No second allowlist to drift: `install-jail-helper.sh --print` is the
    # helper exactly as it would be installed for this checkout, and sourcing
    # it defines `v_in_jail_client` without acting (its last lines return when
    # sourced). That function checks it is inside $NS and a private mount
    # namespace, builds the view, refuses on any socket outside the
    # allowlist, sets WINEDLLOVERRIDES=winebrowser.exe=d, and drops with
    # `setpriv --clear-groups --no-new-privs` through the in-jail group gate.
    #
    # Root reads a file you can write here -- but this branch is `sudo` with
    # your password, which already trusts the repo (it ran jail-verify.sh as
    # root above); the no-repo-code-as-root rule is the helper's, and the
    # helper path keeps it. SUDO_UID/SUDO_GID are passed explicitly so that a
    # run-client.sh which is itself under sudo still names you, not root.
    gen=$(as_user mktemp)
    EVE_JAIL_NS=$NS "$REPO/tools/install-jail-helper.sh" --print | as_user tee "$gen" >/dev/null
    bash -n "$gen" || { echo "run-client.sh: the generated helper does not parse; refusing" >&2; rm -f "$gen"; exit 1; }
    sudo ip netns exec "$NS" \
        env SUDO_UID="$RUN_UID" SUDO_GID="$RUN_GID" \
        unshare --mount --propagation private -- \
        bash -c 'source "$1" && v_in_jail_client' eve-offline-jail-direct "$gen" || true
    rm -f "$gen"
fi
# The launch has read the launch-env (proton has returned), so the password
# leaves the file now rather than when the session ends.
scrub_token

# Wait for the CLIENT, not for Proton.
#
# `proton run` returns as soon as it has handed the executable to wineserver,
# so the game is still running when that command finishes. This script used to
# `exec` it and return at that point -- and up.sh treats our return as "the
# session is over", kills eve-server and eve-dns, and tears the jail down. The
# client then finds itself with no network and no server and says
#
#     unable to connect to server
#
# which reads like a server bug and is nothing of the sort: the rug was pulled
# out from under a client that was still playing. So wait for the process
# itself to go away before returning.
echo "== client handed off to wineserver; waiting for it to exit"
# Through `client_procs`, not a bare `pgrep -f`. A bare one counts any command
# line that mentions the pattern -- including a `pgrep` of our own and any
# shell running a diagnostic -- so it can hold this loop open forever on a
# process that is not a client.
while [[ -n $(client_procs) ]]; do
    sleep 2
done
echo "   client exited"
