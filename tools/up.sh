#!/usr/bin/env bash
# Bring the whole stack up: jail, stub resolver, game server, and the client.
#
#   sudo -v && tools/up.sh                 # everything, including the client
#   sudo -v && tools/up.sh --keep          # ...and leave the jail AND the
#                                          # services up when the client exits,
#                                          # which is what a second client needs
#   sudo -v && tools/up.sh --no-client     # services only, leave them running
#   tools/up.sh --no-build                 # skip the container build step
#   sudo -v && tools/up.sh --signup        # the first run: your login signs up
#                                          # (as a player) on your own listener
#   sudo -v && tools/up.sh --catch-all -- --character Alice --character Bob
#                                          # the old dev mode: no accounts, a
#                                          # fresh universe, everyone GM on the
#                                          # jail's address; everything after
#                                          # `--` goes to eve-server
#   tools/up.sh --stock-crypto             # launch with the stock crypto pack;
#                                          # login cannot succeed, diagnostic only
#   tools/up.sh --no-chat                  # start eve-server WITHOUT --chat
#                                          # (in-game chat is on by default)
#   tools/up.sh --release                  # a release eve-server: the default
#                                          # since O14 (~5x faster per bind)
#   tools/up.sh --debug                    # the debug build (index asserts on)
#
# **Accounts (ACC2, O9/O10).** By default the server runs --world-accounts on
# the world var/world.dat (the one tools/serve.sh hosts; `-- --world FILE`
# picks another), and your client logs in with the name and password in
# ~/.config/eve-offline/login (asked for once; tools/account.sh login). A
# new name signs up only with --signup. Every account starts as a player;
# make yourself GM with the server stopped: tools/account.sh grant NAME gm.
#
# What runs where:
#
#   host, 10.137.0.1:53     eve-dns     *.servers.eveonline.com -> us
#   host, 10.137.0.1:26000  eve-server  machoNet
#   namespace eveoffline    the client, with no route to anything else
#
# Services run on the HOST end of the veth pair, not inside the namespace.
# That is the point: the namespace has no default route, so the host end is
# the only address the client can reach at all, and the nftables rules
# (jail-setup.sh) narrow even that to these two ports.
#
# Everything is torn down when the first client exits -- the jail, eve-dns and
# eve-server -- unless --keep is given, which leaves all three up. Use --keep
# whenever a second client is in play: this script waits on the FIRST client,
# so without it the second player is disconnected when the first one quits.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
# **The clean stop** (lane shutdown): eve-server is stopped through its stop
# file, so it hangs up every session and writes the world before it exits 0;
# SIGTERM only after EVE_STOP_TIMEOUT (default 30 s). Passed explicitly so it
# works in every accounts mode, with or without a --world.
STOP_FILE=$REPO/var/up-server.stop
# shellcheck source=tools/stop-server.sh
source "$REPO/tools/stop-server.sh"
SERVER_PID=''
HOST_IP=${EVE_JAIL_HOST_IP:-10.137.0.1}
NS=${EVE_JAIL_NS:-eveoffline}

# Run this as YOURSELF, not under sudo. The script calls sudo for the three
# things that need root (the jail, binding port 53, entering the namespace)
# and nothing else.
#
# Under `sudo tools/up.sh` everything inherits root: logs and captures land
# root-owned, and worse, run-client.sh resolves the launching user with
# `id -un`, which becomes "root" with HOME=/root -- so the client starts with
# no Wine prefix and no X authority and cannot work. That is exactly what
# happened the first time this was run, and it is a confusing failure because
# the services come up perfectly first.
if [[ $EUID -eq 0 ]]; then
    cat >&2 <<'MSG'
up.sh: do not run this under sudo. Run it as yourself:

    sudo -v && tools/up.sh

The script calls sudo itself where root is actually needed. Running the
whole thing as root makes the client launch as root, with no Wine prefix
and no display access.
MSG
    exit 1
fi
RUN_CLIENT=1
BUILD=1
KEEP=0
STOCK_CRYPTO=0
CHAT=1
CATCH_ALL=0
SIGNUP=0
# **The build eve-server runs from.** Release by default: the owner answered
# O14 yes (tracker §81; `docs/review/rulings.md` ruling 13). The review
# measured 52 ms per bind on debug against 11 ms on release (V3 F1, R2).
# `--debug` opts out; soaks stay on debug (`tools/soak.sh`).
#
# eve-dns is NOT switched: the jail helper's `dns` verb starts
# `target/debug/eve-dns` by a path fixed in the root-owned helper, and the
# reaper below matches that path too. So a release run also builds the debug
# resolver; it answers a handful of queries and its speed is irrelevant.
PROFILE=release
CAPTURE=${EVE_CAPTURE_DIR:-$REPO/captures}

SERVER_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-client) RUN_CLIENT=0 ;;
        --stock-crypto) STOCK_CRYPTO=1 ;;
        --no-build)  BUILD=0 ;;
        --keep)      KEEP=1 ;;
        --no-chat)   CHAT=0 ;;
        --release)   PROFILE=release ;;
        --debug)     PROFILE=debug ;;
        --catch-all) CATCH_ALL=1 ;;
        --signup)    SIGNUP=1 ;;
        -h|--help)   sed -n '2,37p' "${BASH_SOURCE[0]}"; exit 0 ;;
        # Everything after `--` goes to eve-server, which is how the
        # two-player check gets its roster without a second script.
        --) shift; SERVER_ARGS=("$@"); break ;;
        *) echo "up.sh: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

# **Accounts, decided before anything is built** (ACC2; docs/lanes/acc2tools.md).
# World accounts: the world file, its lock, and your login, all checked now so
# a mistake costs seconds, not a boot. Catch-all: the old dev mode, where any
# token is the GM and the roster comes from --character.
WORLD_ARGS=()
if (( CATCH_ALL )); then
    (( ! SIGNUP )) || { echo "up.sh: --signup needs world accounts; drop --catch-all" >&2; exit 2; }
    export EVE_SSO_TOKEN=${EVE_SSO_TOKEN:-offline}
    echo "== accounts: catch-all (every login is the GM; loopback and jail only)"
else
    WORLD=''
    for ((k = 0; k < ${#SERVER_ARGS[@]}; k++)); do
        case ${SERVER_ARGS[k]} in
            --character|--user-id|--accounts)
                echo "up.sh: ${SERVER_ARGS[k]} is for the catch-all or token-file modes; with world" >&2
                echo "       accounts each login brings its own character. Add --catch-all for the old mode." >&2
                exit 2 ;;
            --world) WORLD=${SERVER_ARGS[k + 1]:-} ;;
        esac
    done
    if [[ -z $WORLD ]]; then
        WORLD=$REPO/var/world.dat
        WORLD_ARGS=(--world "$WORLD")
    fi
    WORLD_ARGS+=(--world-accounts)
    mkdir -p "$(dirname "$WORLD")"
    if [[ -e $WORLD.lock ]] && ! flock -n "$WORLD.lock" true 2>/dev/null; then
        echo "up.sh: another eve-server (serve.sh, or an up.sh --keep) runs $WORLD; stop it first" >&2
        exit 1
    fi
    if [[ -s $WORLD ]]; then
        mkdir -p "$REPO/var/backups"
        cp -p "$WORLD" "$REPO/var/backups/$(date -u +%Y%m%dT%H%M%SZ)-up.dat"
    fi
    naccounts=$( { awk -F'\t' '$1 == "delta" && $2 == "account"' "$WORLD" 2>/dev/null || true; } | wc -l)
    echo "== accounts: in the world ${WORLD#"$REPO"/} ($naccounts so far); sign-up $( ((SIGNUP)) && echo on || echo off) on $HOST_IP:26000"
    if (( naccounts == 0 && ! SIGNUP )); then
        echo "   note: no account yet, and without --signup a new name is refused. The first run is:"
        echo "         sudo -v && tools/up.sh --signup     (docs/guides/playing-with-friends.md, 'The owner')"
    fi
    if (( RUN_CLIENT )) && [[ -z ${EVE_SSO_TOKEN+set} ]]; then
        # shellcheck source=tools/login-lib.sh
        source "$REPO/tools/login-lib.sh"
        login=$(eve_login_path "$HOME")
        [[ -e $login ]] || eve_login_prompt "$login" \
            || { echo "up.sh: no login file; make it with tools/account.sh login" >&2; exit 1; }
        eve_login_token "$login" "$(id -u)" >/dev/null || exit 1
        echo "   your login: $(eve_login_user "$login")"
    fi
fi

if [[ $BUILD -eq 1 ]]; then
    echo "== building (in the container; nothing is installed on this machine)"
    if [[ $PROFILE == release ]]; then
        "$REPO/tools/dev.sh" cargo build --locked --release --workspace --bins
        "$REPO/tools/dev.sh" cargo build --locked -p eve-dns --bins
    else
        "$REPO/tools/dev.sh" cargo build --locked --workspace --bins
    fi
fi

SERVER=$REPO/server/target/$PROFILE/$SRV
echo "== eve-server build: $PROFILE"
DNS=$REPO/server/target/debug/eve-dns
for b in "$SERVER" "$DNS"; do
    [[ -x "$b" ]] || { echo "up.sh: $b missing; run without --no-build" >&2; exit 1; }
done

# **The content bundle**, `out/content/` (`docs/content/plan.md` §1.3),
# extracted from the owner's own client install by `tools/content.py` and
# never committed: it is CCP's data. Done here, before sudo and the jail, so
# a bad bundle stops the run before anything is up.
#
# Re-extracted when any of these says it is stale:
#   - there is no MANIFEST.tsv;
#   - a `source` md5 differs from the client's resfileindex.txt;
#   - an `input` sha256 differs from the repo's own `content/` file;
#   - the `extractor` hash differs from the one `tools/content.py` itself
#     computes now (schema.md §1.2).
# Then `content.py --check` runs every time, and any failure refuses to boot.
#
# **Which bundle** is decided by the client's build (tools/content-lib.sh): the
# pinned build, and any `equivalent` one, uses out/content exactly as before;
# an `unverified` build (a launcher update, the 11-09 freeze before it is
# measured) uses out/content-<build> and is refused out/content, so a stray
# EVE_CLIENT_DIR can never extract the wrong build over the good bundle.
# EVE_CONTENT=out/some-dir picks the directory by hand.
: "${EVE_CLIENT_DIR:=tq-offline}"
: "${EVE_CLIENT:=$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/$EVE_CLIENT_DIR}"
# shellcheck source=tools/content-lib.sh
source "$REPO/tools/content-lib.sh"
eve_content_select "$EVE_CLIENT" "$REPO" || { echo "up.sh: refusing to pick a content bundle (EVE_CLIENT=$EVE_CLIENT)" >&2; exit 1; }
CONTENT=$REPO/$CONTENT_REL
MANIFEST=$CONTENT/MANIFEST.tsv
if [[ $CONTENT_REL != out/content ]]; then
    echo "== client build $EVE_CBUILD (${EVE_CSTATUS}): content bundle $CONTENT_REL"
fi
# The client lives in the offline client's own Wine prefix (O19), not in the
# online prefix 8500. Said here, once, because everything below that reads
# the client would otherwise fail with "not found" and no reason.
if [[ ! -d $EVE_CLIENT && ! -d $HOME/.local/share/eve-offline/prefix/pfx ]]; then
    echo "up.sh: no client at $EVE_CLIENT" >&2
    echo "  The jailed client has a Wine prefix of its own now (O19). Make it once with:" >&2
    echo "      tools/offline-prefix.sh" >&2
fi

# **The extractor hash, asked of content.py** (IP6, CA2; V7 F9). It used
# to be re-implemented here over a hand-copied reader list, an [A] reading
# of schema.md §1.2 that would drift the day content.py's `READERS` grew.
# Now there is one definition, `content.extractor_hash()`, and this asks it.
# Host python3 is enough: content.py imports only the stdlib at load time,
# and `-I` keeps tools/dis.py from shadowing the stdlib's. A failure prints
# nothing, which content_stale reports as stale, so the worst case is one
# needless re-extraction, never a stale boot.
extractor_hash() {
    python3 -I -c '
import importlib.util, sys
spec = importlib.util.spec_from_file_location("content", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
print(mod.extractor_hash())' "$REPO/tools/content.py" 2>/dev/null || true
}

# Prints why the bundle is stale, one reason per line; prints nothing when
# it is current.
content_stale() {
    if [[ ! -s $MANIFEST ]]; then echo "no $MANIFEST"; return; fi
    local want; want=$(extractor_hash)
    local have; have=$(awk -F'\t' '$1=="extractor"{print $2; exit}' "$MANIFEST")
    if [[ -z $want ]]; then
        echo "cannot compute the extractor hash (python3 -I tools/content.py failed)"
    elif [[ $have != "$want" ]]; then
        echo "the extractor changed (manifest ${have:0:8}, tools ${want:0:8})"
    fi
    if [[ $CONTENT_REL != out/content && -n $EVE_CBUILD ]]; then
        local mb; mb=$(awk -F'\t' '$1=="client_build"{print $2; exit}' "$MANIFEST")
        [[ $mb == "$EVE_CBUILD" ]] || echo "the bundle is build ${mb:-?}, the client is $EVE_CBUILD"
    fi
    local rel sha
    while IFS=$'\t' read -r rel sha; do
        [[ -f $REPO/$rel ]] && [[ $(sha256sum "$REPO/$rel" | cut -d' ' -f1) == "$sha" ]] \
            || echo "the authored input $rel changed"
    done < <(awk -F'\t' '$1=="input"{print $2 "\t" $3}' "$MANIFEST")
    local index=$EVE_CLIENT/resfileindex.txt
    if [[ -r $index ]]; then
        # resfileindex.txt: `res:/path,resfile,md5,...`; column 3 is the md5.
        awk -F'\t' 'NR==FNR { if ($1=="source") want[$2]=$3; next }
                    { split($0, f, ","); if (f[1] in want) { got[f[1]]=f[3] } }
                    END { for (p in want) if (got[p] != want[p])
                              print "the client file " p " changed" }' \
            "$MANIFEST" "$index" | head -n 5 || true
    else
        echo "   (no $index: cannot compare source md5s)" >&2
    fi
    return 0
}

content_tool() {
    # EVE_CLIENT is passed down so analysis.sh mounts the client chosen above,
    # not its own default (which is tq-offline whatever EVE_CLIENT_DIR says).
    EVE_CLIENT="$EVE_CLIENT" "$REPO/tools/analysis.sh" python3 -I /work/tools/content.py \
        --client /client --resfiles /client-root/ResFiles --out "/work/$CONTENT_REL" \
        ${EVE_CONTENT_TOOL_ARGS[@]+"${EVE_CONTENT_TOOL_ARGS[@]}"} "$@"
}

if [[ ! -f $REPO/tools/content.py ]]; then
    echo "up.sh: tools/content.py is missing, so there is no way to make the content bundle." >&2
    exit 1
fi
stale=$(content_stale)
if [[ -n $stale ]]; then
    echo "== extracting the content bundle into $CONTENT"
    printf '%s\n' "$stale" | sed 's/^/   because: /'
    content_tool || { echo "up.sh: tools/content.py failed; not booting on a stale bundle" >&2; exit 1; }
fi
mkdir -p "$REPO/logs"
echo "== checking the content bundle (content.py --check)"
if ! content_tool --check >"$REPO/logs/content-check.log" 2>&1; then
    tail -n 20 "$REPO/logs/content-check.log" | sed 's/^/   /' >&2
    echo "up.sh: content.py --check failed (logs/content-check.log); refusing to boot." >&2
    exit 1
fi

# The client's own start.ini decides whether the post-handshake stream is
# encrypted. With cryptoPack = CryptoAPI every MachoPacket after the handshake
# is encrypted under CCP's public key, which we do not have; Placebo makes
# EncryptedRead/EncryptedWrite the identity (docs/reference/login-path.md section 3).
# A stock install says CryptoAPI, so this will fail until it is changed.
#
# We check rather than edit. It is the owner's installation, and a script that
# both launches things and rewrites files in a game directory is not one you
# want to run without reading first.
START_INI=$EVE_CLIENT/start.ini
if [[ -f "$START_INI" ]]; then
    pack=$(sed -n 's/^[[:space:]]*cryptoPack[[:space:]]*=[[:space:]]*//p' "$START_INI" | tr -d '\r')
    if [[ "$pack" != "Placebo" && $STOCK_CRYPTO -eq 1 ]]; then
        # --stock-crypto: run the client with its stock crypto pack on
        # purpose. The login cannot succeed -- everything after the handshake
        # would be encrypted against a key we do not have -- but that is not
        # what this mode is for. It exists to launch a client whose files all
        # match manifest.dat, so that a startup failure can be attributed to
        # the launch method rather than to our one-line edit. Diagnostic only.
        echo "== client: cryptoPack = $pack (--stock-crypto; login WILL fail, this is a diagnostic run)"
    elif [[ "$pack" != "Placebo" ]]; then
        cat >&2 <<MSG
up.sh: $START_INI has cryptoPack = ${pack:-<unset>}, and it must be Placebo.

  Do NOT just edit the line by hand: start.ini is covered by manifest.dat,
  which stores a SHA-256 of it, so a hand edit makes the client refuse to
  start with "Verification Failure". Use the tool, which also updates the
  manifest to match:

      tools/client-prepare.sh

  (operates on the copy at \$EVE_CLIENT; reversible with --restore).
MSG
        exit 1
    else
        echo "== client: cryptoPack = Placebo"
    fi
fi

# **The jail helper, if it is installed.**
#
# `/usr/local/sbin/eve-offline-jail` is root-owned and backed by a sudoers
# entry granting exactly seven verbs, so when it is present the privileged half
# of this script needs no password and no cached timestamp -- which is what
# makes an unattended run possible at all. `docs/reference/unattended-root.md` has the
# design and what it does and does not grant.
#
# When it is absent, every path below is exactly what it has always been. A
# fresh clone on a machine with no helper behaves identically, and nothing in
# this repo depends on a privilege that may not be there.
JAIL_HELPER=/usr/local/sbin/eve-offline-jail
if [[ -x $JAIL_HELPER ]] && sudo -n "$JAIL_HELPER" status >/dev/null 2>&1; then
    USE_HELPER=1
    echo "== jail helper: $JAIL_HELPER (no password needed)"
else
    USE_HELPER=0
    # One sudo prompt up front rather than three at random moments, and a
    # clear failure now rather than halfway through bringing the jail up.
    echo "== sudo (jail setup needs root; so does binding port 53)"
    sudo -v
fi

# The three privileged operations, each one line either way.
jail_up()       { if (( USE_HELPER )); then sudo -n "$JAIL_HELPER" up; else sudo "$REPO/tools/jail-setup.sh"; fi; }
jail_verify()   { if (( USE_HELPER )); then sudo -n "$JAIL_HELPER" verify; else sudo "$REPO/tools/jail-verify.sh"; fi; }
jail_down()     { if (( USE_HELPER )); then sudo -n "$JAIL_HELPER" down >/dev/null 2>&1 || true; else sudo "$REPO/tools/jail-teardown.sh" >/dev/null 2>&1 || true; fi; }

# Reap our own corpses while the sudo timestamp is fresh.
#
# A previous run's eve-dns can outlive it -- see the note in cleanup() -- and
# it holds 10.137.0.1:53, so this run's resolver dies on bind with "Address
# already in use" and up.sh reports only "a service died on startup". That is
# three steps removed from the actual cause.
#
# Matching on the full binary path, not the name: this must never pick off a
# process that merely looks like ours.
stale=$(pgrep -f "^$REPO/server/target/debug/eve-dns " 2>/dev/null || true)
if [[ -n "$stale" ]]; then
    echo "== reaping a stranded eve-dns from an earlier run (pid $stale)"
    # shellcheck disable=SC2086
    kill $stale 2>/dev/null || sudo -n kill $stale 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pgrep -f "^$REPO/server/target/debug/eve-dns " >/dev/null 2>&1 || break
        sleep 0.2
    done
    if pgrep -f "^$REPO/server/target/debug/eve-dns " >/dev/null 2>&1; then
        echo "up.sh: could not stop the old eve-dns; run: sudo pkill -f eve-dns" >&2
        exit 1
    fi
fi

pids=()
cleanup() {
    local code=$?
    echo
    echo "== shutting down"
    # A client that outlived us would be left with no resolver, no server and
    # no veth, and would report "unable to connect to server" -- a failure
    # that looks like ours and is not. run-client.sh waits for the client, so
    # reaching here with one still up means something went wrong; say so
    # rather than tearing the network out from under it silently.
    if pgrep -f 'tq-offline[^ ]*.bin64.exefile' >/dev/null 2>&1 && [[ $KEEP -eq 0 ]]; then
        echo "   WARNING: an EVE client is still running." >&2
        echo "   It will lose the jail and the server when this finishes," >&2
        echo "   and will then report that it cannot connect. Close it." >&2
    fi
    # `sudo -n`, and the failure is REPORTED rather than swallowed.
    #
    # This used to be `sudo kill "$p" 2>/dev/null || true`, and that one
    # `|| true` cost a launch. By the time the trap runs, the sudo timestamp
    # taken at startup may well have expired -- a session is easily longer
    # than the grace period -- so the kill fails, the message is thrown away,
    # and eve-dns survives us still holding 10.137.0.1:53. The NEXT run then
    # dies with "Address already in use", which reads like a bug in the
    # resolver and is really our own corpse from an hour ago.
    #
    # The pre-flight reaper below means this is no longer fatal when it
    # happens, but a thing that silently does not happen is worth a line.
    # **`--keep` keeps the services too, and that is the whole point of it.**
    #
    # It used to guard only the jail teardown, while the usage text said
    # "everything is torn down on exit ... unless --keep". With one client
    # that gap never showed: this script waits on the client, so by the time
    # the trap ran there was nobody left to disconnect.
    #
    # With two, it cost the second player their session. `run-client.sh`
    # waits for the FIRST client, so when Alice's client exited this trap ran
    # and killed eve-server out from under Bob -- who was still playing, and
    # saw it as the server dropping him. A one-player assumption with a
    # two-player symptom, and nothing in any log said so because nothing had
    # gone wrong.
    if [[ $KEEP -eq 1 ]]; then
        echo "   services left up (--keep): eve-dns, eve-server"
        echo "   stop them with: touch $STOP_FILE (eve-server writes its world and exits);"
        echo "   sudo pkill -f 'eve-dns --bind'"
        echo "   jail left up; tear down with sudo tools/jail-teardown.sh"
        exit $code
    fi
    local stranded=()
    # eve-server first, through its stop file: the world is written before
    # the resolver and the jail go.
    if [[ -n $SERVER_PID ]] && ! stop_server "$SERVER_PID" "$STOP_FILE"; then
        echo "   WARNING: eve-server did not stop on $STOP_FILE; it was sent SIGTERM" >&2
    fi
    for p in "${pids[@]:-}"; do
        [[ -n "$p" ]] || continue
        [[ $p == "$SERVER_PID" ]] && ! kill -0 "$p" 2>/dev/null && continue
        kill "$p" 2>/dev/null && continue
        sudo -n kill "$p" 2>/dev/null && continue
        kill -0 "$p" 2>/dev/null && stranded+=("$p")
    done
    if [[ ${#stranded[@]} -gt 0 ]]; then
        echo "   WARNING: could not stop ${stranded[*]} (sudo timestamp expired)." >&2
        echo "   The next run will reap them; or: sudo kill ${stranded[*]}" >&2
    fi
    # The phone-home record needs no saving: it is logs/eve-dns.log, which is
    # ours and outlives the jail. An earlier version shelled out to
    # jail-drops.sh here to snapshot the nftables counters before teardown --
    # wrong on two counts. It needed sudo in an exit trap, where the
    # timestamp from startup may well have expired; and the counters are a
    # narrow slice anyway, because with no default route almost every
    # phone-home dies at routing and never reaches the output hook.
    if [[ -s "$REPO/logs/eve-dns.log" ]]; then
        echo "   phone-home attempts: tools/jail-drops.sh"
    fi
    if [[ $KEEP -eq 0 ]]; then
        jail_down
        echo "   jail torn down"
    else
        echo "   jail left up (--keep); tear down with sudo tools/jail-teardown.sh"
    fi
    exit $code
}
trap cleanup EXIT INT TERM

# Tear down any jail left behind before building a new one.
#
# **This is the generalisation that should have been made twice already.**
# The stranded `eve-dns` above, the counter snapshot removed in 04e89c6, and
# a stranded netns are three instances of one bug: a previous run left
# something behind, and the next run met it as a confusing failure a long way
# from the cause. Each was fixed in place; none was generalised.
#
# `jail-teardown.sh` is idempotent by construction -- every line ends in
# `|| true` or `2>/dev/null` -- so running it unconditionally costs one
# command and removes the whole class. The exit trap still tears down on the
# way out; this is for the runs where that never got to happen, because the
# process was SIGKILLed or the sudo timestamp had expired.
jail_down

# A launch-env from before O19 names the online prefix and fails the verify.
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"
eve_drop_stale_launch_env "$REPO"

echo "== jail"
jail_up
jail_verify

mkdir -p "$CAPTURE" "$REPO/logs" "$REPO/var"

# An earlier `sudo tools/up.sh` leaves these root-owned, and the next correct
# run then cannot write to them. Reclaim rather than fail with a permission
# error three lines later.
for d in "$CAPTURE" "$REPO/logs"; do
    [[ -w "$d" ]] || { echo "   reclaiming $d from root"; sudo chown -R "$(id -u):$(id -g)" "$d"; }
done

echo "== stub resolver on $HOST_IP:53"
if (( USE_HELPER )); then
    # **The helper runs it as us**, with `CAP_NET_BIND_SERVICE` and nothing
    # else, so root never executes a binary this repo rebuilds on every run.
    # The redirect is opened by *this* shell, as the invoking user, which is
    # why it may point at a file in the repo at all -- the helper itself
    # refuses to open one, because its redirect would be performed by root.
    sudo -n "$JAIL_HELPER" dns >"$REPO/logs/eve-dns.log" 2>&1
    # Not in `pids`: it is `setsid`-detached and is not our child, so `wait`
    # would never see it. `jail_down` in the exit trap stops it.
else
    # **Not as root** (ruling 3; V6 F13). `sudo "$DNS"` ran a binary this repo
    # rebuilds on every run as full root, with every supplementary group --
    # docker and libvirt among them, each root by another door. Now root only
    # execs setpriv, which becomes us with NO supplementary groups and only
    # CAP_NET_BIND_SERVICE (inheritable + ambient, so it survives the uid
    # change and the exec), before exec'ing the resolver. The same line as
    # the helper's `dns` verb (tools/eve-offline-jail.in, v_dns).
    sudo setpriv --reuid="$(id -u)" --regid="$(id -g)" --clear-groups \
        --inh-caps=+net_bind_service --ambient-caps=+net_bind_service \
        "$DNS" --bind "$HOST_IP:53" --target "$HOST_IP" >"$REPO/logs/eve-dns.log" 2>&1 &
    pids+=($!)
fi

# **The content bundle** (`docs/content/plan.md` §1.3) was made and checked
# before the jail went up; the server reads it with `--content`. The five old
# per-file flags still work after `--`, each overriding one file of it.
BOOT_ARGS=(--content "$CONTENT")
# An `unverified` build is accepted only by naming it (the registry's opt-in),
# unless the caller already did after `--`.
if (( ${#EVE_SERVER_BUILD_ARGS[@]} )); then
    case " ${SERVER_ARGS[*]-} " in
        *" --client-build "*) ;;
        *) BOOT_ARGS+=("${EVE_SERVER_BUILD_ARGS[@]}") ;;
    esac
fi

# **Our authored tables by absolute path** (CA2; V7 F2, X4). eve-server's
# own default is `content` relative to its working directory, so a run from
# anywhere but the repo root silently fell back to the compiled-in copies.
# `$REPO` is absolute, and the server's `[authored]` lines echo it.
BOOT_ARGS+=(--authored "$REPO/content")

# **In-game chat (XMPP), on by default** since 7194f8c; `--no-chat` leaves
# it off. The server's own default is off, so a bare `eve-server` is
# unchanged; this script is the place that turns it on. `docs/lanes/chat.md`.
(( CHAT )) && BOOT_ARGS+=(--chat)

# The world and its accounts (decided at the top). Sign-up only with
# --signup: this listener is reachable from this machine and the jail alone,
# its ceiling is gm, and a sign-up is still only ever a player (O10).
BOOT_ARGS+=(${WORLD_ARGS[@]+"${WORLD_ARGS[@]}"})
LISTENER=$HOST_IP:26000
(( SIGNUP )) && LISTENER+=",signup=on"

echo "== game server on $HOST_IP:26000 (capturing to $CAPTURE)"
"$SERVER" --bind "$LISTENER" --capture "$CAPTURE" \
    --stop-file "$STOP_FILE" "${BOOT_ARGS[@]}" "${SERVER_ARGS[@]}" \
    >"$REPO/logs/eve-server.log" 2>&1 &
SERVER_PID=$!
pids+=("$SERVER_PID")

sleep 1
# Say WHAT died and WHY, here, rather than "see logs/".
#
# The logs are two files in a directory, one of them is almost always empty,
# and the reason is invariably the last line of the other -- so pointing at
# the directory just moves the work. A real instance: eve-dns could not bind
# because a previous run's copy was still holding the port, and all this said
# was "a service died on startup".
for p in "${pids[@]}"; do
    kill -0 "$p" 2>/dev/null && continue
    echo "up.sh: a service died on startup." >&2
    for log in "$REPO/logs/eve-dns.log" "$REPO/logs/eve-server.log"; do
        [[ -s "$log" ]] || continue
        echo "   --- $(basename "$log") ---" >&2
        tail -n 5 "$log" | sed 's/^/   /' >&2
    done
    exit 1
done

# **Prove the resolver dropped what ruling 3 says it drops**, from its own
# /proc entry: our uid, no supplementary groups, and no capability but
# net_bind_service (bit 10, 0x400) effective. Both start paths, because the
# helper's copy is installed separately and may predate --clear-groups.
dns_pid=$(pgrep -f "^$DNS " 2>/dev/null | head -n 1 || true)
if [[ -n $dns_pid && -r /proc/$dns_pid/status ]]; then
    dns_uid=$(awk '$1=="Uid:"{print $2}' "/proc/$dns_pid/status")
    dns_groups=$(awk '$1=="Groups:"{$1=""; sub(/^ +/, ""); print}' "/proc/$dns_pid/status")
    dns_capeff=$(awk '$1=="CapEff:"{print $2}' "/proc/$dns_pid/status")
    if [[ $dns_uid != "$(id -u)" || -n $dns_groups ]] \
        || (( (16#${dns_capeff:-0} & ~16#400) != 0 )); then
        echo "up.sh: eve-dns (pid $dns_pid) runs as uid $dns_uid, groups '${dns_groups}'," >&2
        echo "       CapEff $dns_capeff; expected uid $(id -u), no groups, net_bind_service only." >&2
        echo "       an old installed helper? tools/install-jail-helper.sh --diff" >&2
        exit 1
    fi
    echo "== eve-dns runs as uid $dns_uid, no supplementary groups, CapEff $dns_capeff"
fi

# **Wait for the game port before probing it.** Booting from the full content
# bundle takes seconds, not milliseconds (2.1M rocks and 481k celestials in a
# debug build), and the preconditions below probe the port once, with a 3 s
# timeout, one second after the start. The first unattended run on the bundle
# failed exactly there, "not accepting connections", while the server was
# still loading. So: poll from the host side, up to EVE_BOOT_WAIT seconds,
# for as long as every service is alive.
boot_wait=${EVE_BOOT_WAIT:-120}
for ((i = 0; i < boot_wait; i++)); do
    timeout 1 bash -c "exec 3<>/dev/tcp/$HOST_IP/26000" 2>/dev/null && break
    for p in "${pids[@]}"; do
        kill -0 "$p" 2>/dev/null && continue
        echo "up.sh: a service died while booting." >&2
        tail -n 5 "$REPO/logs/eve-server.log" 2>/dev/null | sed 's/^/   /' >&2
        exit 1
    done
    sleep 1
done
((i == 0)) || echo "== game server accepting after ~${i}s"

# Prove the client's two preconditions FROM INSIDE THE NAMESPACE, which is
# where the client sits. Checking them from the host proves nothing about the
# jail: the host can always reach its own veth address and its own resolver.
#
# This is the check that was missing. The first client launch failed with
#   (info) Could not connect to the specified address.
# while the host-side probe had happily reported "resolver ok" -- because it
# asked the resolver directly, over loopback, outside the jail entirely.
#
# Two preconditions, in the order the client needs them:
#   1. the hostname must resolve to us THROUGH the namespace's resolv.conf
#   2. tcp 26000 must actually accept a connection from inside
nsx() { sudo ip netns exec "$NS" "$@"; }

# Two preconditions, in the order the client needs them:
#   1. the hostname must resolve to us THROUGH the namespace's resolv.conf
#   2. tcp 26000 must actually accept a connection from inside
#
# **Both can only be answered from inside the namespace, and entering one
# needs root.** That is why this is the helper's `preflight` verb and not
# `sudo ip netns exec`: with the helper installed and no cached timestamp --
# an unattended run, the case the helper exists for -- the sudo form fails,
# the probe comes back empty, and this script reports "resolved to nothing"
# and tears the whole stack down. It happened twice; the resolver was healthy
# both times and answered 10.137.0.1 correctly when asked directly. The check
# was broken, not the thing it checked.
echo "== preconditions, checked from inside the jail"

# The weaker, unprivileged version, for a helper that predates `preflight`.
# It tests the same three facts from the host side: the namespace's resolv.conf
# (a file we wrote, world-readable while the jail is up), the resolver being
# alive, and the game port accepting. What it CANNOT see is the namespace's
# own routing -- so it is a fallback, and it says so.
preflight_host_side() {
    local failed=0 conf="/etc/netns/$NS/resolv.conf" ns_server=''
    [[ -r $conf ]] && ns_server=$(awk '$1=="nameserver"{print $2; exit}' "$conf")
    if [[ $ns_server == "$HOST_IP" ]]; then
        echo "   ok    resolv.conf in the namespace points at $HOST_IP"
    else
        echo "up.sh: $conf names '${ns_server:-nothing}', expected $HOST_IP" >&2; failed=1
    fi
    if pgrep -f "eve-dns --bind" >/dev/null 2>&1; then
        echo "   ok    the resolver is running"
    else
        echo "up.sh: the resolver is NOT running -- see $REPO/logs/eve-dns.log" >&2; failed=1
    fi
    if timeout 3 bash -c "exec 3<>/dev/tcp/$HOST_IP/26000" 2>/dev/null; then
        echo "   ok    $HOST_IP:26000 accepts a connection (from the host)"
    else
        echo "up.sh: $HOST_IP:26000 is not accepting connections" >&2; failed=1
    fi
    return $failed
}

if (( USE_HELPER )); then
    # Classified on the helper's own sentinels rather than on its exit status,
    # because "the verb is not installed" and "the verb says no" both exit 1.
    # `sudo -n -l <cmd>` cannot tell them apart either: a user with a blanket
    # `(ALL) ALL` rule is reported as permitted for every argument, password
    # requirement and all. Measured, not assumed.
    pf=$(sudo -n "$JAIL_HELPER" preflight 2>&1 || true)
    if [[ $pf == *"PREFLIGHT OK"* ]]; then
        printf '%s\n' "$pf" | sed 's/^/   /'
    elif [[ $pf == *"PREFLIGHT FAILED"* || $pf == *"FAIL  "* ]]; then
        printf '%s\n' "$pf" | sed 's/^/   /' >&2
        echo "up.sh: the client would not find the server. the stack is not usable." >&2
        exit 1
    else
        # Say which of the two it is. The old message named both causes in
        # one breath and listed the sudoers edit last, which on 2026-10-24
        # sent the owner to edit a sudoers file that was already correct
        # while the installed helper -- a copy from before the verb existed
        # -- was the whole problem. The installed script is readable, so
        # this is answerable rather than a guess.
        if ! grep -q '^[[:space:]]*preflight)' "$JAIL_HELPER" 2>/dev/null; then
            echo "   note: the INSTALLED helper has no 'preflight' verb. $JAIL_HELPER"
            echo "         predates it; your sudoers entry is not the problem. reinstall:"
            echo "             tools/install-jail-helper.sh --diff"
            echo "             sudo tools/install-jail-helper.sh"
        else
            echo "   note: the installed helper HAS 'preflight', so the sudoers entry"
            echo "         does not grant it. add this line to"
            echo "         /etc/sudoers.d/eve-offline (validate with visudo -c):"
            echo "             /usr/local/sbin/eve-offline-jail preflight"
        fi
        echo "         falling back to host-side checks, which cannot see the"
        echo "         namespace's routing."
        preflight_host_side || exit 1
    fi
else
    # Resolve the server name from inside the namespace. The `|| true` matters:
    # getent exits non-zero on NXDOMAIN, and under `set -euo pipefail` a failing
    # command substitution aborts the whole script before the diagnostics below
    # can run. We want the diagnosis, not a silent exit. A fresh resolver can also
    # need a beat, so retry a few times.
    got=""
    for attempt in 1 2 3 4 5; do
        got=$(nsx timeout 3 getent hosts tranquility.servers.eveonline.com 2>/dev/null | awk '{print $1; exit}' || true)
        [[ -n "$got" ]] && break
        sleep 0.5
    done
    if [[ "$got" == "$HOST_IP" ]]; then
        echo "   resolves: tranquility.servers.eveonline.com -> $got"
    else
        echo "up.sh: inside the jail, tranquility.servers.eveonline.com resolved to '${got:-nothing}', expected $HOST_IP." >&2
        echo "       the client will not find the server. diagnostics:" >&2
        echo "       --- /etc/netns/$NS/resolv.conf ---" >&2
        # Read it as ourselves first: the file is world-readable while the jail
        # is up, and `sudo -n cat ... | sed` under `pipefail` EXITS THE SCRIPT
        # when sudo refuses -- killing the rest of the diagnosis, which is the
        # useful part. That is what hid the real cause of the two failed runs.
        { cat "/etc/netns/$NS/resolv.conf" 2>/dev/null \
          || sudo -n cat "/etc/netns/$NS/resolv.conf" 2>&1 \
          || echo "(cannot read it: needs root and there is no cached timestamp)"; } \
            | sed 's/^/       /' >&2 || true
        if ! pgrep -f "eve-dns --bind" >/dev/null 2>&1; then
            echo "       the resolver is NOT RUNNING. It bound successfully and then" >&2
            echo "       exited -- check that it is detached from the shell that" >&2
            echo "       started it (the helper's 'dns' verb uses setsid for this;" >&2
            echo "       an older installed copy of it does not: reinstall with" >&2
            echo "       sudo tools/install-jail-helper.sh)." >&2
        fi
        echo "       --- did the resolver receive anything? (eve-dns.log tail) ---" >&2
        tail -n 5 "$REPO/logs/eve-dns.log" 2>&1 | sed 's/^/       /' >&2 || true
        echo "       --- can the namespace reach $HOST_IP:53 at all? ---" >&2
        if nsx timeout 3 bash -c "exec 3<>/dev/udp/$HOST_IP/53" 2>/dev/null; then
            echo "       udp $HOST_IP:53 is reachable, so this is a resolver/nsswitch problem, not routing" >&2
        else
            echo "       udp $HOST_IP:53 is NOT reachable -- a jail routing/nftables problem" >&2
        fi
        exit 1
    fi

    if nsx timeout 3 bash -c "exec 3<>/dev/tcp/$HOST_IP/26000" 2>/dev/null; then
        echo "   reachable: $HOST_IP:26000 accepts a connection"
    else
        echo "up.sh: $HOST_IP:26000 is NOT reachable from inside the jail." >&2
        echo "       eve-server is listening on the host, but the namespace cannot" >&2
        echo "       get to it -- check the nftables output chain in jail-setup.sh." >&2
        exit 1
    fi
fi

echo
echo "stack is up."
echo "  logs:     $REPO/logs/eve-dns.log, $REPO/logs/eve-server.log"
echo "  captures: $CAPTURE  (read one with tools/dev.sh cargo run -q -p eve-server --bin evecap -- FILE --tags)"

if [[ $RUN_CLIENT -eq 1 ]]; then
    echo
    echo "== client"
    EVE_OFFLINE_STOCK_CRYPTO=$STOCK_CRYPTO "$REPO/tools/run-client.sh" || echo "up.sh: the client exited $?"
else
    echo
    echo "services only; ctrl-c to stop"
    wait
fi
